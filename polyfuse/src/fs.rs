//! Filesystem abstraction.

use crate::{
    common::{FileLock, Forget},
    reply::{
        send_msg,
        ReplyAttr, //
        ReplyBmap,
        ReplyCreate,
        ReplyData,
        ReplyEmpty,
        ReplyEntry,
        ReplyLk,
        ReplyOpen,
        ReplyOpendir,
        ReplyPoll,
        ReplyReadlink,
        ReplyStatfs,
        ReplyWrite,
        ReplyXattr,
    },
    request::RequestHeader,
    session::{Interrupt, Session},
};
use async_trait::async_trait;
use futures::io::AsyncWrite;
use std::{ffi::OsStr, fmt, future::Future, io, pin::Pin};

/// Contextural information about an incoming request.
pub struct Context<'a, W: ?Sized> {
    header: &'a RequestHeader,
    writer: Option<&'a mut W>,
    session: &'a Session,
}

impl<W: ?Sized> fmt::Debug for Context<'_, W> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Context").finish()
    }
}

impl<'a, W: ?Sized> Context<'a, W> {
    pub(crate) fn new(header: &'a RequestHeader, writer: &'a mut W, session: &'a Session) -> Self {
        Self {
            header,
            writer: Some(writer),
            session,
        }
    }

    /// Return the user ID of the calling process.
    pub fn uid(&self) -> u32 {
        self.header.uid()
    }

    /// Return the group ID of the calling process.
    pub fn gid(&self) -> u32 {
        self.header.gid()
    }

    /// Return the process ID of the calling process.
    pub fn pid(&self) -> u32 {
        self.header.pid()
    }

    #[inline]
    pub(crate) async fn reply(&mut self, data: &[u8]) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        self.reply_vectored(&[data]).await
    }

    #[inline]
    pub(crate) async fn reply_vectored(&mut self, data: &[&[u8]]) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        if let Some(ref mut writer) = self.writer.take() {
            send_msg(writer, self.header.unique(), 0, data).await?;
        }
        Ok(())
    }

    /// Reply to the kernel with an error code.
    pub async fn reply_err(&mut self, error: i32) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        if let Some(ref mut writer) = self.writer.take() {
            send_msg(writer, self.header.unique(), -error, &[]).await?;
        }
        Ok(())
    }

    /// Register the request with the sesssion and get a signal
    /// that will be notified when the request is canceld by the kernel.
    pub async fn on_interrupt(&mut self) -> Interrupt {
        self.session.enable_interrupt(self.header.unique()).await
    }

    pub(crate) fn is_replied(&self) -> bool {
        self.writer.is_none()
    }
}

/// The filesystem running on the user space.
#[async_trait]
pub trait Filesystem<T>: Sync {
    /// Handle a FUSE request from the kernel and reply with its result.
    #[allow(unused_variables)]
    async fn call<W: ?Sized>(&self, cx: &mut Context<'_, W>, op: Operation<'_, T>) -> io::Result<()>
    where
        T: Send + 'async_trait,
        W: AsyncWrite + Send + Unpin,
    {
        Ok(())
    }
}

impl<'a, F: ?Sized, T> Filesystem<T> for &'a F
where
    F: Filesystem<T>,
{
    #[inline]
    fn call<'l1, 'l2, 'l3, 'l4, 'async_trait, W: ?Sized>(
        &'l1 self,
        cx: &'l2 mut Context<'l3, W>,
        op: Operation<'l4, T>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
    where
        'l1: 'async_trait,
        'l2: 'async_trait,
        'l3: 'async_trait,
        'l4: 'async_trait,
        T: Send + 'async_trait,
        W: AsyncWrite + Send + Unpin + 'async_trait,
    {
        (**self).call(cx, op)
    }
}

impl<F: ?Sized, T> Filesystem<T> for Box<F>
where
    F: Filesystem<T>,
{
    #[inline]
    fn call<'l1, 'l2, 'l3, 'l4, 'async_trait, W: ?Sized>(
        &'l1 self,
        cx: &'l2 mut Context<'l3, W>,
        op: Operation<'l4, T>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
    where
        'l1: 'async_trait,
        'l2: 'async_trait,
        'l3: 'async_trait,
        'l4: 'async_trait,
        T: Send + 'async_trait,
        W: AsyncWrite + Send + Unpin + 'async_trait,
    {
        (**self).call(cx, op)
    }
}

