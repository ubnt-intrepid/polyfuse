use crate::{conn::Writer, util::as_bytes, write};
use polyfuse_kernel as kernel;
use polyfuse_types::{
    reply::{self, EntryOptions, OpenOptions},
    types::{FileAttr, FileLock, FsStatistics},
};
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

impl polyfuse_types::error::Error for Error {
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

impl reply::Reply for ReplyEntry<'_> {
    type Ok = Ok;
    type Error = Error;
}

impl reply::ReplyEntry for ReplyEntry<'_> {
    fn entry(mut self, attr: &FileAttr, opts: &EntryOptions) -> Result {
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
        .map_err(polyfuse_types::error::io)?;

        Ok(())
    }
}

pub struct ReplyAttr<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_attr_out,
}

impl reply::Reply for ReplyAttr<'_> {
    type Ok = Ok;
    type Error = Error;
}

impl reply::ReplyAttr for ReplyAttr<'_> {
    fn attr(mut self, attr: &FileAttr, ttl: Option<Duration>) -> Result {
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
        .map_err(polyfuse_types::error::io)?;

        Ok(())
    }
}

pub struct ReplyOk<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
}

impl reply::Reply for ReplyOk<'_> {
    type Ok = Ok;
    type Error = Error;
}

impl reply::ReplyOk for ReplyOk<'_> {
    fn ok(self) -> Result<Self::Ok, Self::Error> {
        write::send_reply(self.writer, self.header.unique, &[])
            .map_err(polyfuse_types::error::io)?;
        Ok(())
    }
}

pub struct ReplyData<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
}

impl reply::Reply for ReplyData<'_> {
    type Ok = Ok;
    type Error = Error;
}

impl reply::ReplyBytes for ReplyData<'_> {
    fn data<T>(self, data: T) -> Result
    where
        T: AsRef<[u8]>,
    {
        write::send_reply(self.writer, self.header.unique, data.as_ref())
            .map_err(polyfuse_types::error::io)?;

        Ok(())
    }
}

pub struct ReplyOpen<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_open_out,
}

impl reply::Reply for ReplyOpen<'_> {
    type Ok = Ok;
    type Error = Error;
}

impl reply::ReplyOpen for ReplyOpen<'_> {
    fn open(mut self, fh: u64, opts: &OpenOptions) -> Result<Self::Ok, Self::Error> {
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
        .map_err(polyfuse_types::error::io)?;

        Ok(())
    }
}

pub struct ReplyWrite<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_write_out,
}

impl reply::Reply for ReplyWrite<'_> {
    type Ok = Ok;
    type Error = Error;
}

impl reply::ReplyWrite for ReplyWrite<'_> {
    fn size(mut self, size: u32) -> Result<Self::Ok, Self::Error> {
        self.arg.size = size;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(polyfuse_types::error::io)?;

        Ok(())
    }
}

pub struct ReplyStatfs<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_statfs_out,
}

impl reply::Reply for ReplyStatfs<'_> {
    type Ok = Ok;
    type Error = Error;
}

impl reply::ReplyStatfs for ReplyStatfs<'_> {
    fn stat(mut self, stat: &FsStatistics) -> Result<Self::Ok, Self::Error> {
        fill_statfs(stat, &mut self.arg.st);

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(polyfuse_types::error::io)?;

        Ok(())
    }
}

pub struct ReplyXattr<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_getxattr_out,
}

impl reply::Reply for ReplyXattr<'_> {
    type Ok = Ok;
    type Error = Error;
}

impl reply::ReplyXattr for ReplyXattr<'_> {
    fn size(mut self, size: u32) -> Result<Self::Ok, Self::Error> {
        self.arg.size = size;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(polyfuse_types::error::io)?;

        Ok(())
    }

    fn data<T>(self, data: T) -> Result<Self::Ok, Self::Error>
    where
        T: AsRef<[u8]>,
    {
        write::send_reply(self.writer, self.header.unique, data.as_ref())
            .map_err(polyfuse_types::error::io)?;

        Ok(())
    }
}

pub struct ReplyLk<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_lk_out,
}

impl reply::Reply for ReplyLk<'_> {
    type Ok = Ok;
    type Error = Error;
}

impl reply::ReplyLk for ReplyLk<'_> {
    fn lk(mut self, lk: &FileLock) -> Result<Self::Ok, Self::Error> {
        fill_lk(lk, &mut self.arg.lk);

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(polyfuse_types::error::io)?;

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

impl reply::Reply for ReplyCreate<'_> {
    type Ok = Ok;
    type Error = Error;
}

impl reply::ReplyCreate for ReplyCreate<'_> {
    fn create(
        mut self,
        fh: u64,
        attr: &FileAttr,
        entry_opts: &EntryOptions,
        open_opts: &OpenOptions,
    ) -> Result<Self::Ok, Self::Error> {
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
        .map_err(polyfuse_types::error::io)?;

        Ok(())
    }
}

pub struct ReplyBmap<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_bmap_out,
}

impl reply::Reply for ReplyBmap<'_> {
    type Ok = Ok;
    type Error = Error;
}

impl reply::ReplyBmap for ReplyBmap<'_> {
    fn block(mut self, block: u64) -> Result<Self::Ok, Self::Error> {
        self.arg.block = block;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(polyfuse_types::error::io)?;

        Ok(())
    }
}

pub struct ReplyPoll<'req> {
    pub(crate) writer: &'req Writer,
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: kernel::fuse_poll_out,
}

impl reply::Reply for ReplyPoll<'_> {
    type Ok = Ok;
    type Error = Error;
}

impl reply::ReplyPoll for ReplyPoll<'_> {
    fn revents(mut self, revents: u32) -> Result<Self::Ok, Self::Error> {
        self.arg.revents = revents;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(polyfuse_types::error::io)?;

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
