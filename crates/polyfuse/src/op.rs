use crate::{
    parse,
    reply::{
        ReplyAttr, ReplyBmap, ReplyCreate, ReplyData, ReplyEntry, ReplyLk, ReplyOk, ReplyOpen,
        ReplyPoll, ReplyStatfs, ReplyWrite, ReplyXattr,
    },
    types::{FileLock, LockOwner, Timespec},
};
use either::Either;
use polyfuse_kernel as kernel;
use std::{ffi::OsStr, time::SystemTime, u32, u64};

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

impl<'req> traits::Lookup for Lookup<'req> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }
}

pub type Getattr<'req> = Op<'req, parse::Getattr<'req>>;

impl<'req> traits::Getattr for Getattr<'req> {
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

impl<'req> traits::Setattr for Setattr<'req> {
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

impl<'req> traits::Readlink for Readlink<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }
}

pub type Symlink<'req> = Op<'req, parse::Symlink<'req>>;

impl<'req> traits::Symlink for Symlink<'req> {
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

impl<'req> traits::Mknod for Mknod<'req> {
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

impl<'req> traits::Mkdir for Mkdir<'req> {
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

impl<'req> traits::Unlink for Unlink<'req> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }
}

pub type Rmdir<'req> = Op<'req, parse::Rmdir<'req>>;

impl<'req> traits::Rmdir for Rmdir<'req> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }
}

pub type Rename<'req> = Op<'req, Either<parse::Rename<'req>, parse::Rename2<'req>>>;

impl<'req> traits::Rename for Rename<'req> {
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

impl<'req> traits::Link for Link<'req> {
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

impl<'req> traits::Open for Open<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn flags(&self) -> u32 {
        self.arg.arg.flags
    }
}

pub type Read<'req> = Op<'req, parse::Read<'req>>;

impl<'req> traits::Read for Read<'req> {
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

impl<'req> traits::Write for Write<'req> {
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

impl<'req> traits::Release for Release<'req> {
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

impl<'req> traits::Statfs for Statfs<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }
}

pub type Fsync<'req> = Op<'req, parse::Fsync<'req>>;

impl<'req> traits::Fsync for Fsync<'req> {
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

impl<'req> traits::Setxattr for Setxattr<'req> {
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

impl<'req> traits::Getxattr for Getxattr<'req> {
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

impl<'req> traits::Listxattr for Listxattr<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn size(&self) -> u32 {
        self.arg.arg.size
    }
}

pub type Removexattr<'req> = Op<'req, parse::Removexattr<'req>>;

impl<'req> traits::Removexattr for Removexattr<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }
}

pub type Flush<'req> = Op<'req, parse::Flush<'req>>;

impl<'req> traits::Flush for Flush<'req> {
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

impl<'req> traits::Opendir for Opendir<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn flags(&self) -> u32 {
        self.arg.arg.flags
    }
}

pub type Readdir<'req> = Op<'req, parse::Readdir<'req>>;

impl<'req> traits::Readdir for Readdir<'req> {
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

impl<'req> traits::Readdir for Readdirplus<'req> {
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

impl<'req> traits::Releasedir for Releasedir<'req> {
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

impl<'req> traits::Fsyncdir for Fsyncdir<'req> {
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

impl<'req> traits::Getlk for Getlk<'req> {
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

impl<'req> traits::Setlk for Setlk<'req> {
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

impl<'req> traits::Flock for Flock<'req> {
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

impl<'req> traits::Access for Access<'req> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn mask(&self) -> u32 {
        self.arg.arg.mask
    }
}

pub type Create<'req> = Op<'req, parse::Create<'req>>;

impl<'req> traits::Create for Create<'req> {
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

impl<'req> traits::Bmap for Bmap<'req> {
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

impl<'req> traits::Fallocate for Fallocate<'req> {
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

impl<'req> traits::CopyFileRange for CopyFileRange<'req> {
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

impl<'req> traits::Poll for Poll<'req> {
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

/// The time value requested to be set.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum SetAttrTime {
    /// Set the specified time value.
    Timespec(Timespec),

