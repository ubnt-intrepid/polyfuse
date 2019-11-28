//! Replies to the kernel.

#![allow(clippy::needless_update)]

use crate::common::{FileAttr, FileLock, StatFs};
use futures::{future::poll_fn, io::AsyncWrite};
use polyfuse_sys::kernel::{
    fuse_attr_out, //
    fuse_bmap_out,
    fuse_entry_out,
    fuse_getxattr_out,
    fuse_lk_out,
    fuse_open_out,
    fuse_out_header,
    fuse_poll_out,
    fuse_statfs_out,
    fuse_write_out,
};
use smallvec::SmallVec;
use std::{
    convert::TryFrom,
    io::{self, IoSlice},
    mem,
    pin::Pin,
};

#[inline(always)]
pub(crate) unsafe fn as_bytes<T: Sized>(t: &T) -> &[u8] {
    std::slice::from_raw_parts(t as *const T as *const u8, std::mem::size_of::<T>())
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
    pub fn attr(&mut self, attr: FileAttr) {
        self.0.attr = attr.into_inner();
    }

    /// Set the validity timeout for attributes.
    pub fn attr_valid(&mut self, secs: u64, nsecs: u32) {
        self.0.attr_valid = secs;
        self.0.attr_valid_nsec = nsecs;
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

impl ReplyEntry {
    /// Create a new `ReplyEntry`.
    pub fn new(attr: FileAttr) -> Self {
        Self(fuse_entry_out {
            attr: attr.into_inner(),
            ..Default::default()
        })
    }

    /// Set the attribute value of this entry.
    pub fn attr(&mut self, attr: FileAttr) {
        self.0.attr = attr.into_inner();
    }

    /// Set the validity timeout for inode attributes.
    ///
    /// The operations should set this value to very large
    /// when the changes of inode attributes are caused
    /// only by FUSE requests.
    pub fn attr_valid(&mut self, sec: u64, nsec: u32) {
        self.0.attr_valid = sec;
        self.0.attr_valid_nsec = nsec;
    }

    /// Set the validity timeout for the name.
    ///
    /// The operations should set this value to very large
    /// when the changes/deletions of directory entries are
    /// caused only by FUSE requests.
    pub fn entry_valid(&mut self, sec: u64, nsec: u32) {
        self.0.entry_valid = sec;
        self.0.entry_valid_nsec = nsec;
    }

    /// Sets the generation of this entry.
    ///
    /// The parameter `generation` is used to distinguish the inode
    /// from the past one when the filesystem reuse inode numbers.
    /// That is, the operations must ensure that the pair of
    /// entry's inode number and `generation` is unique for
    /// the lifetime of filesystem.
    pub fn generation(&mut self, generation: u64) {
        self.0.generation = generation;
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
    pub fn fh(&mut self, fh: u64) {
        self.0.fh = fh;
    }

    /// Indicates that the direct I/O is used on this file.
    pub fn direct_io(&mut self, enabled: bool) {
        self.set_flag(polyfuse_sys::kernel::FOPEN_DIRECT_IO, enabled);
    }

    /// Indicates that the currently cached file data in the kernel
    /// need not be invalidated.
    pub fn keep_cache(&mut self, enabled: bool) {
        self.set_flag(polyfuse_sys::kernel::FOPEN_KEEP_CACHE, enabled);
    }

    /// Indicates that the opened file is not seekable.
    pub fn nonseekable(&mut self, enabled: bool) {
        self.set_flag(polyfuse_sys::kernel::FOPEN_NONSEEKABLE, enabled);
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
    pub fn size(&mut self, size: u32) {
        self.0.size = size;
    }
}

/// Reply with an opened directory.
#[derive(Debug)]
#[must_use]
pub struct ReplyOpendir(fuse_open_out);

impl AsRef<Self> for ReplyOpendir {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl ReplyOpendir {
    /// Create a new `ReplyOpendir`
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
    pub fn fh(&mut self, fh: u64) {
        self.0.fh = fh;
    }

    // MEMO: should we add direct_io()?

    /// Enable caching of entries returned by `readdir`.
    pub fn cache_dir(&mut self, enabled: bool) {
        self.set_flag(polyfuse_sys::kernel::FOPEN_CACHE_DIR, enabled);
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
    pub fn size(&mut self, size: u32) {
        self.0.size = size;
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
    pub fn stat(&mut self, st: StatFs) {
        self.0.st = st.into_inner();
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
    pub fn lock(&mut self, lk: FileLock) {
        self.0.lk = lk.into_inner();
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
    pub fn block(&mut self, block: u64) {
        self.0.block = block;
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
    pub fn revents(&mut self, revents: u32) {
        self.0.revents = revents;
    }
}

pub(crate) async fn send_msg<W: ?Sized>(
    writer: &mut W,
    unique: u64,
    error: i32,
    data: &[&[u8]],
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let data_len: usize = data.iter().map(|t| t.len()).sum();
    let len = u32::try_from(mem::size_of::<fuse_out_header>() + data_len) //
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "the total length of data is too long: {}",
            )
        })?;

    let out_header = fuse_out_header { unique, error, len };

    // Unfortunately, IoSlice<'_> does not implement Send and
    // the data vector must be created in `poll` function.
    poll_fn(move |cx| {
        let vec: SmallVec<[_; 4]> = Some(IoSlice::new(unsafe { as_bytes(&out_header) }))
            .into_iter()
            .chain(data.iter().map(|t| IoSlice::new(&*t)))
            .collect();
        Pin::new(&mut *writer).poll_write_vectored(cx, &*vec)
    })
    .await?;

    tracing::debug!("Reply to kernel: unique={}: error={}", unique, error);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;

    #[inline]
    fn bytes(bytes: &[u8]) -> &[u8] {
        bytes
    }
    macro_rules! b {
        ($($b:expr),*$(,)?) => ( *bytes(&[$($b),*]) );
    }

    #[test]
    fn send_msg_empty() {
        let mut dest = Vec::<u8>::new();
        block_on(send_msg(&mut dest, 42, 4, &[])).unwrap();
        assert_eq!(dest[0..4], b![0x10, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(dest[4..8], b![0x04, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            dest[8..16],
            b![0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
    }

    #[test]
    fn send_msg_single_data() {
        let mut dest = Vec::<u8>::new();
        block_on(send_msg(&mut dest, 42, 0, &["hello".as_ref()])).unwrap();
        assert_eq!(dest[0..4], b![0x15, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(dest[4..8], b![0x00, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            dest[8..16],
            b![0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
        assert_eq!(dest[16..], b![0x68, 0x65, 0x6c, 0x6c, 0x6f], "payload");
    }

    #[test]
    fn send_msg_chunked_data() {
        let payload: &[&[u8]] = &[
            "hello, ".as_ref(), //
            "this ".as_ref(),
            "is a ".as_ref(),
            "message.".as_ref(),
        ];
        let mut dest = Vec::<u8>::new();
        block_on(send_msg(&mut dest, 26, 0, payload)).unwrap();
        assert_eq!(dest[0..4], b![0x29, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(dest[4..8], b![0x00, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            dest[8..16],
            b![0x1a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
        assert_eq!(dest[16..], *b"hello, this is a message.", "payload");
    }
}
