use crate::bytes::{DecodeError, Decoder};
use polyfuse_kernel::*;
use std::{ffi::OsStr, fmt, time::Duration, u32, u64};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("during decoding: {}", _0)]
    Decode(#[from] DecodeError),

    #[error("unsupported opcode")]
    UnsupportedOpcode,
}

/// The kind of filesystem operation requested by the kernel.
#[non_exhaustive]
pub enum Operation<'op, T> {
    Lookup(Lookup<'op>),
    Getattr(Getattr<'op>),
    Setattr(Setattr<'op>),
    Readlink(Readlink<'op>),
    Symlink(Symlink<'op>),
    Mknod(Mknod<'op>),
    Mkdir(Mkdir<'op>),
    Unlink(Unlink<'op>),
    Rmdir(Rmdir<'op>),
    Rename(Rename<'op>),
    Link(Link<'op>),
    Open(Open<'op>),
    Read(Read<'op>),
    Write(Write<'op>, T),
    Release(Release<'op>),
    Statfs(Statfs<'op>),
    Fsync(Fsync<'op>),
    Setxattr(Setxattr<'op>),
    Getxattr(Getxattr<'op>),
    Listxattr(Listxattr<'op>),
    Removexattr(Removexattr<'op>),
    Flush(Flush<'op>),
    Opendir(Opendir<'op>),
    Readdir(Readdir<'op>),
    Releasedir(Releasedir<'op>),
    Fsyncdir(Fsyncdir<'op>),
    Getlk(Getlk<'op>),
    Setlk(Setlk<'op>),
    Flock(Flock<'op>),
    Access(Access<'op>),
    Create(Create<'op>),
    Bmap(Bmap<'op>),
    Fallocate(Fallocate<'op>),
    CopyFileRange(CopyFileRange<'op>),
    Poll(Poll<'op>),

    Forget(Forgets<'op>),
    Interrupt(Interrupt<'op>),
    NotifyReply(NotifyReply<'op>, T),
}

impl<T> fmt::Debug for Operation<'_, T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Operation::Lookup(op) => op.fmt(f),
            Operation::Getattr(op) => op.fmt(f),
            Operation::Setattr(op) => op.fmt(f),
            Operation::Readlink(op) => op.fmt(f),
            Operation::Symlink(op) => op.fmt(f),
            Operation::Mknod(op) => op.fmt(f),
            Operation::Mkdir(op) => op.fmt(f),
            Operation::Unlink(op) => op.fmt(f),
            Operation::Rmdir(op) => op.fmt(f),
            Operation::Rename(op) => op.fmt(f),
            Operation::Link(op) => op.fmt(f),
            Operation::Open(op) => op.fmt(f),
            Operation::Read(op) => op.fmt(f),
            Operation::Release(op) => op.fmt(f),
            Operation::Statfs(op) => op.fmt(f),
            Operation::Fsync(op) => op.fmt(f),
            Operation::Setxattr(op) => op.fmt(f),
            Operation::Getxattr(op) => op.fmt(f),
            Operation::Listxattr(op) => op.fmt(f),
            Operation::Removexattr(op) => op.fmt(f),
            Operation::Flush(op) => op.fmt(f),
            Operation::Opendir(op) => op.fmt(f),
            Operation::Readdir(op) => op.fmt(f),
            Operation::Releasedir(op) => op.fmt(f),
            Operation::Fsyncdir(op) => op.fmt(f),
            Operation::Getlk(op) => op.fmt(f),
            Operation::Setlk(op) => op.fmt(f),
            Operation::Flock(op) => op.fmt(f),
            Operation::Access(op) => op.fmt(f),
            Operation::Create(op) => op.fmt(f),
            Operation::Bmap(op) => op.fmt(f),
            Operation::Fallocate(op) => op.fmt(f),
            Operation::CopyFileRange(op) => op.fmt(f),
            Operation::Poll(op) => op.fmt(f),
            Operation::Forget(op) => op.fmt(f),
            Operation::Interrupt(op) => op.fmt(f),

            Operation::Write(op, data) => f
                .debug_struct("Write")
                .field("op", op)
                .field("data", data)
                .finish(),
            Operation::NotifyReply(op, data) => f
                .debug_struct("NotifyReply")
                .field("op", op)
                .field("data", data)
                .finish(),
        }
    }
}

impl<'op, T> Operation<'op, T> {
    pub(crate) fn decode(
        header: &'op fuse_in_header,
        opcode: fuse_opcode,
        arg: &'op [u8],
        data: T,
    ) -> Result<Self, Error> {
        let mut decoder = Decoder::new(arg);

        match opcode {
            fuse_opcode::FUSE_FORGET => {
                let arg: &fuse_forget_in = decoder.fetch()?;
                let forget = fuse_forget_one {
                    nodeid: header.nodeid,
                    nlookup: arg.nlookup,
                };
                Ok(Operation::Forget(Forgets {
                    inner: ForgetsInner::Single(forget),
                }))
            }

            fuse_opcode::FUSE_BATCH_FORGET => {
                let arg: &fuse_batch_forget_in = decoder.fetch()?;
                let forgets = decoder.fetch_array::<fuse_forget_one>(arg.count as usize)?;
                Ok(Operation::Forget(Forgets {
                    inner: ForgetsInner::Batch(forgets),
                }))
            }

            fuse_opcode::FUSE_INTERRUPT => {
                let arg = decoder.fetch()?;
                Ok(Operation::Interrupt(Interrupt { header, arg }))
            }

            fuse_opcode::FUSE_NOTIFY_REPLY => {
                let arg = decoder.fetch()?;
                Ok(Operation::NotifyReply(NotifyReply { header, arg }, data))
            }

            fuse_opcode::FUSE_LOOKUP => {
                let name = decoder.fetch_str()?;
                Ok(Operation::Lookup(Lookup { header, name }))
            }

            fuse_opcode::FUSE_GETATTR => {
                let arg = decoder.fetch()?;
                Ok(Operation::Getattr(Getattr { header, arg }))
            }

            fuse_opcode::FUSE_SETATTR => {
                let arg = decoder.fetch()?;
                Ok(Operation::Setattr(Setattr { header, arg }))
            }

            fuse_opcode::FUSE_READLINK => Ok(Operation::Readlink(Readlink { header })),

            fuse_opcode::FUSE_SYMLINK => {
                let name = decoder.fetch_str()?;
                let link = decoder.fetch_str()?;
                Ok(Operation::Symlink(Symlink { header, name, link }))
            }

            fuse_opcode::FUSE_MKNOD => {
                let arg = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                Ok(Operation::Mknod(Mknod { header, arg, name }))
            }

            fuse_opcode::FUSE_MKDIR => {
                let arg = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                Ok(Operation::Mkdir(Mkdir { header, arg, name }))
            }

            fuse_opcode::FUSE_UNLINK => {
                let name = decoder.fetch_str()?;
                Ok(Operation::Unlink(Unlink { header, name }))
            }

            fuse_opcode::FUSE_RMDIR => {
                let name = decoder.fetch_str()?;
                Ok(Operation::Rmdir(Rmdir { header, name }))
            }

            fuse_opcode::FUSE_RENAME => {
                let arg = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                let newname = decoder.fetch_str()?;
                Ok(Operation::Rename(Rename {
                    header,
                    arg: RenameArg::V1(arg),
                    name,
                    newname,
                }))
            }

            fuse_opcode::FUSE_RENAME2 => {
                let arg = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                let newname = decoder.fetch_str()?;
                Ok(Operation::Rename(Rename {
                    header,
                    arg: RenameArg::V2(arg),
                    name,
                    newname,
                }))
            }

            fuse_opcode::FUSE_LINK => {
                let arg = decoder.fetch()?;
                let newname = decoder.fetch_str()?;
                Ok(Operation::Link(Link {
                    header,
                    arg,
                    newname,
                }))
            }

            fuse_opcode::FUSE_OPEN => {
                let arg = decoder.fetch()?;
                Ok(Operation::Open(Open { header, arg }))
            }

            fuse_opcode::FUSE_READ => {
                let arg = decoder.fetch()?;
                Ok(Operation::Read(Read { header, arg }))
            }

            fuse_opcode::FUSE_WRITE => {
                let arg = decoder.fetch()?;
                Ok(Operation::Write(Write { header, arg }, data))
            }

            fuse_opcode::FUSE_RELEASE => {
                let arg = decoder.fetch()?;
                Ok(Operation::Release(Release { header, arg }))
            }

            fuse_opcode::FUSE_STATFS => Ok(Operation::Statfs(Statfs { header })),

            fuse_opcode::FUSE_FSYNC => {
                let arg = decoder.fetch()?;
                Ok(Operation::Fsync(Fsync { header, arg }))
            }

            fuse_opcode::FUSE_SETXATTR => {
                let arg = decoder.fetch::<fuse_setxattr_in>()?;
                let name = decoder.fetch_str()?;
                let value = decoder.fetch_bytes(arg.size as usize)?;
                Ok(Operation::Setxattr(Setxattr {
                    header,
                    arg,
                    name,
                    value,
                }))
            }

            fuse_opcode::FUSE_GETXATTR => {
                let arg = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                Ok(Operation::Getxattr(Getxattr { header, arg, name }))
            }

            fuse_opcode::FUSE_LISTXATTR => {
                let arg = decoder.fetch()?;
                Ok(Operation::Listxattr(Listxattr { header, arg }))
            }

            fuse_opcode::FUSE_REMOVEXATTR => {
                let name = decoder.fetch_str()?;
                Ok(Operation::Removexattr(Removexattr { header, name }))
            }

            fuse_opcode::FUSE_FLUSH => {
                let arg = decoder.fetch()?;
                Ok(Operation::Flush(Flush { header, arg }))
            }

            fuse_opcode::FUSE_OPENDIR => {
                let arg = decoder.fetch()?;
                Ok(Operation::Opendir(Opendir { header, arg }))
            }

            fuse_opcode::FUSE_READDIR => {
                let arg = decoder.fetch()?;
                Ok(Operation::Readdir(Readdir {
                    header,
                    arg,
                    mode: ReaddirMode::Normal,
                }))
            }

            fuse_opcode::FUSE_READDIRPLUS => {
                let arg = decoder.fetch()?;
                Ok(Operation::Readdir(Readdir {
                    header,
                    arg,
                    mode: ReaddirMode::Plus,
                }))
            }

            fuse_opcode::FUSE_RELEASEDIR => {
                let arg = decoder.fetch()?;
                Ok(Operation::Releasedir(Releasedir { header, arg }))
            }

            fuse_opcode::FUSE_FSYNCDIR => {
                let arg = decoder.fetch()?;
                Ok(Operation::Fsyncdir(Fsyncdir { header, arg }))
            }

            fuse_opcode::FUSE_GETLK => {
                let arg = decoder.fetch()?;
                Ok(Operation::Getlk(Getlk { header, arg }))
            }

            opcode @ fuse_opcode::FUSE_SETLK | opcode @ fuse_opcode::FUSE_SETLKW => {
                let arg: &fuse_lk_in = decoder.fetch()?;
                let sleep = match opcode {
                    fuse_opcode::FUSE_SETLK => false,
                    fuse_opcode::FUSE_SETLKW => true,
                    _ => unreachable!(),
                };

                if arg.lk_flags & FUSE_LK_FLOCK == 0 {
                    Ok(Operation::Setlk(Setlk { header, arg, sleep }))
                } else {
                    let op = convert_to_flock_op(arg.lk.typ, sleep).unwrap_or(0);
                    Ok(Operation::Flock(Flock { header, arg, op }))
                }
            }

            fuse_opcode::FUSE_ACCESS => {
                let arg = decoder.fetch()?;
                Ok(Operation::Access(Access { header, arg }))
            }

            fuse_opcode::FUSE_CREATE => {
                let arg = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                Ok(Operation::Create(Create { header, arg, name }))
            }

            fuse_opcode::FUSE_BMAP => {
                let arg = decoder.fetch()?;
                Ok(Operation::Bmap(Bmap { header, arg }))
            }

            fuse_opcode::FUSE_FALLOCATE => {
                let arg = decoder.fetch()?;
                Ok(Operation::Fallocate(Fallocate { header, arg }))
            }

            fuse_opcode::FUSE_COPY_FILE_RANGE => {
                let arg = decoder.fetch()?;
                Ok(Operation::CopyFileRange(CopyFileRange { header, arg }))
            }

            fuse_opcode::FUSE_POLL => {
                let arg = decoder.fetch()?;
                Ok(Operation::Poll(Poll { header, arg }))
            }

            _ => Err(Error::UnsupportedOpcode),
        }
    }
}

#[inline]
fn convert_to_flock_op(lk_type: u32, sleep: bool) -> Option<u32> {
    const F_RDLCK: u32 = libc::F_RDLCK as u32;
    const F_WRLCK: u32 = libc::F_WRLCK as u32;
    const F_UNLCK: u32 = libc::F_UNLCK as u32;

    let mut op = match lk_type {
        F_RDLCK => libc::LOCK_SH as u32,
        F_WRLCK => libc::LOCK_EX as u32,
        F_UNLCK => libc::LOCK_UN as u32,
        _ => return None,
    };

    if !sleep {
        op |= libc::LOCK_NB as u32;
    }
    Some(op)
}

/// The identifier for locking operations.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct LockOwner(u64);

impl fmt::Debug for LockOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LockOwner {{ .. }}")
    }
}

impl LockOwner {
    /// Create a `LockOwner` from the raw value.
    #[inline]
    pub const fn from_raw(id: u64) -> Self {
        Self(id)
    }

    /// Take the raw value of this identifier.
    #[inline]
    pub const fn into_raw(self) -> u64 {
        self.0
    }
}

/// A set of forget information removed from the kernel's internal caches.
pub struct Forgets<'op> {
    inner: ForgetsInner<'op>,
}

impl fmt::Debug for Forgets<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.as_ref()).finish()
    }
}

enum ForgetsInner<'op> {
    Single(fuse_forget_one),
    Batch(&'op [fuse_forget_one]),
}

impl<'op> std::ops::Deref for Forgets<'op> {
    type Target = [Forget];

    #[inline]
    fn deref(&self) -> &Self::Target {
        let (ptr, len) = match &self.inner {
            ForgetsInner::Single(forget) => (forget as *const fuse_forget_one, 1),
            ForgetsInner::Batch(forgets) => (forgets.as_ptr(), forgets.len()),
        };
        unsafe {
            // Safety: Forget has the same layout with fuse_forget_one
            std::slice::from_raw_parts(ptr as *const Forget, len)
        }
    }
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
    pub fn ino(&self) -> u64 {
        self.forget.nodeid
    }

    /// Return the released lookup count of the target inode.
    #[inline]
    pub fn nlookup(&self) -> u64 {
        self.forget.nlookup
    }
}

/// A reply to a `NOTIFY_RETRIEVE` notification.
pub struct NotifyReply<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_notify_retrieve_in,
}

impl fmt::Debug for NotifyReply<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("NotifyReply").finish()
    }
}

impl<'op> NotifyReply<'op> {
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
pub struct Interrupt<'op> {
    #[allow(dead_code)]
    header: &'op fuse_in_header,
    arg: &'op fuse_interrupt_in,
}

impl fmt::Debug for Interrupt<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Interrupt").finish()
    }
}

impl<'op> Interrupt<'op> {
    /// Return the target unique ID to be interrupted.
    #[inline]
    pub fn unique(&self) -> u64 {
        self.arg.unique
    }
}

/// Lookup a directory entry by name.
///
/// If a matching entry is found, the filesystem replies to the kernel
/// with its attribute using `ReplyEntry`.  In addition, the lookup count
/// of the corresponding inode is incremented on success.
///
/// See also the documentation of `ReplyEntry` for tuning the reply parameters.
pub struct Lookup<'op> {
    header: &'op fuse_in_header,
    name: &'op OsStr,
}

impl fmt::Debug for Lookup<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Lookup").finish()
    }
}

impl<'op> Lookup<'op> {
    /// Return the inode number of the parent directory.
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the name of the entry to be looked up.
    pub fn name(&self) -> &OsStr {
        self.name
    }
}

/// Get file attributes.
///
/// The obtained attribute values are replied using `ReplyAttr`.
///
/// If writeback caching is enabled, the kernel might ignore
/// some of the attribute values, such as `st_size`.
pub struct Getattr<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_getattr_in,
}

impl fmt::Debug for Getattr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Getattr").finish()
    }
}

impl<'op> Getattr<'op> {
    /// Return the inode number for obtaining the attribute value.
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened file, if specified.
    pub fn fh(&self) -> Option<u64> {
        if self.arg.getattr_flags & FUSE_GETATTR_FH != 0 {
            Some(self.arg.fh)
        } else {
            None
        }
    }
}

/// Set file attributes.
///
/// When the setting of attribute values succeeds, the filesystem replies its value
/// to the kernel using `ReplyAttr`.
pub struct Setattr<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_setattr_in,
}

impl fmt::Debug for Setattr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Setattr").finish()
    }
}

impl<'op> Setattr<'op> {
    #[inline(always)]
    fn get<R>(&self, flag: u32, f: impl FnOnce(&fuse_setattr_in) -> R) -> Option<R> {
        if self.arg.valid & flag != 0 {
            Some(f(self.arg))
        } else {
            None
        }
    }

    /// Return the inode number to be set the attribute values.
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened file, if specified.
    #[inline]
    pub fn fh(&self) -> Option<u64> {
        self.get(FATTR_FH, |arg| arg.fh)
    }

    /// Return the file mode to be set.
    #[inline]
    pub fn mode(&self) -> Option<u32> {
        self.get(FATTR_MODE, |arg| arg.mode)
    }

    /// Return the user id to be set.
    #[inline]
    pub fn uid(&self) -> Option<u32> {
        self.get(FATTR_UID, |arg| arg.uid)
    }

    /// Return the group id to be set.
    #[inline]
    pub fn gid(&self) -> Option<u32> {
        self.get(FATTR_GID, |arg| arg.gid)
    }

    /// Return the size of the file content to be set.
    #[inline]
    pub fn size(&self) -> Option<u64> {
        self.get(FATTR_SIZE, |arg| arg.size)
    }

    /// Return the last accessed time to be set.
    #[inline]
    pub fn atime(&self) -> Option<SetAttrTime> {
        self.get(FATTR_ATIME, |arg| {
            if arg.valid & FATTR_ATIME_NOW != 0 {
                SetAttrTime::Now
            } else {
                SetAttrTime::Timespec(Duration::new(arg.atime, arg.atimensec))
            }
        })
    }

    /// Return the last modified time to be set.
    #[inline]
    pub fn mtime(&self) -> Option<SetAttrTime> {
        self.get(FATTR_MTIME, |arg| {
            if arg.valid & FATTR_MTIME_NOW != 0 {
                SetAttrTime::Now
            } else {
                SetAttrTime::Timespec(Duration::new(arg.mtime, arg.mtimensec))
            }
        })
    }

    /// Return the last creation time to be set.
    #[inline]
    pub fn ctime(&self) -> Option<Duration> {
        self.get(FATTR_CTIME, |arg| Duration::new(arg.ctime, arg.ctimensec))
    }

    /// Return the identifier of lock owner.
    #[inline]
    pub fn lock_owner(&self) -> Option<LockOwner> {
        self.get(FATTR_LOCKOWNER, |arg| LockOwner::from_raw(arg.lock_owner))
    }
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
pub struct Readlink<'op> {
    header: &'op fuse_in_header,
}

impl fmt::Debug for Readlink<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Readlink").finish()
    }
}

impl<'op> Readlink<'op> {
    /// Return the inode number to be read the link value.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }
}

/// Create a symbolic link.
///
/// When the link is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
pub struct Symlink<'op> {
    header: &'op fuse_in_header,
    name: &'op OsStr,
    link: &'op OsStr,
}

impl fmt::Debug for Symlink<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Symlink").finish()
    }
}

impl<'op> Symlink<'op> {
    /// Return the inode number of the parent directory.
    #[inline]
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the name of the symbolic link to create.
    #[inline]
    pub fn name(&self) -> &OsStr {
        self.name
    }

    /// Return the contents of the symbolic link.
    #[inline]
    pub fn link(&self) -> &OsStr {
        self.link
    }
}

/// Create a file node.
///
/// When the file node is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
pub struct Mknod<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_mknod_in,
    name: &'op OsStr,
}

impl fmt::Debug for Mknod<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Mknod").finish()
    }
}

impl<'op> Mknod<'op> {
    /// Return the inode number of the parent directory.
    #[inline]
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the file name to create.
    #[inline]
    pub fn name(&self) -> &OsStr {
        self.name
    }

    /// Return the file type and permissions used when creating the new file.
    #[inline]
    pub fn mode(&self) -> u32 {
        self.arg.mode
    }

    /// Return the device number for special file.
    ///
    /// This value is meaningful only if the created node is a device file
    /// (i.e. the file type is specified either `S_IFCHR` or `S_IFBLK`).
    #[inline]
    pub fn rdev(&self) -> u32 {
        self.arg.rdev
    }

    #[doc(hidden)] // TODO: dox
    pub fn umask(&self) -> u32 {
        self.arg.umask
    }
}

/// Create a directory node.
///
/// When the directory is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
pub struct Mkdir<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_mkdir_in,
    name: &'op OsStr,
}

impl fmt::Debug for Mkdir<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Mkdir").finish()
    }
}

impl<'op> Mkdir<'op> {
    /// Return the inode number of the parent directory where the directory is created.
    #[inline]
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the name of the directory to be created.
    #[inline]
    pub fn name(&self) -> &OsStr {
        self.name
    }

    /// Return the file type and permissions used when creating the new directory.
    #[inline]
    pub fn mode(&self) -> u32 {
        self.arg.mode
    }

    #[doc(hidden)] // TODO: dox
    pub fn umask(&self) -> u32 {
        self.arg.umask
    }
}

// TODO: description about lookup count.

/// Remove a file.
pub struct Unlink<'op> {
    header: &'op fuse_in_header,
    name: &'op OsStr,
}

impl fmt::Debug for Unlink<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Unlink").finish()
    }
}

