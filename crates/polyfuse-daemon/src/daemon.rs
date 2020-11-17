use crate::op::{self, Operation};
use futures::future::Future;
use std::{ffi::OsStr, path::Path};

/// The instance of FUSE daemon for interaction with the kernel driver.
pub struct Daemon {
    _p: (),
}

impl Daemon {
    /// Start a FUSE daemon mount on the specified path.
    pub async fn mount(
        _mountpoint: impl AsRef<Path>,
        _mountopts: &[&OsStr],
    ) -> anyhow::Result<Self> {
        todo!()
    }

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
        Fut: Future<Output = op::Result>,
    {
        todo!()
    }
}
