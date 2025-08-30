use std::time::Duration;

use polyfuse_kernel::{fuse_attr, fuse_file_lock, fuse_kstatfs};

/// Attributes about a file.
#[repr(transparent)]
pub struct FileAttr {
    attr: fuse_attr,
}

impl FileAttr {
    #[inline]
    pub(crate) fn from_attr_mut(attr: &mut fuse_attr) -> &mut FileAttr {
        unsafe { &mut *(attr as *mut fuse_attr as *mut FileAttr) }
    }

    /// Set the inode number.
    #[inline]
    pub fn ino(&mut self, ino: u64) {
        self.attr.ino = ino;
    }

    /// Set the size of content.
    #[inline]
    pub fn size(&mut self, size: u64) {
        self.attr.size = size;
    }

    /// Set the permission of the inode.
    #[inline]
    pub fn mode(&mut self, mode: u32) {
        self.attr.mode = mode;
    }

    /// Set the number of hard links.
    #[inline]
    pub fn nlink(&mut self, nlink: u32) {
        self.attr.nlink = nlink;
    }

    /// Set the user ID.
    #[inline]
    pub fn uid(&mut self, uid: u32) {
        self.attr.uid = uid;
    }

    /// Set the group ID.
    #[inline]
    pub fn gid(&mut self, gid: u32) {
        self.attr.gid = gid;
    }

    /// Set the device ID.
    #[inline]
    pub fn rdev(&mut self, rdev: u32) {
        self.attr.rdev = rdev;
    }

    /// Set the block size.
    #[inline]
    pub fn blksize(&mut self, blksize: u32) {
        self.attr.blksize = blksize;
    }

    /// Set the number of allocated blocks.
    #[inline]
    pub fn blocks(&mut self, blocks: u64) {
        self.attr.blocks = blocks;
    }

    /// Set the last accessed time.
    #[inline]
    pub fn atime(&mut self, atime: Duration) {
        self.attr.atime = atime.as_secs();
        self.attr.atimensec = atime.subsec_nanos();
    }

    /// Set the last modification time.
    #[inline]
    pub fn mtime(&mut self, mtime: Duration) {
        self.attr.mtime = mtime.as_secs();
        self.attr.mtimensec = mtime.subsec_nanos();
    }

    /// Set the last created time.
    #[inline]
    pub fn ctime(&mut self, ctime: Duration) {
        self.attr.ctime = ctime.as_secs();
        self.attr.ctimensec = ctime.subsec_nanos();
    }
}

#[derive(Default)]
pub struct Statfs {
    st: fuse_kstatfs,
}

impl Statfs {
    #[inline]
    pub(crate) fn from_kstatfs_mut(st: &mut fuse_kstatfs) -> &mut Statfs {
        unsafe { &mut *(st as *mut fuse_kstatfs as *mut Statfs) }
    }

    /// Set the block size.
    pub fn bsize(&mut self, bsize: u32) {
        self.st.bsize = bsize;
    }

    /// Set the fragment size.
    pub fn frsize(&mut self, frsize: u32) {
        self.st.frsize = frsize;
    }

    /// Set the number of blocks in the filesystem.
    pub fn blocks(&mut self, blocks: u64) {
        self.st.blocks = blocks;
    }

    /// Set the number of free blocks.
    pub fn bfree(&mut self, bfree: u64) {
        self.st.bfree = bfree;
    }

    /// Set the number of free blocks for non-priviledge users.
    pub fn bavail(&mut self, bavail: u64) {
        self.st.bavail = bavail;
    }

    /// Set the number of inodes.
    pub fn files(&mut self, files: u64) {
        self.st.files = files;
    }

    /// Set the number of free inodes.
    pub fn ffree(&mut self, ffree: u64) {
        self.st.ffree = ffree;
    }

    /// Set the maximum length of file names.
    pub fn namelen(&mut self, namelen: u32) {
        self.st.namelen = namelen;
    }
}

#[repr(transparent)]
pub struct FileLock {
    lk: fuse_file_lock,
}

impl FileLock {
    #[inline]
    pub(crate) fn from_file_lock_mut(lk: &mut fuse_file_lock) -> &mut Self {
        unsafe { &mut *(lk as *mut fuse_file_lock as *mut Self) }
    }

    /// Set the type of this lock.
    pub fn typ(&mut self, typ: u32) {
        self.lk.typ = typ;
    }

    /// Set the starting offset to be locked.
    pub fn start(&mut self, start: u64) {
        self.lk.start = start;
    }

    /// Set the ending offset to be locked.
    pub fn end(&mut self, end: u64) {
        self.lk.end = end;
    }

    /// Set the process ID.
    pub fn pid(&mut self, pid: u32) {
        self.lk.pid = pid;
    }
}
