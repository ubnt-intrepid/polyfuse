use crate::{
    conn::SpliceRead,
    nix::{Pipe, PipeReader},
    Operation,
};
use libc::{EINTR, ENODEV, ENOENT};
use polyfuse_kernel::{fuse_in_header, fuse_opcode, fuse_write_in, FUSE_MIN_READ_BUFFER};
use std::{
    io::{self, prelude::*},
    mem,
};
use zerocopy::IntoBytes as _;

// MEMO: FUSE_MIN_READ_BUFFER に到達する or 超える可能性のある opcode の一覧 (要調査)
// * FUSE_BATCH_FORGET
//    - read buffer ギリギリまで fuse_forget_one を詰めようとする
//    - 超えることはないが、場合によっては大量にメモリコピーが発生する可能性がある
// * FUSE_WRITE / FUSE_NOTIFY_REPLY
//    - これらは当然あり得る
// * FUSE_SETXATTR
//    - name については XATTR_NAME_MAX=256 で制限されているので良い
//    - value 側が非常に大きくなる可能性がある
// * その他
//   - 基本的には NAME_MAX=256, PATH_MAX=4096 による制限があるので、8096 bytes に到達することはないはず

/// The buffer to store a processing FUSE request received from the kernel driver.
pub struct RequestBuffer {
    header: fuse_in_header,
    opcode: Option<fuse_opcode>,
    mode: ReadMode,
}

enum ReadMode {
    Splice {
        // MEMO:
        // * 再アロケートされる可能性があるので Vec<u8> で持つ
        // * デフォルトの system allocator を使用している限りは alignment の心配をする必要は基本的はないはず (malloc依存)
        arg: Vec<u8>,
        pipe: Pipe,
        bufsize: usize,
    },
    Fallback {
        // どうせ再アロケートすることはないので、最初に確保した分で固定してしまう
        arg: Box<[u8]>,
        pos: usize,
    },
}

impl RequestBuffer {
    pub(crate) fn new_splice(bufsize: usize) -> io::Result<Self> {
        Ok(Self {
            header: fuse_in_header::default(),
            opcode: None,
            mode: ReadMode::Splice {
                arg: {
                    let capacity = FUSE_MIN_READ_BUFFER as usize - mem::size_of::<fuse_in_header>();
                    let mut vec = vec![0; capacity]; // ensure that the underlying buffer is zeroed.
                    vec.truncate(0);
                    vec
                },
                pipe: crate::nix::pipe()?,
                bufsize,
            },
        })
    }

    pub(crate) fn new_fallback(bufsize: usize) -> io::Result<Self> {
        Ok(Self {
            header: fuse_in_header::default(),
            opcode: None,
            mode: ReadMode::Fallback {
                arg: vec![0u8; bufsize - mem::size_of::<fuse_in_header>()].into_boxed_slice(),
                pos: 0,
            },
        })
    }

    /// Return the unique ID of the request.
    #[inline]
    pub fn unique(&self) -> u64 {
        self.header.unique
    }

    /// Return the user ID of the calling process.
    #[inline]
    pub fn uid(&self) -> u32 {
        self.header.uid
    }

    /// Return the group ID of the calling process.
    #[inline]
    pub fn gid(&self) -> u32 {
        self.header.gid
    }

    /// Return the process ID of the calling process.
    #[inline]
    pub fn pid(&self) -> u32 {
        self.header.pid
    }

    pub(crate) fn opcode(&self) -> fuse_opcode {
        self.opcode.expect("The request has not been received yet")
    }

    /// Decode the argument of this request.
    pub fn operation(&self) -> Result<(Operation<'_>, RemainingData<'_>), crate::op::Error> {
        let (arg, data) = match &self.mode {
            ReadMode::Splice { arg, pipe, .. } => (&arg[..], RemainingData::Splice(&pipe.reader)),
            ReadMode::Fallback { arg, pos } => {
                let (arg, remaining) = arg.split_at(*pos);
                (arg, RemainingData::Fallback(remaining))
            }
        };
        let op = Operation::decode(&self.header, self.opcode(), arg)?;
        Ok((op, data))
    }