    /// Set the current time.
    Now,
}

impl SetAttrTime {
    /// Convert itself into a `SystemTime`.
    #[inline]
    pub fn as_system_time(self) -> SystemTime {
        match self {
            SetAttrTime::Timespec(ts) => ts.as_system_time(),
            SetAttrTime::Now => SystemTime::now(),
        }
    }
}

pub mod traits {
    use super::SetAttrTime;
    use crate::types::{FileLock, LockOwner, Timespec};
    use std::ffi::OsStr;

    /// A forget information.
    pub trait Forget {
        /// Return the inode number of the target inode.
        fn ino(&self) -> u64;

        /// Return the released lookup count of the target inode.
        fn nlookup(&self) -> u64;
    }

    /// Lookup a directory entry by name.
    ///
    /// If a matching entry is found, the filesystem replies to the kernel
    /// with its attribute using `ReplyEntry`.  In addition, the lookup count
    /// of the corresponding inode is incremented on success.
    ///
    /// See also the documentation of `ReplyEntry` for tuning the reply parameters.
    pub trait Lookup {
        /// Return the inode number of the parent directory.
        fn parent(&self) -> u64;

        /// Return the name of the entry to be looked up.
        fn name(&self) -> &OsStr;
    }

    /// Get file attributes.
    ///
    /// The obtained attribute values are replied using `ReplyAttr`.
    ///
    /// If writeback caching is enabled, the kernel might ignore
    /// some of the attribute values, such as `st_size`.
    pub trait Getattr {
        /// Return the inode number for obtaining the attribute value.
        fn ino(&self) -> u64;

        /// Return the handle of opened file, if specified.
        fn fh(&self) -> Option<u64>;
    }

    /// Set file attributes.
    ///
    /// When the setting of attribute values succeeds, the filesystem replies its value
    /// to the kernel using `ReplyAttr`.
    pub trait Setattr {
        /// Return the inode number to be set the attribute values.
        fn ino(&self) -> u64;

        /// Return the handle of opened file, if specified.
        fn fh(&self) -> Option<u64>;

        /// Return the file mode to be set.
        fn mode(&self) -> Option<u32>;

        /// Return the user id to be set.
        fn uid(&self) -> Option<u32>;

        /// Return the group id to be set.
        fn gid(&self) -> Option<u32>;

        /// Return the size of the file content to be set.
        fn size(&self) -> Option<u64>;

        /// Return the last accessed time to be set.
        fn atime(&self) -> Option<SetAttrTime>;

        /// Return the last modified time to be set.
        fn mtime(&self) -> Option<SetAttrTime>;

        /// Return the last creation time to be set.
        fn ctime(&self) -> Option<Timespec>;

        /// Return the identifier of lock owner.
        fn lock_owner(&self) -> Option<LockOwner>;
    }

    /// Read a symbolic link.
    pub trait Readlink {
        /// Return the inode number to be read the link value.
        fn ino(&self) -> u64;
    }

    /// Create a symbolic link.
    ///
    /// When the link is successfully created, the filesystem must send
    /// its attribute values using `ReplyEntry`.
    pub trait Symlink {
        /// Return the inode number of the parent directory.
        fn parent(&self) -> u64;

        /// Return the name of the symbolic link to create.
        fn name(&self) -> &OsStr;

        /// Return the contents of the symbolic link.
        fn link(&self) -> &OsStr;
    }

    /// Create a file node.
    ///
    /// When the file node is successfully created, the filesystem must send
    /// its attribute values using `ReplyEntry`.
    pub trait Mknod {
        /// Return the inode number of the parent directory.
        fn parent(&self) -> u64;

        /// Return the file name to create.
        fn name(&self) -> &OsStr;

        /// Return the file type and permissions used when creating the new file.
        fn mode(&self) -> u32;

        /// Return the device number for special file.
        ///
        /// This value is meaningful only if the created node is a device file
        /// (i.e. the file type is specified either `S_IFCHR` or `S_IFBLK`).
        fn rdev(&self) -> u32;

