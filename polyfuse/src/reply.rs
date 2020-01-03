//! Replies to the kernel.

#![allow(clippy::needless_update)]

use crate::{
    common::{FileAttr, FileLock, StatFs},
    io::{Collector, ScatteredBytes},
    kernel::{
        fuse_attr_out, //
        fuse_bmap_out,
        fuse_entry_out,
        fuse_getxattr_out,
        fuse_lk_out,
        fuse_open_out,
        fuse_poll_out,
        fuse_statfs_out,
        fuse_write_out,
    },
};
use std::time::Duration;

macro_rules! impl_scattered_buf {
    ($($t:ty),*$(,)?) => {$(
        impl ScatteredBytes for $t {
            #[inline]
            fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
            where
                T: Collector<'a>,
            {
                collector.append(unsafe { crate::util::as_bytes(self) })
            }
        }
    )*};
}

impl_scattered_buf! {
    ReplyAttr,
    ReplyEntry,
    ReplyOpen,
    ReplyWrite,
    ReplyXattr,
    ReplyStatfs,
    ReplyBmap,
    ReplyLk,
    ReplyPoll,
}

/// Reply with the inode attributes.
#[derive(Debug)]
#[must_use]
pub struct ReplyAttr(fuse_attr_out);

impl AsRef<Self> for ReplyAttr {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl ReplyAttr {
    /// Create a new `ReplyAttr`.
    pub fn new(attr: FileAttr) -> Self {
        Self(fuse_attr_out {
            attr: attr.into_inner(),
            ..Default::default()
        })
    }

    /// Set the attribute value.
    pub fn attr(&mut self, attr: FileAttr) -> &mut Self {
        self.0.attr = attr.into_inner();
        self
    }

    /// Set the validity timeout for attributes.
    pub fn ttl_attr(&mut self, duration: Duration) -> &mut Self {
        self.0.attr_valid = duration.as_secs();
        self.0.attr_valid_nsec = duration.subsec_nanos();
        self
    }
}

/// Reply with entry params.
#[derive(Debug)]
#[must_use]
pub struct ReplyEntry(fuse_entry_out);

impl AsRef<Self> for ReplyEntry {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl Default for ReplyEntry {
    fn default() -> Self {
        Self(fuse_entry_out::default())
    }
}

impl ReplyEntry {
    #[doc(hidden)]
    #[deprecated(
        since = "0.3.1",
        note = "The assumption used here is incorrect. \
                See also https://github.com/ubnt-intrepid/polyfuse/issues/65."
    )]
    pub fn new(attr: FileAttr) -> Self {
        let attr = attr.into_inner();
        let nodeid = attr.ino;
        Self(fuse_entry_out {
            nodeid,
            attr,
            ..Default::default()
        })
    }

    /// Set the inode number of this entry.
    ///
    /// If this value is zero, it means that the entry is *negative*.
    /// Returning a negative entry is also possible with the `ENOENT` error,
    /// but the *zeroed* entries also have the ability to specify the lifetime
    /// of the entry cache by using the `ttl_entry` parameter.
    ///
    /// The default value is `0`.
    #[inline]
    pub fn ino(&mut self, ino: u64) -> &mut Self {
        self.0.nodeid = ino;
        self
    }

    /// Set the attribute value of this entry.
    pub fn attr(&mut self, attr: FileAttr) -> &mut Self {
        self.0.attr = attr.into_inner();
        self
    }

    /// Set the validity timeout for inode attributes.
    ///
    /// The operations should set this value to very large
    /// when the changes of inode attributes are caused
    /// only by FUSE requests.
    pub fn ttl_attr(&mut self, duration: Duration) -> &mut Self {
        self.0.attr_valid = duration.as_secs();
        self.0.attr_valid_nsec = duration.subsec_nanos();
        self
    }

    /// Set the validity timeout for the name.
    ///
    /// The operations should set this value to very large
    /// when the changes/deletions of directory entries are
    /// caused only by FUSE requests.
    pub fn ttl_entry(&mut self, duration: Duration) -> &mut Self {
        self.0.entry_valid = duration.as_secs();
        self.0.entry_valid_nsec = duration.subsec_nanos();
        self
    }

    /// Sets the generation of this entry.
    ///
    /// The parameter `generation` is used to distinguish the inode
    /// from the past one when the filesystem reuse inode numbers.
    /// That is, the operations must ensure that the pair of
    /// entry's inode number and `generation` are unique for
    /// the lifetime of the filesystem.
    pub fn generation(&mut self, generation: u64) -> &mut Self {
        self.0.generation = generation;
        self
    }
}

/// Reply with an opened file.
#[derive(Debug)]
#[must_use]
pub struct ReplyOpen(fuse_open_out);

