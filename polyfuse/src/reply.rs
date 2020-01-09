//! Replies to the kernel.

#![allow(clippy::needless_update)]

use crate::{
    common::{FileAttr, FileLock, StatFs},
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
        FOPEN_CACHE_DIR,
        FOPEN_DIRECT_IO,
        FOPEN_KEEP_CACHE,
        FOPEN_NONSEEKABLE,
    },
};
use std::{
    ffi::{OsStr, OsString},
    fmt,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    time::Duration,
};

/// A trait that represents the data structure contained in the reply sent to the kernel.
///
/// The trait is roughly a generalization of `AsRef<[u8]>`, representing *scattered* bytes
/// that are not necessarily in contiguous memory space.
pub trait Reply {
    /// Collect the *scattered* bytes in the `collector`.
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>;
}

// ==== pointer types ====

macro_rules! impl_reply_body_for_pointers {
    () => {
        #[inline]
        fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
        where
            T: Collector<'a>,
        {
            (**self).collect_bytes(collector)
        }
    }
}

impl<R: ?Sized> Reply for &R
where
    R: Reply,
{
    impl_reply_body_for_pointers!();
}

impl<R: ?Sized> Reply for &mut R
where
    R: Reply,
{
    impl_reply_body_for_pointers!();
}

impl<R: ?Sized> Reply for Box<R>
where
    R: Reply,
{
    impl_reply_body_for_pointers!();
}

impl<R: ?Sized> Reply for std::rc::Rc<R>
where
    R: Reply,
{
    impl_reply_body_for_pointers!();
}

impl<R: ?Sized> Reply for std::sync::Arc<R>
where
    R: Reply,
{
    impl_reply_body_for_pointers!();
}

// ==== empty bytes ====

impl Reply for () {
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, _: &mut T)
    where
        T: Collector<'a>,
    {
    }
}

impl Reply for [u8; 0] {
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, _: &mut T)
    where
        T: Collector<'a>,
    {
    }
}

// ==== compound types ====

macro_rules! impl_reply_for_tuple {
    ($($T:ident),+ $(,)?) => {
        impl<$($T),+> Reply for ($($T,)+)
        where
            $( $T: Reply, )+
        {
            #[allow(nonstandard_style)]
            #[inline]
            fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
            where
                T: Collector<'a>,
            {
                let ($($T,)+) = self;
                $(
                    $T.collect_bytes(collector);
                )+
            }
        }
    }
}

impl_reply_for_tuple!(T1);
impl_reply_for_tuple!(T1, T2);
impl_reply_for_tuple!(T1, T2, T3);
impl_reply_for_tuple!(T1, T2, T3, T4);
impl_reply_for_tuple!(T1, T2, T3, T4, T5);

impl<R> Reply for [R]
where
    R: Reply,
{
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        for t in self {
            t.collect_bytes(collector);
        }
    }
}

impl<R> Reply for Vec<R>
where
    R: Reply,
{
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        for t in self {
            t.collect_bytes(collector);
        }
    }
}

// ==== Option<T> ====

impl<T> Reply for Option<T>
where
    T: Reply,
{
    #[inline]
    fn collect_bytes<'a, C: ?Sized>(&'a self, collector: &mut C)
    where
        C: Collector<'a>,
    {
        if let Some(ref reply) = self {
            reply.collect_bytes(collector);
        }
    }
}

// ==== continuous bytes ====

mod impl_scattered_bytes_for_cont {
    use super::*;

    #[inline(always)]
    fn as_bytes(t: &(impl AsRef<[u8]> + ?Sized)) -> &[u8] {
        t.as_ref()
    }

    macro_rules! impl_reply {
        ($($t:ty),*$(,)?) => {$(
            impl Reply for $t {
                #[inline]
                fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
                where
                    T: Collector<'a>,
                {
                    let this = as_bytes(self);
                    if !this.is_empty() {
                        collector.append(this);
                    }
                }
            }
        )*};
    }

    impl_reply! {
        [u8],
        str,
        String,
        Vec<u8>,
        std::borrow::Cow<'_, [u8]>,
    }
}

impl Reply for OsStr {
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        self.as_bytes().collect_bytes(collector)
    }
}

impl Reply for OsString {
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        (**self).collect_bytes(collector)
    }
}

impl Reply for Path {
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        self.as_os_str().collect_bytes(collector)
    }
}

impl Reply for PathBuf {
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        (**self).collect_bytes(collector)
    }
}

