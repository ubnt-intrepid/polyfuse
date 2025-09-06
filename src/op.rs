use crate::types::{
    DeviceID, FileID, FileLock, FileMode, FilePermissions, LockOwnerID, NodeID, NotifyID,
    PollEvents, PollWakeupID, RequestID, GID, PID, UID,
};
use bitflags::bitflags;
use polyfuse_kernel::*;
use std::{ffi::OsStr, fmt, os::unix::fs::OpenOptionsExt, time::Duration};

pub trait Op {
    type Lookup: Lookup;
    type Getattr: Getattr;
    type Setattr: Setattr;
    type Readlink: Readlink;
    type Symlink: Symlink;
    type Mknod: Mknod;
    type Mkdir: Mkdir;
    type Unlink: Unlink;
    type Rmdir: Rmdir;
    type Rename: Rename;
    type Link: Link;
    type Open: Open;
    type Read: Read;
    type Write: Write;
    type Release: Release;
    type Statfs: Statfs;
    type Fsync: Fsync;
    type Setxattr: Setxattr;
    type Getxattr: Getxattr;
    type Listxattr: Listxattr;
    type Removexattr: Removexattr;
    type Flush: Flush;
    type Opendir: Opendir;
    type Readdir: Readdir;
    type Releasedir: Releasedir;
    type Fsyncdir: Fsyncdir;

    type Forgets: AsRef<[Forget]>;
    type Interrupt: Interrupt;
    type NotifyReply: NotifyReply;
}

/// The kind of filesystem operation requested by the kernel.
#[derive(Debug)]
#[non_exhaustive]
pub enum Operation<T: Op> {
    Lookup(T::Lookup),
    Getattr(T::Getattr),
    Setattr(T::Setattr),
    Readlink(T::Readlink),
    Symlink(T::Symlink),
    Mknod(T::Mknod),
    Mkdir(T::Mkdir),
    Unlink(T::Unlink),
    Rmdir(T::Rmdir),
    Rename(T::Rename),
    Link(T::Link),
    Open(T::Open),
    Read(T::Read),
    Write(T::Write),
    Release(T::Release),
    Statfs(T::Statfs),
    Fsync(T::Fsync),
    Setxattr(T::Setxattr),
    Getxattr(T::Getxattr),
    Listxattr(T::Listxattr),
    Removexattr(T::Removexattr),
    Flush(T::Flush),
    Opendir(T::Opendir),
    Readdir(T::Readdir),
    Releasedir(T::Releasedir),
    Fsyncdir(T::Fsyncdir),
    // Getlk(Getlk<'op>),
    // Setlk(Setlk<'op>),
    // Flock(Flock<'op>),
    // Access(Access<'op>),
    // Create(Create<'op>),
    // Bmap(Bmap<'op>),
    // Fallocate(Fallocate<'op>),
    // CopyFileRange(CopyFileRange<'op>),
    // Poll(Poll<'op>),
    // Lseek(Lseek<'op>),
    Forget(T::Forgets),
    Interrupt(T::Interrupt),
    NotifyReply(T::NotifyReply),
}

/// A forget information.
#[repr(transparent)]
pub struct Forget {
    forget: fuse_forget_one,
}

impl fmt::Debug for Forget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Forget").finish()
    }
}

impl Forget {
    /// Return the inode number of the target inode.
    #[inline]
    pub fn ino(&self) -> NodeID {
        NodeID::from_raw(self.forget.nodeid)
    }

    /// Return the released lookup count of the target inode.
    #[inline]
    pub fn nlookup(&self) -> u64 {
        self.forget.nlookup
    }
}

/// A reply to a `NOTIFY_RETRIEVE` notification.
pub trait NotifyReply {
    /// Return the unique ID of the corresponding notification message.
    fn unique(&self) -> NotifyID;

    /// Return the inode number corresponding with the cache data.
    fn ino(&self) -> NodeID;