        #[doc(hidden)] // TODO: dox
        fn umask(&self) -> u32;
    }

    /// Create a directory node.
    ///
    /// When the directory is successfully created, the filesystem must send
    /// its attribute values using `ReplyEntry`.
    pub trait Mkdir {
        /// Return the inode number of the parent directory where the directory is created.
        fn parent(&self) -> u64;

        /// Return the name of the directory to be created.
        fn name(&self) -> &OsStr;

        /// Return the file type and permissions used when creating the new directory.
        fn mode(&self) -> u32;

        #[doc(hidden)] // TODO: dox
        fn umask(&self) -> u32;
    }

    // TODO: description about lookup count.

    /// Remove a file.
    pub trait Unlink {
        /// Return the inode number of the parent directory.
        fn parent(&self) -> u64;

        /// Return the file name to be removed.
        fn name(&self) -> &OsStr;
    }

    /// Remove a directory.
    pub trait Rmdir {
        // TODO: description about lookup count.

        /// Return the inode number of the parent directory.
        fn parent(&self) -> u64;

        /// Return the directory name to be removed.
        fn name(&self) -> &OsStr;
    }

    /// Rename a file.
    pub trait Rename {
        /// Return the inode number of the old parent directory.
        fn parent(&self) -> u64;

        /// Return the old name of the target node.
        fn name(&self) -> &OsStr;

        /// Return the inode number of the new parent directory.
        fn newparent(&self) -> u64;

        /// Return the new name of the target node.
        fn newname(&self) -> &OsStr;

        /// Return the rename flags.
        fn flags(&self) -> u32;
    }

    /// Create a hard link.
    ///
    /// When the link is successfully created, the filesystem must send
    /// its attribute values using `ReplyEntry`.
    pub trait Link {
        /// Return the *original* inode number which links to the created hard link.
        fn ino(&self) -> u64;

        /// Return the inode number of the parent directory where the hard link is created.
        fn newparent(&self) -> u64;

        /// Return the name of the hard link to be created.
        fn newname(&self) -> &OsStr;
    }

    /// Open a file.
    ///
    /// If the file is successfully opened, the filesystem must send the identifier
    /// of the opened file handle to the kernel using `ReplyOpen`. This parameter is
    /// set to a series of requests, such as `read` and `write`, until releasing
    /// the file, and is able to be utilized as a "pointer" to the state during
    /// handling the opened file.
    ///
    /// See also the documentation of `ReplyOpen` for tuning the reply parameters.
    pub trait Open {
        // TODO: Description of behavior when writeback caching is enabled.

        /// Return the inode number to be opened.
        fn ino(&self) -> u64;

        /// Return the open flags.
        ///
        /// The creating flags (`O_CREAT`, `O_EXCL` and `O_NOCTTY`) are removed and
        /// these flags are handled by the kernel.
        ///
        /// If the mount option contains `-o default_permissions`, the access mode flags
        /// (`O_RDONLY`, `O_WRONLY` and `O_RDWR`) might be handled by the kernel and in that case,
        /// these flags are omitted before issuing the request. Otherwise, the filesystem should
        /// handle these flags and return an `EACCES` error when provided access mode is
        /// invalid.
        fn flags(&self) -> u32;
    }

    /// Read data from a file.
    ///
    /// The total amount of the replied data must be within `size`.
    ///
    /// When the file is opened in `direct_io` mode, the result replied will be
    /// reflected in the caller's result of `read` syscall.
    ///
    /// When the file is not opened in `direct_io` mode (i.e. the page caching is enabled),
    /// the filesystem should send *exactly* the specified range of file content to the
    /// kernel. If the length of the passed data is shorter than `size`, the rest of
    /// the data will be substituted with zeroes.
    pub trait Read {
        /// Return the inode number to be read.
        fn ino(&self) -> u64;

        /// Return the handle of opened file.
        fn fh(&self) -> u64;

        /// Return the starting position of the content to be read.
        fn offset(&self) -> u64;