/// Container for collecting the scattered bytes.
pub trait Collector<'a> {
    /// Append a chunk of bytes into itself.
    fn append(&mut self, buf: &'a [u8]);
}

macro_rules! impl_reply {
    ($t:ty) => {
        impl Reply for $t {
            #[inline]
            fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
            where
                T: Collector<'a>,
            {
                collector.append(unsafe { crate::util::as_bytes(self) })
            }
        }
    };
}

/// Reply with the inode attributes.
#[must_use]
pub struct ReplyAttr(fuse_attr_out);

impl_reply!(ReplyAttr);

impl AsRef<Self> for ReplyAttr {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl fmt::Debug for ReplyAttr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplyAttr")
            .field("attr", unsafe {
                &*(&self.0.attr as *const _ as *const FileAttr)
            })
            .field(
                "ttl",
                &Duration::new(self.0.attr_valid, self.0.attr_valid_nsec),
            )
            .finish()
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
#[must_use]
pub struct ReplyEntry(fuse_entry_out);

impl_reply!(ReplyEntry);

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

impl fmt::Debug for ReplyEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplyEntry")
            .field("nodeid", &self.0.nodeid)
            .field("generation", &self.0.generation)
            .field("attr", unsafe {
                &*(&self.0.attr as *const _ as *const FileAttr)
            })
            .field(
                "ttl_attr",
                &Duration::new(self.0.attr_valid, self.0.attr_valid_nsec),
            )
            .field(
                "ttl_entry",
                &Duration::new(self.0.entry_valid, self.0.entry_valid_nsec),
            )
            .finish()
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
#[must_use]
pub struct ReplyOpen(fuse_open_out);

impl_reply!(ReplyOpen);

impl AsRef<Self> for ReplyOpen {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl fmt::Debug for ReplyOpen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplyOpen")
            .field("fh", &self.0.fh)
            .field("direct_io", &self.get_flag(FOPEN_DIRECT_IO))
            .field("keep_cache", &self.get_flag(FOPEN_KEEP_CACHE))
            .field("nonseekable", &self.get_flag(FOPEN_NONSEEKABLE))
            .field("cache_dir", &self.get_flag(FOPEN_CACHE_DIR))
            .finish()
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

    fn get_flag(&self, flag: u32) -> bool {
        self.0.open_flags & flag != 0
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

/// Reply with the information about written data.
#[must_use]
pub struct ReplyWrite(fuse_write_out);

impl_reply!(ReplyWrite);

impl AsRef<Self> for ReplyWrite {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl fmt::Debug for ReplyWrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplyWrite")
            .field("size", &self.0.size)
            .finish()
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
#[must_use]
pub struct ReplyXattr(fuse_getxattr_out);

impl_reply!(ReplyXattr);

impl AsRef<Self> for ReplyXattr {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl fmt::Debug for ReplyXattr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplyXattr")
            .field("size", &self.0.size)
            .finish()
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
#[must_use]
pub struct ReplyStatfs(fuse_statfs_out);

impl_reply!(ReplyStatfs);

impl AsRef<Self> for ReplyStatfs {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl fmt::Debug for ReplyStatfs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplyStatfs")
            .field("st", &self.get_stat())
            .finish()
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

    fn get_stat(&self) -> &StatFs {
        unsafe { &*(&self.0.st as *const _ as *const StatFs) }
    }
}

/// Reply with a file lock.
#[must_use]
pub struct ReplyLk(fuse_lk_out);

impl_reply!(ReplyLk);

impl AsRef<Self> for ReplyLk {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl fmt::Debug for ReplyLk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplyLk") //
            .field("lk", &self.get_lock())
            .finish()
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

    fn get_lock(&self) -> &FileLock {
        unsafe { &*(&self.0.lk as *const _ as *const FileLock) }
    }
}

/// Reply with the mapped block index.
#[must_use]
pub struct ReplyBmap(fuse_bmap_out);

impl_reply!(ReplyBmap);

impl AsRef<Self> for ReplyBmap {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl fmt::Debug for ReplyBmap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplyBmap") //
            .field("block", &self.0.block)
            .finish()
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
#[must_use]
pub struct ReplyPoll(fuse_poll_out);

impl_reply!(ReplyPoll);

impl AsRef<Self> for ReplyPoll {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl fmt::Debug for ReplyPoll {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplyPoll") //
            .field("revents", &self.0.revents)
            .finish()
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
