use crate::{
    nix::{Pipe, PipeReader},
    raw::conn::SpliceRead,
    types::{RequestID, GID, PID, UID},
};
use libc::{EINTR, ENODEV, ENOENT};
use polyfuse_kernel::{fuse_in_header, fuse_opcode, fuse_write_in, FUSE_MIN_READ_BUFFER};
use std::{
    io::{self, prelude::*},
    mem,
};
use zerocopy::{FromZeros as _, Immutable, IntoBytes, KnownLayout};

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

#[derive(IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct RequestHeader {
    raw: fuse_in_header,
}

impl Default for RequestHeader {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestHeader {
    pub fn new() -> Self {
        Self {
            raw: fuse_in_header::new_zeroed(),
        }
    }

    /// Return the unique ID of the request.
    #[inline]
    pub const fn unique(&self) -> RequestID {
        RequestID::from_raw(self.raw.unique)
    }

    pub fn opcode(&self) -> Option<fuse_opcode> {
        self.raw.opcode.try_into().ok()
    }

    /// Return the user ID of the calling process.
    #[inline]
    pub const fn uid(&self) -> UID {
        UID::from_raw(self.raw.uid)
    }

    /// Return the group ID of the calling process.
    #[inline]
    pub const fn gid(&self) -> GID {
        GID::from_raw(self.raw.gid)
    }

    /// Return the process ID of the calling process.
    #[inline]
    pub const fn pid(&self) -> PID {
        PID::from_raw(self.raw.pid)
    }

    pub(crate) fn raw(&self) -> &fuse_in_header {
        &self.raw
    }

    pub(crate) fn reset(&mut self) {
        self.raw.zero();
    }
}

/// The buffer to store a processing FUSE request received from the kernel driver.
pub trait RequestBuf {
    type RemainingData<'a>: io::Read + Send + Unpin
    where
        Self: 'a;

    fn reset(&mut self) -> io::Result<()>;

    fn try_receive<T>(
        &mut self,
        conn: T,
        header: &mut RequestHeader,
    ) -> Result<fuse_opcode, ReceiveError>
    where
        T: SpliceRead;

    fn parts(&mut self) -> (&[u8], Self::RemainingData<'_>);
}

pub struct SpliceBuf {
    // MEMO:
    // * 再アロケートされる可能性があるので Vec<u8> で持つ
    // * デフォルトの system allocator を使用している限りは alignment の心配をする必要は基本的はないはず (malloc依存)
    arg: Vec<u8>,
    pipe: Pipe,
    bufsize: usize,
}

impl SpliceBuf {
    pub fn new(bufsize: usize) -> io::Result<Self> {
        Ok(Self {
            arg: {
                let capacity = FUSE_MIN_READ_BUFFER as usize - mem::size_of::<fuse_in_header>();
                let mut vec = vec![0; capacity]; // ensure that the underlying buffer is zeroed.
                vec.truncate(0);
                vec
            },
            pipe: crate::nix::pipe()?,
            bufsize,
        })
    }
}

impl RequestBuf for SpliceBuf {
    type RemainingData<'a> = &'a PipeReader;

    fn parts(&mut self) -> (&[u8], Self::RemainingData<'_>) {
        (&self.arg[..], &self.pipe.reader)
    }

    fn reset(&mut self) -> io::Result<()> {
        self.arg.truncate(0);
        if self.pipe.reader.remaining_bytes()? > 0 {
            // パイプにデータが残っている可能性があるので、別パイプに差し替える
            let _ = mem::replace(&mut self.pipe, crate::nix::pipe()?);
        }
        Ok(())
    }

    fn try_receive<T>(
        &mut self,
        mut conn: T,
        header: &mut RequestHeader,
    ) -> Result<fuse_opcode, ReceiveError>
    where
        T: SpliceRead,
    {
        let len = conn
            .splice_read(&self.pipe.writer, self.bufsize)
            .map_err(ReceiveError::from_read_operation)?;

        if len < mem::size_of::<fuse_in_header>() {
            Err(ReceiveError::invalid_data(
                "dequeued request message is too short",
            ))?
        }
        self.pipe.reader.read_exact(header.raw.as_mut_bytes())?;

        if len != header.raw.len as usize {
            Err(ReceiveError::invalid_data(
                "The value in_header.len is mismatched to the result of splice(2)",
            ))?
        }

        let opcode = fuse_opcode::try_from(header.raw.opcode)
            .map_err(|_| ReceiveError::UnrecognizedOpcode(header.raw.opcode))?;

        let arglen = arg_len(header, opcode);
        self.arg.resize(arglen, 0); // MEMO: FUSE_SETXATTR において、非常に大きいサイズの値が設定されたときに再アロケートされる可能性がある
        self.pipe.reader.read_exact(&mut self.arg[..])?;

        Ok(opcode)
    }
}

pub struct FallbackBuf {
    // どうせ再アロケートすることはないので、最初に確保した分で固定してしまう
    arg: Box<[u8]>,
    pos: usize,
}

impl FallbackBuf {
    pub fn new(bufsize: usize) -> Self {
        Self {
            arg: vec![0u8; bufsize - mem::size_of::<fuse_in_header>()].into_boxed_slice(),
            pos: 0,
        }
    }
}

impl RequestBuf for FallbackBuf {
    type RemainingData<'a> = &'a [u8];

    fn parts(&mut self) -> (&[u8], Self::RemainingData<'_>) {
        self.arg.split_at(self.pos)
    }

    fn reset(&mut self) -> io::Result<()> {
        self.pos = 0;
        Ok(())
    }

    fn try_receive<T>(
        &mut self,
        mut conn: T,
        header: &mut RequestHeader,
    ) -> Result<fuse_opcode, ReceiveError>
    where
        T: SpliceRead,
    {
        let len = conn
            .read_vectored(&mut [
                io::IoSliceMut::new(header.raw.as_mut_bytes()),
                io::IoSliceMut::new(&mut self.arg[..]),
            ])
            .map_err(ReceiveError::from_read_operation)?;

        if len != header.raw.len as usize {
            Err(ReceiveError::invalid_data(
                "The value in_header.len is mismatched to the result of splice(2)",
            ))?
        }

        let opcode = fuse_opcode::try_from(header.raw.opcode)
            .map_err(|_| ReceiveError::UnrecognizedOpcode(header.raw.opcode))?;

        self.pos = arg_len(header, opcode);

        Ok(opcode)
    }
}

fn arg_len(header: &RequestHeader, opcode: fuse_opcode) -> usize {
    match opcode {
        fuse_opcode::FUSE_WRITE | fuse_opcode::FUSE_NOTIFY_REPLY => mem::size_of::<fuse_write_in>(),
        _ => header.raw.len as usize - mem::size_of::<fuse_in_header>(),
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
