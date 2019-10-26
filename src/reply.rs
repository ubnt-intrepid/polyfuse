//! Replies to the kernel.

use crate::fs::Context;
use polyfuse_abi::{
    AttrOut, //
    BmapOut,
    EntryOut,
    FileAttr,
    FileLock,
    GetxattrOut,
    LkOut,
    OpenFlags,
    OpenOut,
    Statfs,
    StatfsOut,
    WriteOut,
};
use std::{
    convert::TryInto,
    ffi::OsStr,
    io::{self},
    os::unix::ffi::OsStrExt,
};

pub use crate::dir::DirEntry;

/// Reply with an empty output.
#[derive(Debug, Default)]
#[must_use]
pub struct ReplyEmpty {
    _p: (),
}

impl ReplyEmpty {
    pub async fn ok(self, cx: &mut Context<'_>) -> io::Result<()> {
        cx.send_reply(0, &[]).await
    }
}

/// Reply with arbitrary binary data.
#[derive(Debug)]
#[must_use]
pub struct ReplyData {
    size: u32,
}

impl ReplyData {
    pub(crate) fn new(size: u32) -> Self {
        Self { size }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    /// Reply to the kernel with a data.
    pub async fn data(self, cx: &mut Context<'_>, data: impl AsRef<[u8]>) -> io::Result<()> {
        self.data_vectored(cx, &[data.as_ref()]).await
    }

    /// Reply to the kernel with a *split* data.
    #[allow(clippy::cast_possible_truncation)]
    pub async fn data_vectored(self, cx: &mut Context<'_>, data: &[&[u8]]) -> io::Result<()> {
        let len: u32 = data.iter().map(|t| t.len() as u32).sum();
        if len <= self.size {
            cx.send_reply(0, data).await
        } else {
            cx.reply_err(libc::ERANGE).await
        }
    }

    // TODO: async fn reader(self, impl AsyncRead) -> io::Result<()>
}

/// Reply with the inode attributes.
#[derive(Debug, Default)]
#[must_use]
pub struct ReplyAttr {
    attr_valid: (u64, u32),
}

impl ReplyAttr {
    /// Set the validity timeout for attributes.
    pub fn attr_valid(&mut self, secs: u64, nsecs: u32) {
        self.attr_valid = (secs, nsecs);
    }

    /// Reply to the kernel with the specified attributes.
    pub async fn attr<T>(self, cx: &mut Context<'_>, attr: T) -> io::Result<()>
    where
        T: TryInto<FileAttr>,
        T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let attr_out = AttrOut {
            attr: attr
                .try_into()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?,
            attr_valid: self.attr_valid.0,
            ..Default::default()
        };
        cx.send_reply(0, &[attr_out.as_ref()]).await
    }
}

/// Reply with entry params.
#[derive(Debug, Default)]
#[must_use]
pub struct ReplyEntry {
    entry_valid: (u64, u32),
    attr_valid: (u64, u32),
}

impl ReplyEntry {
    /// Set the validity timeout for inode attributes.
    ///
    /// The operations should set this value to very large
    /// when the changes of inode attributes are caused
    /// only by FUSE requests.
    pub fn attr_valid(&mut self, sec: u64, nsec: u32) {
        self.attr_valid = (sec, nsec);
    }

    /// Set the validity timeout for the name.
    ///
    /// The operations should set this value to very large
    /// when the changes/deletions of directory entries are
    /// caused only by FUSE requests.
    pub fn entry_valid(&mut self, sec: u64, nsec: u32) {
        self.entry_valid = (sec, nsec);
    }

