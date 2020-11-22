//! Components used when processing FUSE requests.

use self::parse::Arg;
use crate::{
    conn::Writer,
    op::{self, SetAttrTime},
    reply::{self, EntryOptions, OpenOptions},
    session::Session,
    types::{FileAttr, FsStatistics, LockOwner},
    util::as_bytes,
    write,
};
use either::Either;
use futures::future::Future;
use polyfuse_kernel as kernel;
use std::{ffi::OsStr, fmt, io, sync::Arc, time::Duration};

#[derive(Debug)]
pub struct Error(ErrorKind);

#[derive(Debug)]
enum ErrorKind {
    Code(i32),
    Fatal(io::Error),
}

impl Error {
    fn fatal(err: io::Error) -> Self {
        Self(ErrorKind::Fatal(err))
    }

    fn code(&self) -> Option<i32> {
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

impl crate::reply::Error for Error {
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

/// Context about an incoming FUSE request.
pub struct Request {
    pub(crate) buf: Vec<u8>,
    pub(crate) session: Arc<Session>,
    pub(crate) writer: Writer,
}

impl Request {
    // TODO: add unique(), uid(), gid() and pid()

    /// Process the request with the provided callback.
    pub async fn process<'req, F, Fut>(&'req self, f: F) -> Result<(), Error>
    where
        F: FnOnce(Operation<'req>) -> Fut,
        Fut: Future<Output = Result<Replied, Error>>,
    {
        if self.session.exited() {
            return Ok(());
        }

        let parse::Request { header, arg, .. } =
            parse::Request::parse(&self.buf[..]).map_err(Error::fatal)?;

        let reply_entry = || ReplyEntry {
            writer: &self.writer,
            header,
            arg: kernel::fuse_entry_out::default(),
        };

        let reply_attr = || ReplyAttr {
            writer: &self.writer,
            header,
            arg: kernel::fuse_attr_out::default(),
        };

        let reply_ok = || ReplyOk {
            writer: &self.writer,
            header,
        };

        let reply_data = || ReplyData {
            writer: &self.writer,
            header,
        };

        let reply_open = || ReplyOpen {
            writer: &self.writer,
            header,
            arg: kernel::fuse_open_out::default(),
        };

        let reply_write = || ReplyWrite {
            writer: &self.writer,
            header,
            arg: Default::default(),
        };

        let reply_statfs = || ReplyStatfs {
            writer: &self.writer,
            header,
            arg: Default::default(),
        };

        let reply_xattr = || ReplyXattr {
            writer: &self.writer,
            header,
            arg: Default::default(),
        };

        let reply_create = || ReplyCreate {
            writer: &self.writer,
            header,
            arg: Default::default(),
        };

        let reply_lk = || ReplyLk {
            writer: &self.writer,
            header,
            arg: Default::default(),
        };

        let reply_bmap = || ReplyBmap {
            writer: &self.writer,
            header,
            arg: Default::default(),
        };

        let reply_poll = || ReplyPoll {
            writer: &self.writer,
            header,
            arg: Default::default(),
        };

        macro_rules! dispatch_op {
            ($Op:ident, $arg:expr, $reply:expr) => {
                f(Operation::$Op {
                    op: $Op { header, arg: $arg },
                    reply: $reply,
                })
                .await
            };
        }

        let res = match arg {
            Arg::Lookup(arg) => dispatch_op!(Lookup, arg, reply_entry()),
            Arg::Getattr(arg) => dispatch_op!(Getattr, arg, reply_attr()),
            Arg::Setattr(arg) => dispatch_op!(Setattr, arg, reply_attr()),
            Arg::Readlink(arg) => dispatch_op!(Readlink, arg, reply_data()),
            Arg::Symlink(arg) => dispatch_op!(Symlink, arg, reply_entry()),
            Arg::Mknod(arg) => dispatch_op!(Mknod, arg, reply_entry()),
            Arg::Mkdir(arg) => dispatch_op!(Mkdir, arg, reply_entry()),
            Arg::Unlink(arg) => dispatch_op!(Unlink, arg, reply_ok()),
            Arg::Rmdir(arg) => dispatch_op!(Rmdir, arg, reply_ok()),
            Arg::Rename(arg) => dispatch_op!(Rename, Either::Left(arg), reply_ok()),
            Arg::Rename2(arg) => dispatch_op!(Rename, Either::Right(arg), reply_ok()),
            Arg::Link(arg) => dispatch_op!(Link, arg, reply_entry()),
            Arg::Open(arg) => dispatch_op!(Open, arg, reply_open()),
            Arg::Read(arg) => dispatch_op!(Read, arg, reply_data()),
            Arg::Write(arg) => dispatch_op!(Write, arg, reply_write()),
            Arg::Release(arg) => dispatch_op!(Release, arg, reply_ok()),
            Arg::Statfs(arg) => dispatch_op!(Statfs, arg, reply_statfs()),
            Arg::Fsync(arg) => dispatch_op!(Fsync, arg, reply_ok()),
            Arg::Setxattr(arg) => dispatch_op!(Setxattr, arg, reply_ok()),
            Arg::Getxattr(arg) => dispatch_op!(Getxattr, arg, reply_xattr()),
            Arg::Listxattr(arg) => dispatch_op!(Listxattr, arg, reply_xattr()),
            Arg::Removexattr(arg) => dispatch_op!(Removexattr, arg, reply_ok()),
            Arg::Flush(arg) => dispatch_op!(Flush, arg, reply_ok()),
            Arg::Opendir(arg) => dispatch_op!(Opendir, arg, reply_open()),
            Arg::Readdir(arg) => dispatch_op!(Readdir, arg, reply_data()),
            Arg::Readdirplus(arg) => dispatch_op!(Readdirplus, arg, reply_data()),
            Arg::Releasedir(arg) => dispatch_op!(Releasedir, arg, reply_ok()),
            Arg::Fsyncdir(arg) => dispatch_op!(Fsyncdir, arg, reply_ok()),
            Arg::Getlk(arg) => dispatch_op!(Getlk, arg, reply_lk()),
            Arg::Setlk(arg) => dispatch_op!(Setlk, arg, reply_ok()),
            Arg::Flock(arg) => dispatch_op!(Flock, arg, reply_ok()),
            Arg::Access(arg) => dispatch_op!(Access, arg, reply_ok()),
            Arg::Create(arg) => dispatch_op!(Create, arg, reply_create()),
            Arg::Bmap(arg) => dispatch_op!(Bmap, arg, reply_bmap()),
            Arg::Fallocate(arg) => dispatch_op!(Fallocate, arg, reply_ok()),
            Arg::CopyFileRange(arg) => dispatch_op!(CopyFileRange, arg, reply_write()),
            Arg::Poll(arg) => dispatch_op!(Poll, arg, reply_poll()),
            op => {
                tracing::warn!("unsupported operation: {:?}", op);
                write::send_error(&self.writer, header.unique, libc::ENOSYS)
                    .map_err(Error::fatal)?;
                return Ok(());
            }
        };

        if let Err(err) = res {
            match err.code() {
                Some(code) => {
                    write::send_error(&self.writer, header.unique, code).map_err(Error::fatal)?;
                }
                None => return Err(err),
            }
        }

        Ok(())
    }
}

/// The kind of filesystem operation requested by the kernel.
#[non_exhaustive]
pub enum Operation<'req> {
    Lookup {
        op: Lookup<'req>,
        reply: ReplyEntry<'req>,
    },
    Getattr {
        op: Getattr<'req>,
        reply: ReplyAttr<'req>,
    },
    Setattr {
        op: Setattr<'req>,
        reply: ReplyAttr<'req>,
    },
    Readlink {
        op: Readlink<'req>,
        reply: ReplyData<'req>,
    },
    Symlink {
        op: Symlink<'req>,
        reply: ReplyEntry<'req>,
    },
    Mknod {
        op: Mknod<'req>,
        reply: ReplyEntry<'req>,
    },
    Mkdir {
        op: Mkdir<'req>,
        reply: ReplyEntry<'req>,
    },
    Unlink {
        op: Unlink<'req>,
        reply: ReplyOk<'req>,
    },
    Rmdir {
        op: Rmdir<'req>,
        reply: ReplyOk<'req>,
    },
    Rename {
        op: Rename<'req>,
        reply: ReplyOk<'req>,
    },
    Link {
        op: Link<'req>,
        reply: ReplyEntry<'req>,
    },
    Open {
        op: Open<'req>,
        reply: ReplyOpen<'req>,
    },
    Read {
        op: Read<'req>,
        reply: ReplyData<'req>,
    },
    Write {
        op: Write<'req>,
        reply: ReplyWrite<'req>,
    },
    Release {
        op: Release<'req>,
        reply: ReplyOk<'req>,
    },
    Statfs {
        op: Statfs<'req>,
        reply: ReplyStatfs<'req>,
    },
    Fsync {
        op: Fsync<'req>,
        reply: ReplyOk<'req>,
    },
    Setxattr {
        op: Setxattr<'req>,
        reply: ReplyOk<'req>,
    },
    Getxattr {
        op: Getxattr<'req>,
        reply: ReplyXattr<'req>,
    },
    Listxattr {
        op: Listxattr<'req>,
        reply: ReplyXattr<'req>,
    },
    Removexattr {
        op: Removexattr<'req>,
        reply: ReplyOk<'req>,
    },
    Flush {
        op: Flush<'req>,
        reply: ReplyOk<'req>,
    },
    Opendir {
        op: Opendir<'req>,
        reply: ReplyOpen<'req>,
    },
    Readdir {
        op: Readdir<'req>,
        reply: ReplyData<'req>,
    },
    Readdirplus {
        op: Readdirplus<'req>,
        reply: ReplyData<'req>,
    },
    Releasedir {
        op: Releasedir<'req>,
        reply: ReplyOk<'req>,
    },
    Fsyncdir {
        op: Fsyncdir<'req>,
        reply: ReplyOk<'req>,
    },
    Getlk {
        op: Getlk<'req>,
        reply: ReplyLk<'req>,
    },
    Setlk {
        op: Setlk<'req>,
        reply: ReplyOk<'req>,
    },
    Flock {
        op: Flock<'req>,
        reply: ReplyOk<'req>,
    },
    Access {
        op: Access<'req>,
        reply: ReplyOk<'req>,
    },
    Create {
        op: Create<'req>,
        reply: ReplyCreate<'req>,
    },
    Bmap {
        op: Bmap<'req>,
        reply: ReplyBmap<'req>,
    },
    Fallocate {
        op: Fallocate<'req>,
        reply: ReplyOk<'req>,
    },
    CopyFileRange {
        op: CopyFileRange<'req>,
        reply: ReplyWrite<'req>,
    },
    Poll {
        op: Poll<'req>,
        reply: ReplyPoll<'req>,
    },
}

// ==== operations ====

#[doc(hidden)]
pub struct Op<'req, Arg> {
    header: &'req kernel::fuse_in_header,
    arg: Arg,
}

pub type Lookup<'req> = Op<'req, parse::Lookup<'req>>;

impl<'req> op::Lookup for Lookup<'req> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }
}

