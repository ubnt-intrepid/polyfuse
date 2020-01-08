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
    util::make_system_time,
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

/// Lookup a directory entry by name.
///
/// If a matching entry is found, the filesystem replies to the kernel
/// with its attribute using `ReplyEntry`.  In addition, the lookup count
/// of the corresponding inode is incremented on success.
///
/// See also the documentation of `ReplyEntry` for tuning the reply parameters.
#[derive(Debug)]
pub struct Lookup<'a> {
    header: &'a fuse_in_header,
    name: &'a OsStr,
}

impl<'a> Lookup<'a> {
    /// Return the inode number of the parent directory.
    #[inline]
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the name of the entry to be looked up.
    #[inline]
    pub fn name(&self) -> &OsStr {
        self.name
    }

    #[inline]
    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(entry.as_ref()).await
    }
}

/// Get file attributes.
///
/// The obtained attribute values are replied using `ReplyAttr`.
///
/// If writeback caching is enabled, the kernel might ignore
/// some of the attribute values, such as `st_size`.
#[derive(Debug)]
pub struct Getattr<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_getattr_in,
}

impl<'a> Getattr<'a> {
    /// Return the inode number for obtaining the attribute value.
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened file, if specified.
    pub fn fh(&self) -> Option<u64> {
        if self.arg.getattr_flags & crate::kernel::FUSE_GETATTR_FH != 0 {
            Some(self.arg.fh)
        } else {
            None
        }
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        attr: impl AsRef<ReplyAttr>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(attr.as_ref()).await
    }
}

/// Set file attributes.
///
/// When the setting of attribute values succeeds, the filesystem replies its value
/// to the kernel using `ReplyAttr`.
#[derive(Debug)]
pub struct Setattr<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_setattr_in,
}

impl<'a> Setattr<'a> {
    /// Return the inode number to be set the attribute values.
    #[inline]
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

    /// Return the handle of opened file, if specified.
    #[inline]
    pub fn fh(&self) -> Option<u64> {
        self.get(crate::kernel::FATTR_FH, |arg| arg.fh)
    }

    /// Return the file mode to be set.
    #[inline]
    pub fn mode(&self) -> Option<u32> {
        self.get(crate::kernel::FATTR_MODE, |arg| arg.mode)
    }

    /// Return the user id to be set.
    #[inline]
    pub fn uid(&self) -> Option<u32> {
        self.get(crate::kernel::FATTR_UID, |arg| arg.uid)
    }

    /// Return the group id to be set.
    #[inline]
    pub fn gid(&self) -> Option<u32> {
        self.get(crate::kernel::FATTR_GID, |arg| arg.gid)
    }

    /// Return the size of the file content to be set.
    #[inline]
    pub fn size(&self) -> Option<u64> {
        self.get(crate::kernel::FATTR_SIZE, |arg| arg.size)
    }

    /// Return the last accessed time to be set.
    #[inline]
    pub fn atime(&self) -> Option<SystemTime> {
        self.atime_raw().map(|(sec, nsec, now)| {
            if now {
                SystemTime::now()
            } else {
                make_system_time((sec, nsec))
            }
        })
    }

    /// Return the last accessed time to be set, in raw form.
    #[inline]
    pub fn atime_raw(&self) -> Option<(u64, u32, bool)> {
        self.get(crate::kernel::FATTR_ATIME, |arg| {
            (
                arg.atime,
                arg.atimensec,
                arg.valid & crate::kernel::FATTR_ATIME_NOW != 0,
            )
        })
    }

    /// Return the last modified time to be set.
    #[inline]
    pub fn mtime(&self) -> Option<SystemTime> {
        self.mtime_raw().map(|(sec, nsec, now)| {
            if now {
                SystemTime::now()
            } else {
                make_system_time((sec, nsec))
            }
        })
    }

    /// Return the last modified time to be set, in raw form.
    #[inline]
    pub fn mtime_raw(&self) -> Option<(u64, u32, bool)> {
        self.get(crate::kernel::FATTR_MTIME, |arg| {
            (
                arg.mtime,
                arg.mtimensec,
                arg.valid & crate::kernel::FATTR_MTIME_NOW != 0,
            )
        })
    }

