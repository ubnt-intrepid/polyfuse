//! Filesystem operations.

#![allow(missing_docs)]

use crate::{
    common::{FileLock, Forget},
    kernel::{
        fuse_access_in, //
        fuse_bmap_in,
        fuse_copy_file_range_in,
        fuse_create_in,
        fuse_fallocate_in,
        fuse_flush_in,
        fuse_fsync_in,
        fuse_getattr_in,
        fuse_getxattr_in,
        fuse_link_in,
        fuse_lk_in,
        fuse_mkdir_in,
        fuse_mknod_in,
        fuse_open_in,
        fuse_poll_in,
        fuse_read_in,
        fuse_release_in,
        fuse_rename2_in,
        fuse_rename_in,
        fuse_setattr_in,
        fuse_setxattr_in,
        fuse_write_in,
    },
    reply::{
        ReplyAttr, //
        ReplyBmap,
        ReplyEntry,
        ReplyLk,
        ReplyOpen,
        ReplyOpendir,
        ReplyPoll,
        ReplyStatfs,
        ReplyWrite,
        ReplyWriter,
        ReplyXattr,
    },
    request::RequestHeader,
};
use futures::io::AsyncWrite;
use std::{ffi::OsStr, io, os::unix::ffi::OsStrExt};

/// The kind of FUSE requests received from the kernel.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum Operation<'a, T> {
    /// Look up a directory entry by name.
    Lookup(Lookup<'a>),

    /// Forget about inodes removed from the kernel's internal caches.
    Forget(&'a [Forget]),

    /// Get file attributes.
    Getattr(Getattr<'a>),

    /// Set file attributes.
    Setattr(Setattr<'a>),

    /// Read a symbolic link.
    Readlink(Readlink<'a>),

    /// Create a symbolic link
    Symlink(Symlink<'a>),

    /// Create a file node.
    Mknod(Mknod<'a>),

    /// Create a directory.
    Mkdir(Mkdir<'a>),

    /// Remove a file.
    Unlink(Unlink<'a>),

    /// Remove a directory.
    Rmdir(Rmdir<'a>),

    /// Rename a file.
    Rename(Rename<'a>),

    /// Create a hard link.
    Link(Link<'a>),

    /// Open a file.
    Open(Open<'a>),

    /// Read data from an opened file.
    Read(Read<'a>),

    /// Write data to an opened file.
    Write(Write<'a>, T),

    /// Release an opened file.
    Release(Release<'a>),

    /// Get the filesystem statistics.
    Statfs(Statfs<'a>),

    /// Synchronize the file contents of an opened file.
    ///
    /// When the parameter `datasync` is true, only the
    /// file contents should be flushed and the metadata
    /// does not have to be flushed.
    Fsync(Fsync<'a>),

    /// Set an extended attribute.
    Setxattr(Setxattr<'a>),

    /// Get an extended attribute.
    ///
    /// The operation should send the length of attribute's value
    /// with `reply.size(n)` when `size` is equal to zero.
    Getxattr(Getxattr<'a>),

    /// List extended attribute names.
    ///
    /// The attribute names must be seperated by a null character
    /// (i.e. `b'\0'`).
    ///
    /// The operation should send the length of attribute names
    /// with `reply.size(n)` when `size` is equal to zero.
    Listxattr(Listxattr<'a>),

    /// Remove an extended attribute.
    Removexattr(Removexattr<'a>),

    /// Close a file descriptor.
    Flush(Flush<'a>),

    /// Open a directory.
    Opendir(Opendir<'a>),

    /// Read contents from an opened directory.
    Readdir(Readdir<'a>),

    /// Release an opened directory.
    Releasedir(Releasedir<'a>),

    /// Synchronize an opened directory contents.
    ///
    /// When the parameter `datasync` is true, only the
    /// directory contents should be flushed and the metadata
    /// does not have to be flushed.
    Fsyncdir(Fsyncdir<'a>),

    /// Test for a POSIX file lock.
    Getlk(Getlk<'a>),

    /// Acquire, modify or release a POSIX file lock.
    Setlk(Setlk<'a>),

    /// Acquire, modify or release a BSD file lock.
    Flock(Flock<'a>),

    /// Check file access permissions.
    Access(Access<'a>),

    /// Create and open a file.
    Create(Create<'a>),

    /// Map block index within a file to block index within device.
    ///
    /// This operation makes sense only for filesystems that use
    /// block devices, and is called only when the mount options
    /// contains `blkdev`.
    Bmap(Bmap<'a>),

    /// Allocate requested space to an opened file.
    Fallocate(Fallocate<'a>),

    /// Copy a range of data from an opened file to another.
    CopyFileRange(CopyFileRange<'a>),

    /// Poll for readiness.
    ///
    /// When `kh` is not `None`, the filesystem should send
    /// the notification about I/O readiness to the kernel.
    Poll(Poll<'a>),
}

// TODO: add operations:
// Ioctl

#[derive(Debug)]
pub struct Lookup<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) name: &'a OsStr,
}

impl<'a> Lookup<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn name(&self) -> &OsStr {
        self.name
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let entry = entry.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(entry) }).await
    }
}

#[derive(Debug)]
pub struct Getattr<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_getattr_in,
}

impl<'a> Getattr<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn fh(&self) -> Option<u64> {
        if self.arg.getattr_flags & crate::kernel::FUSE_GETATTR_FH != 0 {
            Some(self.arg.fh)
        } else {
            None
        }
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        attr: impl AsRef<ReplyAttr>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let attr = attr.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(attr) }).await
    }
}

#[derive(Debug)]
pub struct Setattr<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_setattr_in,
}

impl<'a> Setattr<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    #[inline(always)]
    fn get<R>(&self, flag: u32, f: impl FnOnce(&fuse_setattr_in) -> R) -> Option<R> {
        if self.arg.valid & flag != 0 {
            Some(f(&self.arg))
        } else {
            None
        }
    }

    pub fn fh(&self) -> Option<u64> {
        self.get(crate::kernel::FATTR_FH, |arg| arg.fh)
    }

    pub fn mode(&self) -> Option<u32> {
        self.get(crate::kernel::FATTR_MODE, |arg| arg.mode)
    }

    pub fn uid(&self) -> Option<u32> {
        self.get(crate::kernel::FATTR_UID, |arg| arg.uid)
    }

    pub fn gid(&self) -> Option<u32> {
        self.get(crate::kernel::FATTR_GID, |arg| arg.gid)
    }

    pub fn size(&self) -> Option<u64> {
        self.get(crate::kernel::FATTR_SIZE, |arg| arg.size)
    }

    pub fn atime(&self) -> Option<(u64, u32, bool)> {
        self.get(crate::kernel::FATTR_ATIME, |arg| {
            (
                arg.atime,
                arg.atimensec,
                arg.valid & crate::kernel::FATTR_ATIME_NOW != 0,
            )
        })
    }

    pub fn mtime(&self) -> Option<(u64, u32, bool)> {
        self.get(crate::kernel::FATTR_MTIME, |arg| {
            (
                arg.mtime,
                arg.mtimensec,
                arg.valid & crate::kernel::FATTR_MTIME_NOW != 0,
            )
        })
    }

    pub fn ctime(&self) -> Option<(u64, u32)> {
        self.get(crate::kernel::FATTR_CTIME, |arg| (arg.ctime, arg.ctimensec))
    }

    pub fn lock_owner(&self) -> Option<u64> {
        self.get(crate::kernel::FATTR_LOCKOWNER, |arg| arg.lock_owner)
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        attr: impl AsRef<ReplyAttr>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let attr = attr.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(attr) }).await
    }
}

#[derive(Debug)]
pub struct Readlink<'a> {
    pub(crate) header: &'a RequestHeader,
}

impl Readlink<'_> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    /// Reply to the kernel with the specified link value.
    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        value: impl AsRef<OsStr>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(value.as_ref().as_bytes()).await
    }
}

#[derive(Debug)]
pub struct Symlink<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) name: &'a OsStr,
    pub(crate) link: &'a OsStr,
}

impl<'a> Symlink<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub fn link(&self) -> &OsStr {
        &*self.link
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let entry = entry.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(entry) }).await
    }
}

#[derive(Debug)]
pub struct Mknod<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_mknod_in,
    pub(crate) name: &'a OsStr,
}

impl<'a> Mknod<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub fn mode(&self) -> u32 {
        self.arg.mode
    }

    pub fn rdev(&self) -> u32 {
        self.arg.rdev
    }

    pub fn umask(&self) -> u32 {
        self.arg.umask
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let entry = entry.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(entry) }).await
    }
}

#[derive(Debug)]
pub struct Mkdir<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_mkdir_in,
    pub(crate) name: &'a OsStr,
}

impl<'a> Mkdir<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub fn mode(&self) -> u32 {
        self.arg.mode
    }

    pub fn umask(&self) -> u32 {
        self.arg.umask
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let entry = entry.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(entry) }).await
    }
}

#[derive(Debug)]
pub struct Unlink<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) name: &'a OsStr,
}

