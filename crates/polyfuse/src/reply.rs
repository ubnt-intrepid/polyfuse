#![allow(missing_docs)]

use crate::types::{FileAttr, FileLock, FsStatistics, NonExhaustive};
use std::{io, time::Duration};

/// The object for replying to the FUSE kernel driver.
pub trait Reply: Send {
    /// The value type returned when the reply is successful.
    type Ok;

    /// The error type produced by the filesystem.
    type Error: Error;

    /// Annotate to the backend that the filesystem does not support
    /// this operation.
    fn unimplemented(self) -> Result<Self::Ok, Self::Error>;
}

/// The error type returned from the filesystem.
pub trait Error {
    /// Construct the error value from an I/O error.
    fn from_io_error(io_error: io::Error) -> Self;

    /// Construct the error value from an error number.
    #[inline]
    fn from_raw_os_error(code: i32) -> Self
    where
        Self: Sized,
    {
        Self::from_io_error(io::Error::from_raw_os_error(code))
    }
}

/// A `Reply` for empty data.
pub trait ReplyOk: Reply {
    /// Reply with the empty data.
    fn ok(self) -> Result<Self::Ok, Self::Error>;
}

/// A `Reply` for entry params.
pub trait ReplyEntry: Reply {
    /// Reply with the entry params.
    fn entry(self, attr: &FileAttr, opts: &EntryOptions) -> Result<Self::Ok, Self::Error>;
}

/// Reply with entry params.
#[derive(Copy, Clone, Debug)]
pub struct EntryOptions {
    /// Return the inode number of this entry.
    ///
    /// If this value is zero, it means that the entry is *negative*.
    /// Returning a negative entry is also possible with the `ENOENT` error,
    /// but the *zeroed* entries also have the ability to specify the lifetime
    /// of the entry cache by using the `ttl_entry` parameter.
    ///
    /// The default value is `0`.
    pub ino: u64,

    /// Return the validity timeout for inode attributes.
    ///
    /// The operations should set this value to very large
    /// when the changes of inode attributes are caused
    /// only by FUSE requests.
    pub ttl_attr: Option<Duration>,

    /// Return the validity timeout for the name.
    ///
    /// The operations should set this value to very large
    /// when the changes/deletions of directory entries are
    /// caused only by FUSE requests.
    pub ttl_entry: Option<Duration>,

    /// Return the generation of this entry.
    ///
    /// This parameter is used to distinguish the inode from the past one
    /// when the filesystem reuse inode numbers.  That is, the operations
    /// must ensure that the pair of entry's inode number and generation
    /// are unique for the lifetime of the filesystem.
    pub generation: u64,

    #[doc(hidden)] // non_exhaustive
    pub __non_exhaustive: NonExhaustive,
}

impl Default for EntryOptions {
    fn default() -> Self {
        Self {
            ino: 0,
            ttl_attr: None,
            ttl_entry: None,
            generation: 0,

            __non_exhaustive: NonExhaustive::INIT,
        }
    }
}

pub trait ReplyAttr: Reply {
    fn attr(self, attr: &FileAttr, ttl: Option<Duration>) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyOpen: Reply {
    fn open(self, fh: u64, opts: &OpenOptions) -> Result<Self::Ok, Self::Error>;
}

/// Reply with an opened file.
#[derive(Copy, Clone, Debug)]
pub struct OpenOptions {
    /// Indicates that the direct I/O is used on this file.
    pub direct_io: bool,

    /// Indicates that the currently cached file data in the kernel
    /// need not be invalidated.
    pub keep_cache: bool,

    /// Indicates that the opened file is not seekable.
    pub nonseekable: bool,

    /// Enable caching of entries returned by `readdir`.
    ///
    /// This flag is meaningful only for `opendir` operations.
    pub cache_dir: bool,

    #[doc(hidden)] // non_exhaustive
    pub __non_exhaustive: NonExhaustive,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            direct_io: false,
            keep_cache: false,
            nonseekable: false,
            cache_dir: false,

            __non_exhaustive: NonExhaustive::INIT,
        }
    }
}

pub trait ReplyBytes: Reply {
    fn data<T>(self, bytes: T) -> Result<Self::Ok, Self::Error>
    where
        T: AsRef<[u8]>;
}

pub trait ReplyWrite: Reply {
    fn size(self, size: u32) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyXattr: Reply {
    fn size(self, size: u32) -> Result<Self::Ok, Self::Error>;

    fn data<T>(self, bytes: T) -> Result<Self::Ok, Self::Error>
    where
        T: AsRef<[u8]>;
}

pub trait ReplyStatfs: Reply {
    fn stat(self, stat: &FsStatistics) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyLk: Reply {
    fn lk(self, lk: &FileLock) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyBmap: Reply {
    fn block(self, block: u64) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyCreate: Reply {
    fn create(
        self,
        fh: u64,
        attr: &FileAttr,
        entry_opts: &EntryOptions,
        open_opts: &OpenOptions,
    ) -> Result<Self::Ok, Self::Error>;
}

pub trait ReplyPoll: Reply {
    fn revents(self, revents: u32) -> Result<Self::Ok, Self::Error>;
}
