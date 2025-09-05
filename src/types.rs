//! Common type definitions.

use bitflags::bitflags;
use libc::dev_t;
use std::{fmt, fs::Metadata, os::unix::prelude::*, time::Duration};

macro_rules! define_id_type {
    ($(
        $(#[$($meta:tt)*])*
        pub type $name:ident = $typ:ty;
    )*) => {$(
        $(#[$($meta)*])*
        #[derive(Copy, Clone, PartialEq, Eq, Hash)]
        #[repr(transparent)]
        pub struct $name {
            value: $typ,
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, f)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}#{}", stringify!($name), self.value)
            }
        }

        impl $name {
            pub const fn from_raw(value: $typ) -> Self {
                Self { value }
            }

            pub const fn into_raw(self) -> $typ {
                self.value
            }
        }
    )*};
}

define_id_type! {
    /// The request ID.
    pub type RequestID = u64;

    /// The notification ID.
    pub type NotifyID = u64;

    /// The inode number.
    pub type NodeID = u64;

    /// The file handle.
    pub type FileID = u64;

    /// The user ID.
    pub type UID = u32;

    /// The group ID.
    pub type GID = u32;

    /// The process ID.
    pub type PID = u32;

    /// The poll wakeup ID.
    pub type PollWakeupID = u64;
}

impl NodeID {
    pub const ROOT: Self = Self::from_raw(1);
}

impl UID {
    pub const ROOT: Self = Self::from_raw(0);

    pub fn current() -> Self {
        Self::from_raw(unsafe { libc::getuid() })
    }

    pub fn effective() -> Self {
        Self::from_raw(unsafe { libc::geteuid() })
    }
}

impl GID {
    pub const ROOT: Self = Self::from_raw(0);

    pub fn current() -> Self {
        Self::from_raw(unsafe { libc::getgid() })
    }

    pub fn effective() -> Self {
        Self::from_raw(unsafe { libc::getegid() })
    }
}

/// The lock owner ID.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct LockOwnerID {
    value: u64,
}

impl fmt::Debug for LockOwnerID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LockOwnerID").finish()
    }
}

impl LockOwnerID {
    pub(crate) const fn from_raw(value: u64) -> Self {
        Self { value }
    }
}

/// The device ID.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct DeviceID {
    raw: u32,
}

impl DeviceID {
    const MAJOR_MAX: u32 = (0x1 << (32 - 20)) - 1;
    const MINOR_MAX: u32 = (0x1 << 20) - 1;

    /// Create a `DeviceID` from the corresponding pair of major/minor versions.
    ///
    /// # Panics
    /// In the FUSE protocol, this identifier is encoded as a 32-bit integer.
    /// Therefore, this constructor will panic if the passed arguments is outside
    /// of the following range:
    /// * `major`: 12bits (0x000 - 0xFFF)
    /// * `minor`: 20bits (0x00000 - 0xFFFFF)
    #[inline]
    pub const fn new(major: u32, minor: u32) -> Self {
        assert!(major <= Self::MAJOR_MAX, "DeviceID.major");
        assert!(minor <= Self::MINOR_MAX, "DeviceID.minor");
        Self {
            raw: (major & 0xfff << 20) | (minor & 0xfffff),
        }
    }

    /// Return the major number of this ID.
    #[inline]
    pub const fn major(&self) -> u32 {
        self.raw >> 20
    }

    /// Return the minor number of this ID.
    #[inline]
    pub const fn minor(&self) -> u32 {
        self.raw & 0xfffff
    }

    /// Convert the value of userland `dev_t` to `DeviceID`.
    ///
    /// # Panics
    /// See also the documentation of `DeviceID::new` for
    /// the range limitations of major/minor numbers.
    #[inline]
    pub const fn from_userspace_dev(rdev: dev_t) -> Self {
        Self::new(libc::major(rdev), libc::minor(rdev))
    }

    #[inline]
    pub const fn from_kernel_dev(rdev: u32) -> Self {
        Self { raw: rdev }
    }

    #[inline]
    pub const fn into_userspace_dev(self) -> dev_t {
        libc::makedev(self.major(), self.minor())
    }