    /// Return the starting position of the cache data.
    fn offset(&self) -> u64;

    /// Return the length of the retrieved cache data.
    fn size(&self) -> u32;
}

/// Interrupt a previous FUSE request.
pub trait Interrupt {
    /// Return the target unique ID to be interrupted.
    fn interrupted(&self) -> RequestID;
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
    fn parent(&self) -> NodeID;

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
    fn ino(&self) -> NodeID;

    /// Return the handle of opened file, if specified.
    fn fh(&self) -> Option<FileID>;
}

/// Set file attributes.
///
/// When the setting of attribute values succeeds, the filesystem replies its value
/// to the kernel using `ReplyAttr`.
pub trait Setattr {
    /// Return the inode number to be set the attribute values.
    fn ino(&self) -> NodeID;

    /// Return the handle of opened file, if specified.
    fn fh(&self) -> Option<FileID>;

    /// Return the file mode to be set.
    fn mode(&self) -> Option<FileMode>;

    /// Return the user id to be set.
    fn uid(&self) -> Option<UID>;

    /// Return the group id to be set.
    fn gid(&self) -> Option<GID>;

    /// Return the size of the file content to be set.
    fn size(&self) -> Option<u64>;

    /// Return the last accessed time to be set.
    fn atime(&self) -> Option<SetAttrTime>;

    /// Return the last modified time to be set.
    fn mtime(&self) -> Option<SetAttrTime>;

    /// Return the last creation time to be set.
    fn ctime(&self) -> Option<Duration>;

    /// Return the identifier of lock owner.
    fn lock_owner(&self) -> Option<LockOwnerID>;
}

/// The time value requested to be set.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum SetAttrTime {
    /// Set the specified time value.
    Timespec(Duration),

    /// Set the current time.
    Now,
}

/// Read a symbolic link.
pub trait Readlink {
    /// Return the inode number to be read the link value.
    fn ino(&self) -> NodeID;
}

/// Create a symbolic link.
///
/// When the link is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
pub trait Symlink {
    /// Return the inode number of the parent directory.
    fn parent(&self) -> NodeID;

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
    fn parent(&self) -> NodeID;

    /// Return the file name to create.
    fn name(&self) -> &OsStr;

    /// Return the file type and permissions used when creating the new file.
    fn mode(&self) -> FileMode;

    /// Return the device number for special file.
    ///
    /// This value is meaningful only if the created node is a device file
    /// (i.e. the file type is specified either `S_IFCHR` or `S_IFBLK`).
    fn rdev(&self) -> DeviceID;

    fn umask(&self) -> u32;
}

/// Create a directory node.
///
/// When the directory is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
pub trait Mkdir {
    /// Return the inode number of the parent directory where the directory is created.
    fn parent(&self) -> NodeID;

    /// Return the name of the directory to be created.
    fn name(&self) -> &OsStr;

    /// Return the file type and permissions used when creating the new directory.
    fn permissions(&self) -> FilePermissions;

    fn umask(&self) -> u32;
}

// TODO: description about lookup count.

/// Remove a file.
pub trait Unlink {
    /// Return the inode number of the parent directory.
    fn parent(&self) -> NodeID;

    /// Return the file name to be removed.
    fn name(&self) -> &OsStr;
}

/// Remove a directory.
pub trait Rmdir {
    /// Return the inode number of the parent directory.
    fn parent(&self) -> NodeID;

    /// Return the directory name to be removed.
    fn name(&self) -> &OsStr;
}

/// Rename a file.
pub trait Rename {
    /// Return the inode number of the old parent directory.
    fn parent(&self) -> NodeID;

    /// Return the old name of the target node.
    fn name(&self) -> &OsStr;

    /// Return the inode number of the new parent directory.
    fn newparent(&self) -> NodeID;

    /// Return the new name of the target node.
    fn newname(&self) -> &OsStr;