pub type Getattr<'req> = Op<'req, parse::Getattr<'req>>;

impl<'req> op::Getattr for Getattr<'req> {
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
                SetAttrTime::Timespec(Duration::new(arg.atime, arg.atimensec))
            }
        })
    }

    #[inline]
    fn mtime(&self) -> Option<SetAttrTime> {
        self.get(kernel::FATTR_MTIME, |arg| {
            if arg.valid & kernel::FATTR_MTIME_NOW != 0 {
                SetAttrTime::Now
            } else {
                SetAttrTime::Timespec(Duration::new(arg.mtime, arg.mtimensec))
            }
        })
    }

    #[inline]
    fn ctime(&self) -> Option<Duration> {
        self.get(kernel::FATTR_CTIME, |arg| {
            Duration::new(arg.ctime, arg.ctimensec)
        })
    }

    #[inline]
    fn lock_owner(&self) -> Option<LockOwner> {
        self.get(kernel::FATTR_LOCKOWNER, |arg| {
            LockOwner::from_raw(arg.lock_owner)
        })
    }
}

pub type Readlink<'req> = Op<'req, parse::Readlink<'req>>;

impl<'req> op::Readlink for Readlink<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }
}

pub type Symlink<'req> = Op<'req, parse::Symlink<'req>>;