    /// Return the last creation time to be set.
    #[inline]
    pub fn ctime(&self) -> Option<SystemTime> {
        self.ctime_raw().map(make_system_time)
    }

    /// Return the last creation time to be set, in raw form.
    #[inline]
    pub fn ctime_raw(&self) -> Option<(u64, u32)> {
        self.get(crate::kernel::FATTR_CTIME, |arg| (arg.ctime, arg.ctimensec))
    }

    #[doc(hidden)] // TODO: dox
    #[inline]
    pub fn lock_owner(&self) -> Option<u64> {
        self.get(crate::kernel::FATTR_LOCKOWNER, |arg| arg.lock_owner)
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        attr: impl AsRef<ReplyAttr>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(attr.as_ref()).await
    }
}

/// Read a symbolic link.
#[derive(Debug)]
pub struct Readlink<'a> {
    header: &'a fuse_in_header,
}

impl Readlink<'_> {
    /// Return the inode number to be read the link value.
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        value: impl AsRef<OsStr>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(value.as_ref()).await
    }
}

/// Create a symbolic link.
///
/// When the link is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
#[derive(Debug)]
pub struct Symlink<'a> {
    header: &'a fuse_in_header,
    name: &'a OsStr,
    link: &'a OsStr,
}

impl<'a> Symlink<'a> {
    /// Return the inode number of the parent directory.
    #[inline]
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the name of the symbolic link to create.
    #[inline]
    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    /// Return the contents of the symbolic link.
    #[inline]
    pub fn link(&self) -> &OsStr {
        &*self.link
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(entry.as_ref()).await
    }
}

/// Create a file node.
///
/// When the file node is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
#[derive(Debug)]
pub struct Mknod<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_mknod_in,
    name: &'a OsStr,
}

impl<'a> Mknod<'a> {
    /// Return the inode number of the parent directory.
    #[inline]
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the file name to create.
    #[inline]
    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    /// Return the file type and permissions used when creating the new file.
    #[inline]
    pub fn mode(&self) -> u32 {
        self.arg.mode
    }

    /// Return the device number for special file.
    ///
    /// This value is only meaningful only if the created node is a device file
    /// (i.e. the file type is specified either `S_IFCHR` or `S_IFBLK`).
    #[inline]
    pub fn rdev(&self) -> u32 {
        self.arg.rdev
    }

    #[doc(hidden)] // TODO: dox
    #[inline]
    pub fn umask(&self) -> u32 {
        self.arg.umask
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(entry.as_ref()).await
    }
}

/// Create a directory node.
///
/// When the directory is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
#[derive(Debug)]
pub struct Mkdir<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_mkdir_in,
    name: &'a OsStr,
}

impl<'a> Mkdir<'a> {
    /// Return the inode number of the parent directory where the directory is created.
    #[inline]
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the name of the directory to be created.
    #[inline]
    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    /// Return the file type and permissions used when creating the new directory.
    #[inline]
    pub fn mode(&self) -> u32 {
        self.arg.mode
    }

    #[doc(hidden)] // TODO: dox
    #[inline]
    pub fn umask(&self) -> u32 {
        self.arg.umask
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(entry.as_ref()).await
    }
}

// TODO: description about lookup count.

/// Remove a file.
#[derive(Debug)]
pub struct Unlink<'a> {
    header: &'a fuse_in_header,
    name: &'a OsStr,
}

impl<'a> Unlink<'a> {
    /// Return the inode number of the parent directory.
    #[inline]
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the file name to be removed.
    #[inline]
    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(()).await
    }
}

/// Remove a directory.
#[derive(Debug)]
pub struct Rmdir<'a> {
    header: &'a fuse_in_header,
    name: &'a OsStr,
}

// TODO: description about lookup count.