    /// Reply to the kernel with the specified entry parameters.
    ///
    /// The parameter `generation` is used to distinguish the inode
    /// from the past one when the filesystem reuse inode numbers.
    /// That is, the operations must ensure that the pair of
    /// entry's inode number and `generation` is unique for
    /// the lifetime of filesystem.
    pub async fn entry<T>(self, cx: &mut Context<'_>, attr: T, generation: u64) -> io::Result<()>
    where
        T: TryInto<FileAttr>,
        T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let attr = attr
            .try_into()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let entry_out = EntryOut {
            nodeid: attr.ino,
            generation,
            entry_valid: self.entry_valid.0,
            entry_valid_nsec: self.entry_valid.1,
            attr_valid: self.attr_valid.0,
            attr_valid_nsec: self.attr_valid.1,
            attr,
            ..Default::default()
        };
        cx.send_reply(0, &[entry_out.as_ref()]).await
    }
}

/// Reply with the read link value.
#[derive(Debug, Default)]
#[must_use]
pub struct ReplyReadlink {
    _p: (),
}

impl ReplyReadlink {
    /// Reply to the kernel with the specified link value.
    pub async fn link(self, cx: &mut Context<'_>, value: impl AsRef<OsStr>) -> io::Result<()> {
        cx.send_reply(0, &[value.as_ref().as_bytes()]).await
    }
}

/// Reply with an opened file.
#[derive(Debug)]
#[must_use]
pub struct ReplyOpen {
    open_flags: OpenFlags,
}

impl Default for ReplyOpen {
    fn default() -> Self {
        Self {
            open_flags: OpenFlags::empty(),
        }
    }
}

impl ReplyOpen {
    fn set_flag(&mut self, flag: OpenFlags, enabled: bool) {
        if enabled {
            self.open_flags.insert(flag);
        } else {
            self.open_flags.remove(flag);
        }
    }

    /// Indicates that the direct I/O is used on this file.
    pub fn direct_io(&mut self, enabled: bool) {
        self.set_flag(OpenFlags::DIRECT_IO, enabled);
    }

    /// Indicates that the currently cached file data in the kernel
    /// need not be invalidated.
    pub fn keep_cache(&mut self, enabled: bool) {
        self.set_flag(OpenFlags::KEEP_CACHE, enabled);
    }

    /// Indicates that the opened file is not seekable.
    pub fn nonseekable(&mut self, enabled: bool) {
        self.set_flag(OpenFlags::NONSEEKABLE, enabled);
    }

    /// Reply to the kernel with the specified file handle and flags.
    pub async fn open(self, cx: &mut Context<'_>, fh: u64) -> io::Result<()> {
        let out = OpenOut {
            fh,
            open_flags: self.open_flags,
            ..Default::default()
        };
        cx.send_reply(0, &[out.as_ref()]).await
    }
}

/// Reply with the information about written data.
#[derive(Debug, Default)]
#[must_use]
pub struct ReplyWrite {
    _p: (),
}

impl ReplyWrite {
    /// Reply to the kernel with the total length of written data.
    pub async fn write(self, cx: &mut Context<'_>, size: u32) -> io::Result<()> {
        let out = WriteOut {
            size,
            ..Default::default()
        };
        cx.send_reply(0, &[out.as_ref()]).await
    }
}

/// Reply with an opened directory.
#[derive(Debug)]
#[must_use]
pub struct ReplyOpendir {
    open_flags: OpenFlags,
}

impl Default for ReplyOpendir {
    fn default() -> Self {
        Self {
            open_flags: OpenFlags::empty(),
        }
    }
}

impl ReplyOpendir {
    fn set_flag(&mut self, flag: OpenFlags, enabled: bool) {
        if enabled {
            self.open_flags.insert(flag);
        } else {
            self.open_flags.remove(flag);
        }
    }

    // MEMO: should we add direct_io()?

    /// Enable caching of entries returned by `readdir`.
    pub fn cache_dir(&mut self, enabled: bool) {
        self.set_flag(OpenFlags::CACHE_DIR, enabled);
    }

    /// Reply to the kernel with the specified file handle and flags.
    pub async fn open(self, cx: &mut Context<'_>, fh: u64) -> io::Result<()> {
        let out = OpenOut {
            fh,
            open_flags: self.open_flags,
            ..Default::default()
        };
        cx.send_reply(0, &[out.as_ref()]).await
    }
}

/// Reply to a request about extended attributes.
#[derive(Debug, Default)]
#[must_use]
pub struct ReplyXattr {
    _p: (),
}

impl ReplyXattr {
    /// Reply to the kernel with the specified size value.
    pub async fn size(self, cx: &mut Context<'_>, size: u32) -> io::Result<()> {
        let out = GetxattrOut {
            size,
            ..Default::default()
        };
        cx.send_reply(0, &[out.as_ref()]).await
    }

