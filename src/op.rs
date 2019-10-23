#![allow(clippy::needless_lifetimes)]

use crate::reply::{
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
};
use polyfuse_abi::{FileLock, FileMode, Gid, Nodeid, Pid, Uid};
use std::future::Future;
use std::{ffi::OsStr, io, pin::Pin};

type ImplFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Contextural information about an incoming request.
#[derive(Debug, Copy, Clone)]
pub struct Context {
    /// The user ID of the calling process.
    pub uid: Uid,

    /// The group ID of the calling process.
    pub gid: Gid,

    /// The process ID of the calling process.
    pub pid: Pid,

    pub(crate) _p: (),
}

/// A set of attributes to be set.
#[derive(Debug, Default, Copy, Clone)]
pub struct AttrSet {
    pub mode: Option<FileMode>,

    pub uid: Option<Uid>,

    pub gid: Option<Gid>,

    pub size: Option<u64>,

    pub atime: Option<(u64, u32, bool)>,

    pub mtime: Option<(u64, u32, bool)>,

    pub ctime: Option<(u64, u32)>,

    pub lock_owner: Option<u64>,

    pub(crate) _p: (),
}

#[allow(unused_variables)]
pub trait Operations<T> {
    /// Look up a directory entry by name.
    fn lookup<'a>(
        &mut self,
        cx: &Context,
        parent: Nodeid,
        name: &OsStr,
        reply: ReplyEntry<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Forget about inodes removed from the kernel's internal caches.
    fn forget(
        &mut self,
        cx: &Context,
        entries: &[(Nodeid, u64)],
    ) -> ImplFuture<'static, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    /// Get file attributes.
    fn getattr<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        fh: Option<u64>,
        reply: ReplyAttr<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Set file attributes.
    fn setattr<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        fh: Option<u64>,
        attr: AttrSet,
        reply: ReplyAttr<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Read a symbolic link.
    fn readlink<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        reply: ReplyReadlink<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Create a symbolic link
    fn symlink<'a>(
        &mut self,
        cx: &Context,
        parent: Nodeid,
        name: &OsStr,
        link: &OsStr,
        reply: ReplyEntry<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Create a file node.
    fn mknod<'a>(
        &mut self,
        cx: &Context,
        parent: Nodeid,
        name: &OsStr,
        mode: FileMode,
        rdev: u32,
        umask: Option<u32>,
        reply: ReplyEntry<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Create a directory.
    fn mkdir<'a>(
        &mut self,
        cx: &Context,
        parent: Nodeid,
        name: &OsStr,
        mode: FileMode,
        umask: Option<u32>,
        reply: ReplyEntry<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Remove a file.
    fn unlink<'a>(
        &mut self,
        cx: &Context,
        parent: Nodeid,
        name: &OsStr,
        reply: ReplyEmpty<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Remove a directory.
    fn rmdir<'a>(
        &mut self,
        cx: &Context,
        parent: Nodeid,
        name: &OsStr,
        reply: ReplyEmpty<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Rename a file.
    fn rename<'a>(
        &mut self,
        cx: &Context,
        parent: Nodeid,
        name: &OsStr,
        newparent: Nodeid,
        newname: &OsStr,
        flags: u32,
        reply: ReplyEmpty<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Create a hard link.
    fn link<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        newparent: Nodeid,
        newname: &OsStr,
        reply: ReplyEntry<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Open a file.
    fn open<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        flags: u32,
        reply: ReplyOpen<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Read data from an opened file.
    fn read<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        fh: u64,
        offset: u64,
        flags: u32,
        lock_owner: Option<u64>,
        reply: ReplyData<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Write data to an opened file.
    fn write<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        fh: u64,
        offset: u64,
        data: T,
        size: u32,
        flags: u32,
        lock_owner: Option<u64>,
        reply: ReplyWrite<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Release an opened file.
    fn release<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        fh: u64,
        flags: u32,
        lock_owner: Option<u64>,
        flush: bool,
        flock_release: bool,
        reply: ReplyEmpty<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Get the filesystem statistics.
    fn statfs<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        reply: ReplyStatfs<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Synchronize the file contents of an opened file.
    ///
    /// When the parameter `datasync` is true, only the
    /// file contents should be flushed and the metadata
    /// does not have to be flushed.
    fn fsync<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        fh: u64,
        datasync: bool,
        reply: ReplyEmpty<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Set an extended attribute.
    fn setxattr<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        name: &OsStr,
        value: &[u8],
        flags: u32,
        reply: ReplyEmpty<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Get an extended attribute.
    ///
    /// The operation should send the length of attribute's value
    /// with `reply.size(n)` when `size` is equal to zero.
    fn getxattr<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        name: &OsStr,
        size: u32,
        reply: ReplyXattr<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// List extended attribute names.
    ///
    /// The attribute names must be seperated by a null character
    /// (i.e. `b'\0'`).
    ///
    /// The operation should send the length of attribute names
    /// with `reply.size(n)` when `size` is equal to zero.
    fn listxattr<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        size: u32,
        reply: ReplyXattr<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Remove an extended attribute.
    fn removexattr<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        name: &OsStr,
        reply: ReplyEmpty<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Close a file descriptor.
    fn flush<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        fh: u64,
        lock_owner: u64,
        reply: ReplyEmpty<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Open a directory.
    fn opendir<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        flags: u32,
        reply: ReplyOpendir<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Read contents from an opened directory.
    fn readdir<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        fh: u64,
        offset: u64,
        reply: ReplyData<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Release an opened directory.
    fn releasedir<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        fh: u64,
        flags: u32,
        reply: ReplyEmpty<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Synchronize an opened directory contents.
    ///
    /// When the parameter `datasync` is true, only the
    /// directory contents should be flushed and the metadata
    /// does not have to be flushed.
    fn fsyncdir<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        fh: u64,
        datasync: bool,
        reply: ReplyEmpty<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Test for a POSIX file lock.
    fn getlk<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        fh: u64,
        owner: u64,
        lock: &FileLock,
        reply: ReplyLk<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Acquire, modify or release a POSIX file lock.
    fn setlk<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        fh: u64,
        owner: u64,
        lock: &FileLock,
        sleep: bool,
        reply: ReplyEmpty<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Acquire, modify or release a BSD file lock.
    fn flock<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        fh: u64,
        owner: u64,
        op: u32,
        reply: ReplyEmpty<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Check file access permissions.
    fn access<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        mask: u32,
        reply: ReplyEmpty<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Create and open a file.
    fn create<'a>(
        &mut self,
        cx: &Context,
        parent: Nodeid,
        name: &OsStr,
        mode: FileMode,
        umask: Option<u32>,
        open_flags: u32,
        reply: ReplyCreate<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    /// Map block index within a file to block index within device.
    ///
    /// This callback makes sense only for filesystems that use
    /// block devices, and is called only when the mount options
    /// contains `blkdev`.
    fn bmap<'a>(
        &mut self,
        cx: &Context,
        ino: Nodeid,
        block: u64,
        blocksize: u32,
        reply: ReplyBmap<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    // interrupt
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
