use crate::bytes::{Bytes, Collector};
use polyfuse_kernel::{
    fuse_attr, fuse_attr_out, fuse_bmap_out, fuse_dirent, fuse_entry_out, fuse_file_lock,
    fuse_getxattr_out, fuse_kstatfs, fuse_lk_out, fuse_open_out, fuse_poll_out, fuse_statfs_out,
    fuse_write_out, FOPEN_CACHE_DIR, FOPEN_DIRECT_IO, FOPEN_KEEP_CACHE, FOPEN_NONSEEKABLE,
};
use std::{convert::TryInto as _, ffi::OsStr, mem, os::unix::prelude::*, time::Duration};
use zerocopy::AsBytes as _;

/// Attributes about a file.
#[repr(transparent)]
pub struct FileAttr {
    attr: fuse_attr,
}

impl FileAttr {
    #[inline]
    fn from_attr_mut(attr: &mut fuse_attr) -> &mut FileAttr {
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
pub struct EntryOut {
    out: fuse_entry_out,
}

impl Bytes for EntryOut {
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        collector.append(self.out.as_bytes());
    }
}

impl EntryOut {
    /// Return the object to fill attribute values about this entry.
    #[inline]
    pub fn attr(&mut self) -> &mut FileAttr {
        FileAttr::from_attr_mut(&mut self.out.attr)
    }

    /// Set the inode number of this entry.
    ///
    /// If this value is zero, it means that the entry is *negative*.
    /// Returning a negative entry is also possible with the `ENOENT` error,
    /// but the *zeroed* entries also have the ability to specify the lifetime
    /// of the entry cache by using the `ttl_entry` parameter.
    #[inline]
    pub fn ino(&mut self, ino: u64) {
        self.out.nodeid = ino;
    }

    /// Set the generation of this entry.
    ///
    /// This parameter is used to distinguish the inode from the past one
    /// when the filesystem reuse inode numbers.  That is, the operations
    /// must ensure that the pair of entry's inode number and generation
    /// are unique for the lifetime of the filesystem.
    pub fn generation(&mut self, generation: u64) {
        self.out.generation = generation;
    }

    /// Set the validity timeout for inode attributes.
    ///
    /// The operations should set this value to very large
    /// when the changes of inode attributes are caused
    /// only by FUSE requests.
    pub fn ttl_attr(&mut self, ttl: Duration) {
        self.out.attr_valid = ttl.as_secs();
        self.out.attr_valid_nsec = ttl.subsec_nanos();
    }

    /// Set the validity timeout for the name.
    ///
    /// The operations should set this value to very large
    /// when the changes/deletions of directory entries are
    /// caused only by FUSE requests.
    pub fn ttl_entry(&mut self, ttl: Duration) {
        self.out.entry_valid = ttl.as_secs();
        self.out.entry_valid_nsec = ttl.subsec_nanos();
    }
}

#[derive(Default)]
pub struct AttrOut {
    out: fuse_attr_out,
}

impl AttrOut {
    /// Return the object to fill attribute values.
    #[inline]
    pub fn attr(&mut self) -> &mut FileAttr {
        FileAttr::from_attr_mut(&mut self.out.attr)
    }

    /// Set the validity timeout for this attribute.
    pub fn ttl(&mut self, ttl: Duration) {
        self.out.attr_valid = ttl.as_secs();
        self.out.attr_valid_nsec = ttl.subsec_nanos();
    }
}

impl Bytes for AttrOut {
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        collector.append(self.out.as_bytes());
    }
}

#[derive(Default)]
pub struct OpenOut {
    out: fuse_open_out,
}

impl Bytes for OpenOut {
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        collector.append(self.out.as_bytes());
    }
}

impl OpenOut {
    /// Set the handle of opened file.
    pub fn fh(&mut self, fh: u64) {
        self.out.fh = fh;
    }

    #[inline]
    fn set_flag(&mut self, flag: u32, enabled: bool) {
        if enabled {
            self.out.open_flags |= flag;
        } else {
            self.out.open_flags &= !flag;
        }
    }

    /// Indicates that the direct I/O is used on this file.
    pub fn direct_io(&mut self, enabled: bool) {
        self.set_flag(FOPEN_DIRECT_IO, enabled);
    }

    /// Indicates that the currently cached file data in the kernel
    /// need not be invalidated.
    pub fn keep_cache(&mut self, enabled: bool) {
        self.set_flag(FOPEN_KEEP_CACHE, enabled);
    }

    /// Indicates that the opened file is not seekable.
    pub fn nonseekable(&mut self, enabled: bool) {
        self.set_flag(FOPEN_NONSEEKABLE, enabled);
    }

    /// Enable caching of entries returned by `readdir`.
    ///
    /// This flag is meaningful only for `opendir` operations.
    pub fn cache_dir(&mut self, enabled: bool) {
        self.set_flag(FOPEN_CACHE_DIR, enabled);
    }
}

#[derive(Default)]
pub struct WriteOut {
    out: fuse_write_out,
}

impl Bytes for WriteOut {
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        collector.append(self.out.as_bytes());
    }
}

impl WriteOut {
    pub fn size(&mut self, size: u32) {
        self.out.size = size;
    }
}

#[derive(Default)]
pub struct StatfsOut {
    out: fuse_statfs_out,
}

impl Bytes for StatfsOut {
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        collector.append(self.out.as_bytes());
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

impl Bytes for XattrOut {
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        collector.append(self.out.as_bytes());
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

impl Bytes for LkOut {
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        collector.append(self.out.as_bytes());
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

impl Bytes for BmapOut {
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        collector.append(self.out.as_bytes());
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

impl Bytes for PollOut {
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        collector.append(self.out.as_bytes());
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

impl Bytes for ReaddirOut {
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        collector.append(&self.buf[..]);
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
        self.buf.extend_from_slice(dirent.as_bytes());
        self.buf.extend_from_slice(name);
        self.buf.resize(self.buf.len() + aligned_entry_size, 0);

        false
    }
}

#[inline]
const fn aligned(len: usize) -> usize {
    (len + mem::size_of::<u64>() - 1) & !(mem::size_of::<u64>() - 1)
}
