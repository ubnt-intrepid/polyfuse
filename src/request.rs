use crate::abi::fuse_opcode::*;
use crate::abi::{
    fuse_flush_in,
    fuse_forget_in,
    fuse_getattr_in, //
    fuse_getxattr_in,
    fuse_in_header,
    fuse_init_in,
    fuse_link_in,
    fuse_mkdir_in,
    fuse_mknod_in,
    fuse_opcode,
    fuse_open_in,
    fuse_read_in,
    fuse_release_in,
    fuse_rename_in,
    fuse_setattr_in,
    fuse_setxattr_in,
};
use bitflags::bitflags;
use std::{ffi::OsStr, fmt, io, mem, os::unix::ffi::OsStrExt};

#[repr(transparent)]
pub struct Header(fuse_in_header);

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Header")
            .field("len", &self.len())
            .field("opcode", &self.opcode())
            .field("unique", &self.unique())
            .field("nodeid", &self.nodeid())
            .field("uid", &self.uid())
            .field("gid", &self.gid())
            .field("pid", &self.pid())
            .finish()
    }
}

impl Header {
    pub fn len(&self) -> u32 {
        self.0.len
    }

    pub fn opcode(&self) -> fuse_opcode {
        unsafe { mem::transmute(self.0.opcode) }
    }

    pub fn unique(&self) -> u64 {
        self.0.unique
    }

    pub fn nodeid(&self) -> u64 {
        self.0.nodeid
    }

    pub fn uid(&self) -> u32 {
        self.0.uid
    }

    pub fn gid(&self) -> u32 {
        self.0.gid
    }

    pub fn pid(&self) -> u32 {
        self.0.pid
    }
}

#[repr(transparent)]
pub struct OpInit(fuse_init_in);

impl fmt::Debug for OpInit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpInit")
            .field("major", &self.major())
            .field("minor", &self.minor())
            .field("max_readahead", &self.max_readahead())
            .field("flags", &self.flags())
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

    pub fn flags(&self) -> CapFlags {
        CapFlags::from_bits_truncate(self.0.flags)
    }
}

bitflags! {
    pub struct CapFlags: u32 {
        const ASYNC_READ = crate::abi::FUSE_ASYNC_READ;
        const POSIX_LOCKS = crate::abi::FUSE_POSIX_LOCKS;
        const FILE_OPS = crate::abi::FUSE_FILE_OPS;
        const ATOMIC_O_TRUNC = crate::abi::FUSE_ATOMIC_O_TRUNC;
        const EXPORT_SUPPORT = crate::abi::FUSE_EXPORT_SUPPORT;
        const BIG_WRITES = crate::abi::FUSE_BIG_WRITES;
        const DONT_MASK = crate::abi::FUSE_DONT_MASK;
        const SPLICE_WRITE = crate::abi::FUSE_SPLICE_WRITE;
        const SPLICE_MOVE = crate::abi::FUSE_SPLICE_MOVE;
        const SPLICE_READ = crate::abi::FUSE_SPLICE_READ;
        const FLOCK_LOCKS = crate::abi::FUSE_FLOCK_LOCKS;
        const HAS_IOCTL_DIR = crate::abi::FUSE_HAS_IOCTL_DIR;
        const AUTO_INVAL_DATA = crate::abi::FUSE_AUTO_INVAL_DATA;
        const DO_READDIRPLUS = crate::abi::FUSE_DO_READDIRPLUS;
        const READDIRPLUS_AUTO = crate::abi::FUSE_READDIRPLUS_AUTO;
        const ASYNC_DIO = crate::abi::FUSE_ASYNC_DIO;
        const WRITEBACK_CACHE = crate::abi::FUSE_WRITEBACK_CACHE;
        const NO_OPEN_SUPPORT = crate::abi::FUSE_NO_OPEN_SUPPORT;
        const PARALLEL_DIROPS = crate::abi::FUSE_PARALLEL_DIROPS;
        const HANDLE_KILLPRIV = crate::abi::FUSE_HANDLE_KILLPRIV;
        const POSIX_ACL = crate::abi:: FUSE_POSIX_ACL;
        const ABORT_ERROR = crate::abi::FUSE_ABORT_ERROR;

        // 7.28
        //const MAX_PAGES = crate::abi::FUSE_MAX_PAGES;
        //const CACHE_SYMLINKS = crate::abi::FUSE_CACHE_SYMLINKS;

        // 7.29
        //const NO_OPENDIR_SUPPORT = crate::abi::FUSE_NO_OPENDIR_SUPPORT;
    }
}