    /// Return the rename flags.
    fn flags(&self) -> RenameFlags;
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct RenameFlags: u32 {
        const EXCHANGE = libc::RENAME_EXCHANGE;
        const NOREPLACE = libc::RENAME_NOREPLACE;
        const WHITEOUT = libc::RENAME_WHITEOUT;
    }
}

/// Create a hard link.
///
/// When the link is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
pub trait Link {
    /// Return the *original* inode number which links to the created hard link.
    fn ino(&self) -> NodeID;

    /// Return the inode number of the parent directory where the hard link is created.
    fn newparent(&self) -> NodeID;

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
    fn ino(&self) -> NodeID;

    /// Return the access mode and creation/status flags of the opened file.
    fn options(&self) -> OpenOptions;
}

/// The compound type of the access mode and auxiliary flags of opened files.
///
/// * If the mount option contains `-o default_permissions`, the access mode
///   flags (`O_RDONLY`, `O_WRONLY` and `O_RDWR`) might be handled by the kernel
///   and in that case, these flags are omitted before issuing the request.
///   Otherwise, the filesystem should handle these flags and return an `EACCES`
///   error when provided access mode is invalid.
/// * Some parts of the creating flags (`O_CREAT`, `O_EXCL` and `O_NOCTTY`) are
///   removed and these flags are handled by the kernel.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct OpenOptions {
    raw: u32,
}

impl OpenOptions {
    pub(crate) const fn from_raw(raw: u32) -> Self {
        Self { raw }
    }

    pub const fn access_mode(self) -> Option<AccessMode> {
        match self.raw as i32 & libc::O_ACCMODE {
            libc::O_RDONLY => Some(AccessMode::ReadOnly),
            libc::O_WRONLY => Some(AccessMode::WriteOnly),
            libc::O_RDWR => Some(AccessMode::ReadWrite),
            _ => None,
        }
    }

    pub const fn flags(self) -> OpenFlags {
        OpenFlags::from_bits_truncate(self.raw)
    }

    pub const fn remove(mut self, flags: OpenFlags) -> Self {
        self.raw &= !flags.bits();
        self
    }
}