impl<'a> Unlink<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub async fn reply<W: ?Sized>(self, writer: &mut ReplyWriter<'_, W>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(&[]).await
    }
}

#[derive(Debug)]
pub struct Rmdir<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) name: &'a OsStr,
}

impl<'a> Rmdir<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub async fn reply<W: ?Sized>(self, writer: &mut ReplyWriter<'_, W>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(&[]).await
    }
}

#[derive(Debug)]
pub struct Rename<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: RenameKind<'a>,
    pub(crate) name: &'a OsStr,
    pub(crate) newname: &'a OsStr,
}

#[derive(Debug)]
pub(crate) enum RenameKind<'a> {
    V1(&'a fuse_rename_in),
    V2(&'a fuse_rename2_in),
}

impl<'a> From<&'a fuse_rename_in> for RenameKind<'a> {
    fn from(arg: &'a fuse_rename_in) -> Self {
        Self::V1(arg)
    }
}

impl<'a> From<&'a fuse_rename2_in> for RenameKind<'a> {
    fn from(arg: &'a fuse_rename2_in) -> Self {
        Self::V2(arg)
    }
}

impl<'a> Rename<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub fn newparent(&self) -> u64 {
        match self.arg {
            RenameKind::V1(arg) => arg.newdir,
            RenameKind::V2(arg) => arg.newdir,
        }
    }

    pub fn newname(&self) -> &OsStr {
        &*self.newname
    }

    pub fn flags(&self) -> u32 {
        match self.arg {
            RenameKind::V1(..) => 0,
            RenameKind::V2(arg) => arg.flags,
        }
    }

    pub async fn reply<W: ?Sized>(self, writer: &mut ReplyWriter<'_, W>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(&[]).await
    }
}