    /// Reply to the kernel with the specified value.
    pub async fn value(self, cx: &mut Context<'_>, value: impl AsRef<[u8]>) -> io::Result<()> {
        cx.send_reply(0, &[value.as_ref()]).await
    }
}

/// Reply with the filesystem staticstics.
#[derive(Debug, Default)]
#[must_use]
pub struct ReplyStatfs {
    _p: (),
}

impl ReplyStatfs {
    /// Reply to the kernel with the specified statistics.
    pub async fn stat<T>(self, cx: &mut Context<'_>, st: T) -> io::Result<()>
    where
        T: TryInto<Statfs>,
        T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let out = StatfsOut {
            st: st
                .try_into()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?,
            ..Default::default()
        };
        cx.send_reply(0, &[out.as_ref()]).await
    }
}

/// Reply with a file lock.
#[derive(Debug, Default)]
#[must_use]
pub struct ReplyLk {
    _p: (),
}

impl ReplyLk {
    /// Reply to the kernel with the specified file lock.
    pub async fn lock<T>(self, cx: &mut Context<'_>, lk: T) -> io::Result<()>
    where
        T: TryInto<FileLock>,
        T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let out = LkOut {
            lk: lk
                .try_into()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?,
            ..Default::default()
        };
        cx.send_reply(0, &[out.as_ref()]).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyCreate {
    entry_valid: (u64, u32),
    attr_valid: (u64, u32),
    open_flags: OpenFlags,
}

impl Default for ReplyCreate {
    fn default() -> Self {
        Self {
            entry_valid: (0, 0),
            attr_valid: (0, 0),
            open_flags: OpenFlags::empty(),
        }
    }
}

impl ReplyCreate {
    /// Set the validity timeout for inode attributes.
    ///
    /// The operations should set this value to very large
    /// when the changes of inode attributes are caused
    /// only by FUSE requests.
    pub fn attr_valid(&mut self, sec: u64, nsec: u32) {
        self.attr_valid = (sec, nsec);
    }

    /// Set the validity timeout for the name.
    ///
    /// The operations should set this value to very large
    /// when the changes/deletions of directory entries are
    /// caused only by FUSE requests.
    pub fn entry_valid(&mut self, sec: u64, nsec: u32) {
        self.entry_valid = (sec, nsec);
    }

    fn set_flag(&mut self, flag: OpenFlags, enabled: bool) {
        if enabled {
            self.open_flags.insert(flag);
        } else {
            self.open_flags.remove(flag);
        }
    }

    /// Indicates that the direct I/O is used on this file.
    pub fn direct_io(&mut self, enabled: bool) {
        self.set_flag(OpenFlags::DIRECT_IO, enabled);
    }

    /// Indicates that the currently cached file data in the kernel
    /// need not be invalidated.
    pub fn keep_cache(&mut self, enabled: bool) {
        self.set_flag(OpenFlags::KEEP_CACHE, enabled);
    }

    /// Indicates that the opened file is not seekable.
    pub fn nonseekable(&mut self, enabled: bool) {
        self.set_flag(OpenFlags::NONSEEKABLE, enabled);
    }

    /// Reply to the kernel with the specified entry parameters and file handle.
    pub async fn create<T>(
        self,
        cx: &mut Context<'_>,
        attr: T,
        generation: u64,
        fh: u64,
    ) -> io::Result<()>
    where
        T: TryInto<FileAttr>,
        T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let attr = attr
            .try_into()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let entry_out = EntryOut {
            nodeid: attr.ino,
            generation,
            entry_valid: self.entry_valid.0,
            entry_valid_nsec: self.entry_valid.1,
            attr_valid: self.attr_valid.0,
            attr_valid_nsec: self.attr_valid.1,
            attr,
            ..Default::default()
        };

        let open_out = OpenOut {
            fh,
            open_flags: self.open_flags,
            ..Default::default()
        };

        cx.send_reply(0, &[entry_out.as_ref(), open_out.as_ref()])
            .await
    }
}

#[derive(Debug, Default)]
#[must_use]
pub struct ReplyBmap {
    _p: (),
}

impl ReplyBmap {
    pub async fn block(self, cx: &mut Context<'_>, block: u64) -> io::Result<()> {
        let out = BmapOut {
            block,
            ..Default::default()
        };
        cx.send_reply(0, &[out.as_ref()]).await
    }
}
