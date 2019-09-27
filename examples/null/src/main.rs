#![warn(clippy::unimplemented)]

use async_trait::async_trait;
use fuse_async::{
    abi::{
        AttrOut, //
        GetattrIn,
        InHeader,
        OpenIn,
        OpenOut,
        ReadIn,
        ReleaseIn,
        SetattrIn,
        WriteIn,
        WriteOut,
    },
    Error, Operations, Session,
};
use fuse_async_channel::tokio::Channel;
use futures::{future::FusedFuture, prelude::*};
use std::{borrow::Cow, env, io, path::PathBuf};
use tokio::net::signal::ctrl_c;

const BUFFER_SIZE: usize = fuse_async::MAX_WRITE_SIZE + 4096;

fn shutdown_signal() -> io::Result<impl FusedFuture<Output = ()> + Unpin> {
    Ok(ctrl_c()?.into_future().map(|_| ()))
}

#[tokio::main]
async fn main() -> io::Result<()> {
    std::env::set_var("RUST_LOG", "fuse_async=debug");
    pretty_env_logger::init();

    let mountpoint = env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, ""))?;
    if !mountpoint.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "the mountpoint must be a regular file",
        ));
    }

    let mut ch = Channel::builder("null").mount(mountpoint)?;

    let mut buf = Vec::with_capacity(BUFFER_SIZE);
    let mut session = Session::new();
    let mut op = Null;
    let sig = shutdown_signal()?;
    session.run_until(&mut ch, &mut buf, &mut op, sig).await?;

    Ok(())
}

struct Null;

#[async_trait]
impl Operations for Null {
    async fn getattr<'a>(
        &'a mut self,
        header: &'a InHeader,
        _: &'a GetattrIn,
    ) -> fuse_async::Result<AttrOut> {
        match header.nodeid() {
            1 => Ok(root_attr().into()),
            _ => Err(Error(libc::ENOENT)),
        }
    }

    async fn setattr<'a>(
        &'a mut self,
        header: &'a InHeader,
        _: &'a SetattrIn,
    ) -> fuse_async::Result<AttrOut> {
        match header.nodeid() {
            1 => Ok(root_attr().into()),
            _ => Err(Error(libc::ENOENT)),
        }
    }

    async fn open<'a>(
        &'a mut self,
        header: &'a InHeader,
        _: &'a OpenIn,
    ) -> fuse_async::Result<OpenOut> {
        match header.nodeid() {
            1 => Ok(OpenOut::default()),
            _ => Err(Error(libc::ENOENT)),
        }
    }

    async fn read<'a>(
        &'a mut self,
        header: &'a InHeader,
        _: &'a ReadIn,
    ) -> fuse_async::Result<Cow<'a, [u8]>> {
        match header.nodeid() {
            1 => Ok(Cow::Borrowed(&[] as &[u8])),
            _ => Err(Error(libc::ENOENT)),
        }
    }

    async fn write<'a>(
        &'a mut self,
        header: &'a InHeader,
        _: &'a WriteIn,
        buf: &'a [u8],
    ) -> fuse_async::Result<WriteOut> {
        match header.nodeid() {
            1 => {
                let mut out = WriteOut::default();
                out.set_size(buf.len() as u32);
                Ok(out)
            }
            _ => Err(Error(libc::ENOENT)),
        }
    }

    async fn release<'a>(
        &'a mut self,
        header: &'a InHeader,
        _: &'a ReleaseIn,
    ) -> fuse_async::Result<()> {
        match header.nodeid() {
            1 => Ok(()),
            _ => Err(Error(libc::ENOENT)),
        }
    }
}

fn root_attr() -> libc::stat {
    let mut attr: libc::stat = unsafe { std::mem::zeroed() };
    attr.st_mode = libc::S_IFREG | 0o644;
    attr.st_nlink = 1;
    attr.st_uid = unsafe { libc::getuid() };
    attr.st_gid = unsafe { libc::getgid() };
    attr
}