impl<'a> Rmdir<'a> {
    /// Return the inode number of the parent directory.
    #[inline]
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the directory name to be removed.
    #[inline]
    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(()).await
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

impl<'a> Rename<'a> {
    /// Return the inode number of the old parent directory.
    #[inline]
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the old name of the target node.
    #[inline]
    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    /// Return the inode number of the new parent directory.
    #[inline]
    pub fn newparent(&self) -> u64 {
        match self.arg {
            RenameKind::V1(arg) => arg.newdir,
            RenameKind::V2(arg) => arg.newdir,
        }
    }

    /// Return the new name of the target node.
    #[inline]
    pub fn newname(&self) -> &OsStr {
        &*self.newname
    }

    /// Return the rename flags.
    #[inline]
    pub fn flags(&self) -> u32 {
        match self.arg {
            RenameKind::V1(..) => 0,
            RenameKind::V2(arg) => arg.flags,
        }
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(()).await
    }
}

/// Create a hard link.
///
/// When the link is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
#[derive(Debug)]
pub struct Link<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_link_in,
    newname: &'a OsStr,
}

impl<'a> Link<'a> {
    /// Return the *original* inode number which links to the created hard link.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.arg.oldnodeid
    }

    /// Return the inode number of the parent directory where the hard link is created.
    #[inline]
    pub fn newparent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the name of the hard link to be created.
    #[inline]
    pub fn newname(&self) -> &OsStr {
        &*self.newname
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        entry: impl AsRef<ReplyEntry>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(entry.as_ref()).await
    }
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
#[derive(Debug)]
pub struct Open<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_open_in,
}

// TODO: Description of behavior when writeback caching is enabled.

impl<'a> Open<'a> {
    /// Return the inode number to be opened.
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

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
    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyOpen>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(out.as_ref()).await
    }
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
#[derive(Debug)]
pub struct Read<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_read_in,
}

impl<'a> Read<'a> {
    /// Return the inode number to be read.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened file.
    #[inline]
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return the starting position of the content to be read.
    #[inline]
    pub fn offset(&self) -> u64 {
        self.arg.offset
    }

    /// Return the length of the data to be read.
    #[inline]
    pub fn size(&self) -> u32 {
        self.arg.size
    }

    /// Return the flags specified at opening the file.
    #[inline]
    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    /// Return the identifier of lock owner, if specified.
    #[inline]
    pub fn lock_owner(&self) -> Option<u64> {
        if self.arg.read_flags & crate::kernel::FUSE_READ_LOCKOWNER != 0 {
            Some(self.arg.lock_owner)
        } else {
            None
        }
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
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
            cx.reply(data).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
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
            cx.reply(data).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }
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
#[derive(Debug)]
pub struct Write<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_write_in,
}

impl<'a> Write<'a> {
    /// Return the inode number to be written.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened file.
    #[inline]
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return the starting position of contents to be written.
    #[inline]
    pub fn offset(&self) -> u64 {
        self.arg.offset
    }

    /// Return the length of contents to be written.
    #[inline]
    pub fn size(&self) -> u32 {
        self.arg.size
    }

    /// Return the flags specified at opening the file.
    #[inline]
    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    /// Return the identifier of lock owner.
    #[inline]
    pub fn lock_owner(&self) -> Option<u64> {
        if self.arg.write_flags & crate::kernel::FUSE_WRITE_LOCKOWNER != 0 {
            Some(self.arg.lock_owner)
        } else {
            None
        }
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyWrite>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(out.as_ref()).await
    }
}

/// Release an opened file.
#[derive(Debug)]
pub struct Release<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_release_in,
}

impl<'a> Release<'a> {
    /// Return the inode number of opened file.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened file.
    #[inline]
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return the flags specified at opening the file.
    #[inline]
    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    /// Return the identifier of lock owner.
    #[inline]
    pub fn lock_owner(&self) -> u64 {
        // NOTE: fuse_release_in.lock_owner is available since ABI 7.8.
        self.arg.lock_owner
    }

    /// Return whether the operation indicates a flush.
    #[inline]
    pub fn flush(&self) -> bool {
        self.arg.release_flags & crate::kernel::FUSE_RELEASE_FLUSH != 0
    }