impl<'op> Unlink<'op> {
    /// Return the inode number of the parent directory.
    #[inline]
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the file name to be removed.
    #[inline]
    pub fn name(&self) -> &OsStr {
        self.name
    }
}

/// Remove a directory.
pub struct Rmdir<'op> {
    header: &'op fuse_in_header,
    name: &'op OsStr,
}

impl fmt::Debug for Rmdir<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Rmdir").finish()
    }
}

impl<'op> Rmdir<'op> {
    // TODO: description about lookup count.

    /// Return the inode number of the parent directory.
    #[inline]
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the directory name to be removed.
    #[inline]
    pub fn name(&self) -> &OsStr {
        self.name
    }
}

/// Rename a file.
pub struct Rename<'op> {
    header: &'op fuse_in_header,
    arg: RenameArg<'op>,
    name: &'op OsStr,
    newname: &'op OsStr,
}

enum RenameArg<'op> {
    V1(&'op fuse_rename_in),
    V2(&'op fuse_rename2_in),
}

impl fmt::Debug for Rename<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Rename").finish()
    }
}

impl<'op> Rename<'op> {
    /// Return the inode number of the old parent directory.
    #[inline]
    pub fn parent(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the old name of the target node.
    #[inline]
    pub fn name(&self) -> &OsStr {
        self.name
    }

    /// Return the inode number of the new parent directory.
    #[inline]
    pub fn newparent(&self) -> u64 {
        match self.arg {
            RenameArg::V1(arg) => arg.newdir,
            RenameArg::V2(arg) => arg.newdir,
        }
    }

    /// Return the new name of the target node.
    #[inline]
    pub fn newname(&self) -> &OsStr {
        self.newname
    }

    /// Return the rename flags.
    #[inline]
    pub fn flags(&self) -> u32 {
        match self.arg {
            RenameArg::V1(..) => 0,
            RenameArg::V2(arg) => arg.flags,
        }
    }
}

/// Create a hard link.
///
/// When the link is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
pub struct Link<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_link_in,
    newname: &'op OsStr,
}

