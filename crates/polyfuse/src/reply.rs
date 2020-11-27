use std::{ffi::OsStr, time::Duration};

/// Attributes about a file.
pub trait FileAttr {
    /// Set the inode number.
    fn ino(&mut self, ino: u64);

    /// Set the size of content.
    fn size(&mut self, size: u64);

    /// Set the permission of the inode.
    fn mode(&mut self, mode: u32);

    /// Set the number of hard links.
    fn nlink(&mut self, nlink: u32);

    /// Set the user ID.
    fn uid(&mut self, uid: u32);

    /// Set the group ID.
    fn gid(&mut self, gid: u32);

    /// Set the device ID.
    fn rdev(&mut self, rdev: u32);

    /// Set the block size.
    fn blksize(&mut self, blksize: u32);

    /// Set the number of allocated blocks.
    fn blocks(&mut self, blocks: u64);

    /// Set the last accessed time.
    fn atime(&mut self, sec: u64, nsec: u32);

    /// Set the last modification time.
    fn mtime(&mut self, sec: u64, nsec: u32);

    /// Set the last created time.
    fn ctime(&mut self, sec: u64, nsec: u32);
}

pub trait ReplyEntry {
    type Ok;
    type Error;

    fn send<F>(self, f: F) -> Result<Self::Ok, Self::Error>
    where
        F: FnOnce(&mut dyn EntryOut);

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait EntryOut {
    /// Return the object to fill attribute values about this entry.
    fn attr(&mut self) -> &mut dyn FileAttr;

    /// Set the inode number of this entry.
    ///
    /// If this value is zero, it means that the entry is *negative*.
    /// Returning a negative entry is also possible with the `ENOENT` error,
    /// but the *zeroed* entries also have the ability to specify the lifetime
    /// of the entry cache by using the `ttl_entry` parameter.
    fn ino(&mut self, ino: u64);

    /// Set the generation of this entry.
    ///
    /// This parameter is used to distinguish the inode from the past one
    /// when the filesystem reuse inode numbers.  That is, the operations
    /// must ensure that the pair of entry's inode number and generation
    /// are unique for the lifetime of the filesystem.
    fn generation(&mut self, generation: u64);

    /// Set the validity timeout for inode attributes.
    ///
    /// The operations should set this value to very large
    /// when the changes of inode attributes are caused
    /// only by FUSE requests.
    fn ttl_attr(&mut self, ttl: Duration);

    /// Set the validity timeout for the name.
    ///
    /// The operations should set this value to very large
    /// when the changes/deletions of directory entries are
    /// caused only by FUSE requests.
    fn ttl_entry(&mut self, ttl: Duration);
}

pub trait ReplyAttr {
    type Ok;
    type Error;

    fn send<F>(self, f: F) -> Result<Self::Ok, Self::Error>
    where
        F: FnOnce(&mut dyn AttrOut);

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait AttrOut {
    /// Return the object to fill attribute values.
    fn attr(&mut self) -> &mut dyn FileAttr;

    /// Set the validity timeout for this attribute.
    fn ttl(&mut self, ttl: Duration);
}

pub trait ReplyOk {
    type Ok;
    type Error;