        /// Return the length of the data to be read.
        fn size(&self) -> u32;

        /// Return the flags specified at opening the file.
        fn flags(&self) -> u32;

        /// Return the identifier of lock owner.
        fn lock_owner(&self) -> Option<LockOwner>;
    }

    /// Write data to a file.
    ///
    /// If the data is successfully written, the filesystem must send the amount of the written
    /// data using `ReplyWrite`.
    ///
    /// When the file is opened in `direct_io` mode, the result replied will be reflected
    /// in the caller's result of `write` syscall.
    ///
    /// When the file is not opened in `direct_io` mode (i.e. the page caching is enabled),
    /// the filesystem should receive *exactly* the specified range of file content from the kernel.
    pub trait Write {
        /// Return the inode number to be written.
        fn ino(&self) -> u64;

        /// Return the handle of opened file.
        fn fh(&self) -> u64;

        /// Return the starting position of contents to be written.
        fn offset(&self) -> u64;

        /// Return the length of contents to be written.
        fn size(&self) -> u32;

        /// Return the flags specified at opening the file.
        fn flags(&self) -> u32;

        /// Return the identifier of lock owner.
        fn lock_owner(&self) -> Option<LockOwner>;
    }

    /// Release an opened file.
    pub trait Release {
        /// Return the inode number of opened file.
        fn ino(&self) -> u64;

        /// Return the handle of opened file.
        fn fh(&self) -> u64;

        /// Return the flags specified at opening the file.
        fn flags(&self) -> u32;

        /// Return the identifier of lock owner.
        fn lock_owner(&self) -> LockOwner;

        /// Return whether the operation indicates a flush.
        fn flush(&self) -> bool;

        /// Return whether the `flock` locks for this file should be released.
        fn flock_release(&self) -> bool;
    }

    /// Get the filesystem statistics.
    ///
    /// The obtained statistics must be sent to the kernel using `ReplyStatfs`.
    pub trait Statfs {
        /// Return the inode number or `0` which means "undefined".
        fn ino(&self) -> u64;
    }

    /// Synchronize the file contents.
    pub trait Fsync {
        /// Return the inode number to be synchronized.
        fn ino(&self) -> u64;

        /// Return the handle of opened file.
        fn fh(&self) -> u64;

        /// Return whether to synchronize only the file contents.
        ///
        /// When this method returns `true`, the metadata does not have to be flushed.
        fn datasync(&self) -> bool;
    }

    /// Set an extended attribute.
    pub trait Setxattr {
        /// Return the inode number to set the value of extended attribute.
        fn ino(&self) -> u64;

        /// Return the name of extended attribute to be set.
        fn name(&self) -> &OsStr;

        /// Return the value of extended attribute.
        fn value(&self) -> &[u8];

        /// Return the flags that specifies the meanings of this operation.
        fn flags(&self) -> u32;
    }

    /// Get an extended attribute.
    ///
    /// This operation needs to switch the reply value according to the
    /// value of `size`:
    ///
    /// * When `size` is zero, the filesystem must send the length of the
    ///   attribute value for the specified name using `ReplyXattr`.
    ///
    /// * Otherwise, returns the attribute value with the specified name.
    ///   The filesystem should send an `ERANGE` error if the specified
    ///   size is too small for the attribute value.
    pub trait Getxattr {
        /// Return the inode number to be get the extended attribute.
        fn ino(&self) -> u64;

        /// Return the name of the extend attribute.
        fn name(&self) -> &OsStr;

        /// Return the maximum length of the attribute value to be replied.
        fn size(&self) -> u32;
    }

    /// List extended attribute names.
    ///
    /// Each element of the attribute names list must be null-terminated.
    /// As with `Getxattr`, the filesystem must send the data length of the attribute
    /// names using `ReplyXattr` if `size` is zero.
    pub trait Listxattr {
        /// Return the inode number to be obtained the attribute names.
        fn ino(&self) -> u64;

        /// Return the maximum length of the attribute names to be replied.
        fn size(&self) -> u32;
    }