    #[inline]
    pub const fn into_kernel_dev(self) -> u32 {
        self.raw
    }
}

/// The file mode.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct FileMode {
    raw: u32,
}

impl fmt::Debug for FileMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileMode")
            .field("file_type", &self.file_type())
            .field("permissions", &self.permissions())
            .finish()
    }
}

impl FileMode {
    pub const fn new(typ: FileType, perm: FilePermissions) -> Self {
        Self::from_raw(typ.into_raw() | perm.bits())
    }

    pub const fn from_raw(raw: u32) -> Self {
        // TODO: type check
        Self { raw }
    }

    pub const fn file_type(self) -> Option<FileType> {
        FileType::from_raw(self.raw)
    }

    pub const fn permissions(self) -> FilePermissions {
        FilePermissions::from_bits_truncate(self.raw)
    }

    pub const fn into_raw(self) -> u32 {
        self.raw
    }
}

/// File types.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum FileType {
    Regular,
    Directory,
    CharacterDevice,
    BlockDevice,
    SymbolicLink,
    Fifo,
    Socket,
}

impl FileType {
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw & libc::S_IFMT {
            libc::S_IFREG => Some(Self::Regular),
            libc::S_IFDIR => Some(Self::Directory),
            libc::S_IFCHR => Some(Self::CharacterDevice),
            libc::S_IFBLK => Some(Self::BlockDevice),
            libc::S_IFLNK => Some(Self::SymbolicLink),
            libc::S_IFIFO => Some(Self::Fifo),
            libc::S_IFSOCK => Some(Self::Socket),
            _ => None,
        }
    }

    pub const fn into_raw(self) -> u32 {
        match self {
            Self::Regular => libc::S_IFREG,
            Self::Directory => libc::S_IFDIR,
            Self::CharacterDevice => libc::S_IFCHR,
            Self::BlockDevice => libc::S_IFBLK,
            Self::SymbolicLink => libc::S_IFLNK,
            Self::Fifo => libc::S_IFIFO,
            Self::Socket => libc::S_IFSOCK,
        }
    }
}

bitflags! {
    /// File permissions.
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct FilePermissions: u32 {
        const READ_USER = libc::S_IRUSR;
        const READ_GROUP = libc::S_IRGRP;
        const READ_OTHER = libc::S_IROTH;

        const WRITE_USER = libc::S_IWUSR;
        const WRITE_GROUP = libc::S_IWGRP;
        const WRITE_OTHER = libc::S_IWOTH;

        const EXEC_USER = libc::S_IXUSR;
        const EXEC_GROUP = libc::S_IXGRP;
        const EXEC_OTHER = libc::S_IXOTH;

        const SETUID = libc::S_ISUID;
        const SETGID = libc::S_ISGID;

        const STICKY = libc::S_ISVTX;
    }
}

impl FilePermissions {
    pub const READ: Self = Self::READ_USER
        .union(Self::READ_GROUP)
        .union(Self::READ_OTHER);

    pub const WRITE: Self = Self::WRITE_USER
        .union(Self::WRITE_GROUP)
        .union(Self::WRITE_OTHER);

    pub const EXEC: Self = Self::EXEC_USER
        .union(Self::EXEC_GROUP)
        .union(Self::EXEC_OTHER);
}

impl From<FilePermissions> for std::fs::Permissions {
    fn from(perm: FilePermissions) -> Self {
        Self::from_mode(perm.bits())
    }
}

impl From<std::fs::Permissions> for FilePermissions {
    fn from(perm: std::fs::Permissions) -> Self {
        Self::from_bits_truncate(perm.mode())
    }
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct PollEvents: u32 {
        const IN = libc::POLLIN as u32;
        const OUT = libc::POLLOUT as u32;
        const RDHUP = libc::POLLRDHUP as u32;
        const ERR = libc::POLLERR as u32;
        const HUP = libc::POLLHUP as u32;
        const INVAL = libc::POLLNVAL as u32;
    }
}

/// Attributes about a file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct FileAttr {
    pub ino: NodeID,
    pub size: u64,
    pub mode: FileMode,
    pub nlink: u32,
    pub uid: UID,
    pub gid: GID,
    pub rdev: DeviceID,
    pub blksize: u32,
    pub blocks: u64,
    pub atime: Duration,
    pub mtime: Duration,
    pub ctime: Duration,
}