impl fmt::Debug for Link<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Link").finish()
    }
}

impl<'op> Link<'op> {
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
        self.newname
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
pub struct Open<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_open_in,
}

impl fmt::Debug for Open<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Open").finish()
    }
}

impl<'op> Open<'op> {
    // TODO: Description of behavior when writeback caching is enabled.

    /// Return the inode number to be opened.
    #[inline]
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
    #[inline]
    pub fn flags(&self) -> u32 {
        self.arg.flags
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
pub struct Read<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_read_in,
}

impl fmt::Debug for Read<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Read").finish()
    }
}

impl<'op> Read<'op> {
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

    /// Return the identifier of lock owner.
    #[inline]
    pub fn lock_owner(&self) -> Option<LockOwner> {
        if self.arg.read_flags & FUSE_READ_LOCKOWNER != 0 {
            Some(LockOwner::from_raw(self.arg.lock_owner))
        } else {
            None
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
pub struct Write<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_write_in,
}

impl fmt::Debug for Write<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Write").finish()
    }
}

impl<'op> Write<'op> {
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
    pub fn lock_owner(&self) -> Option<LockOwner> {
        if self.arg.write_flags & FUSE_WRITE_LOCKOWNER != 0 {
            Some(LockOwner::from_raw(self.arg.lock_owner))
        } else {
            None
        }
    }
}

