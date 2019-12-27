//! Filesystem operations.

use crate::{
    common::{FileLock, Forget},
    context::Context,
    io::Writer,
    kernel::{
        fuse_access_in, //
        fuse_batch_forget_in,
        fuse_bmap_in,
        fuse_copy_file_range_in,
        fuse_create_in,
        fuse_fallocate_in,
        fuse_flush_in,
        fuse_forget_in,
        fuse_fsync_in,
        fuse_getattr_in,
        fuse_getxattr_in,
        fuse_in_header,
        fuse_init_in,
        fuse_interrupt_in,
        fuse_link_in,
        fuse_lk_in,
        fuse_mkdir_in,
        fuse_mknod_in,
        fuse_notify_retrieve_in,
        fuse_opcode,
        fuse_open_in,
        fuse_poll_in,
        fuse_read_in,
        fuse_release_in,
        fuse_rename2_in,
        fuse_rename_in,
        fuse_setattr_in,
        fuse_setxattr_in,
        fuse_write_in,
        FUSE_LK_FLOCK,
    },
    reply::{
        ReplyAttr, //
        ReplyBmap,
        ReplyEntry,
        ReplyLk,
        ReplyOpen,
        ReplyPoll,
        ReplyStatfs,
        ReplyWrite,
        ReplyXattr,
    },
    util::{as_bytes, make_system_time},
};
use std::{
    convert::TryFrom, //
    ffi::OsStr,
    io,
    mem,
    os::unix::ffi::OsStrExt,
    time::SystemTime,
};

/// The kind of FUSE requests received from the kernel.
#[derive(Debug)]
#[allow(missing_docs)]
#[non_exhaustive]
pub enum Operation<'a> {
    Lookup(Lookup<'a>),
    Forget(Forgets<'a>),
    Getattr(Getattr<'a>),
    Setattr(Setattr<'a>),
    Readlink(Readlink<'a>),
    Symlink(Symlink<'a>),
    Mknod(Mknod<'a>),
    Mkdir(Mkdir<'a>),
    Unlink(Unlink<'a>),
    Rmdir(Rmdir<'a>),
    Rename(Rename<'a>),
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
    Interrupt(Interrupt<'a>),
    NotifyReply(NotifyReply<'a>),

    #[doc(hidden)]
    Unknown,
}

// TODO: add operations:
// Ioctl

/// Look up a directory entry by name.
#[derive(Debug)]
pub struct Lookup<'a> {
    header: &'a fuse_in_header,
    name: &'a OsStr,
}

#[allow(missing_docs)]
impl<'a> Lookup<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    pub fn name(&self) -> &OsStr {
        self.name
    }

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let entry = entry.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(entry) }]).await
    }
}

/// Get file attributes.
#[derive(Debug)]
pub struct Getattr<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_getattr_in,
}

#[allow(missing_docs)]
impl<'a> Getattr<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    pub fn fh(&self) -> Option<u64> {
        if self.arg.getattr_flags & crate::kernel::FUSE_GETATTR_FH != 0 {
            Some(self.arg.fh)
        } else {
            None
        }
    }

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        attr: impl AsRef<ReplyAttr>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let attr = attr.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(attr) }]).await
    }
}

/// Set file attributes.
#[derive(Debug)]
pub struct Setattr<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_setattr_in,
}

#[allow(missing_docs)]
impl<'a> Setattr<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
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

    pub fn atime(&self) -> Option<SystemTime> {
        self.atime_raw().map(|(sec, nsec, now)| {
            if now {
                SystemTime::now()
            } else {
                make_system_time((sec, nsec))
            }
        })
    }

    pub fn atime_raw(&self) -> Option<(u64, u32, bool)> {
        self.get(crate::kernel::FATTR_ATIME, |arg| {
            (
                arg.atime,
                arg.atimensec,
                arg.valid & crate::kernel::FATTR_ATIME_NOW != 0,
            )
        })
    }

    pub fn mtime(&self) -> Option<SystemTime> {
        self.mtime_raw().map(|(sec, nsec, now)| {
            if now {
                SystemTime::now()
            } else {
                make_system_time((sec, nsec))
            }
        })
    }

    pub fn mtime_raw(&self) -> Option<(u64, u32, bool)> {
        self.get(crate::kernel::FATTR_MTIME, |arg| {
            (
                arg.mtime,
                arg.mtimensec,
                arg.valid & crate::kernel::FATTR_MTIME_NOW != 0,
            )
        })
    }

    pub fn ctime(&self) -> Option<SystemTime> {
        self.ctime_raw().map(make_system_time)
    }

    pub fn ctime_raw(&self) -> Option<(u64, u32)> {
        self.get(crate::kernel::FATTR_CTIME, |arg| (arg.ctime, arg.ctimensec))
    }

    pub fn lock_owner(&self) -> Option<u64> {
        self.get(crate::kernel::FATTR_LOCKOWNER, |arg| arg.lock_owner)
    }

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        attr: impl AsRef<ReplyAttr>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let attr = attr.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(attr) }]).await
    }
}