impl<F: ?Sized, T> Filesystem<T> for std::sync::Arc<F>
where
    F: Filesystem<T> + Send,
{
    #[inline]
    fn call<'l1, 'l2, 'l3, 'l4, 'async_trait, W: ?Sized>(
        &'l1 self,
        cx: &'l2 mut Context<'l3, W>,
        op: Operation<'l4, T>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
    where
        'l1: 'async_trait,
        'l2: 'async_trait,
        'l3: 'async_trait,
        'l4: 'async_trait,
        T: Send + 'async_trait,
        W: AsyncWrite + Send + Unpin + 'async_trait,
    {
        (**self).call(cx, op)
    }
}

/// The kind of FUSE requests received from the kernel.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum Operation<'a, T> {
    /// Look up a directory entry by name.
    Lookup {
        parent: u64,
        name: &'a OsStr,
        reply: ReplyEntry,
    },

    /// Forget about inodes removed from the kernel's internal caches.
    Forget { forgets: &'a [Forget] },

    /// Get file attributes.
    Getattr {
        ino: u64,
        fh: Option<u64>,
        reply: ReplyAttr,
    },

    /// Set file attributes.
    Setattr {
        ino: u64,
        fh: Option<u64>,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<(u64, u32, bool)>,
        mtime: Option<(u64, u32, bool)>,
        ctime: Option<(u64, u32)>,
        lock_owner: Option<u64>,
        reply: ReplyAttr,
    },

    /// Read a symbolic link.
    Readlink { ino: u64, reply: ReplyReadlink },

    /// Create a symbolic link
    Symlink {
        parent: u64,
        name: &'a OsStr,
        link: &'a OsStr,
        reply: ReplyEntry,
    },

    /// Create a file node.
    Mknod {
        parent: u64,
        name: &'a OsStr,
        mode: u32,
        rdev: u32,
        umask: Option<u32>,
        reply: ReplyEntry,
    },

    /// Create a directory.
    Mkdir {
        parent: u64,
        name: &'a OsStr,
        mode: u32,
        umask: Option<u32>,
        reply: ReplyEntry,
    },

    /// Remove a file.
    Unlink {
        parent: u64,
        name: &'a OsStr,
        reply: ReplyEmpty,
    },

    /// Remove a directory.
    Rmdir {
        parent: u64,
        name: &'a OsStr,
        reply: ReplyEmpty,
    },

    /// Rename a file.
    Rename {
        parent: u64,
        name: &'a OsStr,
        newparent: u64,
        newname: &'a OsStr,
        flags: u32,
        reply: ReplyEmpty,
    },

    /// Create a hard link.
    Link {
        ino: u64,
        newparent: u64,
        newname: &'a OsStr,
        reply: ReplyEntry,
    },

    /// Open a file.
    Open {
        ino: u64,
        flags: u32,
        reply: ReplyOpen,
    },

    /// Read data from an opened file.
    Read {
        ino: u64,
        fh: u64,
        offset: u64,
        flags: u32,
        lock_owner: Option<u64>,
        reply: ReplyData,
    },

    /// Write data to an opened file.
    Write {
        ino: u64,
        fh: u64,
        offset: u64,
        data: T,
        flags: u32,
        lock_owner: Option<u64>,
        reply: ReplyWrite,
    },

    /// Release an opened file.
    Release {
        ino: u64,
        fh: u64,
        flags: u32,
        lock_owner: Option<u64>,
        flush: bool,
        flock_release: bool,
        reply: ReplyEmpty,
    },

    /// Get the filesystem statistics.
    Statfs { ino: u64, reply: ReplyStatfs },

    /// Synchronize the file contents of an opened file.
    ///
    /// When the parameter `datasync` is true, only the
    /// file contents should be flushed and the metadata
    /// does not have to be flushed.
    Fsync {
        ino: u64,
        fh: u64,
        datasync: bool,
        reply: ReplyEmpty,
    },

    /// Set an extended attribute.
    Setxattr {
        ino: u64,
        name: &'a OsStr,
        value: &'a [u8],
        flags: u32,
        reply: ReplyEmpty,
    },

    /// Get an extended attribute.
    ///
    /// The operation should send the length of attribute's value
    /// with `reply.size(n)` when `size` is equal to zero.
    Getxattr {
        ino: u64,
        name: &'a OsStr,
        size: u32,
        reply: ReplyXattr,
    },

    /// List extended attribute names.
    ///
    /// The attribute names must be seperated by a null character
    /// (i.e. `b'\0'`).
    ///
    /// The operation should send the length of attribute names
    /// with `reply.size(n)` when `size` is equal to zero.
    Listxattr {
        ino: u64,
        size: u32,
        reply: ReplyXattr,
    },

    /// Remove an extended attribute.
    Removexattr {
        ino: u64,
        name: &'a OsStr,
        reply: ReplyEmpty,
    },

    /// Close a file descriptor.
    Flush {
        ino: u64,
        fh: u64,
        lock_owner: u64,
        reply: ReplyEmpty,
    },

    /// Open a directory.
    Opendir {
        ino: u64,
        flags: u32,
        reply: ReplyOpendir,
    },

    /// Read contents from an opened directory.
    Readdir {
        ino: u64,
        fh: u64,
        offset: u64,
        reply: ReplyData,
    },

    /// Release an opened directory.
    Releasedir {
        ino: u64,
        fh: u64,
        flags: u32,
        reply: ReplyEmpty,
    },

    /// Synchronize an opened directory contents.
    ///
    /// When the parameter `datasync` is true, only the
    /// directory contents should be flushed and the metadata
    /// does not have to be flushed.
    Fsyncdir {
        ino: u64,
        fh: u64,
        datasync: bool,
        reply: ReplyEmpty,
    },

    /// Test for a POSIX file lock.
    Getlk {
        ino: u64,
        fh: u64,
        owner: u64,
        lk: &'a FileLock,
        reply: ReplyLk,
    },

    /// Acquire, modify or release a POSIX file lock.
    Setlk {
        ino: u64,
        fh: u64,
        owner: u64,
        lk: &'a FileLock,
        sleep: bool,
        reply: ReplyEmpty,
    },

    /// Acquire, modify or release a BSD file lock.
    Flock {
        ino: u64,
        fh: u64,
        owner: u64,
        op: u32,
        reply: ReplyEmpty,
    },

    /// Check file access permissions.
    Access {
        ino: u64,
        mask: u32,
        reply: ReplyEmpty,
    },

    /// Create and open a file.
    Create {
        parent: u64,
        name: &'a OsStr,
        mode: u32,
        umask: Option<u32>,
        open_flags: u32,
        reply: ReplyCreate,
    },

    /// Map block index within a file to block index within device.
    ///
    /// This operation makes sense only for filesystems that use
    /// block devices, and is called only when the mount options
    /// contains `blkdev`.
    Bmap {
        ino: u64,
        block: u64,
        blocksize: u32,
        reply: ReplyBmap,
    },

    /// Allocate requested space to an opened file.
    Fallocate {
        ino: u64,
        fh: u64,
        offset: u64,
        length: u64,
        mode: u32,
        reply: ReplyEmpty,
    },

    /// Copy a range of data from an opened file to another.
    CopyFileRange {
        ino_in: u64,
        off_in: u64,
        fh_in: u64,
        ino_out: u64,
        off_out: u64,
        fh_out: u64,
        len: u64,
        flags: u64,
        reply: ReplyWrite,
    },

    /// Poll for readiness.
    ///
    /// When `kh` is not `None`, the filesystem should send
    /// the notification about I/O readiness to the kernel.
    Poll {
        ino: u64,
        fh: u64,
        events: u32,
        kh: Option<u64>,
        reply: ReplyPoll,
    },
}

// TODO: add operations:
// Ioctl