    fn send(self) -> Result<Self::Ok, Self::Error>;
    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyReadlink {
    type Ok;
    type Error;

    fn send<T>(self, link: T) -> Result<Self::Ok, Self::Error>
    where
        T: AsRef<OsStr> + Send + 'static;

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyData {
    type Ok;
    type Error;

    /// Return the remaining length of bytes that can be added.
    fn remaining(&self) -> usize;

    /// Append a chunk of bytes to the end of the reply.
    fn chunk<T>(&mut self, chunk: T) -> Option<T>
    where
        T: AsRef<[u8]> + Send + 'static;

    fn send(self) -> Result<Self::Ok, Self::Error>;
    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyOpen {
    type Ok;
    type Error;

    fn send<F>(self, f: F) -> Result<Self::Ok, Self::Error>
    where
        F: FnOnce(&mut dyn OpenOut);

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait OpenOut {
    /// Set the handle of opened file.
    fn fh(&mut self, fh: u64);

    /// Indicates that the direct I/O is used on this file.
    fn direct_io(&mut self, enabled: bool);

    /// Indicates that the currently cached file data in the kernel
    /// need not be invalidated.
    fn keep_cache(&mut self, enabled: bool);

    /// Indicates that the opened file is not seekable.
    fn nonseekable(&mut self, enabled: bool);

    /// Enable caching of entries returned by `readdir`.
    ///
    /// This flag is meaningful only for `opendir` operations.
    fn cache_dir(&mut self, enabled: bool);
}

pub trait ReplyWrite {
    type Ok;
    type Error;

    fn send(self, size: u32) -> Result<Self::Ok, Self::Error>;
    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyStatfs {
    type Ok;
    type Error;

    fn send<F>(self, f: F) -> Result<Self::Ok, Self::Error>
    where
        F: FnOnce(&mut dyn StatfsOut);

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait StatfsOut {
    /// Return the object to fill the filesystem statistics.
    fn statfs(&mut self) -> &mut dyn Statfs;
}

pub trait Statfs {
    /// Set the block size.
    fn bsize(&mut self, bsize: u32);

    /// Set the fragment size.
    fn frsize(&mut self, frsize: u32);

    /// Set the number of blocks in the filesystem.
    fn blocks(&mut self, blocks: u64);

    /// Set the number of free blocks.
    fn bfree(&mut self, bfree: u64);

    /// Set the number of free blocks for non-priviledge users.
    fn bavail(&mut self, bavail: u64);

    /// Set the number of inodes.
    fn files(&mut self, files: u64);

    /// Set the number of free inodes.
    fn ffree(&mut self, ffree: u64);

    /// Set the maximum length of file names.
    fn namelen(&mut self, namelen: u32);
}

pub trait ReplyXattrSize {
    type Ok;
    type Error;

    fn send(self, size: u32) -> Result<Self::Ok, Self::Error>;

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyXattrData {
    type Ok;
    type Error;

    fn send<T>(self, data: T) -> Result<Self::Ok, Self::Error>
    where
        T: AsRef<[u8]> + Send + 'static;

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyLk {
    type Ok;
    type Error;

    fn send<F>(self, f: F) -> Result<Self::Ok, Self::Error>
    where
        F: FnOnce(&mut dyn LkOut);

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait LkOut {
    /// Return the object to fill the lock information.
    fn flock(&mut self) -> &mut dyn FileLock;
}

pub trait FileLock {
    /// Set the type of this lock.
    fn typ(&mut self, typ: u32);

    /// Set the starting offset to be locked.
    fn start(&mut self, start: u64);

    /// Set the ending offset to be locked.
    fn end(&mut self, end: u64);

    /// Set the process ID.
    fn pid(&mut self, pid: u32);
}

pub trait ReplyCreate {
    type Ok;
    type Error;

    fn send<F>(self, f: F) -> Result<Self::Ok, Self::Error>
    where
        F: FnOnce(&mut dyn EntryOut, &mut dyn OpenOut);

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyBmap {
    type Ok;
    type Error;

    fn send(self, block: u64) -> Result<Self::Ok, Self::Error>;

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyPoll {
    type Ok;
    type Error;

    fn send(self, revents: u32) -> Result<Self::Ok, Self::Error>;

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyDirs {
    type Ok;
    type Error;

    fn entry<F>(&mut self, name: &OsStr, f: F) -> bool
    where
        F: FnOnce(&mut dyn DirEntry);

    fn send(self) -> Result<Self::Ok, Self::Error>;
    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyDirsPlus {
    type Ok;
    type Error;

    fn entry<F>(&mut self, name: &OsStr, f: F) -> bool
    where
        F: FnOnce(&mut dyn EntryOut, &mut dyn DirEntry);

    fn send(self) -> Result<Self::Ok, Self::Error>;
    fn error(self, code: i32) -> Result<Self::Ok, Self::Error>;
}

pub trait DirEntry {
    /// Set the inode number of this entry.
    fn ino(&mut self, ino: u64);

    /// Set the file type of this entry.
    fn typ(&mut self, typ: u32);

    /// Set the offset value of this entry.
    fn offset(&mut self, offset: u64);
}