/// Read a symbolic link.
#[derive(Debug)]
pub struct Readlink<'a> {
    header: &'a fuse_in_header,
}

#[allow(missing_docs)]
impl Readlink<'_> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Reply to the kernel with the specified link value.
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        value: impl AsRef<OsStr>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[value.as_ref().as_bytes()]).await
    }
}

/// Create a symbolic link.
#[derive(Debug)]
pub struct Symlink<'a> {
    header: &'a fuse_in_header,
    name: &'a OsStr,
    link: &'a OsStr,
}

#[allow(missing_docs)]
impl<'a> Symlink<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub fn link(&self) -> &OsStr {
        &*self.link
    }

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let entry = entry.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(entry) }]).await
    }
}

/// Create a file node.
#[derive(Debug)]
pub struct Mknod<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_mknod_in,
    name: &'a OsStr,
}

#[allow(missing_docs)]
impl<'a> Mknod<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid
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

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let entry = entry.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(entry) }]).await
    }
}

/// Create a directory.
#[derive(Debug)]
pub struct Mkdir<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_mkdir_in,
    name: &'a OsStr,
}

#[allow(missing_docs)]
impl<'a> Mkdir<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid
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

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let entry = entry.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(entry) }]).await
    }
}

/// Remove a file.
#[derive(Debug)]
pub struct Unlink<'a> {
    header: &'a fuse_in_header,
    name: &'a OsStr,
}

#[allow(missing_docs)]
impl<'a> Unlink<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[]).await
    }
}

/// Remove a directory.
#[derive(Debug)]
pub struct Rmdir<'a> {
    header: &'a fuse_in_header,
    name: &'a OsStr,
}

#[allow(missing_docs)]
impl<'a> Rmdir<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[]).await
    }
}

/// Rename a file.
#[derive(Debug)]
pub struct Rename<'a> {
    header: &'a fuse_in_header,
    arg: RenameKind<'a>,
    name: &'a OsStr,
    newname: &'a OsStr,
}

#[derive(Debug)]
enum RenameKind<'a> {
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

#[allow(missing_docs)]
impl<'a> Rename<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid
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

    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[]).await
    }
}

/// Create a hard link.
#[derive(Debug)]
pub struct Link<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_link_in,
    newname: &'a OsStr,
}

#[allow(missing_docs)]
impl<'a> Link<'a> {
    pub fn ino(&self) -> u64 {
        self.arg.oldnodeid
    }

    pub fn newparent(&self) -> u64 {
        self.header.nodeid
    }

    pub fn newname(&self) -> &OsStr {
        &*self.newname
    }

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let entry = entry.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(entry) }]).await
    }
}

/// Open a file.
#[derive(Debug)]
pub struct Open<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_open_in,
}

#[allow(missing_docs)]
impl<'a> Open<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyOpen>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let out = out.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(out) }]).await
    }
}

/// Read data from an opened file.
#[derive(Debug)]
pub struct Read<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_read_in,
}

#[allow(missing_docs)]
impl<'a> Read<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
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

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        data: impl AsRef<[u8]>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let data = data.as_ref();

        if data.len() <= self.size() as usize {
            cx.reply_raw(&[data]).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }

    pub async fn reply_vectored<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        data: &[&[u8]],
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let data_len: usize = data.iter().map(|t| t.len()).sum();
        if data_len <= self.size() as usize {
            cx.reply_raw(data).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }
}

/// Write data to an opened file.
#[derive(Debug)]
pub struct Write<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_write_in,
}

#[allow(missing_docs)]
impl<'a> Write<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
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

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyWrite>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let out = out.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(out) }]).await
    }
}

/// Release an opened file.
#[derive(Debug)]
pub struct Release<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_release_in,
}

