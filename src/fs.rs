#![allow(clippy::needless_lifetimes)]

use crate::{
    reply::{
        ReplyAttr, //
        ReplyBmap,
        ReplyCreate,
        ReplyData,
        ReplyEmpty,
        ReplyEntry,
        ReplyLk,
        ReplyOpen,
        ReplyOpendir,
        ReplyReadlink,
        ReplyStatfs,
        ReplyWrite,
        ReplyXattr,
    },
    session::Background,
};
use polyfuse_abi::{FileLock, FileMode, Gid, Nodeid, Pid, Uid, Unique};
use std::{ffi::OsStr, future::Future, io};

#[async_trait::async_trait(?Send)]
pub trait Filesystem<T> {
    #[allow(unused_variables)]
    async fn call(&mut self, cx: &mut Context<'_>, op: Operation<'_, T>) -> io::Result<()>
    where
        T: 'async_trait, // https://github.com/dtolnay/async-trait/issues/8
    {
        op.reply_default().await
    }
}

/// Contextural information about an incoming request.
#[derive(Debug)]
pub struct Context<'a> {
    pub(crate) uid: Uid,
    pub(crate) gid: Gid,
    pub(crate) pid: Pid,
    pub(crate) background: &'a mut Background,
    pub(crate) unique: Unique,
}

impl Context<'_> {
    /// Return the user ID of the calling process.
    pub fn uid(&self) -> Uid {
        self.uid
    }

    /// Return the group ID of the calling process.
    pub fn gid(&self) -> Gid {
        self.gid
    }

    /// Return the process ID of the calling process.
    pub fn pid(&self) -> Pid {
        self.pid
    }

    pub fn register(&mut self) -> impl Future<Output = ()> {
        let rx = self.background.register(self.unique);
        async move {
            let _ = rx.await;
        }
    }
}

#[derive(Debug)]
pub enum Operation<'a, T> {
    /// Look up a directory entry by name.
    Lookup {
        parent: Nodeid,
        name: &'a OsStr,
        reply: ReplyEntry<'static>,
    },

    /// Forget about inodes removed from the kernel's internal caches.
    Forget {
        nlookups: &'a [(Nodeid, u64)], //
    },

    /// Get file attributes.
    Getattr {
        ino: Nodeid,
        fh: Option<u64>,
        reply: ReplyAttr<'static>,
    },

    /// Set file attributes.
    Setattr {
        ino: Nodeid,
        fh: Option<u64>,
        mode: Option<FileMode>,
        uid: Option<Uid>,
        gid: Option<Gid>,
        size: Option<u64>,
        atime: Option<(u64, u32, bool)>,
        mtime: Option<(u64, u32, bool)>,
        ctime: Option<(u64, u32)>,
        lock_owner: Option<u64>,
        reply: ReplyAttr<'static>,
    },

    /// Read a symbolic link.
    Readlink {
        ino: Nodeid,
        reply: ReplyReadlink<'static>,
    },

    /// Create a symbolic link
    Symlink {
        parent: Nodeid,
        name: &'a OsStr,
        link: &'a OsStr,
        reply: ReplyEntry<'static>,
    },

    /// Create a file node.
    Mknod {
        parent: Nodeid,
        name: &'a OsStr,
        mode: FileMode,
        rdev: u32,
        umask: Option<u32>,
        reply: ReplyEntry<'static>,
    },

    /// Create a directory.
    Mkdir {
        parent: Nodeid,
        name: &'a OsStr,
        mode: FileMode,
        umask: Option<u32>,
        reply: ReplyEntry<'static>,
    },

    /// Remove a file.
    Unlink {
        parent: Nodeid,
        name: &'a OsStr,
        reply: ReplyEmpty<'static>,
    },

    /// Remove a directory.
    Rmdir {
        parent: Nodeid,
        name: &'a OsStr,
        reply: ReplyEmpty<'static>,
    },

    /// Rename a file.
    Rename {
        parent: Nodeid,
        name: &'a OsStr,
        newparent: Nodeid,
        newname: &'a OsStr,
        flags: u32,
        reply: ReplyEmpty<'static>,
    },

    /// Create a hard link.
    Link {
        ino: Nodeid,
        newparent: Nodeid,
        newname: &'a OsStr,
        reply: ReplyEntry<'static>,
    },

    /// Open a file.
    Open {
        ino: Nodeid,
        flags: u32,
        reply: ReplyOpen<'static>,
    },

    /// Read data from an opened file.
    Read {
        ino: Nodeid,
        fh: u64,
        offset: u64,
        flags: u32,
        lock_owner: Option<u64>,
        reply: ReplyData<'static>,
    },

    /// Write data to an opened file.
    Write {
        ino: Nodeid,
        fh: u64,
        offset: u64,
        data: T,
        size: u32,
        flags: u32,
        lock_owner: Option<u64>,
        reply: ReplyWrite<'static>,
    },

    /// Release an opened file.
    Release {
        ino: Nodeid,
        fh: u64,
        flags: u32,
        lock_owner: Option<u64>,
        flush: bool,
        flock_release: bool,
        reply: ReplyEmpty<'static>,
    },

    /// Get the filesystem statistics.
    Statfs {
        ino: Nodeid,
        reply: ReplyStatfs<'static>,
    },

    /// Synchronize the file contents of an opened file.
    ///
    /// When the parameter `datasync` is true, only the
    /// file contents should be flushed and the metadata
    /// does not have to be flushed.
    Fsync {
        ino: Nodeid,
        fh: u64,
        datasync: bool,
        reply: ReplyEmpty<'static>,
    },

    /// Set an extended attribute.
    Setxattr {
        ino: Nodeid,
        name: &'a OsStr,
        value: &'a [u8],
        flags: u32,
        reply: ReplyEmpty<'static>,
    },

    /// Get an extended attribute.
    ///
    /// The operation should send the length of attribute's value
    /// with `reply.size(n)` when `size` is equal to zero.
    Getxattr {
        ino: Nodeid,
        name: &'a OsStr,
        size: u32,
        reply: ReplyXattr<'static>,
    },

    /// List extended attribute names.
    ///
    /// The attribute names must be seperated by a null character
    /// (i.e. `b'\0'`).
    ///
    /// The operation should send the length of attribute names
    /// with `reply.size(n)` when `size` is equal to zero.
    Listxattr {
        ino: Nodeid,
        size: u32,
        reply: ReplyXattr<'static>,
    },

    /// Remove an extended attribute.
    Removexattr {
        ino: Nodeid,
        name: &'a OsStr,
        reply: ReplyEmpty<'static>,
    },

    /// Close a file descriptor.
    Flush {
        ino: Nodeid,
        fh: u64,
        lock_owner: u64,
        reply: ReplyEmpty<'static>,
    },

    /// Open a directory.
    Opendir {
        ino: Nodeid,
        flags: u32,
        reply: ReplyOpendir<'static>,
    },

    /// Read contents from an opened directory.
    Readdir {
        ino: Nodeid,
        fh: u64,
        offset: u64,
        reply: ReplyData<'static>,
    },

    /// Release an opened directory.
    Releasedir {
        ino: Nodeid,
        fh: u64,
        flags: u32,
        reply: ReplyEmpty<'static>,
    },

    /// Synchronize an opened directory contents.
    ///
    /// When the parameter `datasync` is true, only the
    /// directory contents should be flushed and the metadata
    /// does not have to be flushed.
    Fsyncdir {
        ino: Nodeid,
        fh: u64,
        datasync: bool,
        reply: ReplyEmpty<'static>,
    },

    /// Test for a POSIX file lock.
    Getlk {
        ino: Nodeid,
        fh: u64,
        owner: u64,
        lk: &'a FileLock,
        reply: ReplyLk<'static>,
    },

    /// Acquire, modify or release a POSIX file lock.
    Setlk {
        ino: Nodeid,
        fh: u64,
        owner: u64,
        lk: &'a FileLock,
        sleep: bool,
        reply: ReplyEmpty<'static>,
    },

    /// Acquire, modify or release a BSD file lock.
    Flock {
        ino: Nodeid,
        fh: u64,
        owner: u64,
        op: u32,
        reply: ReplyEmpty<'static>,
    },

    /// Check file access permissions.
    Access {
        ino: Nodeid,
        mask: u32,
        reply: ReplyEmpty<'static>,
    },

    /// Create and open a file.
    Create {
        parent: Nodeid,
        name: &'a OsStr,
        mode: FileMode,
        umask: Option<u32>,
        open_flags: u32,
        reply: ReplyCreate<'static>,
    },

    /// Map block index within a file to block index within device.
    ///
    /// This callback makes sense only for filesystems that use
    /// block devices, and is called only when the mount options
    /// contains `blkdev`.
    Bmap {
        ino: Nodeid,
        block: u64,
        blocksize: u32,
        reply: ReplyBmap<'static>,
    },
    // ioctl
    // poll
    // notify_reply
    // batch_forget
    // fallocate
    // readdirplus
    // rename2
    // lseek
    // copy_file_range
}

