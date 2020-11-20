use crate::{
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
use std::{ffi::OsStr, u32, u64};

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

pub struct Op<'req, Arg> {
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: Arg,
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

    fn lk(&self) -> &FileLock {
        &self.arg.lk
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

    fn lk(&self) -> &FileLock {
        &self.arg.lk
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
