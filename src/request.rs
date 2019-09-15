use crate::abi::fuse_opcode::*;
use crate::abi::{
    fuse_flush_in,
    fuse_getattr_in, //
    fuse_in_header,
    fuse_init_in,
    fuse_opcode,
    fuse_open_in,
    fuse_read_in,
    fuse_release_in,
};
use std::{fmt, io, mem};

#[repr(transparent)]
pub struct InHeader(fuse_in_header);

impl InHeader {
    pub fn len(&self) -> u32 {
        self.0.len
    }

    pub fn unique(&self) -> u64 {
        self.0.unique
    }

    pub fn opcode(&self) -> fuse_opcode {
        unsafe { mem::transmute(self.0.opcode) }
    }
}

impl fmt::Debug for InHeader {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("InHeader")
            .field("len", &self.0.len)
            .field("opcode", &self.opcode())
            .field("unique", &self.0.unique)
            .field("nodeid", &self.0.nodeid)
            .field("uid", &self.0.uid)
            .field("gid", &self.0.gid)
            .field("pid", &self.0.pid)
            .finish()
    }
}

#[repr(transparent)]
pub struct OpInit(fuse_init_in);

impl fmt::Debug for OpInit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpInit")
            .field("major", &self.0.major)
            .field("minor", &self.0.minor)
            .field("max_readahead", &self.0.max_readahead)
            .field("flags", &self.0.flags)
            .finish()
    }
}

impl OpInit {
    pub fn major(&self) -> u32 {
        self.0.major
    }

    pub fn minor(&self) -> u32 {
        self.0.minor
    }

    pub fn max_readahead(&self) -> u32 {
        self.0.max_readahead
    }

    pub fn flags(&self) -> u32 {
        self.0.flags
    }
}

#[repr(transparent)]
pub struct OpGetattr(fuse_getattr_in);

impl fmt::Debug for OpGetattr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpGetattr")
            .field("flags", &self.flags())
            .field("fh", &self.fh())
            .finish()
    }
}

impl OpGetattr {
    pub fn flags(&self) -> u32 {
        self.0.getattr_flags
    }

    pub fn fh(&self) -> u64 {
        self.0.fh
    }
}

#[repr(transparent)]
pub struct OpOpen(fuse_open_in);

impl fmt::Debug for OpOpen {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpOpen")
            .field("flags", &self.flags())
            .finish()
    }
}

impl OpOpen {
    pub fn flags(&self) -> u32 {
        self.0.flags
    }
}

#[repr(transparent)]
pub struct OpRead(fuse_read_in);

impl fmt::Debug for OpRead {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpRead")
            .field("fh", &self.fh())
            .field("offset", &self.offset())
            .field("size", &self.size())
            .field("read_flags", &self.read_flags())
            .field("lock_owner", &self.lock_owner())
            .field("flags", &self.flags())
            .finish()
    }
}

impl OpRead {
    pub fn fh(&self) -> u64 {
        self.0.fh
    }

    pub fn offset(&self) -> u64 {
        self.0.offset
    }

    pub fn size(&self) -> u32 {
        self.0.size
    }

    pub fn read_flags(&self) -> u32 {
        self.0.read_flags
    }

    pub fn lock_owner(&self) -> u64 {
        self.0.lock_owner
    }

    pub fn flags(&self) -> u32 {
        self.0.flags
    }
}

#[repr(transparent)]
pub struct OpFlush(fuse_flush_in);

impl fmt::Debug for OpFlush {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpFlush")
            .field("fh", &self.fh())
            .field("lock_owner", &self.lock_owner())
            .finish()
    }
}

impl OpFlush {
    pub fn fh(&self) -> u64 {
        self.0.fh
    }

    pub fn lock_owner(&self) -> u64 {
        self.0.lock_owner
    }
}

#[repr(transparent)]
pub struct OpRelease(fuse_release_in);

impl fmt::Debug for OpRelease {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpRelease")
            .field("fh", &self.fh())
            .field("flags", &self.flags())
            .field("release_flags", &self.release_flags())
            .field("lock_owner", &self.lock_owner())
            .finish()
    }
}

impl OpRelease {
    pub fn fh(&self) -> u64 {
        self.0.fh
    }

    pub fn flags(&self) -> u32 {
        self.0.flags
    }

    pub fn release_flags(&self) -> u32 {
        self.0.release_flags
    }

    pub fn lock_owner(&self) -> u64 {
        self.0.lock_owner
    }
}

#[derive(Debug)]
pub enum Op<'a> {
    Init(&'a OpInit),
    Destroy,
    Getattr(&'a OpGetattr),
    Open(&'a OpOpen),
    Read(&'a OpRead),
    Flush(&'a OpFlush),
    Release(&'a OpRelease),
}

pub fn parse<'a>(data: &'a [u8]) -> io::Result<(&'a InHeader, Option<Op<'a>>)> {
    let (header, data) = parse_header(data)?;
    let op = match header.opcode() {
        FUSE_INIT => Some(Op::Init(fetch(data)?)),
        FUSE_DESTROY => Some(Op::Destroy),
        FUSE_GETATTR => Some(Op::Getattr(fetch(data)?)),
        FUSE_OPEN => Some(Op::Open(fetch(data)?)),
        FUSE_READ => Some(Op::Read(fetch(data)?)),
        FUSE_FLUSH => Some(Op::Flush(fetch(data)?)),
        FUSE_RELEASE => Some(Op::Release(fetch(data)?)),
        _opcode => None,
    };
    Ok((header, op))
}

fn parse_header<'a>(buf: &'a [u8]) -> io::Result<(&'a InHeader, &'a [u8])> {
    const IN_HEADER_SIZE: usize = mem::size_of::<fuse_in_header>();

    if buf.len() < IN_HEADER_SIZE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "in_header"));
    }
    let (header, remains) = buf.split_at(IN_HEADER_SIZE);

    let header = unsafe { &*(header.as_ptr() as *const InHeader) };

    Ok((header, remains))
}

fn fetch<'a, T>(buf: &'a [u8]) -> io::Result<&'a T> {
    if buf.len() < mem::size_of::<T>() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(unsafe { &*(buf.as_ptr() as *const T) })
}

#[derive(Debug)]
pub struct Request<'a> {
    pub(crate) in_header: &'a InHeader,
}

impl Request<'_> {
    pub fn nodeid(&self) -> u64 {
        self.in_header.0.nodeid
    }

    pub fn uid(&self) -> u32 {
        self.in_header.0.uid
    }

    pub fn gid(&self) -> u32 {
        self.in_header.0.gid
    }

    pub fn pid(&self) -> u32 {
        self.in_header.0.pid
    }
}