impl From<OpenOptions> for std::fs::OpenOptions {
    fn from(src: OpenOptions) -> Self {
        let mut options = std::fs::OpenOptions::new();
        match src.access_mode() {
            Some(AccessMode::ReadOnly) => {
                options.read(true);
            }
            Some(AccessMode::WriteOnly) => {
                options.write(true);
            }
            Some(AccessMode::ReadWrite) => {
                options.read(true).write(true);
            }
            _ => (),
        }
        options.custom_flags(src.flags().bits() as i32);
        options
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AccessMode {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

impl AccessMode {
    pub const fn into_raw(self) -> i32 {
        match self {
            Self::ReadOnly => libc::O_RDONLY,
            Self::WriteOnly => libc::O_WRONLY,
            Self::ReadWrite => libc::O_RDWR,
        }
    }
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct OpenFlags: u32 {
        // file creation flags.
        const CLOEXEC = libc::O_CLOEXEC as u32;
        const DIRECTORY = libc::O_DIRECTORY as u32;
        const NOFOLLOW = libc::O_NOFOLLOW as u32;
        const TMPFILE = libc::O_TMPFILE as u32;
        const TRUNC = libc::O_TRUNC as u32;

        // file status flags.
        const APPEND = libc::O_APPEND as u32;
        const ASYNC = libc::O_ASYNC as u32;
        const DIRECT = libc::O_DIRECT as u32;
        const DSYNC = libc::O_DSYNC as u32;
        const LARGEFILE = libc::O_LARGEFILE as u32;
        const NOATIME = libc::O_NOATIME as u32;
        const NONBLOCK = libc::O_NONBLOCK as u32;
        const NDELAY = libc::O_NDELAY as u32;
        const PATH = libc::O_PATH as u32;
        const SYNC = libc::O_SYNC as u32;
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
pub trait Read {
    /// Return the inode number to be read.
    fn ino(&self) -> NodeID;

    /// Return the handle of opened file.
    fn fh(&self) -> FileID;

    /// Return the starting position of the content to be read.
    fn offset(&self) -> u64;

    /// Return the length of the data to be read.
    fn size(&self) -> u32;

    /// Return the flags specified at opening the file.
    fn options(&self) -> OpenOptions;

    /// Return the identifier of lock owner.
    fn lock_owner(&self) -> Option<LockOwnerID>;
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
    fn ino(&self) -> NodeID;

    /// Return the handle of opened file.
    fn fh(&self) -> FileID;

    /// Return the starting position of contents to be written.
    fn offset(&self) -> u64;

    /// Return the length of contents to be written.
    fn size(&self) -> u32;

    /// Return the flags specified at opening the file.
    fn options(&self) -> OpenOptions;

    /// Return the identifier of lock owner.
    fn lock_owner(&self) -> Option<LockOwnerID>;
}

/// Release an opened file.
pub trait Release {
    /// Return the inode number of opened file.
    fn ino(&self) -> NodeID;

    /// Return the handle of opened file.
    fn fh(&self) -> FileID;

    /// Return the flags specified at opening the file.
    fn options(&self) -> OpenOptions;

    /// Return the identifier of lock owner.
    fn lock_owner(&self) -> LockOwnerID;

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
    fn ino(&self) -> NodeID;
}

/// Synchronize the file contents.
pub trait Fsync {
    /// Return the inode number to be synchronized.
    fn ino(&self) -> NodeID;

    /// Return the handle of opened file.
    fn fh(&self) -> FileID;

    /// Return whether to synchronize only the file contents.
    ///
    /// When this method returns `true`, the metadata does not have to be flushed.
    fn datasync(&self) -> bool;
}

/// Set an extended attribute.
pub trait Setxattr {
    /// Return the inode number to set the value of extended attribute.
    fn ino(&self) -> NodeID;

    /// Return the name of extended attribute to be set.
    fn name(&self) -> &OsStr;

    /// Return the value of extended attribute.
    fn value(&self) -> &[u8];

    /// Return the flags that specifies the meanings of this operation.
    fn flags(&self) -> SetxattrFlags;
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct SetxattrFlags: u32 {
        const CREATE = libc::XATTR_CREATE as u32;
        const REPLACE = libc::XATTR_REPLACE as u32;
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
pub trait Getxattr {
    /// Return the inode number to be get the extended attribute.
    fn ino(&self) -> NodeID;

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
    fn ino(&self) -> NodeID;

    /// Return the maximum length of the attribute names to be replied.
    fn size(&self) -> u32;
}

/// Remove an extended attribute.
pub trait Removexattr {
    /// Return the inode number to remove the extended attribute.
    fn ino(&self) -> NodeID;

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
    fn ino(&self) -> NodeID;

    /// Return the handle of opened file.
    fn fh(&self) -> FileID;

    /// Return the identifier of lock owner.
    fn lock_owner(&self) -> LockOwnerID;
}

/// Open a directory.
///
/// If the directory is successfully opened, the filesystem must send
/// the identifier to the opened directory handle using `ReplyOpen`.
pub trait Opendir {
    /// Return the inode number to be opened.
    fn ino(&self) -> NodeID;

    /// Return the open flags.
    fn options(&self) -> OpenOptions;
}

/// Read contents from an opened directory.
pub trait Readdir {
    /// Return the inode number to be read.
    fn ino(&self) -> NodeID;

    /// Return the handle of opened directory.
    fn fh(&self) -> FileID;

    /// Return the *offset* value to continue reading the directory stream.
    fn offset(&self) -> u64;

    /// Return the maximum length of returned data.
    fn size(&self) -> u32;

    fn mode(&self) -> ReaddirMode;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReaddirMode {
    Normal,
    Plus,
}

/// Release an opened directory.
pub trait Releasedir {
    /// Return the inode number of opened directory.
    fn ino(&self) -> NodeID;

    /// Return the handle of opened directory.
    fn fh(&self) -> FileID;

    /// Return the open flags.
    fn options(&self) -> OpenOptions;
}

/// Synchronize the directory contents.
pub trait Fsyncdir {
    /// Return the inode number to be synchronized.
    fn ino(&self) -> NodeID;

    /// Return the handle of opened directory.
    fn fh(&self) -> FileID;

    /// Return whether to synchronize only the directory contents.
    ///
    /// When this method returns `true`, the metadata does not have to be flushed.
    fn datasync(&self) -> bool;
}

/// Test for a POSIX file lock.
///
/// The lock result must be replied using `ReplyLk`.
pub struct Getlk<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_lk_in,
}

impl fmt::Debug for Getlk<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Getlk").finish()
    }
}

impl<'op> Getlk<'op> {
    /// Return the inode number to be tested the lock.
    #[inline]
    pub fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }

    /// Return the handle of opened file.
    #[inline]
    pub fn fh(&self) -> FileID {
        FileID::from_raw(self.arg.fh)
    }

    /// Return the identifier of lock owner.
    #[inline]
    pub fn owner(&self) -> LockOwnerID {
        LockOwnerID::from_raw(self.arg.owner)
    }

    #[inline]
    pub fn file_lock(&self) -> FileLock {
        FileLock {
            typ: self.arg.lk.typ,
            start: self.arg.lk.start,
            end: self.arg.lk.end,
            pid: PID::from_raw(self.arg.lk.pid),
        }
    }
}

/// Acquire, modify or release a POSIX file lock.
pub struct Setlk<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_lk_in,
    sleep: bool,
}

impl fmt::Debug for Setlk<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Setlk").finish()
    }
}

impl<'op> Setlk<'op> {
    /// Return the inode number to be obtained the lock.
    #[inline]
    pub fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }

