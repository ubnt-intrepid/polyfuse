use polyfuse::{
    error, op,
    reply::{self, EntryOptions},
    types::FileAttr,
};
use std::{ffi::OsStr, io, time::Duration};

pub type Ok = ();

pub struct Error(io::Error);

impl error::Error for Error {
    fn from_io_error(io_error: io::Error) -> Self {
        Self(io_error)
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
        todo!()
    }
}

// ==== Lookup ====

pub struct Lookup<'req> {
    _marker: std::marker::PhantomData<&'req ()>,
}

impl op::Operation for Lookup<'_> {
    type Ok = Ok;
    type Error = Error;

    fn unique(&self) -> u64 {
        todo!()
    }

    fn uid(&self) -> u32 {
        todo!()
    }

    fn gid(&self) -> u32 {
        todo!()
    }

    fn pid(&self) -> u32 {
        todo!()
    }

    fn unimplemented(self) -> Result {
        todo!()
    }
}

impl<'req> op::Lookup for Lookup<'req> {
    type Reply = ReplyEntry<'req>;

    fn parent(&self) -> u64 {
        todo!()
    }

    fn name(&self) -> &OsStr {
        todo!()
    }

    fn reply(self) -> Self::Reply {
        todo!()
    }
}

// ==== Getattr ====

pub struct Getattr<'req> {
    _marker: std::marker::PhantomData<&'req ()>,
}

impl op::Operation for Getattr<'_> {
    type Ok = Ok;
    type Error = Error;

    fn unique(&self) -> u64 {
        todo!()
    }

    fn uid(&self) -> u32 {
        todo!()
    }

    fn gid(&self) -> u32 {
        todo!()
    }

    fn pid(&self) -> u32 {
        todo!()
    }

    fn unimplemented(self) -> Result {
        todo!()
    }
}

impl<'req> op::Getattr for Getattr<'req> {
    type Reply = ReplyAttr<'req>;

    fn ino(&self) -> u64 {
        todo!()
    }

    fn fh(&self) -> Option<u64> {
        todo!()
    }

    fn reply(self) -> Self::Reply {
        todo!()
    }
}

// ==== replies ====

pub struct ReplyEntry<'req> {
    _marker: std::marker::PhantomData<&'req ()>,
}

impl reply::Reply for ReplyEntry<'_> {
    type Ok = Ok;
    type Error = Error;
}

impl reply::ReplyEntry for ReplyEntry<'_> {
    fn entry(self, _attr: &FileAttr, _opts: &EntryOptions) -> Result {
        todo!()
    }
}

pub struct ReplyAttr<'req> {
    _marker: std::marker::PhantomData<&'req ()>,
}

impl reply::Reply for ReplyAttr<'_> {
    type Ok = Ok;
    type Error = Error;
}

impl reply::ReplyAttr for ReplyAttr<'_> {
    fn attr(self, _attr: &FileAttr, _ttl: Option<Duration>) -> Result {
        todo!()
    }
}