/// Release an opened file.
pub struct Release<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_release_in,
}

impl fmt::Debug for Release<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Release").finish()
    }
}

impl<'op> Release<'op> {
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
    pub fn lock_owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.lock_owner)
    }

    /// Return whether the operation indicates a flush.
    #[inline]
    pub fn flush(&self) -> bool {
        self.arg.release_flags & FUSE_RELEASE_FLUSH != 0
    }

    /// Return whether the `flock` locks for this file should be released.
    #[inline]
    pub fn flock_release(&self) -> bool {
        self.arg.release_flags & FUSE_RELEASE_FLOCK_UNLOCK != 0
    }
}

/// Get the filesystem statistics.
///
/// The obtained statistics must be sent to the kernel using `ReplyStatfs`.
pub struct Statfs<'op> {
    header: &'op fuse_in_header,
}

impl fmt::Debug for Statfs<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Statfs").finish()
    }
}

impl<'op> Statfs<'op> {
    /// Return the inode number or `0` which means "undefined".
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }
}

/// Synchronize the file contents.
pub struct Fsync<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_fsync_in,
}

impl fmt::Debug for Fsync<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Fsync").finish()
    }
}

impl<'op> Fsync<'op> {
    /// Return the inode number to be synchronized.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened file.
    #[inline]
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return whether to synchronize only the file contents.
    ///
    /// When this method returns `true`, the metadata does not have to be flushed.
    #[inline]
    pub fn datasync(&self) -> bool {
        self.arg.fsync_flags & FUSE_FSYNC_FDATASYNC != 0
    }
}