    /// Return the handle of opened file.
    #[inline]
    pub fn fh(&self) -> FileID {
        FileID::from_raw(self.arg.fh)
    }

    #[inline]
    pub fn file_lock(&self) -> FileLock {
        FileLock {
            typ: self.arg.lk.typ,
            start: self.arg.lk.start,
            end: self.arg.lk.end,
            pid: PID::from_raw(self.arg.lk.pid),
        }
    }

    /// Return whether the locking operation might sleep until a lock is obtained.
    #[inline]
    pub fn sleep(&self) -> bool {
        self.sleep
    }
}

/// Acquire, modify or release a BSD file lock.
pub struct Flock<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_lk_in,
    sleep: bool,
}

impl fmt::Debug for Flock<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Flock").finish()
    }
}

impl<'op> Flock<'op> {
    /// Return the target inode number.
    #[inline]
    pub fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }

    /// Return the handle of opened file.
    #[inline]
    pub fn fh(&self) -> FileID {
        FileID::from_raw(self.arg.fh)
    }

    /// Return the identifier of lock owner.
    #[inline]
    pub fn owner(&self) -> LockOwnerID {
        LockOwnerID::from_raw(self.arg.owner)
    }

    /// Return the locking operation.
    ///
    /// See [`flock(2)`][flock] for details.
    ///
    /// [flock]: http://man7.org/linux/man-pages/man2/flock.2.html
    #[inline]
    pub fn op(&self) -> Option<FlockOp> {
        match self.arg.lk.typ as i32 {
            libc::F_RDLCK if self.sleep => Some(FlockOp::Shared),
            libc::F_RDLCK => Some(FlockOp::SharedNonblock),
            libc::F_WRLCK if self.sleep => Some(FlockOp::Exclusive),
            libc::F_WRLCK => Some(FlockOp::ExclusiveNonblock),
            libc::F_UNLCK => Some(FlockOp::Unlock),
            _ => None,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[repr(i32)]
pub enum FlockOp {
    Shared = libc::LOCK_SH,
    SharedNonblock = libc::LOCK_SH | libc::LOCK_NB,
    Exclusive = libc::LOCK_EX,
    ExclusiveNonblock = libc::LOCK_EX | libc::LOCK_NB,
    Unlock = libc::LOCK_UN,
}

impl FlockOp {
    pub const fn into_raw(self) -> i32 {
        self as i32
    }
}

/// Check file access permissions.
pub struct Access<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_access_in,
}

impl fmt::Debug for Access<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Access").finish()
    }
}