#[repr(transparent)]
pub struct OpForget(fuse_forget_in);

impl fmt::Debug for OpForget {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpForget")
            .field("nlookup", &self.nlookup())
            .finish()
    }
}

impl OpForget {
    pub fn nlookup(&self) -> u64 {
        self.0.nlookup
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
    pub fn flags(&self) -> GetattrFlags {
        GetattrFlags::from_bits_truncate(self.0.getattr_flags)
    }

    pub fn fh(&self) -> u64 {
        self.0.fh
    }
}

bitflags! {
    pub struct GetattrFlags: u32 {
        const FH = crate::abi::FUSE_GETATTR_FH;
    }
}

#[repr(transparent)]
pub struct OpSetattr(fuse_setattr_in);

impl fmt::Debug for OpSetattr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpSetattr")
            .field("valid", &self.valid())
            .field("fh", &self.fh())
            .field("size", &self.size())
            .field("lock_owner", &self.lock_owner())
            .field("atime", &self.atime())
            .field("mtime", &self.mtime())
            .field("ctime", &self.ctime())
            .field("mode", &self.mode())
            .field("uid", &self.uid())
            .field("gid", &self.gid())
            .finish()
    }
}

impl OpSetattr {
    pub fn valid(&self) -> SetattrFlags {
        SetattrFlags::from_bits_truncate(self.0.valid)
    }

    pub fn fh(&self) -> u64 {
        self.0.fh
    }

    pub fn size(&self) -> u64 {
        self.0.size
    }

    pub fn lock_owner(&self) -> u64 {
        self.0.lock_owner
    }

    pub fn atime(&self) -> (u64, u32) {
        (self.0.atime, self.0.atimensec)
    }

    pub fn mtime(&self) -> (u64, u32) {
        (self.0.mtime, self.0.mtimensec)
    }

    pub fn ctime(&self) -> (u64, u32) {
        (self.0.ctime, self.0.ctimensec)
    }

    pub fn mode(&self) -> u32 {
        self.0.mode
    }

    pub fn uid(&self) -> u32 {
        self.0.uid
    }

    pub fn gid(&self) -> u32 {
        self.0.gid
    }
}

bitflags! {
    pub struct SetattrFlags: u32 {
        const MODE = crate::abi::FATTR_MODE;
        const UID = crate::abi::FATTR_UID;
        const GID = crate::abi::FATTR_GID;
        const SIZE = crate::abi::FATTR_SIZE;
        const ATIME = crate::abi::FATTR_ATIME;
        const MTIME = crate::abi::FATTR_MTIME;
        const FH = crate::abi::FATTR_FH;
        const ATIME_NOW = crate::abi::FATTR_ATIME_NOW;
        const MTIME_NOW = crate::abi::FATTR_MTIME_NOW;
        const LOCKOWNER = crate::abi::FATTR_LOCKOWNER;
        const CTIME = crate::abi::FATTR_CTIME;
    }
}

#[repr(transparent)]
pub struct OpMknod(fuse_mknod_in);

impl fmt::Debug for OpMknod {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpMknod")
            .field("mode", &self.mode())
            .field("rdev", &self.rdev())
            .field("umask", &self.umask())
            .finish()
    }
}

impl OpMknod {
    pub fn mode(&self) -> u32 {
        self.0.mode
    }

    pub fn rdev(&self) -> u32 {
        self.0.mode
    }

    pub fn umask(&self) -> u32 {
        self.0.mode
    }
}

#[repr(transparent)]
pub struct OpMkdir(fuse_mkdir_in);

impl fmt::Debug for OpMkdir {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpMkdir")
            .field("mode", &self.mode())
            .field("umask", &self.umask())
            .finish()
    }
}