#[derive(Debug)]
pub struct Link<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_link_in,
    pub(crate) newname: &'a OsStr,
}

impl<'a> Link<'a> {
    pub fn ino(&self) -> u64 {
        self.arg.oldnodeid
    }

    pub fn newparent(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn newname(&self) -> &OsStr {
        &*self.newname
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let entry = entry.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(entry) }).await
    }
}

#[derive(Debug)]
pub struct Open<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_open_in,
}

impl<'a> Open<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        out: impl AsRef<ReplyOpen>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let out = out.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(out) }).await
    }
}

#[derive(Debug)]
pub struct Read<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_read_in,
}

impl<'a> Read<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn offset(&self) -> u64 {
        self.arg.offset
    }

    pub fn size(&self) -> u32 {
        self.arg.size
    }

    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    pub fn lock_owner(&self) -> Option<u64> {
        if self.arg.read_flags & crate::kernel::FUSE_READ_LOCKOWNER != 0 {
            Some(self.arg.lock_owner)
        } else {
            None
        }
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        data: impl AsRef<[u8]>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let data = data.as_ref();

        if data.len() <= self.size() as usize {
            writer.reply(data).await
        } else {
            writer.reply_err(libc::ERANGE).await
        }
    }

    pub async fn reply_vectored<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        data: &[&[u8]],
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let data_len: usize = data.iter().map(|t| t.len()).sum();
        if data_len <= self.size() as usize {
            writer.reply_vectored(data).await
        } else {
            writer.reply_err(libc::ERANGE).await
        }
    }
}