#[allow(missing_docs)]
impl<'a> Release<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
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

    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[]).await
    }
}

/// Get the filesystem statistics.
#[derive(Debug)]
pub struct Statfs<'a> {
    header: &'a fuse_in_header,
}

#[allow(missing_docs)]
impl Statfs<'_> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyStatfs>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let out = out.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(out) }]).await
    }
}

/// Synchronize the file contents of an opened file.
///
/// When the parameter `datasync` is true, only the
/// file contents should be flushed and the metadata
/// does not have to be flushed.
#[derive(Debug)]
pub struct Fsync<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_fsync_in,
}

#[allow(missing_docs)]
impl<'a> Fsync<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn datasync(&self) -> bool {
        self.arg.fsync_flags & crate::kernel::FUSE_FSYNC_FDATASYNC != 0
    }

    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[]).await
    }
}

/// Set an extended attribute.
#[derive(Debug)]
pub struct Setxattr<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_setxattr_in,
    name: &'a OsStr,
    value: &'a [u8],
}

#[allow(missing_docs)]
impl<'a> Setxattr<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
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

    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[]).await
    }
}

/// Get an extended attribute.
///
/// The operation should send the length of attribute's value
/// with `reply.size(n)` when `size` is equal to zero.
#[derive(Debug)]
pub struct Getxattr<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_getxattr_in,
    name: &'a OsStr,
}

#[allow(missing_docs)]
impl<'a> Getxattr<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub fn size(&self) -> u32 {
        self.arg.size
    }

    pub async fn reply_size<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyXattr>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let out = out.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(out) }]).await
    }

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        data: impl AsRef<[u8]>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let data = data.as_ref();

        if data.len() <= self.size() as usize {
            cx.reply_raw(&[data]).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }

    pub async fn reply_vectored<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        data: &[&[u8]],
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let data_len: usize = data.iter().map(|t| t.len()).sum();
        if data_len <= self.size() as usize {
            cx.reply_raw(data).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }
}

/// List extended attribute names.
///
/// The attribute names must be seperated by a null character
/// (i.e. `b'\0'`).
///
/// The operation should send the length of attribute names
/// with `reply.size(n)` when `size` is equal to zero.#[derive(Debug)]
#[derive(Debug)]
pub struct Listxattr<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_getxattr_in,
}

#[allow(missing_docs)]
impl<'a> Listxattr<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    pub fn size(&self) -> u32 {
        self.arg.size
    }

    pub async fn reply_size<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyXattr>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let out = out.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(out) }]).await
    }

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        data: impl AsRef<[u8]>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let data = data.as_ref();

        if data.len() <= self.size() as usize {
            cx.reply_raw(&[data]).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }

    pub async fn reply_vectored<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        data: &[&[u8]],
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let data_len: usize = data.iter().map(|t| t.len()).sum();
        if data_len <= self.size() as usize {
            cx.reply_raw(data).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }
}

/// Remove an extended attribute.
#[derive(Debug)]
pub struct Removexattr<'a> {
    header: &'a fuse_in_header,
    name: &'a OsStr,
}

#[allow(missing_docs)]
impl<'a> Removexattr<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[]).await
    }
}

/// Close a file descriptor.
#[derive(Debug)]
pub struct Flush<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_flush_in,
}

#[allow(missing_docs)]
impl<'a> Flush<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn lock_owner(&self) -> u64 {
        self.arg.lock_owner
    }

    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[]).await
    }
}

/// Open a directory.
#[derive(Debug)]
pub struct Opendir<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_open_in,
}

#[allow(missing_docs)]
impl<'a> Opendir<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyOpen>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let out = out.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(out) }]).await
    }
}

/// Read contents from an opened directory.
#[derive(Debug)]
pub struct Readdir<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_read_in,
    is_plus: bool,
}

#[allow(missing_docs)]
impl<'a> Readdir<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
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

    pub fn is_plus(&self) -> bool {
        self.is_plus
    }

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        data: impl AsRef<[u8]>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let data = data.as_ref();

        if data.len() <= self.size() as usize {
            cx.reply_raw(&[data]).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }

    pub async fn reply_vectored<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        data: &[&[u8]],
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let data_len: usize = data.iter().map(|t| t.len()).sum();
        if data_len <= self.size() as usize {
            cx.reply_raw(data).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }
}

/// Release an opened directory.
#[derive(Debug)]
pub struct Releasedir<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_release_in,
}

