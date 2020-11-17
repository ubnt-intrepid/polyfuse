use crate::{conn::Writer, parse, util::as_bytes, write};
use polyfuse::{
    op,
    reply::{self, EntryOptions},
    types::FileAttr,
};
use polyfuse_kernel as kernel;
use std::{ffi::OsStr, fmt, io, time::Duration, u32, u64};

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

impl polyfuse::error::Error for Error {
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

pub type Result = std::result::Result<Ok, Error>;

/// The kind of filesystem operation requested by the kernel.
#[non_exhaustive]
pub enum Operation<'req> {
    Lookup(Lookup<'req>),
    Getattr(Getattr<'req>),
}

impl<'req> Operation<'req> {
    /// Annotate that the operation is not supported by the filesystem.
    pub async fn unimplemented(self) -> Result {
        Err(polyfuse::error::code(libc::ENOSYS))
    }
}

pub struct Op<'req, Arg> {
    pub(crate) header: &'req kernel::fuse_in_header,
    pub(crate) arg: Arg,
    pub(crate) writer: &'req Writer,
}

impl<Arg> op::Operation for Op<'_, Arg> {
    type Ok = Ok;
    type Error = Error;

    fn unique(&self) -> u64 {
        self.header.unique
    }

    fn uid(&self) -> u32 {
        self.header.uid
    }

    fn gid(&self) -> u32 {
        self.header.gid
    }

    fn pid(&self) -> u32 {
        self.header.pid
    }

    fn unimplemented(self) -> Result {
        Err(polyfuse::error::code(libc::ENOSYS))
    }
}

// ==== Lookup ====

pub type Lookup<'req> = Op<'req, parse::Lookup<'req>>;

impl<'req> op::Lookup for Lookup<'req> {
    type Reply = ReplyEntry<'req>;

    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.arg.name
    }

    fn reply(self) -> Self::Reply {
        ReplyEntry {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_entry_out::default(),
        }
    }
}

pub type Getattr<'req> = Op<'req, parse::Getattr<'req>>;

impl<'req> op::Getattr for Getattr<'req> {
    type Reply = ReplyAttr<'req>;

    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> Option<u64> {
        if self.arg.arg.getattr_flags & kernel::FUSE_GETATTR_FH != 0 {
            Some(self.arg.arg.fh)
        } else {
            None
        }
    }

    fn reply(self) -> Self::Reply {
        ReplyAttr {
            writer: self.writer,
            header: self.header,
            arg: kernel::fuse_attr_out::default(),
        }
    }
}

// ==== replies ====

pub struct ReplyEntry<'req> {
    writer: &'req Writer,
    header: &'req kernel::fuse_in_header,
    arg: kernel::fuse_entry_out,
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
        .map_err(polyfuse::error::io)?;

        Ok(())
    }
}

pub struct ReplyAttr<'req> {
    writer: &'req Writer,
    header: &'req kernel::fuse_in_header,
    arg: kernel::fuse_attr_out,
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
        .map_err(polyfuse::error::io)?;

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
