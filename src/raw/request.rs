use crate::{
    io::{Pipe, SpliceRead},
    types::RequestID,
};
use polyfuse_kernel::{
    fuse_in_header, fuse_notify_retrieve_in, fuse_opcode, fuse_write_in, FUSE_MIN_READ_BUFFER,
};
use rustix::{
    fs::{Gid, Uid},
    pipe::{PipeFlags, SpliceFlags},
    process::Pid,
};
use std::{
    io::{self, prelude::*},
    mem,
};
use zerocopy::{try_transmute, FromZeros as _, Immutable, IntoBytes, KnownLayout};

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

impl RequestHeader {
    /// Return the unique ID of the request.
    #[inline]
    pub const fn unique(&self) -> RequestID {
        RequestID::from_raw(self.raw.unique)
    }

    pub fn opcode(&self) -> Result<fuse_opcode, u32> {
        try_transmute!(self.raw.opcode).map_err(|e| e.into_src())
    }

    /// Return the user ID of the calling process.
    #[inline]
    pub fn uid(&self) -> Uid {
        Uid::from_raw(self.raw.uid)
    }

    /// Return the group ID of the calling process.
    #[inline]
    pub fn gid(&self) -> Gid {
        Gid::from_raw(self.raw.gid)
    }

    /// Return the process ID of the calling process.
    #[inline]
    pub fn pid(&self) -> Pid {
        Pid::from_raw(self.raw.pid as i32).expect("invalid Pid")
    }

    pub(crate) fn raw(&self) -> &fuse_in_header {
        &self.raw
    }

    fn arg_len(&self) -> usize {
        // MEMO: FUSE_SETXATTR において、非常に大きいサイズの値が来る可能性がある
        match self.opcode().ok() {
            Some(fuse_opcode::FUSE_WRITE) => mem::size_of::<fuse_write_in>(),
            Some(fuse_opcode::FUSE_NOTIFY_REPLY) => mem::size_of::<fuse_notify_retrieve_in>(),
            Some(_opcode) => {
                (self.raw.len as usize).saturating_sub(mem::size_of::<fuse_in_header>())
            }
            None => 0, // unrecognized opcode
        }
    }
}

/// The buffer to store a processing FUSE request received from the kernel driver.
pub trait RequestBuf {
    type RemainingData<'a>: io::Read
    where
        Self: 'a;

    fn reset(&mut self) -> io::Result<()>;

    fn try_receive<T>(&mut self, conn: T) -> io::Result<&RequestHeader>
    where
        T: SpliceRead;

    fn parts(&mut self) -> (&RequestHeader, &[u8], Self::RemainingData<'_>);
}

pub struct SpliceBuf {
    header: RequestHeader,
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
            header: RequestHeader {
                raw: fuse_in_header::new_zeroed(),
            },
            arg: {
                let capacity = FUSE_MIN_READ_BUFFER as usize - mem::size_of::<fuse_in_header>();
                let mut vec = vec![0; capacity]; // ensure that the underlying buffer is zeroed.
                vec.truncate(0);
                vec
            },
            pipe: Pipe::new(PipeFlags::NONBLOCK)?,
            bufsize,
        })
    }
}

impl RequestBuf for SpliceBuf {
    type RemainingData<'a> = &'a mut Pipe;

    fn parts(&mut self) -> (&RequestHeader, &[u8], Self::RemainingData<'_>) {
        (&self.header, &self.arg[..], &mut self.pipe)
    }

    fn reset(&mut self) -> io::Result<()> {
        self.arg.truncate(0);
        if !self.pipe.is_empty() {
            let new_pipe = Pipe::new(PipeFlags::NONBLOCK)?;
            drop(mem::replace(&mut self.pipe, new_pipe));
        }
        Ok(())
    }

    fn try_receive<T>(&mut self, mut conn: T) -> io::Result<&RequestHeader>
    where
        T: SpliceRead,
    {
        let len = conn.splice_read(&mut self.pipe, self.bufsize, SpliceFlags::NONBLOCK)?;

        if len < mem::size_of_val(&self.header.raw) {
            Err(invalid_data("dequeued request message is too short"))?
        }
        self.pipe.read_exact(self.header.raw.as_mut_bytes())?;

        if Pid::from_raw(self.header.raw.pid as i32).is_none() {
            return Err(invalid_data("The value in_header.pid is not a valid Pid"));
        }

        if len != self.header.raw.len as usize {
            Err(invalid_data(
                "The value in_header.len is mismatched to the result of splice(2)",
            ))?
        }

        self.arg.resize(self.header.arg_len(), 0);
        self.pipe.read_exact(&mut self.arg[..])?;

        Ok(&self.header)
    }
}

pub struct FallbackBuf {
    header: RequestHeader,
    // どうせ再アロケートすることはないので、最初に確保した分で固定してしまう
    arg: Box<[u8]>,
    pos: usize,
}

impl FallbackBuf {
    pub fn new(bufsize: usize) -> Self {
        Self {
            header: RequestHeader {
                raw: fuse_in_header::new_zeroed(),
            },
            arg: vec![0u8; bufsize - mem::size_of::<fuse_in_header>()].into_boxed_slice(),
            pos: 0,
        }
    }

    pub fn receive_fallback<T>(&mut self, mut conn: T) -> io::Result<&RequestHeader>
    where
        T: io::Read,
    {
        let len = conn.read_vectored(&mut [
            io::IoSliceMut::new(self.header.raw.as_mut_bytes()),
            io::IoSliceMut::new(&mut self.arg[..]),
        ])?;

        if len != self.header.raw.len as usize {
            Err(invalid_data(
                "The value in_header.len is mismatched to the result of splice(2)",
            ))?
        }

        if Pid::from_raw(self.header.raw.pid as i32).is_none() {
            return Err(invalid_data("The value in_header.pid is not a valid Pid"));
        }

        self.pos = self.header.arg_len();

        Ok(&self.header)
    }
}

impl RequestBuf for FallbackBuf {
    type RemainingData<'a> = &'a [u8];

    fn parts(&mut self) -> (&RequestHeader, &[u8], Self::RemainingData<'_>) {
        let (arg, remains) = self.arg.split_at(self.pos);
        (&self.header, arg, remains)
    }

    fn reset(&mut self) -> io::Result<()> {
        self.pos = 0;
        Ok(())
    }

    fn try_receive<T>(&mut self, conn: T) -> io::Result<&RequestHeader>
    where
        T: SpliceRead,
    {
        self.receive_fallback(conn)
    }
}

fn invalid_data<T>(source: T) -> io::Error
where
    T: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    io::Error::new(io::ErrorKind::InvalidData, source)
}