#[allow(missing_docs)]
impl<'a> Releasedir<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[]).await
    }
}

/// Synchronize an opened directory contents.
///
/// When the parameter `datasync` is true, only the
/// directory contents should be flushed and the metadata
/// does not have to be flushed.
#[derive(Debug)]
pub struct Fsyncdir<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_fsync_in,
}

#[allow(missing_docs)]
impl<'a> Fsyncdir<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    pub fn datasync(&self) -> bool {
        self.arg.fsync_flags & crate::kernel::FUSE_FSYNC_FDATASYNC != 0
    }

    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[]).await
    }
}

/// Test for a POSIX file lock.
#[derive(Debug)]
pub struct Getlk<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_lk_in,
}

#[allow(missing_docs)]
impl<'a> Getlk<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
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

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyLk>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let out = out.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(out) }]).await
    }
}

/// Acquire, modify or release a POSIX file lock.
#[derive(Debug)]
pub struct Setlk<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_lk_in,
    sleep: bool,
}

#[allow(missing_docs)]
impl<'a> Setlk<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
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

    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[]).await
    }
}

/// Acquire, modify or release a BSD file lock.
#[derive(Debug)]
pub struct Flock<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_lk_in,
    sleep: bool,
}

#[allow(missing_docs)]
impl<'a> Flock<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
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

    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[]).await
    }
}

/// Check file access permissions.
#[derive(Debug)]
pub struct Access<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_access_in,
}

#[allow(missing_docs)]
impl<'a> Access<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    pub fn mask(&self) -> u32 {
        self.arg.mask
    }

    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[]).await
    }
}

/// Create and open a file.
#[derive(Debug)]
pub struct Create<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_create_in,
    name: &'a OsStr,
}

#[allow(missing_docs)]
impl<'a> Create<'a> {
    pub fn parent(&self) -> u64 {
        self.header.nodeid
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

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        entry: impl AsRef<ReplyEntry>,
        open: impl AsRef<ReplyOpen>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let entry = entry.as_ref();
        let open = open.as_ref();
        cx.reply_raw(&[
            unsafe { as_bytes(entry) }, //
            unsafe { as_bytes(open) },
        ])
        .await
    }
}

/// Map block index within a file to block index within device.
///
/// This operation makes sense only for filesystems that use
/// block devices, and is called only when the mount options
/// contains `blkdev`.
#[derive(Debug)]
pub struct Bmap<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_bmap_in,
}

#[allow(missing_docs)]
impl<'a> Bmap<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    pub fn block(&self) -> u64 {
        self.arg.block
    }

    pub fn blocksize(&self) -> u32 {
        self.arg.blocksize
    }

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyBmap>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let out = out.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(out) }]).await
    }
}

/// Allocate requested space to an opened file.
#[derive(Debug)]
pub struct Fallocate<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_fallocate_in,
}

#[allow(missing_docs)]
impl<'a> Fallocate<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
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

    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply_raw(&[]).await
    }
}

/// Copy a range of data from an opened file to another.
#[derive(Debug)]
pub struct CopyFileRange<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_copy_file_range_in,
}

#[allow(missing_docs)]
impl<'a> CopyFileRange<'a> {
    pub fn input(&self) -> (u64, u64, u64) {
        (self.header.nodeid, self.arg.fh_in, self.arg.off_in)
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

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyWrite>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let out = out.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(out) }]).await
    }
}

/// Poll for readiness.
///
/// When `kh` is not `None`, the filesystem should send
/// the notification about I/O readiness to the kernel.
#[derive(Debug)]
pub struct Poll<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_poll_in,
}

#[allow(missing_docs)]
impl<'a> Poll<'a> {
    pub fn ino(&self) -> u64 {
        self.header.nodeid
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

    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyPoll>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let out = out.as_ref();
        cx.reply_raw(&[unsafe { as_bytes(out) }]).await
    }
}

/// A set of `Forget`s removed from the kernel's internal caches.
#[allow(missing_docs)]
#[derive(Debug)]
pub enum Forgets<'a> {
    Single(Forget),
    Batch(&'a [Forget]),
}

impl AsRef<[Forget]> for Forgets<'_> {
    fn as_ref(&self) -> &[Forget] {
        match self {
            Self::Single(forget) => unsafe { std::slice::from_raw_parts(forget, 1) },
            Self::Batch(forgets) => &*forgets,
        }
    }
}

