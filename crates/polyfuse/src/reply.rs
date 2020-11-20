use self::non_exhaustive::NonExhaustive;
use crate::{
    conn::Writer,
    types::{FileAttr, FileLock, FsStatistics},
    util::as_bytes,
    write,
};
use polyfuse_kernel as kernel;
use std::{fmt, io, time::Duration};

pub type Ok = ();

#[derive(Debug)]
pub struct Error(ErrorKind);

#[derive(Debug)]
enum ErrorKind {
    Code(i32),
    Fatal(io::Error),
}

impl Error {
    pub(crate) fn code(&self) -> Option<i32> {
        match self.0 {
            ErrorKind::Code(code) => Some(code),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpError").finish()
    }
}

impl std::error::Error for Error {}

impl crate::error::Error for Error {
    fn from_io_error(io_error: io::Error) -> Self {
        Self(ErrorKind::Fatal(io_error))
    }

    fn from_code(code: i32) -> Self
    where
        Self: Sized,
    {
        Self(ErrorKind::Code(code))
    }
}

pub type Result<T = Ok, E = Error> = std::result::Result<T, E>;

pub struct ReplyEntry<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_entry_out,
}

impl ReplyEntry<'_> {
    pub fn entry(mut self, attr: &FileAttr, opts: &EntryOptions) -> Result {
        fill_attr(attr, &mut self.arg.attr);
        self.arg.nodeid = opts.ino;
        self.arg.generation = opts.generation;

        if let Some(ttl) = opts.ttl_attr {
            self.arg.attr_valid = ttl.as_secs();
            self.arg.attr_valid_nsec = ttl.subsec_nanos();
        } else {
            self.arg.attr_valid = u64::MAX;
            self.arg.attr_valid_nsec = u32::MAX;
        }

        if let Some(ttl) = opts.ttl_entry {
            self.arg.entry_valid = ttl.as_secs();
            self.arg.entry_valid_nsec = ttl.subsec_nanos();
        } else {
            self.arg.entry_valid = u64::MAX;
            self.arg.entry_valid_nsec = u32::MAX;
        }

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(crate::error::io)?;

        Ok(())
    }
}

/// The option values for `ReplyEntry`.
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

pub struct ReplyAttr<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_attr_out,
}

impl ReplyAttr<'_> {
    pub fn attr(mut self, attr: &FileAttr, ttl: Option<Duration>) -> Result {
        fill_attr(attr, &mut self.arg.attr);

        if let Some(ttl) = ttl {
            self.arg.attr_valid = ttl.as_secs();
            self.arg.attr_valid_nsec = ttl.subsec_nanos();
        } else {
            self.arg.attr_valid = u64::MAX;
            self.arg.attr_valid_nsec = u32::MAX;
        }

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(crate::error::io)?;

        Ok(())
    }
}

pub struct ReplyOk<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
}

impl ReplyOk<'_> {
    pub fn ok(self) -> Result {
        write::send_reply(self.writer, self.header.unique, &[]).map_err(crate::error::io)?;
        Ok(())
    }
}

pub struct ReplyData<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
}

impl ReplyData<'_> {
    pub fn data<T>(self, data: T) -> Result
    where
        T: AsRef<[u8]>,
    {
        write::send_reply(self.writer, self.header.unique, data.as_ref())
            .map_err(crate::error::io)?;

        Ok(())
    }
}

pub struct ReplyOpen<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_open_out,
}

impl ReplyOpen<'_> {
    pub fn open(mut self, fh: u64, opts: &OpenOptions) -> Result {
        self.arg.fh = fh;

        if opts.direct_io {
            self.arg.open_flags |= kernel::FOPEN_DIRECT_IO;
        }

        if opts.keep_cache {
            self.arg.open_flags |= kernel::FOPEN_KEEP_CACHE;
        }

        if opts.nonseekable {
            self.arg.open_flags |= kernel::FOPEN_NONSEEKABLE;
        }

        if opts.cache_dir {
            self.arg.open_flags |= kernel::FOPEN_CACHE_DIR;
        }

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(crate::error::io)?;

        Ok(())
    }
}

/// The option values for `ReplyOpen`.
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

pub struct ReplyWrite<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_write_out,
}

impl ReplyWrite<'_> {
    pub fn size(mut self, size: u32) -> Result {
        self.arg.size = size;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(crate::error::io)?;

        Ok(())
    }
}

pub struct ReplyStatfs<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_statfs_out,
}

impl ReplyStatfs<'_> {
    pub fn stat(mut self, stat: &FsStatistics) -> Result {
        fill_statfs(stat, &mut self.arg.st);

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(crate::error::io)?;

        Ok(())
    }
}

pub struct ReplyXattr<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_getxattr_out,
}