impl OpMkdir {
    pub fn mode(&self) -> u32 {
        self.0.mode
    }

    pub fn umask(&self) -> u32 {
        self.0.mode
    }
}

#[repr(transparent)]
pub struct OpRename(fuse_rename_in);

impl fmt::Debug for OpRename {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpRename")
            .field("newdir", &self.newdir())
            .finish()
    }
}

impl OpRename {
    pub fn newdir(&self) -> u64 {
        self.0.newdir
    }
}

#[repr(transparent)]
pub struct OpLink(fuse_link_in);

impl fmt::Debug for OpLink {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpLink")
            .field("oldnodeid", &self.oldnodeid())
            .finish()
    }
}

impl OpLink {
    pub fn oldnodeid(&self) -> u64 {
        self.0.oldnodeid
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

    pub fn read_flags(&self) -> ReadFlags {
        ReadFlags::from_bits_truncate(self.0.read_flags)
    }

    pub fn lock_owner(&self) -> u64 {
        self.0.lock_owner
    }

    pub fn flags(&self) -> u32 {
        self.0.flags
    }
}

bitflags! {
    pub struct ReadFlags: u32 {
        const LOCKOWNER = crate::abi::FUSE_READ_LOCKOWNER;
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

    pub fn release_flags(&self) -> ReleaseFlags {
        ReleaseFlags::from_bits_truncate(self.0.release_flags)
    }

    pub fn lock_owner(&self) -> u64 {
        self.0.lock_owner
    }
}

bitflags! {
    pub struct ReleaseFlags: u32 {
        const FLUSH = crate::abi::FUSE_RELEASE_FLUSH;
        const FLOCK_UNLOCK = crate::abi::FUSE_RELEASE_FLOCK_UNLOCK;
    }
}

#[repr(transparent)]
pub struct OpGetxattr(fuse_getxattr_in);

impl fmt::Debug for OpGetxattr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpGetxattr")
            .field("size", &self.size())
            .finish()
    }
}

impl OpGetxattr {
    pub fn size(&self) -> u32 {
        self.0.size
    }
}

#[repr(transparent)]
pub struct OpSetxattr(fuse_setxattr_in);

impl fmt::Debug for OpSetxattr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpSetxattr")
            .field("size", &self.size())
            .field("flags", &self.flags())
            .finish()
    }
}

impl OpSetxattr {
    pub fn size(&self) -> u32 {
        self.0.size
    }

    pub fn flags(&self) -> u32 {
        self.0.flags
    }
}

