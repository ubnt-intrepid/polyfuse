use futures::future::Future;
use polyfuse::types::FileAttr;
use polyfuse::{op, reply};
use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::time::Duration;

/// Start a FUSE daemon mount on the specified path.
pub async fn mount(_mountpoint: impl AsRef<Path>, _mountopts: &[&OsStr]) -> anyhow::Result<Daemon> {
    todo!()
}

/// The instance of FUSE daemon for interaction with the kernel driver.
pub struct Daemon {
    _p: (),
}

impl Daemon {
    /// Receive an incoming FUSE request from the kernel.
    pub async fn next_request(&mut self) -> Option<Request> {
        todo!()
    }
}

/// Context about an incoming FUSE request.
pub struct Request {
    _p: (),
}

impl Request {
    /// Process the request.
    pub async fn process<'a, F, Fut>(&'a mut self, _f: F) -> anyhow::Result<()>
    where
        F: FnOnce(Operation<'a>) -> Fut,
        Fut: Future<Output = Result<(), Error>>,
    {
        todo!()
    }
}

// ==== operations ====

pub struct Error(io::Error);

impl polyfuse::error::Error for Error {
    fn from_io_error(io_error: io::Error) -> Self {
        Self(io_error)
    }
}

#[non_exhaustive]
pub enum Operation<'req> {
    Getattr(Getattr<'req>),
}

impl<'req> Operation<'req> {
    pub async fn default(self) -> Result<(), Error> {
        todo!()
    }
}

pub struct Getattr<'req> {
    _marker: std::marker::PhantomData<&'req ()>,
}

impl op::Operation for Getattr<'_> {
    type Ok = ();
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

    fn unimplemented(self) -> Result<Self::Ok, Self::Error> {
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

pub struct ReplyAttr<'req> {
    _marker: std::marker::PhantomData<&'req ()>,
}

impl reply::Reply for ReplyAttr<'_> {
    type Ok = ();
    type Error = Error;
}

impl reply::ReplyAttr for ReplyAttr<'_> {
    fn attr(self, attr: &FileAttr, ttl: Option<Duration>) -> Result<Self::Ok, Self::Error> {
        todo!()
    }
}