impl<'req> op::Symlink for Symlink<'req> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }

    fn link(&self) -> &OsStr {
        self.arg.link
    }
}

pub type Mknod<'req> = Op<'req, parse::Mknod<'req>>;

impl<'req> op::Mknod for Mknod<'req> {
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
}

pub type Mkdir<'req> = Op<'req, parse::Mkdir<'req>>;

impl<'req> op::Mkdir for Mkdir<'req> {
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
}

pub type Unlink<'req> = Op<'req, parse::Unlink<'req>>;

impl<'req> op::Unlink for Unlink<'req> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }
}

pub type Rmdir<'req> = Op<'req, parse::Rmdir<'req>>;

impl<'req> op::Rmdir for Rmdir<'req> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }
}

pub type Rename<'req> = Op<'req, Either<parse::Rename<'req>, parse::Rename2<'req>>>;

impl<'req> op::Rename for Rename<'req> {
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
}

pub type Link<'req> = Op<'req, parse::Link<'req>>;

impl<'req> op::Link for Link<'req> {
    fn ino(&self) -> u64 {
        self.arg.arg.oldnodeid
    }

    fn newparent(&self) -> u64 {
        self.header.nodeid
    }

    fn newname(&self) -> &OsStr {
        self.arg.newname
    }
}

pub type Open<'req> = Op<'req, parse::Open<'req>>;

impl<'req> op::Open for Open<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn flags(&self) -> u32 {
        self.arg.arg.flags
    }
}

pub type Read<'req> = Op<'req, parse::Read<'req>>;

impl<'req> op::Read for Read<'req> {
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
}

pub type Write<'req> = Op<'req, parse::Write<'req>>;

impl<'req> op::Write for Write<'req> {
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
}

pub type Release<'req> = Op<'req, parse::Release<'req>>;

impl<'req> op::Release for Release<'req> {
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
}

pub type Statfs<'req> = Op<'req, parse::Statfs<'req>>;

impl<'req> op::Statfs for Statfs<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }
}

pub type Fsync<'req> = Op<'req, parse::Fsync<'req>>;

impl<'req> op::Fsync for Fsync<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn datasync(&self) -> bool {
        self.arg.arg.fsync_flags & kernel::FUSE_FSYNC_FDATASYNC != 0
    }
}

pub type Setxattr<'req> = Op<'req, parse::Setxattr<'req>>;

impl<'req> op::Setxattr for Setxattr<'req> {
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
}

pub type Getxattr<'req> = Op<'req, parse::Getxattr<'req>>;

impl<'req> op::Getxattr for Getxattr<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }

    fn size(&self) -> u32 {
        self.arg.arg.size
    }
}

pub type Listxattr<'req> = Op<'req, parse::Listxattr<'req>>;

impl<'req> op::Listxattr for Listxattr<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn size(&self) -> u32 {
        self.arg.arg.size
    }
}

pub type Removexattr<'req> = Op<'req, parse::Removexattr<'req>>;

impl<'req> op::Removexattr for Removexattr<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }
}

pub type Flush<'req> = Op<'req, parse::Flush<'req>>;

impl<'req> op::Flush for Flush<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn lock_owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.arg.lock_owner)
    }
}

pub type Opendir<'req> = Op<'req, parse::Opendir<'req>>;

impl<'req> op::Opendir for Opendir<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn flags(&self) -> u32 {
        self.arg.arg.flags
    }
}

pub type Readdir<'req> = Op<'req, parse::Readdir<'req>>;

impl<'req> op::Readdir for Readdir<'req> {
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
}

pub type Readdirplus<'req> = Op<'req, parse::Readdirplus<'req>>;

impl<'req> op::Readdir for Readdirplus<'req> {
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
}

pub type Releasedir<'req> = Op<'req, parse::Releasedir<'req>>;

impl<'req> op::Releasedir for Releasedir<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn flags(&self) -> u32 {
        self.arg.arg.flags
    }
}

pub type Fsyncdir<'req> = Op<'req, parse::Fsyncdir<'req>>;

impl<'req> op::Fsyncdir for Fsyncdir<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn datasync(&self) -> bool {
        self.arg.arg.fsync_flags & kernel::FUSE_FSYNC_FDATASYNC != 0
    }
}

pub type Getlk<'req> = Op<'req, parse::Getlk<'req>>;

impl<'req> op::Getlk for Getlk<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.arg.owner)
    }

    fn typ(&self) -> u32 {
        self.arg.arg.lk.typ
    }

    fn start(&self) -> u64 {
        self.arg.arg.lk.start
    }

    fn end(&self) -> u64 {
        self.arg.arg.lk.end
    }

    fn pid(&self) -> u32 {
        self.arg.arg.lk.pid
    }
}

pub type Setlk<'req> = Op<'req, parse::Setlk<'req>>;

impl<'req> op::Setlk for Setlk<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.arg.fh
    }

    fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.arg.owner)
    }

    fn typ(&self) -> u32 {
        self.arg.arg.lk.typ
    }

    fn start(&self) -> u64 {
        self.arg.arg.lk.start
    }

    fn end(&self) -> u64 {
        self.arg.arg.lk.end
    }

    fn pid(&self) -> u32 {
        self.arg.arg.lk.pid
    }

    fn sleep(&self) -> bool {
        self.arg.sleep
    }
}

pub type Flock<'req> = Op<'req, parse::Flock<'req>>;

impl<'req> op::Flock for Flock<'req> {
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
}

pub type Access<'req> = Op<'req, parse::Access<'req>>;

impl<'req> op::Access for Access<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn mask(&self) -> u32 {
        self.arg.arg.mask
    }
}

pub type Create<'req> = Op<'req, parse::Create<'req>>;