#[derive(Debug)]
pub struct Write<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_write_in,
}

impl<'a> Write<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn offset(&self) -> u64 {
        self.arg.offset
    }

    pub fn size(&self) -> u32 {
        self.arg.size
    }

    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    pub fn lock_owner(&self) -> Option<u64> {
        if self.arg.write_flags & crate::kernel::FUSE_WRITE_LOCKOWNER != 0 {
            Some(self.arg.lock_owner)
        } else {
            None
        }
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        out: impl AsRef<ReplyWrite>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let out = out.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(out) }).await
    }
}

#[derive(Debug)]
pub struct Release<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_release_in,
}

impl<'a> Release<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    pub fn lock_owner(&self) -> u64 {
        // NOTE: fuse_release_in.lock_owner is available since ABI 7.8.
        self.arg.lock_owner
    }

    pub fn flush(&self) -> bool {
        self.arg.release_flags & crate::kernel::FUSE_RELEASE_FLUSH != 0
    }

    pub fn flock_release(&self) -> bool {
        self.arg.release_flags & crate::kernel::FUSE_RELEASE_FLOCK_UNLOCK != 0
    }

    pub async fn reply<W: ?Sized>(self, writer: &mut ReplyWriter<'_, W>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(&[]).await
    }
}

#[derive(Debug)]
pub struct Statfs<'a> {
    pub(crate) header: &'a RequestHeader,
}

impl Statfs<'_> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        out: impl AsRef<ReplyStatfs>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let out = out.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(out) }).await
    }
}

#[derive(Debug)]
pub struct Fsync<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_fsync_in,
}

impl<'a> Fsync<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn datasync(&self) -> bool {
        self.arg.fsync_flags & crate::kernel::FUSE_FSYNC_FDATASYNC != 0
    }

    pub async fn reply<W: ?Sized>(self, writer: &mut ReplyWriter<'_, W>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(&[]).await
    }
}

#[derive(Debug)]
pub struct Setxattr<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_setxattr_in,
    pub(crate) name: &'a OsStr,
    pub(crate) value: &'a [u8],
}

impl<'a> Setxattr<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub fn value(&self) -> &[u8] {
        &*self.value
    }

    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    pub async fn reply<W: ?Sized>(self, writer: &mut ReplyWriter<'_, W>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(&[]).await
    }
}

#[derive(Debug)]
pub struct Getxattr<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_getxattr_in,
    pub(crate) name: &'a OsStr,
}

impl<'a> Getxattr<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub fn size(&self) -> u32 {
        self.arg.size
    }

    pub async fn reply_size<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        out: impl AsRef<ReplyXattr>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let out = out.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(out) }).await
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        data: impl AsRef<[u8]>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let data = data.as_ref();

        if data.len() <= self.size() as usize {
            writer.reply(data).await
        } else {
            writer.reply_err(libc::ERANGE).await
        }
    }

    pub async fn reply_vectored<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        data: &[&[u8]],
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let data_len: usize = data.iter().map(|t| t.len()).sum();
        if data_len <= self.size() as usize {
            writer.reply_vectored(data).await
        } else {
            writer.reply_err(libc::ERANGE).await
        }
    }
}

#[derive(Debug)]
pub struct Listxattr<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_getxattr_in,
}

