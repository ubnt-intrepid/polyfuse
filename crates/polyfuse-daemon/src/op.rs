use crate::{
    conn::Writer,
    parse,
    reply::{
        ReplyAttr, ReplyBmap, ReplyCreate, ReplyData, ReplyEntry, ReplyLk, ReplyOk, ReplyOpen,
        ReplyPoll, ReplyStatfs, ReplyWrite, ReplyXattr,
    },
};
use either::Either;
use polyfuse::{
    op::{self, SetAttrTime},
    types::{FileLock, LockOwner, Timespec},
};
use polyfuse_kernel as kernel;
use std::{ffi::OsStr, fmt, io, u32, u64};

pub type Ok = ();

#[derive(Debug)]
pub struct Error(ErrorKind);

#[derive(Debug)]
enum ErrorKind {
    Code(i32),
    Fatal(io::Error),
}

impl Error {
    pub(crate) fn code(&self) -> Option<i32> {
        match self.0 {
            ErrorKind::Code(code) => Some(code),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpError").finish()
    }
}

impl std::error::Error for Error {}

impl polyfuse::error::Error for Error {
    fn from_io_error(io_error: io::Error) -> Self {
        Self(ErrorKind::Fatal(io_error))
    }

    fn from_code(code: i32) -> Self
    where
        Self: Sized,
    {
        Self(ErrorKind::Code(code))
    }
}

pub type Result = std::result::Result<Ok, Error>;

/// The kind of filesystem operation requested by the kernel.
#[non_exhaustive]
pub enum Operation<'req> {
    Lookup(Lookup<'req>),
    Getattr(Getattr<'req>),
    Setattr(Setattr<'req>),
    Readlink(Readlink<'req>),
    Symlink(Symlink<'req>),
    Mknod(Mknod<'req>),
    Mkdir(Mkdir<'req>),
    Unlink(Unlink<'req>),
    Rmdir(Rmdir<'req>),
    Rename(Rename<'req>),
    Link(Link<'req>),
    Open(Open<'req>),
    Read(Read<'req>),
    Write(Write<'req>),
    Release(Release<'req>),
    Statfs(Statfs<'req>),
    Fsync(Fsync<'req>),
    Setxattr(Setxattr<'req>),
    Getxattr(Getxattr<'req>),
    Listxattr(Listxattr<'req>),
    Removexattr(Removexattr<'req>),
    Flush(Flush<'req>),
    Opendir(Opendir<'req>),
    Readdir(Readdir<'req>),
    Readdirplus(Readdirplus<'req>),
    Releasedir(Releasedir<'req>),
    Fsyncdir(Fsyncdir<'req>),
    Getlk(Getlk<'req>),
    Setlk(Setlk<'req>),
    Flock(Flock<'req>),
    Access(Access<'req>),
    Create(Create<'req>),
    Bmap(Bmap<'req>),
    Fallocate(Fallocate<'req>),
    CopyFileRange(CopyFileRange<'req>),
    Poll(Poll<'req>),
}

impl<'req> Operation<'req> {
    /// Annotate that the operation is not supported by the filesystem.
    pub async fn unimplemented(self) -> Result {
        Err(polyfuse::error::code(libc::ENOSYS))
    }
}

pub struct Op<'req, Arg> {
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: Arg,
    pub(crate) writer: &'req Writer,
}

impl<Arg> op::Operation for Op<'_, Arg> {
    type Ok = Ok;
    type Error = Error;

    fn unique(&self) -> u64 {
        self.header.unique
    }

    fn uid(&self) -> u32 {
        self.header.uid
    }

    fn gid(&self) -> u32 {
        self.header.gid
    }

    fn pid(&self) -> u32 {
        self.header.pid
    }

    fn unimplemented(self) -> Result {
        Err(polyfuse::error::code(libc::ENOSYS))
    }
}

pub type Lookup<'req> = Op<'req, parse::Lookup<'req>>;

impl<'req> op::Lookup for Lookup<'req> {
    type Reply = ReplyEntry<'req>;

    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }

    fn reply(self) -> Self::Reply {
        ReplyEntry {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_entry_out::default(),
        }
    }
}

pub type Getattr<'req> = Op<'req, parse::Getattr<'req>>;

impl<'req> op::Getattr for Getattr<'req> {
    type Reply = ReplyAttr<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> Option<u64> {
        if self.arg.arg.getattr_flags & kernel::FUSE_GETATTR_FH != 0 {
            Some(self.arg.arg.fh)
        } else {
            None
        }
    }

    fn reply(self) -> Self::Reply {
        ReplyAttr {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_attr_out::default(),
        }
    }
}

pub type Setattr<'req> = Op<'req, parse::Setattr<'req>>;

impl<'req> Setattr<'req> {
    #[inline(always)]
    fn get<R>(&self, flag: u32, f: impl FnOnce(&kernel::fuse_setattr_in) -> R) -> Option<R> {
        if self.arg.arg.valid & flag != 0 {
            Some(f(&self.arg.arg))
        } else {
            None
        }
    }
}

impl<'req> op::Setattr for Setattr<'req> {
    type Reply = ReplyAttr<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    #[inline]
    fn fh(&self) -> Option<u64> {
        self.get(kernel::FATTR_FH, |arg| arg.fh)
    }

    #[inline]
    fn mode(&self) -> Option<u32> {
        self.get(kernel::FATTR_MODE, |arg| arg.mode)
    }

    #[inline]
    fn uid(&self) -> Option<u32> {
        self.get(kernel::FATTR_UID, |arg| arg.uid)
    }

    #[inline]
    fn gid(&self) -> Option<u32> {
        self.get(kernel::FATTR_GID, |arg| arg.gid)
    }

    #[inline]
    fn size(&self) -> Option<u64> {
        self.get(kernel::FATTR_SIZE, |arg| arg.size)
    }

    #[inline]
    fn atime(&self) -> Option<SetAttrTime> {
        self.get(kernel::FATTR_ATIME, |arg| {
            if arg.valid & kernel::FATTR_ATIME_NOW != 0 {
                SetAttrTime::Now
            } else {
                SetAttrTime::Timespec(Timespec {
                    secs: arg.atime,
                    nsecs: arg.atimensec,
                    ..Default::default()
                })
            }
        })
    }

    #[inline]
    fn mtime(&self) -> Option<SetAttrTime> {
        self.get(kernel::FATTR_MTIME, |arg| {
            if arg.valid & kernel::FATTR_MTIME_NOW != 0 {
                SetAttrTime::Now
            } else {
                SetAttrTime::Timespec(Timespec {
                    secs: arg.mtime,
                    nsecs: arg.mtimensec,
                    ..Default::default()
                })
            }
        })
    }

    #[inline]
    fn ctime(&self) -> Option<Timespec> {
        self.get(kernel::FATTR_CTIME, |arg| Timespec {
            secs: arg.ctime,
            nsecs: arg.ctimensec,
            ..Default::default()
        })
    }

    #[inline]
    fn lock_owner(&self) -> Option<LockOwner> {
        self.get(kernel::FATTR_LOCKOWNER, |arg| {
            LockOwner::from_raw(arg.lock_owner)
        })
    }

    fn reply(self) -> Self::Reply {
        ReplyAttr {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_attr_out::default(),
        }
    }
}

pub type Readlink<'req> = Op<'req, parse::Readlink<'req>>;

impl<'req> op::Readlink for Readlink<'req> {
    type Reply = ReplyData<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn reply(self) -> Self::Reply {
        ReplyData {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Symlink<'req> = Op<'req, parse::Symlink<'req>>;

impl<'req> op::Symlink for Symlink<'req> {
    type Reply = ReplyEntry<'req>;

    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }

    fn link(&self) -> &OsStr {
        self.arg.link
    }

    fn reply(self) -> Self::Reply {
        ReplyEntry {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_entry_out::default(),
        }
    }
}

pub type Mknod<'req> = Op<'req, parse::Mknod<'req>>;

impl<'req> op::Mknod for Mknod<'req> {
    type Reply = ReplyEntry<'req>;

    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }

    fn mode(&self) -> u32 {
        self.arg.arg.mode
    }

    fn rdev(&self) -> u32 {
        self.arg.arg.rdev
    }

    fn umask(&self) -> u32 {
        self.arg.arg.umask
    }

    fn reply(self) -> Self::Reply {
        ReplyEntry {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_entry_out::default(),
        }
    }
}

pub type Mkdir<'req> = Op<'req, parse::Mkdir<'req>>;

impl<'req> op::Mkdir for Mkdir<'req> {
    type Reply = ReplyEntry<'req>;

    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }

    fn mode(&self) -> u32 {
        self.arg.arg.mode
    }

    fn umask(&self) -> u32 {
        self.arg.arg.umask
    }

    fn reply(self) -> Self::Reply {
        ReplyEntry {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_entry_out::default(),
        }
    }
}

pub type Unlink<'req> = Op<'req, parse::Unlink<'req>>;

impl<'req> op::Unlink for Unlink<'req> {
    type Reply = ReplyOk<'req>;

    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }

    fn reply(self) -> Self::Reply {
        ReplyOk {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Rmdir<'req> = Op<'req, parse::Rmdir<'req>>;

impl<'req> op::Rmdir for Rmdir<'req> {
    type Reply = ReplyOk<'req>;

    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }

    fn reply(self) -> Self::Reply {
        ReplyOk {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Rename<'req> = Op<'req, Either<parse::Rename<'req>, parse::Rename2<'req>>>;

impl<'req> op::Rename for Rename<'req> {
    type Reply = ReplyOk<'req>;

    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        match self.arg {
            Either::Left(parse::Rename { name, .. }) => name,
            Either::Right(parse::Rename2 { name, .. }) => name,
        }
    }

    fn newparent(&self) -> u64 {
        match self.arg {
            Either::Left(parse::Rename { arg, .. }) => arg.newdir,
            Either::Right(parse::Rename2 { arg, .. }) => arg.newdir,
        }
    }

    fn newname(&self) -> &OsStr {
        match self.arg {
            Either::Left(parse::Rename { newname, .. }) => newname,
            Either::Right(parse::Rename2 { newname, .. }) => newname,
        }
    }

    fn flags(&self) -> u32 {
        match self.arg {
            Either::Left(..) => 0,
            Either::Right(parse::Rename2 { arg, .. }) => arg.flags,
        }
    }

    fn reply(self) -> Self::Reply {
        ReplyOk {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Link<'req> = Op<'req, parse::Link<'req>>;

impl<'req> op::Link for Link<'req> {
    type Reply = ReplyEntry<'req>;

    fn ino(&self) -> u64 {
        self.arg.arg.oldnodeid
    }

    fn newparent(&self) -> u64 {
        self.header.nodeid
    }

    fn newname(&self) -> &OsStr {
        self.arg.newname
    }

    fn reply(self) -> Self::Reply {
        ReplyEntry {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_entry_out::default(),
        }
    }
}

pub type Open<'req> = Op<'req, parse::Open<'req>>;

impl<'req> op::Open for Open<'req> {
    type Reply = ReplyOpen<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn flags(&self) -> u32 {
        self.arg.arg.flags
    }

    fn reply(self) -> Self::Reply {
        ReplyOpen {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_open_out::default(),
        }
    }
}

pub type Read<'req> = Op<'req, parse::Read<'req>>;

impl<'req> op::Read for Read<'req> {
    type Reply = ReplyData<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn offset(&self) -> u64 {
        self.arg.arg.offset
    }

    fn size(&self) -> u32 {
        self.arg.arg.size
    }

    fn flags(&self) -> u32 {
        self.arg.arg.flags
    }

    fn lock_owner(&self) -> Option<LockOwner> {
        if self.arg.arg.read_flags & kernel::FUSE_READ_LOCKOWNER != 0 {
            Some(LockOwner::from_raw(self.arg.arg.lock_owner))
        } else {
            None
        }
    }

    fn reply(self) -> Self::Reply {
        ReplyData {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Write<'req> = Op<'req, parse::Write<'req>>;

impl<'req> op::Write for Write<'req> {
    type Reply = ReplyWrite<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn offset(&self) -> u64 {
        self.arg.arg.offset
    }

    fn size(&self) -> u32 {
        self.arg.arg.size
    }

    fn flags(&self) -> u32 {
        self.arg.arg.flags
    }

    fn lock_owner(&self) -> Option<LockOwner> {
        if self.arg.arg.write_flags & kernel::FUSE_WRITE_LOCKOWNER != 0 {
            Some(LockOwner::from_raw(self.arg.arg.lock_owner))
        } else {
            None
        }
    }

    fn reply(self) -> Self::Reply {
        ReplyWrite {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_write_out::default(),
        }
    }
}

pub type Release<'req> = Op<'req, parse::Release<'req>>;

impl<'req> op::Release for Release<'req> {
    type Reply = ReplyOk<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn flags(&self) -> u32 {
        self.arg.arg.flags
    }

    fn lock_owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.arg.lock_owner)
    }

    fn flush(&self) -> bool {
        self.arg.arg.release_flags & kernel::FUSE_RELEASE_FLUSH != 0
    }

    fn flock_release(&self) -> bool {
        self.arg.arg.release_flags & kernel::FUSE_RELEASE_FLOCK_UNLOCK != 0
    }

    fn reply(self) -> Self::Reply {
        ReplyOk {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Statfs<'req> = Op<'req, parse::Statfs<'req>>;

impl<'req> op::Statfs for Statfs<'req> {
    type Reply = ReplyStatfs<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn reply(self) -> Self::Reply {
        ReplyStatfs {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_statfs_out::default(),
        }
    }
}

pub type Fsync<'req> = Op<'req, parse::Fsync<'req>>;

impl<'req> op::Fsync for Fsync<'req> {
    type Reply = ReplyOk<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn datasync(&self) -> bool {
        self.arg.arg.fsync_flags & kernel::FUSE_FSYNC_FDATASYNC != 0
    }

    fn reply(self) -> Self::Reply {
        ReplyOk {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Setxattr<'req> = Op<'req, parse::Setxattr<'req>>;

impl<'req> op::Setxattr for Setxattr<'req> {
    type Reply = ReplyOk<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }

    fn value(&self) -> &[u8] {
        self.arg.value
    }

    fn flags(&self) -> u32 {
        self.arg.arg.flags
    }

    fn reply(self) -> Self::Reply {
        ReplyOk {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Getxattr<'req> = Op<'req, parse::Getxattr<'req>>;

impl<'req> op::Getxattr for Getxattr<'req> {
    type Reply = ReplyXattr<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }

    fn size(&self) -> u32 {
        self.arg.arg.size
    }

    fn reply(self) -> Self::Reply {
        ReplyXattr {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_getxattr_out::default(),
        }
    }
}

pub type Listxattr<'req> = Op<'req, parse::Listxattr<'req>>;

impl<'req> op::Listxattr for Listxattr<'req> {
    type Reply = ReplyXattr<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn size(&self) -> u32 {
        self.arg.arg.size
    }

    fn reply(self) -> Self::Reply {
        ReplyXattr {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_getxattr_out::default(),
        }
    }
}

pub type Removexattr<'req> = Op<'req, parse::Removexattr<'req>>;

impl<'req> op::Removexattr for Removexattr<'req> {
    type Reply = ReplyOk<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }

    fn reply(self) -> Self::Reply {
        ReplyOk {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Flush<'req> = Op<'req, parse::Flush<'req>>;

impl<'req> op::Flush for Flush<'req> {
    type Reply = ReplyOk<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn lock_owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.arg.lock_owner)
    }

    fn reply(self) -> Self::Reply {
        ReplyOk {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Opendir<'req> = Op<'req, parse::Opendir<'req>>;

impl<'req> op::Opendir for Opendir<'req> {
    type Reply = ReplyOpen<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn flags(&self) -> u32 {
        self.arg.arg.flags
    }

    fn reply(self) -> Self::Reply {
        ReplyOpen {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_open_out::default(),
        }
    }
}

pub type Readdir<'req> = Op<'req, parse::Readdir<'req>>;

impl<'req> op::Readdir for Readdir<'req> {
    type Reply = ReplyData<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn offset(&self) -> u64 {
        self.arg.arg.offset
    }

    fn size(&self) -> u32 {
        self.arg.arg.size
    }

    fn is_plus(&self) -> bool {
        false
    }

    fn reply(self) -> Self::Reply {
        ReplyData {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Readdirplus<'req> = Op<'req, parse::Readdirplus<'req>>;

impl<'req> op::Readdir for Readdirplus<'req> {
    type Reply = ReplyData<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn offset(&self) -> u64 {
        self.arg.arg.offset
    }

    fn size(&self) -> u32 {
        self.arg.arg.size
    }

    fn is_plus(&self) -> bool {
        true
    }

    fn reply(self) -> Self::Reply {
        ReplyData {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Releasedir<'req> = Op<'req, parse::Releasedir<'req>>;

impl<'req> op::Releasedir for Releasedir<'req> {
    type Reply = ReplyOk<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn flags(&self) -> u32 {
        self.arg.arg.flags
    }

    fn reply(self) -> Self::Reply {
        ReplyOk {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Fsyncdir<'req> = Op<'req, parse::Fsyncdir<'req>>;

impl<'req> op::Fsyncdir for Fsyncdir<'req> {
    type Reply = ReplyOk<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn datasync(&self) -> bool {
        self.arg.arg.fsync_flags & kernel::FUSE_FSYNC_FDATASYNC != 0
    }

    fn reply(self) -> Self::Reply {
        ReplyOk {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Getlk<'req> = Op<'req, parse::Getlk<'req>>;

impl<'req> op::Getlk for Getlk<'req> {
    type Reply = ReplyLk<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.arg.owner)
    }

    fn lk(&self) -> &FileLock {
        &self.arg.lk
    }

    fn reply(self) -> Self::Reply {
        ReplyLk {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_lk_out::default(),
        }
    }
}

pub type Setlk<'req> = Op<'req, parse::Setlk<'req>>;

impl<'req> op::Setlk for Setlk<'req> {
    type Reply = ReplyOk<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.arg.owner)
    }

    fn lk(&self) -> &FileLock {
        &self.arg.lk
    }

    fn sleep(&self) -> bool {
        self.arg.sleep
    }

    fn reply(self) -> Self::Reply {
        ReplyOk {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Flock<'req> = Op<'req, parse::Flock<'req>>;

impl<'req> op::Flock for Flock<'req> {
    type Reply = ReplyOk<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.arg.owner)
    }

    fn op(&self) -> Option<u32> {
        Some(self.arg.op)
    }

    fn reply(self) -> Self::Reply {
        ReplyOk {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Access<'req> = Op<'req, parse::Access<'req>>;

impl<'req> op::Access for Access<'req> {
    type Reply = ReplyOk<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn mask(&self) -> u32 {
        self.arg.arg.mask
    }

    fn reply(self) -> Self::Reply {
        ReplyOk {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type Create<'req> = Op<'req, parse::Create<'req>>;

impl<'req> op::Create for Create<'req> {
    type Reply = ReplyCreate<'req>;

    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }

    fn mode(&self) -> u32 {
        self.arg.arg.mode
    }

    fn open_flags(&self) -> u32 {
        self.arg.arg.flags
    }

    fn umask(&self) -> u32 {
        self.arg.arg.umask
    }

    fn reply(self) -> Self::Reply {
        ReplyCreate {
            writer: self.writer,
            header: self.header,
            arg: Default::default(),
        }
    }
}

pub type Bmap<'req> = Op<'req, parse::Bmap<'req>>;

impl<'req> op::Bmap for Bmap<'req> {
    type Reply = ReplyBmap<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn block(&self) -> u64 {
        self.arg.arg.block
    }

    fn blocksize(&self) -> u32 {
        self.arg.arg.blocksize
    }

    fn reply(self) -> Self::Reply {
        ReplyBmap {
            writer: self.writer,
            header: self.header,
            arg: Default::default(),
        }
    }
}

pub type Fallocate<'req> = Op<'req, parse::Fallocate<'req>>;

impl<'req> op::Fallocate for Fallocate<'req> {
    type Reply = ReplyOk<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn offset(&self) -> u64 {
        self.arg.arg.offset
    }

    fn length(&self) -> u64 {
        self.arg.arg.length
    }

    fn mode(&self) -> u32 {
        self.arg.arg.mode
    }

    fn reply(self) -> Self::Reply {
        ReplyOk {
            writer: self.writer,
            header: self.header,
        }
    }
}

pub type CopyFileRange<'req> = Op<'req, parse::CopyFileRange<'req>>;

impl<'req> op::CopyFileRange for CopyFileRange<'req> {
    type Reply = ReplyWrite<'req>;

    fn ino_in(&self) -> u64 {
        self.header.nodeid
    }

    fn fh_in(&self) -> u64 {
        self.arg.arg.fh_in
    }

    fn offset_in(&self) -> u64 {
        self.arg.arg.off_in
    }

    fn ino_out(&self) -> u64 {
        self.arg.arg.nodeid_out
    }

    fn fh_out(&self) -> u64 {
        self.arg.arg.fh_out
    }

    fn offset_out(&self) -> u64 {
        self.arg.arg.off_out
    }

    fn length(&self) -> u64 {
        self.arg.arg.len
    }

    fn flags(&self) -> u64 {
        self.arg.arg.flags
    }

    fn reply(self) -> Self::Reply {
        ReplyWrite {
            writer: self.writer,
            header: self.header,
            arg: Default::default(),
        }
    }
}

pub type Poll<'req> = Op<'req, parse::Poll<'req>>;

impl<'req> op::Poll for Poll<'req> {
    type Reply = ReplyPoll<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn events(&self) -> u32 {
        self.arg.arg.events
    }

    fn kh(&self) -> Option<u64> {
        if self.arg.arg.flags & kernel::FUSE_POLL_SCHEDULE_NOTIFY != 0 {
            Some(self.arg.arg.kh)
        } else {
            None
        }
    }

    fn reply(self) -> Self::Reply {
        ReplyPoll {
            writer: self.writer,
            header: self.header,
            arg: Default::default(),
        }
    }
}