/// Receive the reply for a `NOTIFY_RETRIEVE` notification.
#[derive(Debug)]
pub struct NotifyReply<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_notify_retrieve_in,
}

#[allow(missing_docs)]
impl<'a> NotifyReply<'a> {
    #[inline]
    pub fn unique(&self) -> u64 {
        self.header.unique
    }

    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    #[inline]
    pub fn offset(&self) -> u64 {
        self.arg.offset
    }

    #[inline]
    pub fn size(&self) -> u32 {
        self.arg.size
    }
}

/// Interrupt a previous FUSE request.
#[derive(Debug)]
pub struct Interrupt<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_interrupt_in,
}

impl Interrupt<'_> {
    /// Return the target unique ID to interrupt.
    #[inline]
    pub fn unique(&self) -> u64 {
        self.arg.unique
    }
}

// ==== parse ====

#[derive(Debug)]
pub(crate) enum OperationKind<'a> {
    Operation(Operation<'a>),
    Init { arg: &'a fuse_init_in },
    Destroy,
}

impl<'a> OperationKind<'a> {
    pub(crate) fn parse(header: &'a fuse_in_header, bytes: &'a [u8]) -> io::Result<Self> {
        Parser::new(header, bytes).parse()
    }
}

#[derive(Debug)]
struct Parser<'a> {
    header: &'a fuse_in_header,
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Parser<'a> {
    fn new(header: &'a fuse_in_header, bytes: &'a [u8]) -> Self {
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
            .map(|bytes| unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const T, count) })
    }

    fn fetch_str(&mut self) -> io::Result<&'a OsStr> {
        let len = self.bytes[self.offset..]
            .iter()
            .position(|&b| b == b'\0')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "fetch_str: missing \\0"))?;
        self.fetch_bytes(len).map(OsStr::from_bytes)
    }

    fn fetch<T>(&mut self) -> io::Result<&'a T> {
        self.fetch_bytes(mem::size_of::<T>())
            .map(|data| unsafe { &*(data.as_ptr() as *const T) })
    }

    fn parse(&mut self) -> io::Result<OperationKind<'a>> {
        let header = self.header;
        match fuse_opcode::try_from(header.opcode).ok() {
            Some(fuse_opcode::FUSE_INIT) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Init { arg })
            }
            Some(fuse_opcode::FUSE_DESTROY) => Ok(OperationKind::Destroy),
            Some(fuse_opcode::FUSE_FORGET) => {
                let arg = self.fetch::<fuse_forget_in>()?;
                Ok(OperationKind::Operation(Operation::Forget(
                    Forgets::Single(Forget::new(header.nodeid, arg.nlookup)),
                )))
            }
            Some(fuse_opcode::FUSE_BATCH_FORGET) => {
                let arg = self.fetch::<fuse_batch_forget_in>()?;
                let forgets = self.fetch_array(arg.count as usize)?;
                Ok(OperationKind::Operation(Operation::Forget(Forgets::Batch(
                    forgets,
                ))))
            }
            Some(fuse_opcode::FUSE_INTERRUPT) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Interrupt(Interrupt {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_NOTIFY_REPLY) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::NotifyReply(
                    NotifyReply { header, arg },
                )))
            }

            Some(fuse_opcode::FUSE_LOOKUP) => {
                let name = self.fetch_str()?;
                Ok(OperationKind::Operation(Operation::Lookup(Lookup {
                    header,
                    name,
                })))
            }
            Some(fuse_opcode::FUSE_GETATTR) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Getattr(Getattr {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_SETATTR) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Setattr(Setattr {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_READLINK) => {
                Ok(OperationKind::Operation(Operation::Readlink(Readlink {
                    header,
                })))
            }
            Some(fuse_opcode::FUSE_SYMLINK) => {
                let name = self.fetch_str()?;
                let link = self.fetch_str()?;
                Ok(OperationKind::Operation(Operation::Symlink(Symlink {
                    header,
                    name,
                    link,
                })))
            }
            Some(fuse_opcode::FUSE_MKNOD) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(OperationKind::Operation(Operation::Mknod(Mknod {
                    header,
                    arg,
                    name,
                })))
            }
            Some(fuse_opcode::FUSE_MKDIR) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(OperationKind::Operation(Operation::Mkdir(Mkdir {
                    header,
                    arg,
                    name,
                })))
            }
            Some(fuse_opcode::FUSE_UNLINK) => {
                let name = self.fetch_str()?;
                Ok(OperationKind::Operation(Operation::Unlink(Unlink {
                    header,
                    name,
                })))
            }
            Some(fuse_opcode::FUSE_RMDIR) => {
                let name = self.fetch_str()?;
                Ok(OperationKind::Operation(Operation::Rmdir(Rmdir {
                    header,
                    name,
                })))
            }
            Some(fuse_opcode::FUSE_RENAME) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                let newname = self.fetch_str()?;
                Ok(OperationKind::Operation(Operation::Rename(Rename {
                    header,
                    arg: RenameKind::V1(arg),
                    name,
                    newname,
                })))
            }
            Some(fuse_opcode::FUSE_LINK) => {
                let arg = self.fetch()?;
                let newname = self.fetch_str()?;
                Ok(OperationKind::Operation(Operation::Link(Link {
                    header,
                    arg,
                    newname,
                })))
            }
            Some(fuse_opcode::FUSE_OPEN) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Open(Open {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_READ) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Read(Read {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_WRITE) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Write(Write {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_RELEASE) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Release(Release {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_STATFS) => {
                Ok(OperationKind::Operation(Operation::Statfs(Statfs {
                    header,
                })))
            }
            Some(fuse_opcode::FUSE_FSYNC) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Fsync(Fsync {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_SETXATTR) => {
                let arg = self.fetch::<fuse_setxattr_in>()?;
                let name = self.fetch_str()?;
                let value = self.fetch_bytes(arg.size as usize)?;
                Ok(OperationKind::Operation(Operation::Setxattr(Setxattr {
                    header,
                    arg,
                    name,
                    value,
                })))
            }
            Some(fuse_opcode::FUSE_GETXATTR) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(OperationKind::Operation(Operation::Getxattr(Getxattr {
                    header,
                    arg,
                    name,
                })))
            }
            Some(fuse_opcode::FUSE_LISTXATTR) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Listxattr(Listxattr {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_REMOVEXATTR) => {
                let name = self.fetch_str()?;
                Ok(OperationKind::Operation(Operation::Removexattr(
                    Removexattr { header, name },
                )))
            }
            Some(fuse_opcode::FUSE_FLUSH) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Flush(Flush {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_OPENDIR) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Opendir(Opendir {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_READDIR) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Readdir(Readdir {
                    header,
                    arg,
                    is_plus: false,
                })))
            }
            Some(fuse_opcode::FUSE_RELEASEDIR) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Releasedir(
                    Releasedir { header, arg },
                )))
            }
            Some(fuse_opcode::FUSE_FSYNCDIR) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Fsyncdir(Fsyncdir {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_GETLK) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Getlk(Getlk {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_SETLK) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(new_lock_op(header, arg, false)))
            }
            Some(fuse_opcode::FUSE_SETLKW) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(new_lock_op(header, arg, true)))
            }
            Some(fuse_opcode::FUSE_ACCESS) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Access(Access {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_CREATE) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(OperationKind::Operation(Operation::Create(Create {
                    header,
                    arg,
                    name,
                })))
            }
            Some(fuse_opcode::FUSE_BMAP) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Bmap(Bmap {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_FALLOCATE) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Fallocate(Fallocate {
                    header,
                    arg,
                })))
            }
            Some(fuse_opcode::FUSE_READDIRPLUS) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Readdir(Readdir {
                    header,
                    arg,
                    is_plus: true,
                })))
            }
            Some(fuse_opcode::FUSE_RENAME2) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                let newname = self.fetch_str()?;
                Ok(OperationKind::Operation(Operation::Rename(Rename {
                    header,
                    arg: RenameKind::V2(arg),
                    name,
                    newname,
                })))
            }
            Some(fuse_opcode::FUSE_COPY_FILE_RANGE) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::CopyFileRange(
                    CopyFileRange { header, arg },
                )))
            }
            Some(fuse_opcode::FUSE_POLL) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Operation(Operation::Poll(Poll {
                    header,
                    arg,
                })))
            }
            _ => Ok(OperationKind::Operation(Operation::Unknown)),
        }
    }
}

fn new_lock_op<'a>(header: &'a fuse_in_header, arg: &'a fuse_lk_in, sleep: bool) -> Operation<'a> {
    if arg.lk_flags & FUSE_LK_FLOCK != 0 {
        Operation::Flock(Flock { header, arg, sleep })
    } else {
        Operation::Setlk(Setlk { header, arg, sleep })
    }
}