impl<'req> op::Create for Create<'req> {
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
}

pub type Bmap<'req> = Op<'req, parse::Bmap<'req>>;

impl<'req> op::Bmap for Bmap<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn block(&self) -> u64 {
        self.arg.arg.block
    }

    fn blocksize(&self) -> u32 {
        self.arg.arg.blocksize
    }
}

pub type Fallocate<'req> = Op<'req, parse::Fallocate<'req>>;

impl<'req> op::Fallocate for Fallocate<'req> {
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
}

pub type CopyFileRange<'req> = Op<'req, parse::CopyFileRange<'req>>;

impl<'req> op::CopyFileRange for CopyFileRange<'req> {
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
}

pub type Poll<'req> = Op<'req, parse::Poll<'req>>;

impl<'req> op::Poll for Poll<'req> {
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
}

// === reply ====

#[derive(Debug)]
#[must_use]
pub struct Replied(());

pub struct ReplyAttr<'req> {
    writer: &'req Writer,
    header: &'req kernel::fuse_in_header,
    arg: kernel::fuse_attr_out,
}

impl reply::ReplyAttr for ReplyAttr<'_> {
    type Ok = Replied;
    type Error = Error;

    fn attr<T>(mut self, attr: T, ttl: Option<Duration>) -> Result<Self::Ok, Self::Error>
    where
        T: FileAttr,
    {
        fill_attr(attr, &mut self.arg.attr);

        if let Some(ttl) = ttl {
            self.arg.attr_valid = ttl.as_secs();
            self.arg.attr_valid_nsec = ttl.subsec_nanos();
        } else {
            self.arg.attr_valid = u64::MAX;
            self.arg.attr_valid_nsec = u32::MAX;
        }

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::fatal)?;

        Ok(Replied(()))
    }
}

pub struct ReplyEntry<'req> {
    writer: &'req Writer,
    header: &'req kernel::fuse_in_header,
    arg: kernel::fuse_entry_out,
}

impl reply::ReplyEntry for ReplyEntry<'_> {
    type Ok = Replied;
    type Error = Error;

    fn entry<T>(mut self, attr: T, opts: &EntryOptions) -> Result<Self::Ok, Self::Error>
    where
        T: FileAttr,
    {
        fill_attr(attr, &mut self.arg.attr);
        self.arg.nodeid = opts.ino;
        self.arg.generation = opts.generation;

        if let Some(ttl) = opts.ttl_attr {
            self.arg.attr_valid = ttl.as_secs();
            self.arg.attr_valid_nsec = ttl.subsec_nanos();
        } else {
            self.arg.attr_valid = u64::MAX;
            self.arg.attr_valid_nsec = u32::MAX;
        }

        if let Some(ttl) = opts.ttl_entry {
            self.arg.entry_valid = ttl.as_secs();
            self.arg.entry_valid_nsec = ttl.subsec_nanos();
        } else {
            self.arg.entry_valid = u64::MAX;
            self.arg.entry_valid_nsec = u32::MAX;
        }

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::fatal)?;

        Ok(Replied(()))
    }
}

pub struct ReplyOk<'req> {
    writer: &'req Writer,
    header: &'req kernel::fuse_in_header,
}

impl reply::ReplyOk for ReplyOk<'_> {
    type Ok = Replied;
    type Error = Error;

    fn ok(self) -> Result<Self::Ok, Self::Error> {
        write::send_reply(self.writer, self.header.unique, &[]).map_err(Error::fatal)?;
        Ok(Replied(()))
    }
}

pub struct ReplyData<'req> {
    writer: &'req Writer,
    header: &'req kernel::fuse_in_header,
}

impl reply::ReplyData for ReplyData<'_> {
    type Ok = Replied;
    type Error = Error;

    fn data<T>(self, data: T) -> Result<Self::Ok, Self::Error>
    where
        T: AsRef<[u8]>,
    {
        write::send_reply(self.writer, self.header.unique, data.as_ref()).map_err(Error::fatal)?;

        Ok(Replied(()))
    }
}

pub struct ReplyOpen<'req> {
    writer: &'req Writer,
    header: &'req kernel::fuse_in_header,
    arg: kernel::fuse_open_out,
}

impl reply::ReplyOpen for ReplyOpen<'_> {
    type Ok = Replied;
    type Error = Error;

    fn open(mut self, fh: u64, opts: &OpenOptions) -> Result<Self::Ok, Self::Error> {
        self.arg.fh = fh;

        if opts.direct_io {
            self.arg.open_flags |= kernel::FOPEN_DIRECT_IO;
        }

        if opts.keep_cache {
            self.arg.open_flags |= kernel::FOPEN_KEEP_CACHE;
        }

        if opts.nonseekable {
            self.arg.open_flags |= kernel::FOPEN_NONSEEKABLE;
        }

        if opts.cache_dir {
            self.arg.open_flags |= kernel::FOPEN_CACHE_DIR;
        }

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::fatal)?;

        Ok(Replied(()))
    }
}

pub struct ReplyWrite<'req> {
    writer: &'req Writer,
    header: &'req kernel::fuse_in_header,
    arg: kernel::fuse_write_out,
}

impl reply::ReplyWrite for ReplyWrite<'_> {
    type Ok = Replied;
    type Error = Error;

    fn size(mut self, size: u32) -> Result<Self::Ok, Self::Error> {
        self.arg.size = size;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::fatal)?;

        Ok(Replied(()))
    }
}

pub struct ReplyStatfs<'req> {
    writer: &'req Writer,
    header: &'req kernel::fuse_in_header,
    arg: kernel::fuse_statfs_out,
}

impl reply::ReplyStatfs for ReplyStatfs<'_> {
    type Ok = Replied;
    type Error = Error;

    fn stat<S>(mut self, stat: S) -> Result<Self::Ok, Self::Error>
    where
        S: FsStatistics,
    {
        fill_statfs(stat, &mut self.arg.st);

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::fatal)?;

        Ok(Replied(()))
    }
}

pub struct ReplyXattr<'req> {
    writer: &'req Writer,
    header: &'req kernel::fuse_in_header,
    arg: kernel::fuse_getxattr_out,
}

