use crate::types::{FileAttr, FileID, FileLock, NodeID, PollEvents, Statfs};
use bitflags::bitflags;
use polyfuse_kernel::*;
use std::{fmt, time::Duration};
use zerocopy::{Immutable, IntoBytes, KnownLayout};

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

#[derive(Default, IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct EntryOut {
    raw: fuse_entry_out,
}

impl fmt::Debug for EntryOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("EntryOut").finish()
    }
}

impl EntryOut {
    /// Return the object to fill attribute values about this entry.
    #[inline]
    pub fn attr(&mut self, attr: FileAttr) {
        fill_fuse_attr(&mut self.raw.attr, &attr);
    }

    /// Set the inode number of this entry.
    ///
    /// If this value is zero, it means that the entry is *negative*.
    /// Returning a negative entry is also possible with the `ENOENT` error,
    /// but the *zeroed* entries also have the ability to specify the lifetime
    /// of the entry cache by using the `ttl_entry` parameter.
    #[inline]
    pub fn ino(&mut self, ino: NodeID) {
        self.raw.nodeid = ino.into_raw();
    }

    /// Set the generation of this entry.
    ///
    /// This parameter is used to distinguish the inode from the past one
    /// when the filesystem reuse inode numbers.  That is, the operations
    /// must ensure that the pair of entry's inode number and generation
    /// are unique for the lifetime of the filesystem.
    pub fn generation(&mut self, generation: u64) {
        self.raw.generation = generation;
    }

    /// Set the validity timeout for inode attributes.
    ///
    /// The operations should set this value to very large
    /// when the changes of inode attributes are caused
    /// only by FUSE requests.
    pub fn ttl_attr(&mut self, ttl: Duration) {
        self.raw.attr_valid = ttl.as_secs();
        self.raw.attr_valid_nsec = ttl.subsec_nanos();
    }

    /// Set the validity timeout for the name.
    ///
    /// The operations should set this value to very large
    /// when the changes/deletions of directory entries are
    /// caused only by FUSE requests.
    pub fn ttl_entry(&mut self, ttl: Duration) {
        self.raw.entry_valid = ttl.as_secs();
        self.raw.entry_valid_nsec = ttl.subsec_nanos();
    }
}

#[derive(Default, IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct AttrOut {
    raw: fuse_attr_out,
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
        fill_fuse_attr(&mut self.raw.attr, &attr);
    }

    /// Set the validity timeout for this attribute.
    pub fn ttl(&mut self, ttl: Duration) {
        self.raw.attr_valid = ttl.as_secs();
        self.raw.attr_valid_nsec = ttl.subsec_nanos();
    }
}

#[derive(Default, IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct OpenOut {
    raw: fuse_open_out,
}

impl fmt::Debug for OpenOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("OpenOut").finish()
    }
}

impl OpenOut {
    /// Set the handle of opened file.
    pub fn fh(&mut self, fh: FileID) {
        self.raw.fh = fh.into_raw();
    }

    /// Specify the flags for the opened file.
    pub fn flags(&mut self, flags: OpenOutFlags) {
        self.raw.open_flags = flags.bits();
    }
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct OpenOutFlags: u32 {
        /// Indicates that the direct I/O is used on this file.
        const DIRECT_IO = FOPEN_DIRECT_IO;

        /// Indicates that the currently cached file data in the kernel
        /// need not be invalidated.
        const KEEP_CACHE = FOPEN_KEEP_CACHE;

        /// Indicates that the opened file is not seekable.
        const NONSEEKABLE = FOPEN_NONSEEKABLE;

        /// Enable caching of entries returned by `readdir`.
        ///
        /// This flag is meaningful only for `opendir` operations.
        const CACHE_DIR = FOPEN_CACHE_DIR;
    }
}

#[derive(Default, IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct WriteOut {
    raw: fuse_write_out,
}

impl fmt::Debug for WriteOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("WriteOut").finish()
    }
}

impl WriteOut {
    pub fn size(&mut self, size: u32) {
        self.raw.size = size;
    }
}

#[derive(Default, IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct StatfsOut {
    raw: fuse_statfs_out,
}

impl fmt::Debug for StatfsOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("StatfsOut").finish()
    }
}

impl StatfsOut {
    /// Return the object to fill the filesystem statistics.
    pub fn statfs(&mut self, st: Statfs) {
        self.raw.st = fuse_kstatfs {
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

#[derive(Default, IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct XattrOut {
    raw: fuse_getxattr_out,
}

impl fmt::Debug for XattrOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("XattrOut").finish()
    }
}

impl XattrOut {
    pub fn size(&mut self, size: u32) {
        self.raw.size = size;
    }
}

#[derive(Default, IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct LkOut {
    raw: fuse_lk_out,
}

impl fmt::Debug for LkOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("LkOut").finish()
    }
}

impl LkOut {
    pub fn file_lock(&mut self, lk: &FileLock) {
        self.raw.lk = fuse_file_lock {
            start: lk.start,
            end: lk.end,
            typ: lk.typ,
            pid: lk.pid.into_raw(),
        };
    }
}

#[derive(Default, IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct BmapOut {
    raw: fuse_bmap_out,
}

impl fmt::Debug for BmapOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("BmapOut").finish()
    }
}

impl BmapOut {
    pub fn block(&mut self, block: u64) {
        self.raw.block = block;
    }
}

#[derive(Default, IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct PollOut {
    raw: fuse_poll_out,
}

impl fmt::Debug for PollOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("PollOut").finish()
    }
}

impl PollOut {
    pub fn revents(&mut self, revents: PollEvents) {
        self.raw.revents = revents.bits();
    }
}

#[derive(Default, IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct LseekOut {
    raw: fuse_lseek_out,
}

impl fmt::Debug for LseekOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: add fields.
        f.debug_struct("LseekOut").finish()
    }
}

impl LseekOut {
    pub fn offset(&mut self, offset: u64) {
        self.raw.offset = offset;
    }
}