    pub(crate) fn clear(&mut self) -> io::Result<()> {
        self.header = fuse_in_header::default(); // opcode=0 means that the buffer is not valid.
        match &mut self.mode {
            ReadMode::Splice { arg, pipe, .. } => {
                arg.truncate(0);
                if pipe.reader.remaining_bytes()? > 0 {
                    tracing::warn!(
                        "The remaining data of request(unique={}) is destroyed",
                        self.header.unique
                    );
                    let _ = mem::replace(pipe, crate::nix::pipe()?);
                }
            }
            ReadMode::Fallback { pos, .. } => {
                *pos = 0;
            }
        }
        Ok(())
    }

    pub(crate) fn try_receive<T>(&mut self, mut conn: T) -> Result<(), ReceiveError>
    where
        T: SpliceRead,
    {
        match &mut self.mode {
            ReadMode::Splice { arg, pipe, bufsize } => {
                tracing::debug!("use splice(2)");
                let len = conn
                    .splice_read(&pipe.writer, *bufsize)
                    .map_err(ReceiveError::from_read_operation)?;

                if len < mem::size_of::<fuse_in_header>() {
                    Err(ReceiveError::invalid_data(
                        "dequeued request message is too short",
                    ))?
                }
                pipe.reader.read_exact(self.header.as_mut_bytes())?;

                if len != self.header.len as usize {
                    Err(ReceiveError::invalid_data(
                        "The value in_header.len is mismatched to the result of splice(2)",
                    ))?
                }

                let opcode = fuse_opcode::try_from(self.header.opcode)
                    .map_err(|_| ReceiveError::UnrecognizedOpcode(self.header.opcode))?;
                self.opcode = Some(opcode);

                let arglen = arg_len(&self.header, opcode);
                arg.resize(arglen, 0); // MEMO: FUSE_SETXATTR において、非常に大きいサイズの値が設定されたときに再アロケートされる可能性がある
                pipe.reader.read_exact(&mut arg[..])?;
            }

            ReadMode::Fallback { arg, pos } => {
                tracing::debug!("use fallback");
                let len = conn
                    .read_vectored(&mut [
                        io::IoSliceMut::new(self.header.as_mut_bytes()),
                        io::IoSliceMut::new(&mut arg[..]),
                    ])
                    .map_err(ReceiveError::from_read_operation)?;

                if len != self.header.len as usize {
                    Err(ReceiveError::invalid_data(
                        "The value in_header.len is mismatched to the result of splice(2)",
                    ))?
                }

                let opcode = fuse_opcode::try_from(self.header.opcode)
                    .map_err(|_| ReceiveError::UnrecognizedOpcode(self.header.opcode))?;
                self.opcode = Some(opcode);

                *pos = arg_len(&self.header, opcode);
            }
        }
        Ok(())
    }
}

fn arg_len(header: &fuse_in_header, opcode: fuse_opcode) -> usize {
    match opcode {
        fuse_opcode::FUSE_WRITE | fuse_opcode::FUSE_NOTIFY_REPLY => mem::size_of::<fuse_write_in>(),
        _ => header.len as usize - mem::size_of::<fuse_in_header>(),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReceiveError {
    #[error("The connection is disconnected")]
    Disconnected,

    #[error("The read syscall is interrupted")]
    Interrupted,

    #[error(
        "The opcode `{}' is not recognized by the current version of `polyfuse`",
        _0
    )]
    UnrecognizedOpcode(u32),

    #[error("Unrecoverable I/O error: {}", _0)]
    Fatal(#[from] io::Error),
}

impl ReceiveError {
    fn from_read_operation(err: io::Error) -> Self {
        // ref: https://github.com/libfuse/libfuse/blob/fuse-3.10.5/lib/fuse_lowlevel.c#L2865
        match err.raw_os_error() {
            Some(ENODEV) => Self::Disconnected,
            Some(ENOENT) | Some(EINTR) => Self::Interrupted,
            _ => Self::Fatal(err),
        }
    }

    fn invalid_data<T>(source: T) -> Self
    where
        T: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        Self::Fatal(io::Error::new(io::ErrorKind::InvalidData, source))
    }
}

#[derive(Debug)]
pub enum RemainingData<'req> {
    Splice(&'req PipeReader),
    Fallback(&'req [u8]),
}

impl io::Read for RemainingData<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Splice(pipe) => pipe.read(buf),
            Self::Fallback(vec) => vec.read(buf),
        }
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        match self {
            Self::Splice(pipe) => pipe.read_vectored(bufs),
            Self::Fallback(vec) => vec.read_vectored(bufs),
        }
    }
}