/// Set an extended attribute.
pub struct Setxattr<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_setxattr_in,
    name: &'op OsStr,
    value: &'op [u8],
}

impl fmt::Debug for Setxattr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Setxattr").finish()
    }
}

impl<'op> Setxattr<'op> {
    /// Return the inode number to set the value of extended attribute.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the name of extended attribute to be set.
    #[inline]
    pub fn name(&self) -> &OsStr {
        self.name
    }

    /// Return the value of extended attribute.
    #[inline]
    pub fn value(&self) -> &[u8] {
        self.value
    }

    /// Return the flags that specifies the meanings of this operation.
    #[inline]
    pub fn flags(&self) -> u32 {
        self.arg.flags
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
pub struct Getxattr<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_getxattr_in,
    name: &'op OsStr,
}

impl fmt::Debug for Getxattr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Getxattr").finish()
    }
}

impl<'op> Getxattr<'op> {
    /// Return the inode number to be get the extended attribute.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the name of the extend attribute.
    #[inline]
    pub fn name(&self) -> &OsStr {
        self.name
    }

    /// Return the maximum length of the attribute value to be replied.
    #[inline]
    pub fn size(&self) -> u32 {
        self.arg.size
    }
}

/// List extended attribute names.
///
/// Each element of the attribute names list must be null-terminated.
/// As with `Getxattr`, the filesystem must send the data length of the attribute
/// names using `ReplyXattr` if `size` is zero.
pub struct Listxattr<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_getxattr_in,
}

