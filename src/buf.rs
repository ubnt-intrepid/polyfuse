use crate::{
    io::{Pipe, SpliceRead},
    types::{NodeID, RequestID},
};
use polyfuse_kernel::*;
use rustix::{
    fs::{Gid, Uid},
    pipe::{PipeFlags, SpliceFlags},
    process::Pid,
};
use std::{
    fmt,
    io::{self, prelude::*},
    mem,
};
use zerocopy::{try_transmute, FromZeros as _, IntoBytes as _};

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

#[repr(transparent)]
pub struct InHeader {
    raw: fuse_in_header,
}

impl fmt::Debug for InHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InHeader")
            .field("len", &self.len())
            .field("nodeid", &self.nodeid())
            .field("unique", &self.unique())
            .field("opcode", &self.opcode())
            .field("uid", &self.uid())
            .field("gid", &self.gid())
            .field("pid", &self.pid())
            .finish()
    }
}

impl InHeader {
    /// Return the total amount of bytes received from the kernel.
    #[inline]
    #[allow(clippy::len_without_is_empty)]
    pub const fn len(&self) -> u32 {
        self.raw.len
    }

    /// Return the inode number associated with the request.
    #[inline]
    pub const fn nodeid(&self) -> Option<NodeID> {
        NodeID::from_raw(self.raw.nodeid)
    }

    /// Return the unique ID of the request.
    #[inline]
    pub const fn unique(&self) -> RequestID {
        RequestID::from_raw(self.raw.unique)
    }

    /// Return the opcode of the request.
    #[inline]
    pub fn opcode(&self) -> Result<fuse_opcode, u32> {
        try_transmute!(self.raw.opcode).map_err(|e| e.into_src())
    }

    #[inline]
    const fn is_special_request(&self) -> bool {
        // 下記に該当する opcode のリクエストは、カーネル側で nodeid/uid/gid/pid に値が設定されない
        // ※ FUSE_FORGET に関してのみ、nodeid が使用される
        matches!(
            self.raw.opcode,
            FUSE_INIT
                | FUSE_FORGET
                | FUSE_BATCH_FORGET
                | FUSE_INTERRUPT
                | FUSE_DESTROY
                | FUSE_NOTIFY_REPLY
        )
    }

    /// Return the user ID of the calling process.
    #[inline]
    pub fn uid(&self) -> Option<Uid> {
        if self.is_special_request() {
            return None;
        }
        Some(Uid::from_raw(self.raw.uid))
    }

    /// Return the group ID of the calling process.
    #[inline]
    pub fn gid(&self) -> Option<Gid> {
        if self.is_special_request() {
            return None;
        }
        Some(Gid::from_raw(self.raw.gid))
    }

    /// Return the process ID of the calling process.
    #[inline]
    pub fn pid(&self) -> Option<Pid> {
        if self.is_special_request() {
            return None;
        }
        Pid::from_raw(self.raw.pid as i32)
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

/// The trait that represents the receiving process of an incoming FUSE request from the kernel.
pub trait TryReceive<T: ?Sized> {
    fn try_receive(&mut self, conn: &mut T) -> io::Result<&InHeader>;
}

pub trait ToParts {
    /// The type of object for reading the remaining part of received request.
    type Data<'a>
    where
        Self: 'a;

    fn to_parts(&mut self) -> (&InHeader, &[u8], Self::Data<'_>);
}

pub struct SpliceBuf {
    header: InHeader,
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
            header: InHeader {
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

    fn reset(&mut self) -> io::Result<()> {
        self.arg.truncate(0);
        if !self.pipe.is_empty() {
            let new_pipe = Pipe::new(PipeFlags::NONBLOCK)?;
            drop(mem::replace(&mut self.pipe, new_pipe));
        }
        Ok(())
    }
}

impl ToParts for SpliceBuf {
    type Data<'a> = &'a mut Pipe;

    fn to_parts(&mut self) -> (&InHeader, &[u8], Self::Data<'_>) {
        (&self.header, &self.arg[..], &mut self.pipe)
    }
}

impl<T: ?Sized> TryReceive<T> for SpliceBuf
where
    T: SpliceRead,
{
    fn try_receive(&mut self, conn: &mut T) -> io::Result<&InHeader> {
        self.reset()?;

        let len = conn.splice_read(&mut self.pipe, self.bufsize, SpliceFlags::NONBLOCK)?;

        if len < mem::size_of_val(&self.header.raw) {
            Err(invalid_data("dequeued request message is too short"))?
        }
        self.pipe.read_exact(self.header.raw.as_mut_bytes())?;

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
    header: InHeader,
    // どうせ再アロケートすることはないので、最初に確保した分で固定してしまう
    arg: Box<[u8]>,
    pos: usize,
}

impl FallbackBuf {
    pub fn new(bufsize: usize) -> Self {
        Self {
            header: InHeader {
                raw: fuse_in_header::new_zeroed(),
            },
            arg: vec![0u8; bufsize - mem::size_of::<fuse_in_header>()].into_boxed_slice(),
            pos: 0,
        }
    }
}

impl ToParts for FallbackBuf {
    type Data<'a> = &'a [u8];

    fn to_parts(&mut self) -> (&InHeader, &[u8], Self::Data<'_>) {
        let (arg, remains) = self.arg.split_at(self.pos);
        (&self.header, arg, remains)
    }
}

impl<T: ?Sized> TryReceive<T> for FallbackBuf
where
    T: io::Read,
{
    fn try_receive(&mut self, conn: &mut T) -> io::Result<&InHeader> {
        self.pos = 0;

        let len = conn.read_vectored(&mut [
            io::IoSliceMut::new(self.header.raw.as_mut_bytes()),
            io::IoSliceMut::new(&mut self.arg[..]),
        ])?;

        if len != self.header.raw.len as usize {
            Err(invalid_data(
                "The value in_header.len is mismatched to the result of readv(2)",
            ))?
        }

        self.pos = self.header.arg_len();

        Ok(&self.header)
    }
}

fn invalid_data<T>(source: T) -> io::Error
where
    T: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    io::Error::new(io::ErrorKind::InvalidData, source)
}