impl AsRef<Self> for ReplyOpen {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl ReplyOpen {
    /// Create a new `ReplyOpen`.
    pub fn new(fh: u64) -> Self {
        Self(fuse_open_out {
            fh,
            ..Default::default()
        })
    }

    fn set_flag(&mut self, flag: u32, enabled: bool) {
        if enabled {
            self.0.open_flags |= flag;
        } else {
            self.0.open_flags &= !flag;
        }
    }

    /// Set the file handle.
    pub fn fh(&mut self, fh: u64) -> &mut Self {
        self.0.fh = fh;
        self
    }

    /// Indicates that the direct I/O is used on this file.
    pub fn direct_io(&mut self, enabled: bool) -> &mut Self {
        self.set_flag(crate::kernel::FOPEN_DIRECT_IO, enabled);
        self
    }

    /// Indicates that the currently cached file data in the kernel
    /// need not be invalidated.
    pub fn keep_cache(&mut self, enabled: bool) -> &mut Self {
        self.set_flag(crate::kernel::FOPEN_KEEP_CACHE, enabled);
        self
    }

    /// Indicates that the opened file is not seekable.
    pub fn nonseekable(&mut self, enabled: bool) -> &mut Self {
        self.set_flag(crate::kernel::FOPEN_NONSEEKABLE, enabled);
        self
    }

    /// Enable caching of entries returned by `readdir`.
    ///
    /// This flag is meaningful only for `opendir` operations.
    pub fn cache_dir(&mut self, enabled: bool) -> &mut Self {
        self.set_flag(crate::kernel::FOPEN_CACHE_DIR, enabled);
        self
    }
}

/// Reply with the information about written data.
#[derive(Debug)]
#[must_use]
pub struct ReplyWrite(fuse_write_out);

impl AsRef<Self> for ReplyWrite {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl ReplyWrite {
    /// Create a new `ReplyWrite`.
    pub fn new(size: u32) -> Self {
        Self(fuse_write_out {
            size,
            ..Default::default()
        })
    }

    /// Set the size of written bytes.
    pub fn size(&mut self, size: u32) -> &mut Self {
        self.0.size = size;
        self
    }
}

/// Reply to a request about extended attributes.
#[derive(Debug)]
#[must_use]
pub struct ReplyXattr(fuse_getxattr_out);

impl AsRef<Self> for ReplyXattr {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl ReplyXattr {
    /// Create a new `ReplyXattr`.
    pub fn new(size: u32) -> Self {
        Self(fuse_getxattr_out {
            size,
            ..Default::default()
        })
    }

    /// Set the actual size of attribute value.
    pub fn size(&mut self, size: u32) -> &mut Self {
        self.0.size = size;
        self
    }
}

/// Reply with the filesystem staticstics.
#[derive(Debug)]
#[must_use]
pub struct ReplyStatfs(fuse_statfs_out);

impl AsRef<Self> for ReplyStatfs {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl ReplyStatfs {
    /// Create a new `ReplyStatfs`.
    pub fn new(st: StatFs) -> Self {
        Self(fuse_statfs_out {
            st: st.into_inner(),
            ..Default::default()
        })
    }

    /// Set the value of filesystem statistics.
    pub fn stat(&mut self, st: StatFs) -> &mut Self {
        self.0.st = st.into_inner();
        self
    }
}

/// Reply with a file lock.
#[derive(Debug)]
#[must_use]
pub struct ReplyLk(fuse_lk_out);

impl AsRef<Self> for ReplyLk {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl ReplyLk {
    /// Create a new `ReplyLk`.
    pub fn new(lk: FileLock) -> Self {
        Self(fuse_lk_out {
            lk: lk.into_inner(),
            ..Default::default()
        })
    }

    /// Set the lock information.
    pub fn lock(&mut self, lk: FileLock) -> &mut Self {
        self.0.lk = lk.into_inner();
        self
    }
}

/// Reply with the mapped block index.
#[derive(Debug)]
#[must_use]
pub struct ReplyBmap(fuse_bmap_out);

impl AsRef<Self> for ReplyBmap {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl ReplyBmap {
    /// Create a new `ReplyBmap`.
    pub fn new(block: u64) -> Self {
        Self(fuse_bmap_out {
            block,
            ..Default::default()
        })
    }

    /// Set the index of mapped block.
    pub fn block(&mut self, block: u64) -> &mut Self {
        self.0.block = block;
        self
    }
}

/// Reply with the poll result.
#[derive(Debug)]
#[must_use]
pub struct ReplyPoll(fuse_poll_out);

impl AsRef<Self> for ReplyPoll {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl ReplyPoll {
    /// Create a new `ReplyPoll`.
    pub fn new(revents: u32) -> Self {
        Self(fuse_poll_out {
            revents,
            ..Default::default()
        })
    }

    /// Set the mask of ready events.
    pub fn revents(&mut self, revents: u32) -> &mut Self {
        self.0.revents = revents;
        self
    }
}
