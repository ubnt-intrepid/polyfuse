use crate::{
    bytes::{Bytes, FillBytes},
    types::{FileAttr, FileLock, FsStatistics, NodeID},
};
use polyfuse_kernel::*;
use std::{convert::TryInto as _, ffi::OsStr, fmt, mem, os::unix::prelude::*, time::Duration};
use zerocopy::IntoBytes as _;

fn to_fuse_attr(attr: impl FileAttr) -> fuse_attr {
    let atime = attr.atime();
    let mtime = attr.mtime();
    let ctime = attr.ctime();
    fuse_attr {
        ino: attr.ino().map_or(0, NodeID::into_raw),
        size: attr.size(),
        mode: attr.mode(),
        nlink: attr.nlink(),
        uid: attr.uid(),
        gid: attr.gid(),
        rdev: attr.rdev().to_kernel_dev_t(),
        blksize: attr.blksize(),
        blocks: attr.blocks(),
        atime: atime.as_secs(),
        atimensec: atime.subsec_nanos(),
        mtime: mtime.as_secs(),
        mtimensec: mtime.subsec_nanos(),
        ctime: ctime.as_secs(),
        ctimensec: ctime.subsec_nanos(),
        padding: 0,
    }
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
    /// Set the attribute values about this entry.
    #[inline]
    pub fn attr(&mut self, attr: impl FileAttr) -> &mut Self {
        self.out.attr = to_fuse_attr(attr);
        self
    }

    /// Set the inode number of this entry.
    ///
    /// If this value is zero, it means that the entry is *negative*.
    /// Returning a negative entry is also possible with the `ENOENT` error,
    /// but the *zeroed* entries also have the ability to specify the lifetime
    /// of the entry cache by using the `ttl_entry` parameter.
    #[inline]
    pub fn ino(&mut self, ino: NodeID) -> &mut Self {
        self.out.nodeid = ino.into_raw();
        self
    }

    /// Set the generation of this entry.
    ///
    /// This parameter is used to distinguish the inode from the past one
    /// when the filesystem reuse inode numbers.  That is, the operations
    /// must ensure that the pair of entry's inode number and generation
    /// are unique for the lifetime of the filesystem.
    pub fn generation(&mut self, generation: u64) -> &mut Self {
        self.out.generation = generation;
        self
    }

    /// Set the validity timeout for inode attributes.
    ///
    /// The operations should set this value to very large
    /// when the changes of inode attributes are caused
    /// only by FUSE requests.
    pub fn ttl_attr(&mut self, ttl: Duration) -> &mut Self {
        self.out.attr_valid = ttl.as_secs();
        self.out.attr_valid_nsec = ttl.subsec_nanos();
        self
    }

    /// Set the validity timeout for the name.
    ///
    /// The operations should set this value to very large
    /// when the changes/deletions of directory entries are
    /// caused only by FUSE requests.
    pub fn ttl_entry(&mut self, ttl: Duration) -> &mut Self {
        self.out.entry_valid = ttl.as_secs();
        self.out.entry_valid_nsec = ttl.subsec_nanos();
        self
    }
}

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
    #[inline]
    pub fn new(attr: impl FileAttr) -> Self {
        Self {
            out: fuse_attr_out {
                attr_valid: 0,
                attr_valid_nsec: 0,
                dummy: 0,
                attr: to_fuse_attr(attr),
            },
        }
    }

    /// Set the validity timeout for this attribute.
    pub fn ttl(&mut self, ttl: Duration) -> &mut Self {
        self.out.attr_valid = ttl.as_secs();
        self.out.attr_valid_nsec = ttl.subsec_nanos();
        self
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
    pub const fn new(fh: u64) -> Self {
        Self {
            out: fuse_open_out {
                fh,
                open_flags: 0,
                padding: 0,
            },
        }
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
    pub fn direct_io(&mut self, enabled: bool) -> &mut Self {
        self.set_flag(FOPEN_DIRECT_IO, enabled);
        self
    }

    /// Indicates that the currently cached file data in the kernel
    /// need not be invalidated.
    pub fn keep_cache(&mut self, enabled: bool) -> &mut Self {
        self.set_flag(FOPEN_KEEP_CACHE, enabled);
        self
    }

    /// Indicates that the opened file is not seekable.
    pub fn nonseekable(&mut self, enabled: bool) -> &mut Self {
        self.set_flag(FOPEN_NONSEEKABLE, enabled);
        self
    }

    /// Enable caching of entries returned by `readdir`.
    ///
    /// This flag is meaningful only for `opendir` operations.
    pub fn cache_dir(&mut self, enabled: bool) -> &mut Self {
        self.set_flag(FOPEN_CACHE_DIR, enabled);
        self
    }
}

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
    pub const fn new(size: u32) -> Self {
        Self {
            out: fuse_write_out { size, padding: 0 },
        }
    }
}

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
    pub fn new(st: impl FsStatistics) -> Self {
        Self {
            out: fuse_statfs_out {
                st: fuse_kstatfs {
                    blocks: st.blocks(),
                    bfree: st.bfree(),
                    bavail: st.bavail(),
                    files: st.files(),
                    ffree: st.ffree(),
                    bsize: st.bsize(),
                    namelen: st.namelen(),
                    frsize: st.frsize(),
                    padding: 0,
                    spare: [0; 6],
                },
            },
        }
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
    pub const fn new(size: u32) -> Self {
        Self {
            out: fuse_getxattr_out { size, padding: 0 },
        }
    }
}

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
    pub fn new(lk: impl FileLock) -> Self {
        Self {
            out: fuse_lk_out {
                lk: fuse_file_lock {
                    start: lk.start(),
                    end: lk.end(),
                    typ: lk.typ(),
                    pid: lk.pid(),
                },
            },
        }
    }
}

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
    pub const fn new(block: u64) -> Self {
        Self {
            out: fuse_bmap_out { block },
        }
    }
}

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
    pub const fn new(revents: u32) -> Self {
        Self {
            out: fuse_poll_out {
                revents,
                padding: 0,
            },
        }
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

    pub fn entry(&mut self, name: &OsStr, ino: NodeID, typ: u32, off: u64) -> bool {
        let name = name.as_bytes();
        let remaining = self.buf.capacity() - self.buf.len();

        let entry_size = mem::size_of::<fuse_dirent>() + name.len();
        let aligned_entry_size = aligned(entry_size);

        if remaining < aligned_entry_size {
            return true;
        }

        let dirent = fuse_dirent {
            ino: ino.into_raw(),
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