impl ReplyXattr<'_> {
    pub fn size(mut self, size: u32) -> Result {
        self.arg.size = size;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(crate::error::io)?;

        Ok(())
    }

    pub fn data<T>(self, data: T) -> Result
    where
        T: AsRef<[u8]>,
    {
        write::send_reply(self.writer, self.header.unique, data.as_ref())
            .map_err(crate::error::io)?;

        Ok(())
    }
}

pub struct ReplyLk<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_lk_out,
}

impl ReplyLk<'_> {
    pub fn lk(mut self, lk: &FileLock) -> Result {
        fill_lk(lk, &mut self.arg.lk);

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(crate::error::io)?;

        Ok(())
    }
}

pub struct ReplyCreate<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: CreateArg,
}

#[derive(Default)]
#[repr(C)]
pub(crate) struct CreateArg {
    entry_out: kernel::fuse_entry_out,
    open_out: kernel::fuse_open_out,
}

impl ReplyCreate<'_> {
    pub fn create(
        mut self,
        fh: u64,
        attr: &FileAttr,
        entry_opts: &EntryOptions,
        open_opts: &OpenOptions,
    ) -> Result {
        fill_attr(attr, &mut self.arg.entry_out.attr);
        self.arg.entry_out.nodeid = entry_opts.ino;
        self.arg.entry_out.generation = entry_opts.generation;

        if let Some(ttl) = entry_opts.ttl_attr {
            self.arg.entry_out.attr_valid = ttl.as_secs();
            self.arg.entry_out.attr_valid_nsec = ttl.subsec_nanos();
        } else {
            self.arg.entry_out.attr_valid = u64::MAX;
            self.arg.entry_out.attr_valid_nsec = u32::MAX;
        }

        if let Some(ttl) = entry_opts.ttl_entry {
            self.arg.entry_out.entry_valid = ttl.as_secs();
            self.arg.entry_out.entry_valid_nsec = ttl.subsec_nanos();
        } else {
            self.arg.entry_out.entry_valid = u64::MAX;
            self.arg.entry_out.entry_valid_nsec = u32::MAX;
        }

        self.arg.open_out.fh = fh;

        if open_opts.direct_io {
            self.arg.open_out.open_flags |= kernel::FOPEN_DIRECT_IO;
        }

        if open_opts.keep_cache {
            self.arg.open_out.open_flags |= kernel::FOPEN_KEEP_CACHE;
        }

        if open_opts.nonseekable {
            self.arg.open_out.open_flags |= kernel::FOPEN_NONSEEKABLE;
        }

        if open_opts.cache_dir {
            self.arg.open_out.open_flags |= kernel::FOPEN_CACHE_DIR;
        }

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(crate::error::io)?;

        Ok(())
    }
}

pub struct ReplyBmap<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_bmap_out,
}

impl ReplyBmap<'_> {
    pub fn block(mut self, block: u64) -> Result {
        self.arg.block = block;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(crate::error::io)?;

        Ok(())
    }
}

pub struct ReplyPoll<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_poll_out,
}

impl ReplyPoll<'_> {
    pub fn revents(mut self, revents: u32) -> Result {
        self.arg.revents = revents;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(crate::error::io)?;

        Ok(())
    }
}

fn fill_attr(src: &FileAttr, dst: &mut kernel::fuse_attr) {
    dst.ino = src.ino;
    dst.size = src.size;
    dst.mode = src.mode;
    dst.nlink = src.nlink;
    dst.uid = src.uid;
    dst.gid = src.gid;
    dst.rdev = src.rdev;
    dst.blksize = src.blksize;
    dst.blocks = src.blocks;
    dst.atime = src.atime.secs;
    dst.atimensec = src.atime.nsecs;
    dst.mtime = src.mtime.secs;
    dst.mtimensec = src.mtime.nsecs;
    dst.ctime = src.ctime.secs;
    dst.ctimensec = src.ctime.nsecs;
}

fn fill_statfs(src: &FsStatistics, dst: &mut kernel::fuse_kstatfs) {
    dst.bsize = src.bsize;
    dst.frsize = src.frsize;
    dst.blocks = src.blocks;
    dst.bfree = src.bfree;
    dst.bavail = src.bavail;
    dst.files = src.files;
    dst.ffree = src.ffree;
    dst.namelen = src.namelen;
}

fn fill_lk(src: &FileLock, dst: &mut kernel::fuse_file_lock) {
    dst.typ = src.typ;
    dst.start = src.start;
    dst.end = src.end;
    dst.pid = src.pid;
}

mod non_exhaustive {
    #[derive(Copy, Clone, Debug)]
    pub struct NonExhaustive(());

    impl NonExhaustive {
        pub(crate) const INIT: Self = Self(());
    }
}