    /// Remove an extended attribute.
    pub trait Removexattr {
        /// Return the inode number to remove the extended attribute.
        fn ino(&self) -> u64;

        /// Return the name of extended attribute to be removed.
        fn name(&self) -> &OsStr;
    }

    /// Close a file descriptor.
    ///
    /// This operation is issued on each `close(2)` syscall
    /// for a file descriptor.
    ///
    /// Do not confuse this operation with `Release`.
    /// Since the file descriptor could be duplicated, the multiple
    /// flush operations might be issued for one `Open`.
    /// Also, it is not guaranteed that flush will always be issued
    /// after some writes.
    pub trait Flush {
        /// Return the inode number of target file.
        fn ino(&self) -> u64;

        /// Return the handle of opened file.
        fn fh(&self) -> u64;

        /// Return the identifier of lock owner.
        fn lock_owner(&self) -> LockOwner;
    }

    /// Open a directory.
    ///
    /// If the directory is successfully opened, the filesystem must send
    /// the identifier to the opened directory handle using `ReplyOpen`.
    pub trait Opendir {
        /// Return the inode number to be opened.
        fn ino(&self) -> u64;

        /// Return the open flags.
        fn flags(&self) -> u32;
    }

    /// Read contents from an opened directory.
    pub trait Readdir {
        // TODO: description about `offset` and `is_plus`.

        /// Return the inode number to be read.
        fn ino(&self) -> u64;

        /// Return the handle of opened directory.
        fn fh(&self) -> u64;

        /// Return the *offset* value to continue reading the directory stream.
        fn offset(&self) -> u64;

        /// Return the maximum length of returned data.
        fn size(&self) -> u32;

        /// Return whether the operation is "plus" mode or not.
        fn is_plus(&self) -> bool;
    }

    /// Release an opened directory.
    pub trait Releasedir {
        /// Return the inode number of opened directory.
        fn ino(&self) -> u64;

        /// Return the handle of opened directory.
        fn fh(&self) -> u64;

        /// Return the flags specified at opening the directory.
        fn flags(&self) -> u32;
    }

    /// Synchronize the directory contents.
    pub trait Fsyncdir {
        /// Return the inode number to be synchronized.
        fn ino(&self) -> u64;

        /// Return the handle of opened directory.
        fn fh(&self) -> u64;

        /// Return whether to synchronize only the directory contents.
        ///
        /// When this method returns `true`, the metadata does not have to be flushed.
        fn datasync(&self) -> bool;
    }

    /// Test for a POSIX file lock.
    ///
    /// The lock result must be replied using `ReplyLk`.
    pub trait Getlk {
        /// Return the inode number to be tested the lock.
        fn ino(&self) -> u64;

        /// Return the handle of opened file.
        fn fh(&self) -> u64;

        /// Return the identifier of lock owner.
        fn owner(&self) -> LockOwner;

        /// Return the lock information for testing.
        fn lk(&self) -> &FileLock;
    }

    /// Acquire, modify or release a POSIX file lock.
    pub trait Setlk {
        /// Return the inode number to be obtained the lock.
        fn ino(&self) -> u64;

        /// Return the handle of opened file.
        fn fh(&self) -> u64;

        /// Return the identifier of lock owner.
        fn owner(&self) -> LockOwner;

        /// Return the lock information to be obtained.
        fn lk(&self) -> &FileLock;

        /// Return whether the locking operation might sleep until a lock is obtained.
        fn sleep(&self) -> bool;
    }

    /// Acquire, modify or release a BSD file lock.
    pub trait Flock {
        /// Return the target inode number.
        fn ino(&self) -> u64;

        /// Return the handle of opened file.
        fn fh(&self) -> u64;

        /// Return the identifier of lock owner.
        fn owner(&self) -> LockOwner;

        /// Return the locking operation.
        ///
        /// See [`flock(2)`][flock] for details.
        ///
        /// [flock]: http://man7.org/linux/man-pages/man2/flock.2.html
        fn op(&self) -> Option<u32>;
    }