impl fmt::Debug for Listxattr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Listxattr").finish()
    }
}

impl<'op> Listxattr<'op> {
    /// Return the inode number to be obtained the attribute names.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the maximum length of the attribute names to be replied.
    #[inline]
    pub fn size(&self) -> u32 {
        self.arg.size
    }
}

/// Remove an extended attribute.
pub struct Removexattr<'op> {
    header: &'op fuse_in_header,
    name: &'op OsStr,
}

impl fmt::Debug for Removexattr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Removexattr").finish()
    }
}

impl<'op> Removexattr<'op> {
    /// Return the inode number to remove the extended attribute.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the name of extended attribute to be removed.
    #[inline]
    pub fn name(&self) -> &OsStr {
        self.name
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
pub struct Flush<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_flush_in,
}

impl fmt::Debug for Flush<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Flush").finish()
    }
}

impl<'op> Flush<'op> {
    /// Return the inode number of target file.
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
    pub fn lock_owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.lock_owner)
    }
}

/// Open a directory.
///
/// If the directory is successfully opened, the filesystem must send
/// the identifier to the opened directory handle using `ReplyOpen`.
pub struct Opendir<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_open_in,
}

impl fmt::Debug for Opendir<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Opendir").finish()
    }
}

impl<'op> Opendir<'op> {
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
}

