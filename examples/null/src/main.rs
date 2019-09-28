#![warn(clippy::unimplemented)]
#![allow(clippy::needless_lifetimes)]

use async_trait::async_trait;
use fuse_async::{
    abi::{
        AttrOut, //
        GetattrIn,
        InHeader,
        Nodeid,
        OpenIn,
        OpenOut,
        ReadIn,
        ReleaseIn,
        SetattrIn,
        WriteIn,
        WriteOut,
    },
    Buffer, Error, Operations, Session,
};
use fuse_async_channel::tokio::Channel;
use std::{borrow::Cow, env, io, path::PathBuf};

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

    let mut buf = Buffer::new();
    let mut session = Session::new();
    let mut op = Null;
    session.run(&mut ch, &mut buf, &mut op).await?;

    Ok(())
}

struct Null;

#[async_trait]
impl Operations for Null {
    async fn getattr(&mut self, header: &InHeader, _: &GetattrIn) -> fuse_async::Result<AttrOut> {
        match header.nodeid {
            Nodeid::ROOT => Ok(root_attr().into()),
            _ => Err(Error(libc::ENOENT)),
        }
    }

    async fn setattr(&mut self, header: &InHeader, _: &SetattrIn) -> fuse_async::Result<AttrOut> {
        match header.nodeid {
            Nodeid::ROOT => Ok(root_attr().into()),
            _ => Err(Error(libc::ENOENT)),
        }
    }

    async fn open(&mut self, header: &InHeader, _: &OpenIn) -> fuse_async::Result<OpenOut> {
        match header.nodeid {
            Nodeid::ROOT => Ok(OpenOut::default()),
            _ => Err(Error(libc::ENOENT)),
        }
    }

    async fn read<'a>(
        &'a mut self,
        header: &InHeader,
        _: &ReadIn,
    ) -> fuse_async::Result<Cow<'a, [u8]>> {
        match header.nodeid {
            Nodeid::ROOT => Ok(Cow::Borrowed(&[] as &[u8])),
            _ => Err(Error(libc::ENOENT)),
        }
    }

    async fn write(
        &mut self,
        header: &InHeader,
        _: &WriteIn,
        buf: &[u8],
    ) -> fuse_async::Result<WriteOut> {
        match header.nodeid {
            Nodeid::ROOT => {
                let mut out = WriteOut::default();
                out.size = buf.len() as u32;
                Ok(out)
            }
            _ => Err(Error(libc::ENOENT)),
        }
    }

    async fn release(&mut self, header: &InHeader, _: &ReleaseIn) -> fuse_async::Result<()> {
        match header.nodeid {
            Nodeid::ROOT => Ok(()),
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
