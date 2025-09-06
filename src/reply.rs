use crate::{
    bytes::{Bytes, FillBytes},
    types::{FileAttr, FileID, FileLock, FileType, NodeID, PollEvents, Statfs},
};
use polyfuse_kernel::*;
use std::{convert::TryInto as _, ffi::OsStr, fmt, mem, os::unix::prelude::*, time::Duration};
use zerocopy::IntoBytes as _;

fn fill_fuse_attr(slot: &mut fuse_attr, attr: &FileAttr) {
    slot.ino = attr.ino.into_raw();
    slot.size = attr.size;
    slot.blocks = attr.blocks;
    slot.atime = attr.atime.as_secs();
    slot.mtime = attr.mtime.as_secs();
    slot.ctime = attr.ctime.as_secs();
    slot.atimensec = attr.atime.subsec_nanos();
    slot.mtimensec = attr.mtime.subsec_nanos();
    slot.ctimensec = attr.ctime.subsec_nanos();
    slot.mode = attr.mode.into_raw();
    slot.nlink = attr.nlink;
    slot.uid = attr.uid.into_raw();
    slot.gid = attr.gid.into_raw();
    slot.rdev = attr.rdev.into_kernel_dev();
    slot.blksize = attr.blksize;
}

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
    pub fn attr(&mut self, attr: FileAttr) {
        fill_fuse_attr(&mut self.out.attr, &attr);
    }

    /// Set the inode number of this entry.
    ///
    /// If this value is zero, it means that the entry is *negative*.
    /// Returning a negative entry is also possible with the `ENOENT` error,
    /// but the *zeroed* entries also have the ability to specify the lifetime
    /// of the entry cache by using the `ttl_entry` parameter.
    #[inline]
    pub fn ino(&mut self, ino: NodeID) {
        self.out.nodeid = ino.into_raw();
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
    pub fn attr(&mut self, attr: FileAttr) {
        fill_fuse_attr(&mut self.out.attr, &attr);
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
    pub fn fh(&mut self, fh: FileID) {
        self.out.fh = fh.into_raw();
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
    pub fn statfs(&mut self, st: Statfs) {
        self.out.st = fuse_kstatfs {
            blocks: st.blocks,
            bfree: st.bfree,
            bavail: st.bavail,
            files: st.files,
            ffree: st.ffree,
            bsize: st.bsize,
            namelen: st.namelen,
            frsize: st.frsize,
            padding: 0,
            spare: [0; 6],
        };
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
    pub fn file_lock(&mut self, lk: &FileLock) {
        self.out.lk = fuse_file_lock {
            start: lk.start,
            end: lk.end,
            typ: lk.typ,
            pid: lk.pid.into_raw(),
        };
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
    pub fn revents(&mut self, revents: PollEvents) {
        self.out.revents = revents.bits();
    }
}

#[derive(Default)]
pub struct LseekOut {
    out: fuse_lseek_out,
}

impl fmt::Debug for LseekOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("LseekOut").finish()
    }
}

impl Bytes for LseekOut {
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

impl LseekOut {
    pub fn offset(&mut self, offset: u64) {
        self.out.offset = offset;
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

    pub fn entry(&mut self, name: &OsStr, ino: NodeID, typ: Option<FileType>, off: u64) -> bool {
        let name = name.as_bytes();
        let remaining = self.buf.capacity() - self.buf.len();

        let entry_size = mem::size_of::<fuse_dirent>() + name.len();
        let aligned_entry_size = aligned(entry_size);

        if remaining < aligned_entry_size {
            return true;
        }

        let typ = match typ {
            Some(FileType::BlockDevice) => libc::DT_BLK,
            Some(FileType::CharacterDevice) => libc::DT_CHR,
            Some(FileType::Directory) => libc::DT_DIR,
            Some(FileType::Fifo) => libc::DT_FIFO,
            Some(FileType::SymbolicLink) => libc::DT_LNK,
            Some(FileType::Regular) => libc::DT_REG,
            Some(FileType::Socket) => libc::DT_SOCK,
            None => libc::DT_UNKNOWN,
        };

        let dirent = fuse_dirent {
            ino: ino.into_raw(),
            off,
            namelen: name.len().try_into().expect("name length is too long"),
            typ: typ as u32,
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
