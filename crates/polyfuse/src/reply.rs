use crate::bytes::{Bytes, FillBytes};
use bitflags::bitflags;
use polyfuse_kernel::*;
use std::{convert::TryInto as _, ffi::OsStr, fmt, mem, os::unix::prelude::*, time::Duration};
use zerocopy::IntoBytes as _;

/// Attributes about a file.
#[derive(Default)]
#[repr(transparent)]
pub struct FileAttr {
    pub(crate) attr: fuse_attr,
}

impl FileAttr {
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
    fn from(value: libc::stat) -> Self {
        Self::from(&value)
    }
}

impl From<&libc::stat> for FileAttr {
    fn from(st: &libc::stat) -> Self {
        Self {
            attr: fuse_attr {
                ino: st.st_ino,
                size: st.st_size as u64,
                blocks: st.st_blocks as u64,
                atime: st.st_atime as u64,
                mtime: st.st_mtime as u64,
                ctime: st.st_ctime as u64,
                atimensec: st.st_atime_nsec as u32,
                mtimensec: st.st_mtime_nsec as u32,
                ctimensec: st.st_ctime_nsec as u32,
                mode: st.st_mode,
                nlink: st.st_nlink as u32,
                uid: st.st_uid,
                gid: st.st_gid,
                rdev: st.st_rdev as u32,
                blksize: st.st_blksize as u32,
                padding: 0,
            },
        }
    }
}

bitflags! {
    pub struct OpenFlags: u32 {
        /// The direct I/O is used on the opened file.
        const DIRECT_IO = FOPEN_DIRECT_IO;
        /// The currently cached file data in the kernel need not be invalidated.
        const KEEP_CACHE = FOPEN_KEEP_CACHE;
        /// The opened file is not seekable.
        const NONSEEKABLE = FOPEN_NONSEEKABLE;
        /// Enable caching of entries returned by `readdir`.
        ///
        /// This flag is meaningful only for `opendir` operations.
        const CACHE_DIR = FOPEN_CACHE_DIR;
    }
}

#[derive(Default)]
pub struct StatfsOut {
    out: fuse_statfs_out,
}

impl fmt::Debug for StatfsOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("StatfsOut").finish()
    }
}

impl Bytes for StatfsOut {
    #[inline]
    fn size(&self) -> usize {
        self.out.as_bytes().len()
    }

    #[inline]
    fn count(&self) -> usize {
        1
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        dst.put(self.out.as_bytes());
    }
}

impl StatfsOut {
    /// Return the object to fill the filesystem statistics.
    pub fn statfs(&mut self) -> &mut Statfs {
        Statfs::from_kstatfs_mut(&mut self.out.st)
    }
}

#[derive(Default)]
pub struct Statfs {
    st: fuse_kstatfs,
}

impl Statfs {
    #[inline]
    fn from_kstatfs_mut(st: &mut fuse_kstatfs) -> &mut Statfs {
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

#[derive(Default)]
pub struct XattrOut {
    out: fuse_getxattr_out,
}

impl fmt::Debug for XattrOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("XattrOut").finish()
    }
}

impl Bytes for XattrOut {
    #[inline]
    fn size(&self) -> usize {
        self.out.as_bytes().len()
    }

    #[inline]
    fn count(&self) -> usize {
        1
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        dst.put(self.out.as_bytes());
    }
}

impl XattrOut {
    pub fn size(&mut self, size: u32) {
        self.out.size = size;
    }
}

#[derive(Default)]
pub struct LkOut {
    out: fuse_lk_out,
}

impl fmt::Debug for LkOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("LkOut").finish()
    }
}

impl Bytes for LkOut {
    #[inline]
    fn size(&self) -> usize {
        self.out.as_bytes().len()
    }

    #[inline]
    fn count(&self) -> usize {
        1
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        dst.put(self.out.as_bytes());
    }
}

impl LkOut {
    pub fn file_lock(&mut self) -> &mut FileLock {
        FileLock::from_file_lock_mut(&mut self.out.lk)
    }
}

#[repr(transparent)]
pub struct FileLock {
    lk: fuse_file_lock,
}

impl FileLock {
    #[inline]
    fn from_file_lock_mut(lk: &mut fuse_file_lock) -> &mut Self {
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

#[derive(Default)]
pub struct BmapOut {
    out: fuse_bmap_out,
}

impl fmt::Debug for BmapOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("BmapOut").finish()
    }
}

impl Bytes for BmapOut {
    #[inline]
    fn size(&self) -> usize {
        self.out.as_bytes().len()
    }

    #[inline]
    fn count(&self) -> usize {
        1
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        dst.put(self.out.as_bytes());
    }
}

impl BmapOut {
    pub fn block(&mut self, block: u64) {
        self.out.block = block;
    }
}

#[derive(Default)]
pub struct PollOut {
    out: fuse_poll_out,
}

impl fmt::Debug for PollOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("PollOut").finish()
    }
}

impl Bytes for PollOut {
    #[inline]
    fn size(&self) -> usize {
        self.out.as_bytes().len()
    }

    #[inline]
    fn count(&self) -> usize {
        1
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        dst.put(self.out.as_bytes());
    }
}

impl PollOut {
    pub fn revents(&mut self, revents: u32) {
        self.out.revents = revents;
    }
}

pub struct ReaddirOut {
    buf: Vec<u8>,
}

impl fmt::Debug for ReaddirOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("ReaddirOut").finish()
    }
}

impl Bytes for ReaddirOut {
    #[inline]
    fn size(&self) -> usize {
        self.buf.size()
    }

    #[inline]
    fn count(&self) -> usize {
        self.buf.count()
    }

    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        self.buf.fill_bytes(dst)
    }
}

impl ReaddirOut {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
        }
    }

    pub fn entry(&mut self, name: &OsStr, ino: u64, typ: u32, off: u64) -> bool {
        let name = name.as_bytes();
        let remaining = self.buf.capacity() - self.buf.len();

        let entry_size = mem::size_of::<fuse_dirent>() + name.len();
        let aligned_entry_size = aligned(entry_size);

        if remaining < aligned_entry_size {
            return true;
        }

        let dirent = fuse_dirent {
            ino,
            off,
            namelen: name.len().try_into().expect("name length is too long"),
            typ,
            name: [],
        };
        let lenbefore = self.buf.len();
        self.buf.extend_from_slice(dirent.as_bytes());
        self.buf.extend_from_slice(name);
        self.buf.resize(lenbefore + aligned_entry_size, 0);

        false
    }
}

#[inline]
const fn aligned(len: usize) -> usize {
    (len + mem::size_of::<u64>() - 1) & !(mem::size_of::<u64>() - 1)
}