impl reply::ReplyXattr for ReplyXattr<'_> {
    type Ok = Replied;
    type Error = Error;

    fn size(mut self, size: u32) -> Result<Self::Ok, Self::Error> {
        self.arg.size = size;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::fatal)?;

        Ok(Replied(()))
    }

    fn data<T>(self, data: T) -> Result<Self::Ok, Self::Error>
    where
        T: AsRef<[u8]>,
    {
        write::send_reply(self.writer, self.header.unique, data.as_ref()).map_err(Error::fatal)?;

        Ok(Replied(()))
    }
}

pub struct ReplyLk<'req> {
    writer: &'req Writer,
    header: &'req kernel::fuse_in_header,
    arg: kernel::fuse_lk_out,
}

impl reply::ReplyLk for ReplyLk<'_> {
    type Ok = Replied;
    type Error = Error;

    fn lk(mut self, typ: u32, start: u64, end: u64, pid: u32) -> Result<Self::Ok, Self::Error> {
        self.arg.lk.typ = typ;
        self.arg.lk.start = start;
        self.arg.lk.end = end;
        self.arg.lk.pid = pid;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::fatal)?;

        Ok(Replied(()))
    }
}

pub struct ReplyCreate<'req> {
    writer: &'req Writer,
    header: &'req kernel::fuse_in_header,
    arg: CreateArg,
}

#[derive(Default)]
#[repr(C)]
struct CreateArg {
    entry_out: kernel::fuse_entry_out,
    open_out: kernel::fuse_open_out,
}

impl reply::ReplyCreate for ReplyCreate<'_> {
    type Ok = Replied;
    type Error = Error;

    fn create<T>(
        mut self,
        fh: u64,
        attr: T,
        entry_opts: &EntryOptions,
        open_opts: &OpenOptions,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: FileAttr,
    {
        fill_attr(attr, &mut self.arg.entry_out.attr);
        self.arg.entry_out.nodeid = entry_opts.ino;
        self.arg.entry_out.generation = entry_opts.generation;

        if let Some(ttl) = entry_opts.ttl_attr {
            self.arg.entry_out.attr_valid = ttl.as_secs();
            self.arg.entry_out.attr_valid_nsec = ttl.subsec_nanos();
        } else {
            self.arg.entry_out.attr_valid = u64::MAX;
            self.arg.entry_out.attr_valid_nsec = u32::MAX;
        }

        if let Some(ttl) = entry_opts.ttl_entry {
            self.arg.entry_out.entry_valid = ttl.as_secs();
            self.arg.entry_out.entry_valid_nsec = ttl.subsec_nanos();
        } else {
            self.arg.entry_out.entry_valid = u64::MAX;
            self.arg.entry_out.entry_valid_nsec = u32::MAX;
        }

        self.arg.open_out.fh = fh;

        if open_opts.direct_io {
            self.arg.open_out.open_flags |= kernel::FOPEN_DIRECT_IO;
        }

        if open_opts.keep_cache {
            self.arg.open_out.open_flags |= kernel::FOPEN_KEEP_CACHE;
        }

        if open_opts.nonseekable {
            self.arg.open_out.open_flags |= kernel::FOPEN_NONSEEKABLE;
        }

        if open_opts.cache_dir {
            self.arg.open_out.open_flags |= kernel::FOPEN_CACHE_DIR;
        }

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::fatal)?;

        Ok(Replied(()))
    }
}

pub struct ReplyBmap<'req> {
    writer: &'req Writer,
    header: &'req kernel::fuse_in_header,
    arg: kernel::fuse_bmap_out,
}

impl reply::ReplyBmap for ReplyBmap<'_> {
    type Ok = Replied;
    type Error = Error;

    fn block(mut self, block: u64) -> Result<Self::Ok, Self::Error> {
        self.arg.block = block;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::fatal)?;

        Ok(Replied(()))
    }
}

pub struct ReplyPoll<'req> {
    writer: &'req Writer,
    header: &'req kernel::fuse_in_header,
    arg: kernel::fuse_poll_out,
}

impl reply::ReplyPoll for ReplyPoll<'_> {
    type Ok = Replied;
    type Error = Error;

    fn revents(mut self, revents: u32) -> Result<Self::Ok, Self::Error> {
        self.arg.revents = revents;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::fatal)?;

        Ok(Replied(()))
    }
}

fn fill_attr(src: impl FileAttr, dst: &mut kernel::fuse_attr) {
    dst.ino = src.ino();
    dst.size = src.size();
    dst.mode = src.mode();
    dst.nlink = src.nlink();
    dst.uid = src.uid();
    dst.gid = src.gid();
    dst.rdev = src.rdev();
    dst.blksize = src.blksize();
    dst.blocks = src.blocks();
    dst.atime = src.atime();
    dst.atimensec = src.atime_nsec();
    dst.mtime = src.mtime();
    dst.mtimensec = src.mtime_nsec();
    dst.ctime = src.ctime();
    dst.ctimensec = src.ctime_nsec();
}

fn fill_statfs(src: impl FsStatistics, dst: &mut kernel::fuse_kstatfs) {
    dst.bsize = src.bsize();
    dst.frsize = src.frsize();
    dst.blocks = src.blocks();
    dst.bfree = src.bfree();
    dst.bavail = src.bavail();
    dst.files = src.files();
    dst.ffree = src.ffree();
    dst.namelen = src.namelen();
}

pub(crate) mod parse {
    use polyfuse_kernel::{self as kernel, fuse_opcode};
    use std::{
        convert::TryFrom, ffi::OsStr, fmt, io, marker::PhantomData, mem, os::unix::prelude::*,
    };