impl<'op> Access<'op> {
    /// Return the inode number subject to the access permission check.
    #[inline]
    pub fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }

    /// Return the requested access mode.
    #[inline]
    pub fn mask(&self) -> u32 {
        self.arg.mask
    }
}

/// Create and open a file.
///
/// This operation is a combination of `Mknod` and `Open`. If an `ENOSYS` error is returned
/// for this operation, those operations will be used instead.
///
/// If the file is successfully created and opened, a pair of `ReplyEntry` and `ReplyOpen`
/// with the corresponding attribute values and the file handle must be sent to the kernel.
pub struct Create<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_create_in,
    name: &'op OsStr,
}

impl fmt::Debug for Create<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Create").finish()
    }
}

impl<'op> Create<'op> {
    /// Return the inode number of the parent directory.
    ///
    /// This is the same as `Mknod::parent`.
    #[inline]
    pub fn parent(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }

    /// Return the file name to crate.
    ///
    /// This is the same as `Mknod::name`.
    #[inline]
    pub fn name(&self) -> &OsStr {
        self.name
    }

    /// Return the file type and permissions used when creating the new file.
    ///
    /// This is the same as `Mknod::mode`.
    #[inline]
    pub fn mode(&self) -> FileMode {
        FileMode::from_raw(self.arg.mode)
    }

    /// Return the open flags.
    ///
    /// This is the same as `Open::flags`.
    #[inline]
    pub fn open_options(&self) -> OpenOptions {
        OpenOptions::from_raw(self.arg.flags)
    }

    #[doc(hidden)] // TODO: dox
    #[inline]
    pub fn umask(&self) -> u32 {
        self.arg.umask
    }
}

/// Map block index within a file to block index within device.
///
/// The mapping result must be replied using `ReplyBmap`.
///
/// This operation makes sense only for filesystems that use
/// block devices, and is called only when the mount options
/// contains `blkdev`.
pub struct Bmap<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_bmap_in,
}

impl fmt::Debug for Bmap<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Bmap").finish()
    }
}

impl<'op> Bmap<'op> {
    /// Return the inode number of the file node to be mapped.
    #[inline]
    pub fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }

    /// Return the block index to be mapped.
    #[inline]
    pub fn block(&self) -> u64 {
        self.arg.block
    }

    /// Returns the unit of block index.
    #[inline]
    pub fn blocksize(&self) -> u32 {
        self.arg.blocksize
    }
}

/// Allocate requested space.
///
/// If this operation is successful, the filesystem shall not report
/// the error caused by the lack of free spaces to subsequent write
/// requests.
pub struct Fallocate<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_fallocate_in,
}

impl fmt::Debug for Fallocate<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Fallocate").finish()
    }
}

impl<'op> Fallocate<'op> {
    /// Return the number of target inode to be allocated the space.
    #[inline]
    pub fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }

    /// Return the handle for opened file.
    #[inline]
    pub fn fh(&self) -> FileID {
        FileID::from_raw(self.arg.fh)
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
    pub fn mode(&self) -> FallocateFlags {
        FallocateFlags::from_bits_truncate(self.arg.mode)
    }
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct FallocateFlags: u32 {
        const KEEP_SIZE = libc::FALLOC_FL_KEEP_SIZE as u32;
        const UNSHARE_RANGE = libc::FALLOC_FL_UNSHARE_RANGE as u32;
        const PUNCH_HOLE = libc::FALLOC_FL_PUNCH_HOLE as u32;
        const COLLAPSE_RANGE = libc::FALLOC_FL_COLLAPSE_RANGE as u32;
        const ZERO_RANGE = libc::FALLOC_FL_ZERO_RANGE as u32;
        const INSERT_RANGE = libc::FALLOC_FL_INSERT_RANGE as u32;
    }
}