    /// Return whether the `flock` locks for this file should be released.
    #[inline]
    pub fn flock_release(&self) -> bool {
        self.arg.release_flags & crate::kernel::FUSE_RELEASE_FLOCK_UNLOCK != 0
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(()).await
    }
}

/// Get the filesystem statistics.
///
/// The obtained statistics must be sent to the kernel using `ReplyStatfs`.
#[derive(Debug)]
pub struct Statfs<'a> {
    header: &'a fuse_in_header,
}

impl Statfs<'_> {
    /// Return the inode number or `0` which means "undefined".
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyStatfs>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(out.as_ref()).await
    }
}

/// Synchronize the file contents.
#[derive(Debug)]
pub struct Fsync<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_fsync_in,
}

impl<'a> Fsync<'a> {
    /// Return the inode number to be synchronized.
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened file.
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return whether to synchronize only the file contents.
    ///
    /// When this method returns `true`, the metadata does not have to be flushed.
    #[inline]
    pub fn datasync(&self) -> bool {
        self.arg.fsync_flags & crate::kernel::FUSE_FSYNC_FDATASYNC != 0
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(()).await
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

impl<'a> Setxattr<'a> {
    /// Return the inode number to set the value of extended attribute.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the name of extended attribute to be set.
    #[inline]
    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    /// Return the value of extended attribute.
    #[inline]
    pub fn value(&self) -> &[u8] {
        &*self.value
    }

    /// Return the flags that specifies the meanings of this operation.
    #[inline]
    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(()).await
    }
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
#[derive(Debug)]
pub struct Getxattr<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_getxattr_in,
    name: &'a OsStr,
}

impl<'a> Getxattr<'a> {
    /// Return the inode number to be get the extended attribute.
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the name of the extend attribute.
    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    /// Return the maximum length of the attribute value to be replied.
    pub fn size(&self) -> u32 {
        self.arg.size
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply_size<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyXattr>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(out.as_ref()).await
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
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
            cx.reply(data).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
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
            cx.reply(data).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }
}

/// List extended attribute names.
///
/// Each element of the attribute names list must be null-terminated.
/// As with `Getxattr`, the filesystem must send the data length of the attribute
/// names using `ReplyXattr` if `size` is zero.
#[derive(Debug)]
pub struct Listxattr<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_getxattr_in,
}

impl<'a> Listxattr<'a> {
    /// Return the inode number to be obtained the attribute names.
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the maximum length of the attribute names to be replied.
    pub fn size(&self) -> u32 {
        self.arg.size
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply_size<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyXattr>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(out.as_ref()).await
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
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
            cx.reply(data).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
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
            cx.reply(data).await
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

impl<'a> Removexattr<'a> {
    /// Return the inode number to remove the extended attribute.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the name of extended attribute to be removed.
    #[inline]
    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(()).await
    }
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
#[derive(Debug)]
pub struct Flush<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_flush_in,
}

impl<'a> Flush<'a> {
    /// Return the inode number of target file.
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened file.
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return the identifier of lock owner.
    pub fn lock_owner(&self) -> u64 {
        self.arg.lock_owner
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(()).await
    }
}

/// Open a directory.
///
/// If the directory is successfully opened, the filesystem must send
/// the identifier to the opened directory handle using `ReplyOpen`.
#[derive(Debug)]
pub struct Opendir<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_open_in,
}

impl<'a> Opendir<'a> {
    /// Return the inode number to be opened.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the open flags.
    #[inline]
    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyOpen>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(out.as_ref()).await
    }
}

/// Read contents from an opened directory.
#[derive(Debug)]
pub struct Readdir<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_read_in,
    is_plus: bool,
}

// TODO: description about `offset` and `is_plus`.

impl<'a> Readdir<'a> {
    /// Return the inode number to be read.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened directory.
    #[inline]
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return the *offset* value to continue reading the directory stream.
    #[inline]
    pub fn offset(&self) -> u64 {
        self.arg.offset
    }

