use polyfuse_kernel::{fuse_attr, fuse_file_lock, fuse_kstatfs};
use std::{fs::Metadata, os::unix::prelude::*, time::Duration};

/// Attributes about a file.
#[derive(Default)]
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

impl From<libc::stat> for FileAttr {
    #[inline]
    fn from(st: libc::stat) -> Self {
        Self::from(&st)
    }
}

impl From<&libc::stat> for FileAttr {
    fn from(st: &libc::stat) -> Self {
        Self {
            attr: fuse_attr {
                ino: st.st_ino,
                size: st.st_size.try_into().unwrap(),
                blocks: st.st_blocks.try_into().unwrap(),
                atime: st.st_atime.try_into().unwrap(),
                mtime: st.st_mtime.try_into().unwrap(),
                ctime: st.st_ctime.try_into().unwrap(),
                atimensec: st.st_atime_nsec.try_into().unwrap(),
                mtimensec: st.st_mtime_nsec.try_into().unwrap(),
                ctimensec: st.st_ctime_nsec.try_into().unwrap(),
                mode: st.st_mode,
                nlink: st.st_nlink.try_into().unwrap(),
                uid: st.st_uid,
                gid: st.st_gid,
                rdev: st.st_rdev.try_into().unwrap(),
                blksize: st.st_blksize.try_into().unwrap(),
                padding: 0,
            },
        }
    }
}

impl From<Metadata> for FileAttr {
    fn from(metadata: Metadata) -> Self {
        Self::from(&metadata)
    }
}

impl From<&Metadata> for FileAttr {
    fn from(metadata: &Metadata) -> Self {
        Self {
            attr: fuse_attr {
                ino: metadata.ino(),
                size: metadata.size(),
                blocks: metadata.blocks(),
                atime: metadata.atime().try_into().unwrap(),
                mtime: metadata.mtime().try_into().unwrap(),
                ctime: metadata.ctime().try_into().unwrap(),
                atimensec: metadata.atime_nsec().try_into().unwrap(),
                mtimensec: metadata.mtime_nsec().try_into().unwrap(),
                ctimensec: metadata.ctime_nsec().try_into().unwrap(),
                mode: metadata.mode(),
                nlink: metadata.nlink().try_into().unwrap(),
                uid: metadata.uid(),
                gid: metadata.gid(),
                rdev: metadata.rdev().try_into().unwrap(),
                blksize: metadata.blksize().try_into().unwrap(),
                padding: 0,
            },
        }
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

impl From<libc::statvfs> for Statfs {
    fn from(st: libc::statvfs) -> Self {
        Self::from(&st)
    }
}

impl From<&libc::statvfs> for Statfs {
    fn from(st: &libc::statvfs) -> Self {
        Self {
            st: fuse_kstatfs {
                blocks: st.f_blocks,
                bfree: st.f_bfree,
                bavail: st.f_bavail,
                files: st.f_files,
                ffree: st.f_ffree,
                bsize: st.f_bsize.try_into().unwrap(),
                namelen: st.f_namemax.try_into().unwrap(),
                frsize: st.f_frsize.try_into().unwrap(),
                padding: 0,
                spare: [0; 6],
            },
        }
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