impl<'a> Listxattr<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn size(&self) -> u32 {
        self.arg.size
    }

    pub async fn reply_size<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        out: impl AsRef<ReplyXattr>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let out = out.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(out) }).await
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        data: impl AsRef<[u8]>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let data = data.as_ref();

        if data.len() <= self.size() as usize {
            writer.reply(data).await
        } else {
            writer.reply_err(libc::ERANGE).await
        }
    }

    pub async fn reply_vectored<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        data: &[&[u8]],
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let data_len: usize = data.iter().map(|t| t.len()).sum();
        if data_len <= self.size() as usize {
            writer.reply_vectored(data).await
        } else {
            writer.reply_err(libc::ERANGE).await
        }
    }
}
#[derive(Debug)]
pub struct Removexattr<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) name: &'a OsStr,
}

impl<'a> Removexattr<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub async fn reply<W: ?Sized>(self, writer: &mut ReplyWriter<'_, W>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(&[]).await
    }
}

#[derive(Debug)]
pub struct Flush<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_flush_in,
}

impl<'a> Flush<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn lock_owner(&self) -> u64 {
        self.arg.lock_owner
    }

    pub async fn reply<W: ?Sized>(self, writer: &mut ReplyWriter<'_, W>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(&[]).await
    }
}

#[derive(Debug)]
pub struct Opendir<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_open_in,
}

impl<'a> Opendir<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        out: impl AsRef<ReplyOpendir>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let out = out.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(out) }).await
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ReaddirMode {
    Normal,
    Plus,
}

#[derive(Debug)]
pub struct Readdir<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_read_in,
    pub(crate) mode: ReaddirMode,
}

impl<'a> Readdir<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn offset(&self) -> u64 {
        self.arg.offset
    }

    pub fn size(&self) -> u32 {
        self.arg.size
    }

    pub fn mode(&self) -> ReaddirMode {
        self.mode
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        data: impl AsRef<[u8]>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let data = data.as_ref();

        if data.len() <= self.size() as usize {
            writer.reply(data).await
        } else {
            writer.reply_err(libc::ERANGE).await
        }
    }

    pub async fn reply_vectored<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        data: &[&[u8]],
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let data_len: usize = data.iter().map(|t| t.len()).sum();
        if data_len <= self.size() as usize {
            writer.reply_vectored(data).await
        } else {
            writer.reply_err(libc::ERANGE).await
        }
    }
}

#[derive(Debug)]
pub struct Releasedir<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_release_in,
}

impl<'a> Releasedir<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    pub async fn reply<W: ?Sized>(self, writer: &mut ReplyWriter<'_, W>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(&[]).await
    }
}

#[derive(Debug)]
pub struct Fsyncdir<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_fsync_in,
}

impl<'a> Fsyncdir<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn datasync(&self) -> bool {
        self.arg.fsync_flags & crate::kernel::FUSE_FSYNC_FDATASYNC != 0
    }

    pub async fn reply<W: ?Sized>(self, writer: &mut ReplyWriter<'_, W>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(&[]).await
    }
}

#[derive(Debug)]
pub struct Getlk<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_lk_in,
}

impl<'a> Getlk<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn owner(&self) -> u64 {
        self.arg.owner
    }

    pub fn lk(&self) -> &FileLock {
        FileLock::new(&self.arg.lk)
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        out: impl AsRef<ReplyLk>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let out = out.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(out) }).await
    }
}

#[derive(Debug)]
pub struct Setlk<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_lk_in,
    pub(crate) sleep: bool,
}

impl<'a> Setlk<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn owner(&self) -> u64 {
        self.arg.owner
    }

    pub fn lk(&self) -> &FileLock {
        FileLock::new(&self.arg.lk)
    }

    pub fn sleep(&self) -> bool {
        self.sleep
    }

    pub async fn reply<W: ?Sized>(self, writer: &mut ReplyWriter<'_, W>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(&[]).await
    }
}

#[derive(Debug)]
pub struct Flock<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_lk_in,
    pub(crate) sleep: bool,
}