    pub struct Request<'a> {
        pub header: &'a kernel::fuse_in_header,
        pub arg: Arg<'a>,
    }

    impl<'a> Request<'a> {
        pub fn parse(mut buf: &'a [u8]) -> io::Result<Self> {
            if buf.len() < mem::size_of::<kernel::fuse_in_header>() {
                return Err(io::Error::new(io::ErrorKind::Other, "request is too short"));
            }

            let header = unsafe { &*(buf.as_ptr().cast::<kernel::fuse_in_header>()) };
            buf = &buf[mem::size_of::<kernel::fuse_in_header>()..];

            let arg = Parser::new(header, buf).parse()?;

            Ok(Self { header, arg })
        }
    }

    pub enum Arg<'a> {
        Init {
            arg: &'a kernel::fuse_init_in,
        },
        Destroy,
        Forget {
            arg: &'a kernel::fuse_forget_in,
        },
        BatchForget {
            forgets: &'a [kernel::fuse_forget_one],
        },
        Interrupt(Interrupt<'a>),
        NotifyReply(NotifyReply<'a>),
        Lookup(Lookup<'a>),
        Getattr(Getattr<'a>),
        Setattr(Setattr<'a>),
        Readlink(Readlink<'a>),
        Symlink(Symlink<'a>),
        Mknod(Mknod<'a>),
        Mkdir(Mkdir<'a>),
        Unlink(Unlink<'a>),
        Rmdir(Rmdir<'a>),
        Rename(Rename<'a>),
        Rename2(Rename2<'a>),
        Link(Link<'a>),
        Open(Open<'a>),
        Read(Read<'a>),
        Write(Write<'a>),
        Release(Release<'a>),
        Statfs(Statfs<'a>),
        Fsync(Fsync<'a>),
        Setxattr(Setxattr<'a>),
        Getxattr(Getxattr<'a>),
        Listxattr(Listxattr<'a>),
        Removexattr(Removexattr<'a>),
        Flush(Flush<'a>),
        Opendir(Opendir<'a>),
        Readdir(Readdir<'a>),
        Readdirplus(Readdirplus<'a>),
        Releasedir(Releasedir<'a>),
        Fsyncdir(Fsyncdir<'a>),
        Getlk(Getlk<'a>),
        Setlk(Setlk<'a>),
        Flock(Flock<'a>),
        Access(Access<'a>),
        Create(Create<'a>),
        Bmap(Bmap<'a>),
        Fallocate(Fallocate<'a>),
        CopyFileRange(CopyFileRange<'a>),
        Poll(Poll<'a>),

        Unknown,
    }

    impl fmt::Debug for Arg<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Arg").finish()
        }
    }

    pub struct Interrupt<'a> {
        pub arg: &'a kernel::fuse_interrupt_in,
    }

    pub struct NotifyReply<'a> {
        pub arg: &'a kernel::fuse_notify_retrieve_in,
    }

    pub struct Lookup<'a> {
        pub name: &'a OsStr,
    }

    pub struct Getattr<'a> {
        pub arg: &'a kernel::fuse_getattr_in,
    }

    pub struct Setattr<'a> {
        pub arg: &'a kernel::fuse_setattr_in,
    }

    pub struct Readlink<'a> {
        _marker: PhantomData<&'a ()>,
    }

    pub struct Symlink<'a> {
        pub name: &'a OsStr,
        pub link: &'a OsStr,
    }

    pub struct Mknod<'a> {
        pub arg: &'a kernel::fuse_mknod_in,
        pub name: &'a OsStr,
    }

    pub struct Mkdir<'a> {
        pub arg: &'a kernel::fuse_mkdir_in,
        pub name: &'a OsStr,
    }

    pub struct Unlink<'a> {
        pub name: &'a OsStr,
    }

    pub struct Rmdir<'a> {
        pub name: &'a OsStr,
    }

    pub struct Rename<'a> {
        pub arg: &'a kernel::fuse_rename_in,
        pub name: &'a OsStr,
        pub newname: &'a OsStr,
    }

    pub struct Rename2<'a> {
        pub arg: &'a kernel::fuse_rename2_in,
        pub name: &'a OsStr,
        pub newname: &'a OsStr,
    }

    pub struct Link<'a> {
        pub arg: &'a kernel::fuse_link_in,
        pub newname: &'a OsStr,
    }

    pub struct Open<'a> {
        pub arg: &'a kernel::fuse_open_in,
    }

    pub struct Read<'a> {
        pub arg: &'a kernel::fuse_read_in,
    }

    pub struct Write<'a> {
        pub arg: &'a kernel::fuse_write_in,
    }

    pub struct Release<'a> {
        pub arg: &'a kernel::fuse_release_in,
    }

    pub struct Statfs<'a> {
        _marker: PhantomData<&'a ()>,
    }

    pub struct Fsync<'a> {
        pub arg: &'a kernel::fuse_fsync_in,
    }

    pub struct Setxattr<'a> {
        pub arg: &'a kernel::fuse_setxattr_in,
        pub name: &'a OsStr,
        pub value: &'a [u8],
    }

    pub struct Getxattr<'a> {
        pub arg: &'a kernel::fuse_getxattr_in,
        pub name: &'a OsStr,
    }

    pub struct Listxattr<'a> {
        pub arg: &'a kernel::fuse_getxattr_in,
    }

    pub struct Removexattr<'a> {
        pub name: &'a OsStr,
    }

    pub struct Flush<'a> {
        pub arg: &'a kernel::fuse_flush_in,
    }

    pub struct Opendir<'a> {
        pub arg: &'a kernel::fuse_open_in,
    }

    pub struct Readdir<'a> {
        pub arg: &'a kernel::fuse_read_in,
    }

    pub struct Readdirplus<'a> {
        pub arg: &'a kernel::fuse_read_in,
    }

    pub struct Releasedir<'a> {
        pub arg: &'a kernel::fuse_release_in,
    }

    pub struct Fsyncdir<'a> {
        pub arg: &'a kernel::fuse_fsync_in,
    }

    pub struct Getlk<'a> {
        pub arg: &'a kernel::fuse_lk_in,
    }

    pub struct Setlk<'a> {
        pub arg: &'a kernel::fuse_lk_in,
        pub sleep: bool,
    }

    pub struct Flock<'a> {
        pub arg: &'a kernel::fuse_lk_in,
        pub op: u32,
    }

    pub struct Access<'a> {
        pub arg: &'a kernel::fuse_access_in,
    }

    pub struct Create<'a> {
        pub arg: &'a kernel::fuse_create_in,
        pub name: &'a OsStr,
    }

    pub struct Bmap<'a> {
        pub arg: &'a kernel::fuse_bmap_in,
    }

    pub struct Fallocate<'a> {
        pub arg: &'a kernel::fuse_fallocate_in,
    }

    pub struct CopyFileRange<'a> {
        pub arg: &'a kernel::fuse_copy_file_range_in,
    }

    pub struct Poll<'a> {
        pub arg: &'a kernel::fuse_poll_in,
    }

    // ==== parser ====

    struct Parser<'a> {
        header: &'a kernel::fuse_in_header,
        bytes: &'a [u8],
        offset: usize,
    }

    impl<'a> Parser<'a> {
        fn new(header: &'a kernel::fuse_in_header, bytes: &'a [u8]) -> Self {
            Self {
                header,
                bytes,
                offset: 0,
            }
        }

        fn fetch_bytes(&mut self, count: usize) -> io::Result<&'a [u8]> {
            if self.bytes.len() < self.offset + count {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "fetch"));
            }
            let bytes = &self.bytes[self.offset..self.offset + count];
            self.offset += count;
            Ok(bytes)
        }

        fn fetch_array<T>(&mut self, count: usize) -> io::Result<&'a [T]> {
            self.fetch_bytes(mem::size_of::<T>() * count)
                .map(|bytes| unsafe {
                    std::slice::from_raw_parts(bytes.as_ptr() as *const T, count)
                })
        }

        fn fetch_str(&mut self) -> io::Result<&'a OsStr> {
            let len = self.bytes[self.offset..]
                .iter()
                .position(|&b| b == b'\0')
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "fetch_str: missing \\0")
                })?;
            self.fetch_bytes(len).map(|s| {
                self.offset = std::cmp::min(self.bytes.len(), self.offset + 1);
                OsStr::from_bytes(s)
            })
        }

        fn fetch<T>(&mut self) -> io::Result<&'a T> {
            self.fetch_bytes(mem::size_of::<T>())
                .map(|data| unsafe { &*(data.as_ptr() as *const T) })
        }

        fn parse(&mut self) -> io::Result<Arg<'a>> {
            let header = self.header;
            match fuse_opcode::try_from(header.opcode).ok() {
                Some(fuse_opcode::FUSE_INIT) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Init { arg })
                }
                Some(fuse_opcode::FUSE_DESTROY) => Ok(Arg::Destroy),
                Some(fuse_opcode::FUSE_FORGET) => {
                    let arg = self.fetch::<kernel::fuse_forget_in>()?;
                    Ok(Arg::Forget { arg })
                }
                Some(fuse_opcode::FUSE_BATCH_FORGET) => {
                    let arg = self.fetch::<kernel::fuse_batch_forget_in>()?;
                    let forgets = self.fetch_array(arg.count as usize)?;
                    Ok(Arg::BatchForget { forgets })
                }
                Some(fuse_opcode::FUSE_INTERRUPT) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Interrupt(Interrupt { arg }))
                }
                Some(fuse_opcode::FUSE_NOTIFY_REPLY) => {
                    let arg = self.fetch()?;
                    Ok(Arg::NotifyReply(NotifyReply { arg }))
                }

                Some(fuse_opcode::FUSE_LOOKUP) => {
                    let name = self.fetch_str()?;
                    Ok(Arg::Lookup(Lookup { name }))
                }
                Some(fuse_opcode::FUSE_GETATTR) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Getattr(Getattr { arg }))
                }
                Some(fuse_opcode::FUSE_SETATTR) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Setattr(Setattr { arg }))
                }
                Some(fuse_opcode::FUSE_READLINK) => Ok(Arg::Readlink(Readlink {
                    _marker: PhantomData,
                })),
                Some(fuse_opcode::FUSE_SYMLINK) => {
                    let name = self.fetch_str()?;
                    let link = self.fetch_str()?;
                    Ok(Arg::Symlink(Symlink { name, link }))
                }
                Some(fuse_opcode::FUSE_MKNOD) => {
                    let arg = self.fetch()?;
                    let name = self.fetch_str()?;
                    Ok(Arg::Mknod(Mknod { arg, name }))
                }
                Some(fuse_opcode::FUSE_MKDIR) => {
                    let arg = self.fetch()?;
                    let name = self.fetch_str()?;
                    Ok(Arg::Mkdir(Mkdir { arg, name }))
                }
                Some(fuse_opcode::FUSE_UNLINK) => {
                    let name = self.fetch_str()?;
                    Ok(Arg::Unlink(Unlink { name }))
                }
                Some(fuse_opcode::FUSE_RMDIR) => {
                    let name = self.fetch_str()?;
                    Ok(Arg::Rmdir(Rmdir { name }))
                }

                Some(fuse_opcode::FUSE_RENAME) => {
                    let arg = self.fetch()?;
                    let name = self.fetch_str()?;
                    let newname = self.fetch_str()?;
                    Ok(Arg::Rename(Rename { arg, name, newname }))
                }
                Some(fuse_opcode::FUSE_LINK) => {
                    let arg = self.fetch()?;
                    let newname = self.fetch_str()?;
                    Ok(Arg::Link(Link { arg, newname }))
                }
                Some(fuse_opcode::FUSE_OPEN) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Open(Open { arg }))
                }
                Some(fuse_opcode::FUSE_READ) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Read(Read { arg }))
                }
                Some(fuse_opcode::FUSE_WRITE) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Write(Write { arg }))
                }
                Some(fuse_opcode::FUSE_RELEASE) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Release(Release { arg }))
                }
                Some(fuse_opcode::FUSE_STATFS) => Ok(Arg::Statfs(Statfs {
                    _marker: PhantomData,
                })),
                Some(fuse_opcode::FUSE_FSYNC) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Fsync(Fsync { arg }))
                }
                Some(fuse_opcode::FUSE_SETXATTR) => {
                    let arg = self.fetch::<kernel::fuse_setxattr_in>()?;
                    let name = self.fetch_str()?;
                    let value = self.fetch_bytes(arg.size as usize)?;
                    Ok(Arg::Setxattr(Setxattr { arg, name, value }))
                }
                Some(fuse_opcode::FUSE_GETXATTR) => {
                    let arg = self.fetch()?;
                    let name = self.fetch_str()?;
                    Ok(Arg::Getxattr(Getxattr { arg, name }))
                }
                Some(fuse_opcode::FUSE_LISTXATTR) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Listxattr(Listxattr { arg }))
                }
                Some(fuse_opcode::FUSE_REMOVEXATTR) => {
                    let name = self.fetch_str()?;
                    Ok(Arg::Removexattr(Removexattr { name }))
                }
                Some(fuse_opcode::FUSE_FLUSH) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Flush(Flush { arg }))
                }
                Some(fuse_opcode::FUSE_OPENDIR) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Opendir(Opendir { arg }))
                }
                Some(fuse_opcode::FUSE_READDIR) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Readdir(Readdir { arg }))
                }
                Some(fuse_opcode::FUSE_RELEASEDIR) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Releasedir(Releasedir { arg }))
                }
                Some(fuse_opcode::FUSE_FSYNCDIR) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Fsyncdir(Fsyncdir { arg }))
                }
                Some(fuse_opcode::FUSE_GETLK) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Getlk(Getlk { arg }))
                }
                Some(fuse_opcode::FUSE_SETLK) => {
                    let arg = self.fetch()?;
                    parse_lock_arg(arg, false)
                }
                Some(fuse_opcode::FUSE_SETLKW) => {
                    let arg = self.fetch()?;
                    parse_lock_arg(arg, true)
                }
                Some(fuse_opcode::FUSE_ACCESS) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Access(Access { arg }))
                }
                Some(fuse_opcode::FUSE_CREATE) => {
                    let arg = self.fetch()?;
                    let name = self.fetch_str()?;
                    Ok(Arg::Create(Create { arg, name }))
                }
                Some(fuse_opcode::FUSE_BMAP) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Bmap(Bmap { arg }))
                }
                Some(fuse_opcode::FUSE_FALLOCATE) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Fallocate(Fallocate { arg }))
                }
                Some(fuse_opcode::FUSE_READDIRPLUS) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Readdirplus(Readdirplus { arg }))
                }
                Some(fuse_opcode::FUSE_RENAME2) => {
                    let arg = self.fetch()?;
                    let name = self.fetch_str()?;
                    let newname = self.fetch_str()?;
                    Ok(Arg::Rename2(Rename2 { arg, name, newname }))
                }
                Some(fuse_opcode::FUSE_COPY_FILE_RANGE) => {
                    let arg = self.fetch()?;
                    Ok(Arg::CopyFileRange(CopyFileRange { arg }))
                }
                Some(fuse_opcode::FUSE_POLL) => {
                    let arg = self.fetch()?;
                    Ok(Arg::Poll(Poll { arg }))
                }
                _ => Ok(Arg::Unknown),
            }
        }
    }

    fn parse_lock_arg(arg: &kernel::fuse_lk_in, sleep: bool) -> io::Result<Arg<'_>> {
        if arg.lk_flags & kernel::FUSE_LK_FLOCK == 0 {
            // POSIX file lock.
            return Ok(Arg::Setlk(Setlk { arg, sleep }));
        }

        const F_RDLCK: u32 = libc::F_RDLCK as u32;
        const F_WRLCK: u32 = libc::F_WRLCK as u32;
        const F_UNLCK: u32 = libc::F_UNLCK as u32;

        let mut op = match arg.lk.typ {
            F_RDLCK => libc::LOCK_SH as u32,
            F_WRLCK => libc::LOCK_EX as u32,
            F_UNLCK => libc::LOCK_UN as u32,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "unknown lock operation is specified",
                ))
            }
        };

        if !sleep {
            op |= libc::LOCK_NB as u32;
        }

        Ok(Arg::Flock(Flock { arg, op }))
    }

    #[cfg(test)]
    #[allow(clippy::cast_possible_truncation)]
    mod tests {
        use super::*;
        use std::ffi::CString;

        #[test]
        fn parse_lookup() {
            let name = CString::new("foo").unwrap();
            let parent = 1;

            let header = kernel::fuse_in_header {
                len: (mem::size_of::<kernel::fuse_in_header>() + name.as_bytes_with_nul().len())
                    as u32,
                opcode: kernel::FUSE_LOOKUP,
                unique: 2,
                nodeid: parent,
                uid: 1,
                gid: 1,
                pid: 42,
                padding: 0,
            };
            let mut payload = vec![];
            payload.extend_from_slice(name.as_bytes_with_nul());

            let mut parser = Parser::new(&header, &payload[..]);
            let op = parser.parse().unwrap();
            match op {
                Arg::Lookup(op) => {
                    assert_eq!(op.name.as_bytes(), name.as_bytes());
                }
                _ => panic!("incorret operation is returned"),
            }
        }

        #[test]
        fn parse_symlink() {
            let name = CString::new("foo").unwrap();
            let link = CString::new("bar").unwrap();
            let parent = 1;

            let header = kernel::fuse_in_header {
                len: (mem::size_of::<kernel::fuse_in_header>()
                    + name.as_bytes_with_nul().len()
                    + link.as_bytes_with_nul().len()) as u32,
                opcode: kernel::FUSE_SYMLINK,
                unique: 2,
                nodeid: parent,
                uid: 1,
                gid: 1,
                pid: 42,
                padding: 0,
            };
            let mut payload = vec![];
            payload.extend_from_slice(name.as_bytes_with_nul());
            payload.extend_from_slice(link.as_bytes_with_nul());

            let mut parser = Parser::new(&header, &payload[..]);
            let op = parser.parse().unwrap();
            match op {
                Arg::Symlink(op) => {
                    assert_eq!(op.name.as_bytes(), name.as_bytes());
                    assert_eq!(op.link.as_bytes(), link.as_bytes());
                }
                _ => panic!("incorret operation is returned"),
            }
        }
    }
}