    /// Return the maximum length of returned data.
    #[inline]
    pub fn size(&self) -> u32 {
        self.arg.size
    }

    /// Return whether the operation is "plus" mode or not.
    #[inline]
    pub fn is_plus(&self) -> bool {
        self.is_plus
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
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
            cx.reply(data).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
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
            cx.reply(data).await
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

impl<'a> Releasedir<'a> {
    /// Return the inode number of opened directory.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened directory.
    #[inline]
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return the flags specified at opening the directory.
    #[inline]
    pub fn flags(&self) -> u32 {
        self.arg.flags
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(()).await
    }
}

/// Synchronize the directory contents.
#[derive(Debug)]
pub struct Fsyncdir<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_fsync_in,
}

impl<'a> Fsyncdir<'a> {
    /// Return the inode number to be synchronized.
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened directory.
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return whether to synchronize only the directory contents.
    ///
    /// When this method returns `true`, the metadata does not have to be flushed.
    #[inline]
    pub fn datasync(&self) -> bool {
        self.arg.fsync_flags & crate::kernel::FUSE_FSYNC_FDATASYNC != 0
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(()).await
    }
}

/// Test for a POSIX file lock.
///
/// The lock result must be replied using `ReplyLk`.
#[derive(Debug)]
pub struct Getlk<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_lk_in,
}

impl<'a> Getlk<'a> {
    /// Return the inode number to be tested the lock.
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened file.
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return the identifier of lock owner.
    pub fn owner(&self) -> u64 {
        self.arg.owner
    }

    /// Return the lock information for testing.
    pub fn lk(&self) -> &FileLock {
        FileLock::new(&self.arg.lk)
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyLk>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(out.as_ref()).await
    }
}

/// Acquire, modify or release a POSIX file lock.
#[derive(Debug)]
pub struct Setlk<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_lk_in,
    sleep: bool,
}

impl<'a> Setlk<'a> {
    /// Return the inode number to be obtained the lock.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened file.
    #[inline]
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return the identifier of lock owner.
    #[inline]
    pub fn owner(&self) -> u64 {
        self.arg.owner
    }

    /// Return the lock information to be obtained.
    #[inline]
    pub fn lk(&self) -> &FileLock {
        FileLock::new(&self.arg.lk)
    }

    /// Return whether the locking operation might sleep until a lock is obtained.
    #[inline]
    pub fn sleep(&self) -> bool {
        self.sleep
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(()).await
    }
}

/// Acquire, modify or release a BSD file lock.
#[derive(Debug)]
pub struct Flock<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_lk_in,
    sleep: bool,
}

impl<'a> Flock<'a> {
    /// Return the target inode number.
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened file.
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return the identifier of lock owner.
    pub fn owner(&self) -> u64 {
        self.arg.owner
    }

    /// Return the locking operation.
    ///
    /// See [`flock(2)`][flock] for details.
    ///
    /// [flock]: http://man7.org/linux/man-pages/man2/flock.2.html
    #[allow(clippy::cast_possible_wrap)]
    #[inline]
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

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(()).await
    }
}

/// Check file access permissions.
#[derive(Debug)]
pub struct Access<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_access_in,
}

impl<'a> Access<'a> {
    /// Return the inode number subject to the access permission check.
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the requested access mode.
    pub fn mask(&self) -> u32 {
        self.arg.mask
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(()).await
    }
}

/// Create and open a file.
///
/// This operation is a combination of `Mknod` and `Open`. If an `ENOSYS` error is returned
/// for this operation, those operations will be used instead.
///
/// If the file is successfully created and opened, a pair of `ReplyEntry` and `ReplyOpen`
/// with the corresponding attribute values and the file handle must be sent to the kernel.
#[derive(Debug)]
pub struct Create<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_create_in,
    name: &'a OsStr,
}

impl<'a> Create<'a> {
    /// Return the inode number of the parent directory.
    ///
    /// This is the same as `Mknod::parent`.
    #[inline]
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the file name to crate.
    ///
    /// This is the same as `Mknod::name`.
    #[inline]
    pub fn name(&self) -> &OsStr {
        &*self.name
    }

