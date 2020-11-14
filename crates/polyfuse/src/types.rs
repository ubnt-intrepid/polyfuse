//! Common types used in the filesystem representation.

pub(crate) use non_exhaustive::NonExhaustive;
mod non_exhaustive {
    #[derive(Copy, Clone, Debug)]
    pub struct NonExhaustive(());

    impl NonExhaustive {
        pub(crate) const INIT: Self = Self(());
    }
}

use std::{
    borrow::Cow,
    ffi::OsStr,
    fmt,
    time::{Duration, SystemTime},
};

/// The time value hliding seconds and nanoseconds.
#[derive(Copy, Clone, Debug)]
pub struct Timespec {
    #[allow(missing_docs)]
    pub secs: u64,

    #[allow(missing_docs)]
    pub nsecs: u32,

    #[doc(hidden)] // non_exhaustive
    pub __non_exhaustive: NonExhaustive,
}

impl Default for Timespec {
    #[inline]
    fn default() -> Self {
        Self {
            secs: 0,
            nsecs: 0,

            __non_exhaustive: NonExhaustive::INIT,
        }
    }
}

impl Timespec {
    /// Get the time value converted to [`SystemTime`](std::time::SystemTime).
    ///
    /// The conversion is performed by treating itself as increment
    /// from `SystemTime::UNIX_EPOCH`.
    #[inline]
    pub fn as_system_time(self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::new(self.secs, self.nsecs)
    }
}

/// Attributes about a file.
#[derive(Copy, Clone, Debug)]
pub struct FileAttr {
    /// Return the inode number.
    pub ino: u64,

    /// Return the size of content.
    pub size: u64,

    /// Return the permission of the inode.
    pub mode: u32,

    /// Return the number of hard links.
    pub nlink: u32,

    /// Return the user ID.
    pub uid: u32,

    /// Return the group ID.
    pub gid: u32,

    /// Return the device ID.
    pub rdev: u32,

    /// Return the block size.
    pub blksize: u32,

    /// Return the number of allocated blocks.
    pub blocks: u64,

    /// Return the last accessed time in raw form.
    pub atime: Timespec,

    /// Return the last modification time in raw form.
    pub mtime: Timespec,

    /// Return the last created time in raw form.
    pub ctime: Timespec,

    #[doc(hidden)] // non_exhaustive
    pub __non_exhaustive: NonExhaustive,
}

impl Default for FileAttr {
    #[inline]
    fn default() -> Self {
        Self {
            ino: 0,
            size: 0,
            mode: 0,
            nlink: 0,
            uid: 0,
            gid: 0,
            rdev: 0,
            blksize: 0,
            blocks: 0,
            atime: Timespec::default(),
            mtime: Timespec::default(),
            ctime: Timespec::default(),

            __non_exhaustive: NonExhaustive::INIT,
        }
    }
}

/// File lock information.
#[derive(Copy, Clone, Debug)]
pub struct FileLock {
    /// Return the type of lock.
    pub typ: u32,

    /// Return the starting offset for lock.
    pub start: u64,

    /// Return the ending offset for lock.
    pub end: u64,

    /// Return the process ID blocking the lock.
    pub pid: u32,

    #[doc(hidden)] // non_exhaustive
    pub __non_exhaustive: NonExhaustive,
}

impl Default for FileLock {
    #[inline]
    fn default() -> Self {
        Self {
            typ: 0,
            start: 0,
            end: 0,
            pid: 0,

            __non_exhaustive: NonExhaustive::INIT,
        }
    }
}

/// Filesystem statistics.
#[derive(Copy, Clone, Debug)]
pub struct FsStatistics {
    /// Return the block size.
    pub bsize: u32,

    /// Return the fragment size.
    pub frsize: u32,

    /// Return the number of blocks in the filesystem.
    pub blocks: u64,

    /// Return the number of free blocks.
    pub bfree: u64,

    /// Return the number of free blocks for non-priviledge users.
    pub bavail: u64,

    /// Return the number of inodes.
    pub files: u64,

    /// Return the number of free inodes.
    pub ffree: u64,

    /// Return the maximum length of file names.
    pub namelen: u32,

    #[doc(hidden)] // non_exhaustive
    pub __non_exhaustive: NonExhaustive,
}

impl Default for FsStatistics {
    #[inline]
    fn default() -> Self {
        Self {
            bsize: 0,
            frsize: 0,
            blocks: 0,
            bfree: 0,
            bavail: 0,
            files: 0,
            ffree: 0,
            namelen: 0,

            __non_exhaustive: NonExhaustive::INIT,
        }
    }
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

/// A directory entry replied to the kernel.
#[derive(Clone, Debug)]
pub struct DirEntry {
    /// Return the inode number of this entry.
    pub ino: u64,

    /// Return the offset value of this entry.
    pub offset: u64,

    /// Return the type of this entry.
    pub typ: u32,

    /// Returns the name of this entry.
    pub name: Cow<'static, OsStr>,

    #[doc(hidden)] // non_exhaustive
    pub __non_exhaustive: NonExhaustive,
}

impl Default for DirEntry {
    #[inline]
    fn default() -> Self {
        Self {
            ino: 0,
            offset: 0,
            typ: 0,
            name: Cow::Borrowed("".as_ref()),

            __non_exhaustive: NonExhaustive::INIT,
        }
    }
}