impl<T> Operation<'_, T> {
    pub async fn reply_default(self) -> io::Result<()> {
        match self {
            Self::Forget { .. } => Ok(()),
            Self::Lookup { reply, .. }
            | Self::Symlink { reply, .. }
            | Self::Mknod { reply, .. }
            | Self::Mkdir { reply, .. }
            | Self::Link { reply, .. } => reply.err(libc::ENOSYS).await,
            Self::Getattr { reply, .. } | Self::Setattr { reply, .. } => {
                reply.err(libc::ENOSYS).await
            }
            Self::Readlink { reply, .. } => reply.err(libc::ENOSYS).await,
            Self::Unlink { reply, .. }
            | Self::Rmdir { reply, .. }
            | Self::Rename { reply, .. }
            | Self::Release { reply, .. }
            | Self::Fsync { reply, .. }
            | Self::Setxattr { reply, .. }
            | Self::Removexattr { reply, .. }
            | Self::Flush { reply, .. }
            | Self::Releasedir { reply, .. }
            | Self::Fsyncdir { reply, .. }
            | Self::Setlk { reply, .. }
            | Self::Flock { reply, .. }
            | Self::Access { reply, .. } => reply.err(libc::ENOSYS).await,
            Self::Open { reply, .. } => reply.err(libc::ENOSYS).await,
            Self::Read { reply, .. } | Self::Readdir { reply, .. } => reply.err(libc::ENOSYS).await,
            Self::Write { reply, .. } => reply.err(libc::ENOSYS).await,
            Self::Statfs { reply, .. } => reply.err(libc::ENOSYS).await,
            Self::Getxattr { reply, .. } | Self::Listxattr { reply, .. } => {
                reply.err(libc::ENOSYS).await
            }
            Self::Getlk { reply, .. } => reply.err(libc::ENOSYS).await,
            Self::Opendir { reply, .. } => reply.err(libc::ENOSYS).await,
            Self::Create { reply, .. } => reply.err(libc::ENOSYS).await,
            Self::Bmap { reply, .. } => reply.err(libc::ENOSYS).await,
        }
    }
}
