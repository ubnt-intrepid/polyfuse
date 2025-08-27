use crate::{
    conn::SpliceRead,
    nix::{pipe, Pipe},
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
    // MEMO:
    // * 再アロケートされる可能性があるので Vec<u8> で持つ
    // * デフォルトの system allocator を使用している限りは alignment の心配をする必要は基本的はないはず (malloc依存)
    arg: Vec<u8>,
    bufsize: usize,
    pipe: Pipe,
}

impl RequestBuffer {
    pub(crate) fn new(bufsize: usize) -> io::Result<Self> {
        Ok(Self {
            header: fuse_in_header::default(),
            arg: Vec::with_capacity(
                FUSE_MIN_READ_BUFFER as usize - mem::size_of::<fuse_in_header>(),
            ),
            bufsize,
            pipe: pipe()?,
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

    /// Decode the argument of this request.
    pub fn operation(&self) -> Result<Operation<'_>, crate::op::Error> {
        Operation::decode(&self.header, self.opcode(), &self.arg[..])
    }

    pub(crate) fn clear(&mut self) -> io::Result<()> {
        self.header = fuse_in_header::default(); // opcode=0 means that the buffer is not valid.
        self.arg.truncate(0);
        if self.pipe.reader.remaining_bytes()? > 0 {
            tracing::warn!(
                "The remaining data of request(unique={}) is destroyed",
                self.unique()
            );
            self.pipe = pipe()?;
        }
        Ok(())
    }

    pub(crate) fn opcode(&self) -> fuse_opcode {
        self.header
            .opcode
            .try_into()
            .expect("The request has not been received yet")
    }

    pub(crate) fn try_receive<T>(&mut self, mut conn: T) -> Result<(), ReceiveError>
    where
        T: SpliceRead,
    {
        let len = conn
            .splice_read(&self.pipe.writer, self.bufsize)
            .map_err(|err| {
                // ref: https://github.com/libfuse/libfuse/blob/fuse-3.10.5/lib/fuse_lowlevel.c#L2865
                match err.raw_os_error() {
                    Some(ENODEV) => ReceiveError::Disconnected,
                    Some(ENOENT) | Some(EINTR) => ReceiveError::Interrupted,
                    _ => ReceiveError::Fatal(err),
                }
            })?;

        if len < mem::size_of::<fuse_in_header>() {
            Err(ReceiveError::invalid_data(
                "dequeued request message is too short",
            ))?
        }

        self.pipe.reader.read_exact(self.header.as_mut_bytes())?;
        if len != self.header.len as usize {
            Err(ReceiveError::invalid_data(
                "The value in_header.len is mismatched to the result of splice(2)",
            ))?
        }

        let arglen = match fuse_opcode::try_from(self.header.opcode) {
            Ok(fuse_opcode::FUSE_WRITE) | Ok(fuse_opcode::FUSE_NOTIFY_REPLY) => {
                mem::size_of::<fuse_write_in>()
            }
            Ok(..) => self.header.len as usize - mem::size_of::<fuse_in_header>(),
            Err(..) => Err(ReceiveError::UnrecognizedOpcode(self.header.opcode))?,
        };
        self.arg.resize(arglen, 0); // MEMO: FUSE_SETXATTR において、非常に大きいサイズの値が設定されたときに再アロケートされる可能性がある
        self.pipe.reader.read_exact(&mut self.arg[..])?;

        Ok(())
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
    fn invalid_data<T>(source: T) -> Self
    where
        T: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        Self::Fatal(io::Error::new(io::ErrorKind::InvalidData, source))
    }
}

impl io::Read for RequestBuffer {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buf)
    }
    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        (&*self).read_vectored(bufs)
    }
}

impl io::Read for &RequestBuffer {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.pipe.reader).read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        (&self.pipe.reader).read_vectored(bufs)
    }
}
