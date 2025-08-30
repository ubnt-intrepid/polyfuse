use crate::{
    bytes::{Bytes, FillBytes},
    types::{FileAttr, FileLock, Statfs},
};
use polyfuse_kernel::*;
use std::{convert::TryInto as _, ffi::OsStr, fmt, mem, os::unix::prelude::*, time::Duration};
use zerocopy::IntoBytes as _;

#[derive(Default)]
pub struct EntryOut {
    out: fuse_entry_out,
}

impl fmt::Debug for EntryOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("EntryOut").finish()
    }
}

impl Bytes for EntryOut {
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

impl fmt::Debug for AttrOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("AttrOut").finish()
    }
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

#[derive(Default)]
pub struct OpenOut {
    out: fuse_open_out,
}

impl fmt::Debug for OpenOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("OpenOut").finish()
    }
}

impl Bytes for OpenOut {
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

impl fmt::Debug for WriteOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("WriteOut").finish()
    }
}

impl Bytes for WriteOut {
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

impl WriteOut {
    pub fn size(&mut self, size: u32) {
        self.out.size = size;
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
