//! Replies to the kernel.

use crate::abi::{
    AttrOut, //
    BmapOut,
    EntryOut,
    FileAttr,
    FileLock,
    GetxattrOut,
    LkOut,
    OpenFlags,
    OpenOut,
    OutHeader,
    Statfs,
    StatfsOut,
    Unique,
    WriteOut,
};
use futures_io::AsyncWrite;
use futures_util::io::AsyncWriteExt;
use smallvec::SmallVec;
use std::{
    convert::{TryFrom, TryInto},
    ffi::OsStr,
    fmt,
    io::{self, IoSlice},
    mem,
    os::unix::ffi::OsStrExt,
    pin::Pin,
};

/// A base object to send a reply to the kernel.
pub struct ReplyRaw<'a> {
    unique: Unique,
    writer: Pin<Box<dyn AsyncWrite + Unpin + 'a>>,
}

impl fmt::Debug for ReplyRaw<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ReplyRaw")
            .field("unique", &self.unique)
            .finish()
    }
}

impl<'a> ReplyRaw<'a> {
    pub(crate) fn new(unique: Unique, writer: impl AsyncWrite + Unpin + 'a) -> Self {
        Self {
            unique,
            writer: Box::pin(writer),
        }
    }

    /// Repy the specified data to the kernel.
    pub async fn reply(self, error: i32, data: &[&[u8]]) -> io::Result<()> {
        let mut this = self;

        let data_len: usize = data.iter().map(|t| t.len()).sum();

        let out_header = OutHeader {
            unique: this.unique,
            error: -error,
            len: u32::try_from(mem::size_of::<OutHeader>() + data_len).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("the total length of data is too long: {}", e),
                )
            })?,
        };

        let vec: SmallVec<[_; 4]> = Some(IoSlice::new(out_header.as_ref()))
            .into_iter()
            .chain(data.iter().map(|t| IoSlice::new(&*t)))
            .collect();

        (*this.writer).write_vectored(&*vec).await?;

        Ok(())
    }

    /// Reply an error code to the kernel.
    pub async fn reply_err(self, error: i32) -> io::Result<()> {
        self.reply(error, &[]).await
    }
}

/// Reply with an empty output.
#[derive(Debug)]
#[must_use]
pub struct ReplyEmpty<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyEmpty<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl ReplyEmpty<'_> {
    /// Reply to the kernel.
    pub async fn ok(self) -> io::Result<()> {
        self.raw.reply(0, &[]).await
    }

    /// Reply to the kernel with the specified error number.
    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

/// Reply with arbitrary binary data.
#[derive(Debug)]
#[must_use]
pub struct ReplyData<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyData<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl<'a> ReplyData<'a> {
    /// Reply to the kernel with a data.
    pub async fn data(self, data: impl AsRef<[u8]>) -> io::Result<()> {
        self.data_vectored(&[data.as_ref()]).await
    }

    /// Reply to the kernel with a *split* data.
    pub async fn data_vectored(self, data: &[&[u8]]) -> io::Result<()> {
        self.raw.reply(0, data).await
    }

    // TODO: async fn reader(self, impl AsyncRead) -> io::Result<()>

    /// Reply to the kernel with the specified error number.
    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

/// Reply with the inode attributes.
#[derive(Debug)]
#[must_use]
pub struct ReplyAttr<'a> {
    raw: ReplyRaw<'a>,
    attr_valid: (u64, u32),
}

impl<'a> From<ReplyRaw<'a>> for ReplyAttr<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self {
            raw,
            attr_valid: (0, 0),
        }
    }
}

impl ReplyAttr<'_> {
    /// Set the validity timeout for attributes.
    pub fn attr_valid(&mut self, secs: u64, nsecs: u32) {
        self.attr_valid = (secs, nsecs);
    }

    /// Reply to the kernel with the specified attributes.
    pub async fn attr<T>(self, attr: T) -> io::Result<()>
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
        self.raw.reply(0, &[attr_out.as_ref()]).await
    }

    /// Reply to the kernel with the specified error number.
    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

/// Reply with entry params.
#[derive(Debug)]
#[must_use]
pub struct ReplyEntry<'a> {
    raw: ReplyRaw<'a>,
    entry_valid: (u64, u32),
    attr_valid: (u64, u32),
}

impl<'a> From<ReplyRaw<'a>> for ReplyEntry<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self {
            raw,
            entry_valid: (0, 0),
            attr_valid: (0, 0),
        }
    }
}

impl ReplyEntry<'_> {
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
    pub async fn entry<T>(self, attr: T, generation: u64) -> io::Result<()>
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
        self.raw.reply(0, &[entry_out.as_ref()]).await
    }

    /// Reply to the kernel with the specified error number.
    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