/// Copy a range of data from an opened file to another.
///
/// The length of copied data must be replied using `ReplyWrite`.
pub struct CopyFileRange<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_copy_file_range_in,
}

impl fmt::Debug for CopyFileRange<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("CopyFileRange").finish()
    }
}

impl<'op> CopyFileRange<'op> {
    /// Return the inode number of source file.
    #[inline]
    pub fn ino_in(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }

    /// Return the file handle of source file.
    #[inline]
    pub fn fh_in(&self) -> FileID {
        FileID::from_raw(self.arg.fh_in)
    }

    /// Return the starting point of source file where the data should be read.
    #[inline]
    pub fn offset_in(&self) -> u64 {
        self.arg.off_in
    }

    /// Return the inode number of target file.
    #[inline]
    pub fn ino_out(&self) -> NodeID {
        NodeID::from_raw(self.arg.nodeid_out)
    }

    /// Return the file handle of target file.
    #[inline]
    pub fn fh_out(&self) -> FileID {
        FileID::from_raw(self.arg.fh_out)
    }

    /// Return the starting point of target file where the data should be written.
    #[inline]
    pub fn offset_out(&self) -> u64 {
        self.arg.off_out
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
}

/// Poll for readiness.
///
/// The mask of ready poll events must be replied using `ReplyPoll`.
pub struct Poll<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_poll_in,
}

impl fmt::Debug for Poll<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Poll")
            .field("ino", &self.ino())
            .field("fh", &self.fh())
            .field("events", &self.events())
            .field("kh", &self.kh())
            .finish()
    }
}

impl<'op> Poll<'op> {
    /// Return the inode number to check the I/O readiness.
    #[inline]
    pub fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }

    /// Return the handle of opened file.
    #[inline]
    pub fn fh(&self) -> FileID {
        FileID::from_raw(self.arg.fh)
    }

    /// Return the requested poll events.
    #[inline]
    pub fn events(&self) -> PollEvents {
        PollEvents::from_bits_truncate(self.arg.events)
    }

    /// Return the handle to this poll.
    ///
    /// If the returned value is not `None`, the filesystem should send the notification
    /// when the corresponding I/O will be ready.
    #[inline]
    pub fn kh(&self) -> Option<PollWakeupID> {
        if self.arg.flags & FUSE_POLL_SCHEDULE_NOTIFY != 0 {
            Some(PollWakeupID::from_raw(self.arg.kh))
        } else {
            None
        }
    }
}

/// Reposition the offset of read/write operations.
///
/// See [`lseek(2)`][lseek] for details.
///
/// [lseek]: https://man7.org/linux/man-pages/man2/lseek.2.html
pub struct Lseek<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_lseek_in,
}

impl fmt::Debug for Lseek<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lseek").finish()
    }
}

impl Lseek<'_> {
    pub fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }

    pub fn fh(&self) -> FileID {
        FileID::from_raw(self.arg.fh)
    }

    pub fn offset(&self) -> u64 {
        self.arg.offset
    }

    pub fn whence(&self) -> Option<Whence> {
        match self.arg.whence as i32 {
            libc::SEEK_SET => Some(Whence::Set),
            libc::SEEK_CUR => Some(Whence::Current),
            libc::SEEK_END => Some(Whence::End),
            libc::SEEK_DATA => Some(Whence::Data),
            libc::SEEK_HOLE => Some(Whence::Hole),
            _ => None,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Whence {
    Set,
    Current,
    End,
    Data,
    Hole,
}