impl Default for FileAttr {
    fn default() -> Self {
        Self::new()
    }
}

impl FileAttr {
    pub const fn new() -> Self {
        Self {
            ino: NodeID::ROOT,
            size: 0,
            mode: FileMode::new(FileType::Regular, FilePermissions::READ),
            nlink: 1,
            uid: UID::ROOT,
            gid: GID::ROOT,
            rdev: DeviceID::new(0, 0),
            blksize: 0,
            blocks: 0,
            atime: Duration::new(0, 0),
            mtime: Duration::new(0, 0),
            ctime: Duration::new(0, 0),
        }
    }
}

impl TryFrom<libc::stat> for FileAttr {
    type Error = std::convert::Infallible;

    #[inline]
    fn try_from(st: libc::stat) -> Result<Self, Self::Error> {
        Self::try_from(&st)
    }
}

impl TryFrom<&libc::stat> for FileAttr {
    type Error = std::convert::Infallible;

    fn try_from(st: &libc::stat) -> Result<Self, Self::Error> {
        Ok(Self {
            ino: NodeID::from_raw(st.st_ino),
            size: st.st_size as _,
            mode: FileMode::from_raw(st.st_mode),
            nlink: st.st_nlink as _,
            uid: UID::from_raw(st.st_uid),
            gid: GID::from_raw(st.st_gid),
            rdev: DeviceID::from_userspace_dev(st.st_rdev),
            blksize: st.st_blksize as _,
            blocks: st.st_blocks as _,
            atime: Duration::new(st.st_atime as _, st.st_atime_nsec as _),
            mtime: Duration::new(st.st_mtime as _, st.st_mtime_nsec as _),
            ctime: Duration::new(st.st_ctime as _, st.st_ctime_nsec as _),
        })
    }
}

impl TryFrom<Metadata> for FileAttr {
    type Error = std::convert::Infallible;

    #[inline]
    fn try_from(metadata: Metadata) -> Result<Self, Self::Error> {
        Self::try_from(&metadata)
    }
}

impl TryFrom<&Metadata> for FileAttr {
    type Error = std::convert::Infallible;

    fn try_from(m: &Metadata) -> Result<Self, Self::Error> {
        Ok(Self {
            ino: NodeID::from_raw(m.ino()),
            size: m.size(),
            mode: FileMode::from_raw(m.mode()),
            nlink: m.nlink() as _,
            uid: UID::from_raw(m.uid()),
            gid: GID::from_raw(m.gid()),
            rdev: DeviceID::from_userspace_dev(m.rdev()),
            blksize: m.blksize() as _,
            blocks: m.blocks(),
            atime: Duration::new(m.atime() as _, m.atime_nsec() as _),
            mtime: Duration::new(m.mtime() as _, m.mtime_nsec() as _),
            ctime: Duration::new(m.ctime() as _, m.ctime_nsec() as _),
        })
    }
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Statfs {
    pub bsize: u32,
    pub frsize: u32,
    pub blocks: u64,
    pub bfree: u64,
    pub bavail: u64,
    pub files: u64,
    pub ffree: u64,
    pub namelen: u32,
}

impl TryFrom<libc::statvfs> for Statfs {
    type Error = std::convert::Infallible;

    fn try_from(st: libc::statvfs) -> Result<Self, Self::Error> {
        Self::try_from(&st)
    }
}

impl TryFrom<&libc::statvfs> for Statfs {
    type Error = std::convert::Infallible;

    fn try_from(st: &libc::statvfs) -> Result<Self, Self::Error> {
        Ok(Self {
            bsize: st.f_bsize as _,
            frsize: st.f_frsize as _,
            blocks: st.f_blocks,
            bfree: st.f_bfree,
            bavail: st.f_bavail,
            files: st.f_files,
            ffree: st.f_ffree,
            namelen: st.f_namemax as _,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct FileLock {
    pub typ: u32,
    pub start: u64,
    pub end: u64,
    pub pid: PID,
}