/// Reply with the read link value.
#[derive(Debug)]
#[must_use]
pub struct ReplyReadlink<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyReadlink<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl<'a> ReplyReadlink<'a> {
    /// Reply to the kernel with the specified link value.
    pub async fn link(self, value: impl AsRef<OsStr>) -> io::Result<()> {
        self.raw.reply(0, &[value.as_ref().as_bytes()]).await
    }

    /// Reply to the kernel with the specified error number.
    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

/// Reply with an opened file.
#[derive(Debug)]
#[must_use]
pub struct ReplyOpen<'a> {
    raw: ReplyRaw<'a>,
    open_flags: OpenFlags,
}

impl<'a> From<ReplyRaw<'a>> for ReplyOpen<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self {
            raw,
            open_flags: OpenFlags::empty(),
        }
    }
}

impl ReplyOpen<'_> {
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
    pub async fn open(self, fh: u64) -> io::Result<()> {
        let out = OpenOut {
            fh,
            open_flags: self.open_flags,
            ..Default::default()
        };
        self.raw.reply(0, &[out.as_ref()]).await
    }

    /// Reply to the kernel with the specified error number.
    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

/// Reply with the information about written data.
#[derive(Debug)]
#[must_use]
pub struct ReplyWrite<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyWrite<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl ReplyWrite<'_> {
    /// Reply to the kernel with the total length of written data.
    pub async fn write(self, size: u32) -> io::Result<()> {
        let out = WriteOut {
            size,
            ..Default::default()
        };
        self.raw.reply(0, &[out.as_ref()]).await
    }

    /// Reply to the kernel with the specified error number.
    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

/// Reply with an opened directory.
#[derive(Debug)]
#[must_use]
pub struct ReplyOpendir<'a> {
    raw: ReplyRaw<'a>,
    open_flags: OpenFlags,
}

impl<'a> From<ReplyRaw<'a>> for ReplyOpendir<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self {
            raw,
            open_flags: OpenFlags::empty(),
        }
    }
}

impl ReplyOpendir<'_> {
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
    pub async fn open(self, fh: u64) -> io::Result<()> {
        let out = OpenOut {
            fh,
            open_flags: self.open_flags,
            ..Default::default()
        };
        self.raw.reply(0, &[out.as_ref()]).await
    }

    /// Reply to the kernel with the specified error number.
    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

// placeholder.
pub type ReplyReaddir<'a> = ReplyData<'a>;

/// Reply to a request about extended attributes.
#[derive(Debug)]
#[must_use]
pub struct ReplyXattr<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyXattr<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl<'a> ReplyXattr<'a> {
    /// Reply to the kernel with the specified size value.
    pub async fn size(self, size: u32) -> io::Result<()> {
        let out = GetxattrOut {
            size,
            ..Default::default()
        };
        self.raw.reply(0, &[out.as_ref()]).await
    }

    /// Reply to the kernel with the specified value.
    pub async fn value(self, value: impl AsRef<[u8]>) -> io::Result<()> {
        self.raw.reply(0, &[value.as_ref()]).await
    }

    /// Reply to the kernel with the specified error number.
    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

/// Reply with the filesystem staticstics.
#[derive(Debug)]
#[must_use]
pub struct ReplyStatfs<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyStatfs<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl ReplyStatfs<'_> {
    /// Reply to the kernel with the specified statistics.
    pub async fn stat<T>(self, st: T) -> io::Result<()>
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
        self.raw.reply(0, &[out.as_ref()]).await
    }

    /// Reply to the kernel with the specified error number.
    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

/// Reply with a file lock.
#[derive(Debug)]
#[must_use]
pub struct ReplyLk<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyLk<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl ReplyLk<'_> {
    /// Reply to the kernel with the specified file lock.
    pub async fn lock<T>(self, lk: T) -> io::Result<()>
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
        self.raw.reply(0, &[out.as_ref()]).await
    }

    /// Reply to the kernel with the specified error number.
    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyCreate<'a> {
    raw: ReplyRaw<'a>,
    entry_valid: (u64, u32),
    attr_valid: (u64, u32),
    open_flags: OpenFlags,
}

impl<'a> From<ReplyRaw<'a>> for ReplyCreate<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self {
            raw,
            entry_valid: (0, 0),
            attr_valid: (0, 0),
            open_flags: OpenFlags::empty(),
        }
    }
}

impl ReplyCreate<'_> {
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
    pub async fn create<T>(self, attr: T, generation: u64, fh: u64) -> io::Result<()>
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

        self.raw
            .reply(0, &[entry_out.as_ref(), open_out.as_ref()])
            .await
    }

    /// Reply to the kernel with the specified error number.
    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyBmap<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyBmap<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl ReplyBmap<'_> {
    pub async fn block(self, block: u64) -> io::Result<()> {
        let out = BmapOut {
            block,
            ..Default::default()
        };
        self.raw.reply(0, &[out.as_ref()]).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}