    /// Return the file type and permissions used when creating the new file.
    ///
    /// This is the same as `Mknod::mode`.
    #[inline]
    pub fn mode(&self) -> u32 {
        self.arg.mode
    }

    #[doc(hidden)] // TODO: dox
    pub fn umask(&self) -> u32 {
        self.arg.umask
    }

    /// Return the open flags.
    ///
    /// This is the same as `Open::flags`.
    #[inline]
    pub fn open_flags(&self) -> u32 {
        self.arg.flags
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    #[inline]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        entry: impl AsRef<ReplyEntry>,
        open: impl AsRef<ReplyOpen>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply((entry.as_ref(), open.as_ref())).await
    }
}

/// Map block index within a file to block index within device.
///
/// The mapping result must be replied using `ReplyBmap`.
///
/// This operation makes sense only for filesystems that use
/// block devices, and is called only when the mount options
/// contains `blkdev`.
#[derive(Debug)]
pub struct Bmap<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_bmap_in,
}

impl<'a> Bmap<'a> {
    /// Return the inode number of the file node to be mapped.
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the block index to be mapped.
    pub fn block(&self) -> u64 {
        self.arg.block
    }

    /// Returns the unit of block index.
    pub fn blocksize(&self) -> u32 {
        self.arg.blocksize
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyBmap>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(out.as_ref()).await
    }
}

/// Allocate requested space.
///
/// If this operation is successful, the filesystem shall not report
/// the error caused by the lack of free spaces to subsequent write
/// requests.
#[derive(Debug)]
pub struct Fallocate<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_fallocate_in,
}

impl<'a> Fallocate<'a> {
    /// Return the number of target inode to be allocated the space.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle for opened file.
    #[inline]
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return the starting point of region to be allocated.
    #[inline]
    pub fn offset(&self) -> u64 {
        self.arg.offset
    }

    /// Return the length of region to be allocated.
    #[inline]
    pub fn length(&self) -> u64 {
        self.arg.length
    }

    /// Return the mode that specifies how to allocate the region.
    ///
    /// See [`fallocate(2)`][fallocate] for details.
    ///
    /// [fallocate]: http://man7.org/linux/man-pages/man2/fallocate.2.html
    #[inline]
    pub fn mode(&self) -> u32 {
        self.arg.mode
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(self, cx: &mut Context<'_, T>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(()).await
    }
}

/// Copy a range of data from an opened file to another.
///
/// The length of copied data must be replied using `ReplyWrite`.
#[derive(Debug)]
pub struct CopyFileRange<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_copy_file_range_in,
}

impl<'a> CopyFileRange<'a> {
    #[doc(hidden)]
    #[deprecated(
        since = "0.3.1",
        note = "use `ino_in`, `fh_in` or `offset_in` instead."
    )]
    pub fn input(&self) -> (u64, u64, u64) {
        (self.ino_in(), self.fh_in(), self.offset_in())
    }

    /// Return the inode number of source file.
    #[inline]
    pub fn ino_in(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the file handle of source file.
    #[inline]
    pub fn fh_in(&self) -> u64 {
        self.arg.fh_in
    }

    /// Return the starting point of source file where the data should be read.
    #[inline]
    pub fn offset_in(&self) -> u64 {
        self.arg.fh_in
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.1",
        note = "use `ino_out`, `fh_out` or `offset_out` instead."
    )]
    pub fn output(&self) -> (u64, u64, u64) {
        (self.arg.nodeid_out, self.arg.fh_out, self.arg.off_out)
    }

    /// Return the inode number of target file.
    #[inline]
    pub fn ino_out(&self) -> u64 {
        self.arg.nodeid_out
    }

    /// Return the file handle of target file.
    #[inline]
    pub fn fh_out(&self) -> u64 {
        self.arg.fh_out
    }

    /// Return the starting point of target file where the data should be written.
    #[inline]
    pub fn offset_out(&self) -> u64 {
        self.arg.fh_out
    }

    /// Return the maximum size of data to copy.
    #[inline]
    pub fn length(&self) -> u64 {
        self.arg.len
    }

    /// Return the flag value for `copy_file_range` syscall.
    #[inline]
    pub fn flags(&self) -> u64 {
        self.arg.flags
    }

    #[doc(hidden)]
    #[inline]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyWrite>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(out.as_ref()).await
    }
}