    /// Check file access permissions.
    pub trait Access {
        /// Return the inode number subject to the access permission check.
        fn ino(&self) -> u64;

        /// Return the requested access mode.
        fn mask(&self) -> u32;
    }

    /// Create and open a file.
    ///
    /// This operation is a combination of `Mknod` and `Open`. If an `ENOSYS` error is returned
    /// for this operation, those operations will be used instead.
    ///
    /// If the file is successfully created and opened, a pair of `ReplyEntry` and `ReplyOpen`
    /// with the corresponding attribute values and the file handle must be sent to the kernel.
    pub trait Create {
        /// Return the inode number of the parent directory.
        ///
        /// This is the same as `Mknod::parent`.
        fn parent(&self) -> u64;

        /// Return the file name to crate.
        ///
        /// This is the same as `Mknod::name`.
        fn name(&self) -> &OsStr;

        /// Return the file type and permissions used when creating the new file.
        ///
        /// This is the same as `Mknod::mode`.
        fn mode(&self) -> u32;

        #[doc(hidden)] // TODO: dox
        fn umask(&self) -> u32;

        /// Return the open flags.
        ///
        /// This is the same as `Open::flags`.
        fn open_flags(&self) -> u32;
    }

    /// Map block index within a file to block index within device.
    ///
    /// The mapping result must be replied using `ReplyBmap`.
    ///
    /// This operation makes sense only for filesystems that use
    /// block devices, and is called only when the mount options
    /// contains `blkdev`.
    pub trait Bmap {
        /// Return the inode number of the file node to be mapped.
        fn ino(&self) -> u64;

        /// Return the block index to be mapped.
        fn block(&self) -> u64;

        /// Returns the unit of block index.
        fn blocksize(&self) -> u32;
    }

    /// Allocate requested space.
    ///
    /// If this operation is successful, the filesystem shall not report
    /// the error caused by the lack of free spaces to subsequent write
    /// requests.
    pub trait Fallocate {
        /// Return the number of target inode to be allocated the space.
        fn ino(&self) -> u64;

        /// Return the handle for opened file.
        fn fh(&self) -> u64;

        /// Return the starting point of region to be allocated.
        fn offset(&self) -> u64;

        /// Return the length of region to be allocated.
        fn length(&self) -> u64;

        /// Return the mode that specifies how to allocate the region.
        ///
        /// See [`fallocate(2)`][fallocate] for details.
        ///
        /// [fallocate]: http://man7.org/linux/man-pages/man2/fallocate.2.html
        fn mode(&self) -> u32;
    }

    /// Copy a range of data from an opened file to another.
    ///
    /// The length of copied data must be replied using `ReplyWrite`.
    pub trait CopyFileRange {
        /// Return the inode number of source file.
        fn ino_in(&self) -> u64;

        /// Return the file handle of source file.
        fn fh_in(&self) -> u64;

        /// Return the starting point of source file where the data should be read.
        fn offset_in(&self) -> u64;

        /// Return the inode number of target file.
        fn ino_out(&self) -> u64;

        /// Return the file handle of target file.
        fn fh_out(&self) -> u64;

        /// Return the starting point of target file where the data should be written.
        fn offset_out(&self) -> u64;

        /// Return the maximum size of data to copy.
        fn length(&self) -> u64;

        /// Return the flag value for `copy_file_range` syscall.
        fn flags(&self) -> u64;
    }

    /// Poll for readiness.
    ///
    /// The mask of ready poll events must be replied using `ReplyPoll`.
    pub trait Poll {
        /// Return the inode number to check the I/O readiness.
        fn ino(&self) -> u64;

        /// Return the handle of opened file.
        fn fh(&self) -> u64;

        /// Return the requested poll events.
        fn events(&self) -> u32;

        /// Return the handle to this poll.
        ///
        /// If the returned value is not `None`, the filesystem should send the notification
        /// when the corresponding I/O will be ready.
        fn kh(&self) -> Option<u64>;
    }
}
