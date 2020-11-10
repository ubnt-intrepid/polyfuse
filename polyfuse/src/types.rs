#![allow(missing_docs)]

use std::{ffi::OsStr, fmt};

/// Attributes about a file.
pub trait FileAttr {
    /// Return the inode number.
    fn ino(&self) -> u64;

    /// Return the size of content.
    fn size(&self) -> u64;

    /// Return the permission of the inode.
    fn mode(&self) -> u32;

    /// Return the number of hard links.
    fn nlink(&self) -> u32;

    /// Return the user ID.
    fn uid(&self) -> u32;

    /// Return the group ID.
    fn gid(&self) -> u32;

    /// Return the device ID.
    fn rdev(&self) -> u32;

    /// Return the block size.
    fn blksize(&self) -> u32;

    /// Return the number of allocated blocks.
    fn blocks(&self) -> u64;

    /// Return the last accessed time in raw form.
    fn atime(&self) -> (u64, u32);

    /// Return the last modification time in raw form.
    fn mtime(&self) -> (u64, u32);

    /// Return the last created time in raw form.
    fn ctime(&self) -> (u64, u32);
}

/// File lock information.
pub trait FileLock {
    /// Return the type of lock.
    fn typ(&self) -> u32;

    /// Return the starting offset for lock.
    fn start(&self) -> u64;

    /// Return the ending offset for lock.
    fn end(&self) -> u64;

    /// Return the process ID blocking the lock.
    fn pid(&self) -> u32;
}

/// Filesystem statistics.
pub trait FsStatistics {
    /// Return the block size.
    fn bsize(&self) -> u32;

    /// Return the fragment size.
    fn frsize(&self) -> u32;

    /// Return the number of blocks in the filesystem.
    fn blocks(&self) -> u64;

    /// Return the number of free blocks.
    fn bfree(&self) -> u64;

    /// Return the number of free blocks for non-priviledge users.
    fn bavail(&self) -> u64;

    /// Return the number of inodes.
    fn files(&self) -> u64;

    /// Return the number of free inodes.
    fn ffree(&self) -> u64;

    /// Return the maximum length of file names.
    fn namelen(&self) -> u32;
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
pub trait DirEntry {
    /// Return the inode number of this entry.
    fn ino(&self) -> u64;

    /// Return the offset value of this entry.
    fn offset(&self) -> u64;

    /// Return the type of this entry.
    fn typ(&self) -> u32;

    /// Returns the name of this entry.
    fn name(&self) -> &OsStr;
}