impl<'a> Flock<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn owner(&self) -> u64 {
        self.arg.owner
    }

    #[allow(clippy::cast_possible_wrap)]
    pub fn op(&self) -> Option<u32> {
        const F_RDLCK: u32 = libc::F_RDLCK as u32;
        const F_WRLCK: u32 = libc::F_WRLCK as u32;
        const F_UNLCK: u32 = libc::F_UNLCK as u32;

        let mut op = match self.arg.lk.typ {
            F_RDLCK => libc::LOCK_SH as u32,
            F_WRLCK => libc::LOCK_EX as u32,
            F_UNLCK => libc::LOCK_UN as u32,
            _ => return None,
        };
        if !self.sleep {
            op |= libc::LOCK_NB as u32;
        }

        Some(op)
    }

    pub async fn reply<W: ?Sized>(self, writer: &mut ReplyWriter<'_, W>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(&[]).await
    }
}

#[derive(Debug)]
pub struct Access<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_access_in,
}

impl<'a> Access<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn mask(&self) -> u32 {
        self.arg.mask
    }

    pub async fn reply<W: ?Sized>(self, writer: &mut ReplyWriter<'_, W>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(&[]).await
    }
}

#[derive(Debug)]
pub struct Create<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_create_in,
    pub(crate) name: &'a OsStr,
}

impl<'a> Create<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub fn mode(&self) -> u32 {
        self.arg.mode
    }

    pub fn umask(&self) -> u32 {
        self.arg.umask
    }

    pub fn open_flags(&self) -> u32 {
        self.arg.flags
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        entry: impl AsRef<ReplyEntry>,
        open: impl AsRef<ReplyOpen>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let entry = entry.as_ref();
        let open = open.as_ref();
        writer
            .reply_vectored(&[unsafe { crate::reply::as_bytes(entry) }, unsafe {
                crate::reply::as_bytes(open)
            }])
            .await
    }
}

#[derive(Debug)]
pub struct Bmap<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_bmap_in,
}

impl<'a> Bmap<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn block(&self) -> u64 {
        self.arg.block
    }

    pub fn blocksize(&self) -> u32 {
        self.arg.blocksize
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        out: impl AsRef<ReplyBmap>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let out = out.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(out) }).await
    }
}

#[derive(Debug)]
pub struct Fallocate<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_fallocate_in,
}

impl<'a> Fallocate<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn offset(&self) -> u64 {
        self.arg.offset
    }

    pub fn length(&self) -> u64 {
        self.arg.length
    }

    pub fn mode(&self) -> u32 {
        self.arg.mode
    }

    pub async fn reply<W: ?Sized>(self, writer: &mut ReplyWriter<'_, W>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        writer.reply(&[]).await
    }
}

#[derive(Debug)]
pub struct CopyFileRange<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_copy_file_range_in,
}

impl<'a> CopyFileRange<'a> {
    pub fn input(&self) -> (u64, u64, u64) {
        (self.header.nodeid(), self.arg.fh_in, self.arg.off_in)
    }

    pub fn output(&self) -> (u64, u64, u64) {
        (self.arg.nodeid_out, self.arg.fh_out, self.arg.off_out)
    }

    pub fn length(&self) -> u64 {
        self.arg.len
    }

    pub fn flags(&self) -> u64 {
        self.arg.flags
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        out: impl AsRef<ReplyWrite>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let out = out.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(out) }).await
    }
}

#[derive(Debug)]
pub struct Poll<'a> {
    pub(crate) header: &'a RequestHeader,
    pub(crate) arg: &'a fuse_poll_in,
}

impl<'a> Poll<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid()
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn events(&self) -> u32 {
        self.arg.events
    }

    pub fn kh(&self) -> Option<u64> {
        if self.arg.flags & crate::kernel::FUSE_POLL_SCHEDULE_NOTIFY != 0 {
            Some(self.arg.kh)
        } else {
            None
        }
    }

    pub async fn reply<W: ?Sized>(
        self,
        writer: &mut ReplyWriter<'_, W>,
        out: impl AsRef<ReplyPoll>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let out = out.as_ref();
        writer.reply(unsafe { crate::reply::as_bytes(out) }).await
    }
}