/// Read contents from an opened directory.
pub struct Readdir<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_read_in,
    mode: ReaddirMode,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReaddirMode {
    Normal,
    Plus,
}

impl fmt::Debug for Readdir<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Readdir").finish()
    }
}

impl<'op> Readdir<'op> {
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

    pub fn mode(&self) -> ReaddirMode {
        self.mode
    }
}

/// Release an opened directory.
pub struct Releasedir<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_release_in,
}

impl fmt::Debug for Releasedir<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Releasedir").finish()
    }
}

impl<'op> Releasedir<'op> {
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
}

/// Synchronize the directory contents.
pub struct Fsyncdir<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_fsync_in,
}

impl fmt::Debug for Fsyncdir<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields
        f.debug_struct("Fsyncdir").finish()
    }
}

impl<'op> Fsyncdir<'op> {
    /// Return the inode number to be synchronized.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.header.nodeid
    }

    /// Return the handle of opened directory.
    #[inline]
    pub fn fh(&self) -> u64 {
        self.arg.fh
    }

    /// Return whether to synchronize only the directory contents.
    ///
    /// When this method returns `true`, the metadata does not have to be flushed.
    #[inline]
    pub fn datasync(&self) -> bool {
        self.arg.fsync_flags & FUSE_FSYNC_FDATASYNC != 0
    }
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
    pub fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.owner)
    }

    #[inline]
    pub fn typ(&self) -> u32 {
        self.arg.lk.typ
    }

    #[inline]
    pub fn start(&self) -> u64 {
        self.arg.lk.start
    }

    #[inline]
    pub fn end(&self) -> u64 {
        self.arg.lk.end
    }

    #[inline]
    pub fn pid(&self) -> u32 {
        self.arg.lk.pid
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
    pub fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.owner)
    }

    #[inline]
    pub fn typ(&self) -> u32 {
        self.arg.lk.typ
    }

    #[inline]
    pub fn start(&self) -> u64 {
        self.arg.lk.start
    }

    #[inline]
    pub fn end(&self) -> u64 {
        self.arg.lk.end
    }

    #[inline]
    pub fn pid(&self) -> u32 {
        self.arg.lk.pid
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
    op: u32,
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
    pub fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.owner)
    }

    /// Return the locking operation.
    ///
    /// See [`flock(2)`][flock] for details.
    ///
    /// [flock]: http://man7.org/linux/man-pages/man2/flock.2.html
    #[inline]
    pub fn op(&self) -> Option<u32> {
        Some(self.op)
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
    pub fn ino(&self) -> u64 {
        self.header.nodeid
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
    pub fn parent(&self) -> u64 {
        self.header.nodeid
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
    pub fn mode(&self) -> u32 {
        self.arg.mode
    }

    /// Return the open flags.
    ///
    /// This is the same as `Open::flags`.
    #[inline]
    pub fn open_flags(&self) -> u32 {
        self.arg.flags
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
    pub fn ino(&self) -> u64 {
        self.header.nodeid
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
        self.arg.off_in
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
        if self.arg.flags & FUSE_POLL_SCHEDULE_NOTIFY != 0 {
            Some(self.arg.kh)
        } else {
            None
        }
    }
}