/// Poll for readiness.
///
/// The mask of ready poll events must be replied using `ReplyPoll`.
#[derive(Debug)]
pub struct Poll<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_poll_in,
}

impl<'a> Poll<'a> {
    /// Return the inode number to check the I/O readiness.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened file.
    #[inline]
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return the requested poll events.
    #[inline]
    pub fn events(&self) -> u32 {
        self.arg.events
    }

    /// Return the handle to this poll.
    ///
    /// If the returned value is not `None`, the filesystem should send the notification
    /// when the corresponding I/O will be ready.
    #[inline]
    pub fn kh(&self) -> Option<u64> {
        if self.arg.flags & crate::kernel::FUSE_POLL_SCHEDULE_NOTIFY != 0 {
            Some(self.arg.kh)
        } else {
            None
        }
    }

    #[doc(hidden)]
    #[deprecated(
        since = "0.3.3",
        note = "This method will be removed in the future version."
    )]
    pub async fn reply<T: ?Sized>(
        self,
        cx: &mut Context<'_, T>,
        out: impl AsRef<ReplyPoll>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        cx.reply(out.as_ref()).await
    }
}

/// A set of `Forget`s removed from the kernel's internal caches.
#[derive(Debug)]
pub enum Forgets<'a> {
    #[allow(missing_docs)]
    Single(Forget),
    #[allow(missing_docs)]
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

/// A reply to a `NOTIFY_RETRIEVE` notification.
#[derive(Debug)]
pub struct NotifyReply<'a> {
    header: &'a fuse_in_header,
    arg: &'a fuse_notify_retrieve_in,
}

impl<'a> NotifyReply<'a> {
    /// Return the unique ID of the corresponding notification message.
    #[inline]
    pub fn unique(&self) -> u64 {
        self.header.unique
    }

    /// Return the inode number corresponding with the cache data.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the starting position of the cache data.
    #[inline]
    pub fn offset(&self) -> u64 {
        self.arg.offset
    }

    /// Return the length of the retrieved cache data.
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
    /// Return the target unique ID to be interrupted.
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
        self.fetch_bytes(len).map(|s| {
            self.offset = std::cmp::min(self.bytes.len(), self.offset + 1);
            OsStr::from_bytes(s)
        })
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

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn parse_lookup() {
        let name = CString::new("foo").unwrap();
        let parent = 1;

        let header = fuse_in_header {
            len: (mem::size_of::<fuse_in_header>() + name.as_bytes_with_nul().len()) as u32,
            opcode: crate::kernel::FUSE_LOOKUP,
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
            OperationKind::Operation(Operation::Lookup(op)) => {
                assert_eq!(op.parent(), parent);
                assert_eq!(op.name().as_bytes(), name.as_bytes());
            }
            _ => panic!("incorret operation is returned"),
        }
    }

    #[test]
    fn parse_symlink() {
        let name = CString::new("foo").unwrap();
        let link = CString::new("bar").unwrap();
        let parent = 1;

        let header = fuse_in_header {
            len: (mem::size_of::<fuse_in_header>()
                + name.as_bytes_with_nul().len()
                + link.as_bytes_with_nul().len()) as u32,
            opcode: crate::kernel::FUSE_SYMLINK,
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
            OperationKind::Operation(Operation::Symlink(op)) => {
                assert_eq!(op.parent(), parent);
                assert_eq!(op.name().as_bytes(), name.as_bytes());
                assert_eq!(op.link().as_bytes(), link.as_bytes());
            }
            _ => panic!("incorret operation is returned"),
        }
    }
}