#[derive(Debug)]
pub enum Op<'a> {
    Init(&'a OpInit),
    Destroy,
    Lookup {
        name: &'a OsStr,
    },
    Forget(&'a OpForget),
    Getattr(&'a OpGetattr),
    Setattr(&'a OpSetattr),
    Readlink,
    Symlink {
        name: &'a OsStr,
        link: &'a OsStr,
    },
    Mknod {
        op: &'a OpMknod,
        name: &'a OsStr,
    },
    Mkdir {
        op: &'a OpMkdir,
        name: &'a OsStr,
    },
    Unlink {
        name: &'a OsStr,
    },
    Rmdir {
        name: &'a OsStr,
    },
    Rename {
        op: &'a OpRename,
        name: &'a OsStr,
        newname: &'a OsStr,
    },
    Link {
        op: &'a OpLink,
        newname: &'a OsStr,
    },
    Open(&'a OpOpen),
    Read(&'a OpRead),
    Flush(&'a OpFlush),
    Release(&'a OpRelease),
    Setxattr {
        op: &'a OpSetxattr,
        name: &'a OsStr,
        value: &'a [u8],
    },
    Getxattr {
        op: &'a OpGetxattr,
        name: &'a OsStr,
    },
    Listxattr {
        op: &'a OpGetxattr,
    },
    Removexattr {
        name: &'a OsStr,
    },

    Unknown {
        opcode: fuse_opcode,
        payload: &'a [u8],
    },
}

pub fn parse<'a>(buf: &'a [u8]) -> io::Result<(&'a Header, Op<'a>)> {
    let (header, payload) = parse_header(buf)?;
    if buf.len() < header.len() as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "received data is too short",
        ));
    }

    let op = match header.opcode() {
        FUSE_INIT => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Init(op)
        }
        FUSE_DESTROY => {
            debug_assert!(payload.is_empty());
            Op::Destroy
        }
        FUSE_LOOKUP => {
            let (name, remains) = fetch_str(payload)?;
            debug_assert!(remains.is_empty());
            Op::Lookup { name }
        }
        FUSE_FORGET => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Forget(op)
        }
        FUSE_GETATTR => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Getattr(op)
        }
        FUSE_SETATTR => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Setattr(op)
        }
        FUSE_READLINK => {
            debug_assert!(payload.is_empty());
            Op::Readlink
        }
        FUSE_SYMLINK => {
            let (name, remains) = fetch_str(payload)?;
            let (link, _remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Op::Symlink { name, link }
        }
        FUSE_MKNOD => {
            let (op, remains) = fetch(payload)?;
            let (name, remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Op::Mknod { op, name }
        }
        FUSE_MKDIR => {
            let (op, remains) = fetch(payload)?;
            let (name, remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Op::Mkdir { op, name }
        }
        FUSE_UNLINK => {
            let (name, remains) = fetch_str(payload)?;
            debug_assert!(remains.is_empty());
            Op::Unlink { name }
        }
        FUSE_RMDIR => {
            let (name, remains) = fetch_str(payload)?;
            debug_assert!(remains.is_empty());
            Op::Rmdir { name }
        }
        FUSE_RENAME => {
            let (op, remains) = fetch(payload)?;
            let (name, remains) = fetch_str(remains)?;
            let (newname, _remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Op::Rename { op, name, newname }
        }
        FUSE_LINK => {
            let (op, remains) = fetch(payload)?;
            let (newname, _remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Op::Link { op, newname }
        }
        FUSE_OPEN => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Open(op)
        }
        FUSE_READ => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Read(op)
        }
        FUSE_FLUSH => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Flush(op)
        }
        FUSE_RELEASE => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Release(op)
        }
        FUSE_SETXATTR => {
            let (op, remains) = fetch(payload)?;
            let (name, value) = fetch_str(remains)?;
            Op::Setxattr { op, name, value }
        }
        FUSE_GETXATTR => {
            let (op, remains) = fetch(payload)?;
            let (name, remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Op::Getxattr { op, name }
        }
        FUSE_LISTXATTR => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Listxattr { op }
        }
        FUSE_REMOVEXATTR => {
            let (name, remains) = fetch_str(payload)?;
            debug_assert!(remains.is_empty());
            Op::Removexattr { name }
        }
        opcode => Op::Unknown { opcode, payload },
    };
    Ok((header, op))
}

fn parse_header<'a>(buf: &'a [u8]) -> io::Result<(&'a Header, &'a [u8])> {
    const IN_HEADER_SIZE: usize = mem::size_of::<Header>();

    if buf.len() < IN_HEADER_SIZE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "in_header"));
    }
    let (header, remains) = buf.split_at(IN_HEADER_SIZE);

    let header = unsafe { &*(header.as_ptr() as *const Header) };

    Ok((header, remains))
}

fn fetch<'a, T>(buf: &'a [u8]) -> io::Result<(&'a T, &'a [u8])> {
    if buf.len() < mem::size_of::<T>() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let (data, remains) = buf.split_at(mem::size_of::<T>());
    Ok((unsafe { &*(data.as_ptr() as *const T) }, remains))
}

fn fetch_str<'a>(buf: &'a [u8]) -> io::Result<(&'a OsStr, &'a [u8])> {
    let pos = buf.iter().position(|&b| b == b'\0');
    let (s, remains) = if let Some(pos) = pos {
        let (s, remains) = buf.split_at(pos);
        let remains = &remains[1..]; // skip '\0'
        (s, remains)
    } else {
        (buf, &[] as &[u8])
    };
    Ok((OsStr::from_bytes(s), remains))
}
