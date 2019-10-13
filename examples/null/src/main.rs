#![warn(clippy::unimplemented)]
#![allow(clippy::needless_lifetimes)]

use fuse_async::{
    abi::{
        FileAttr,
        GetattrIn, //
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
    reply::{ReplyAttr, ReplyData, ReplyOpen, ReplyUnit, ReplyWrite},
    Operations,
};
use std::{convert::TryInto, env, future::Future, io, path::PathBuf, pin::Pin};

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

    let op = Null;
    fuse_async::tokio::mount(mountpoint, None::<&str>, op).await?;

    Ok(())
}

struct Null;

impl<'d> Operations<&'d [u8]> for Null {
    fn getattr<'a>(
        &mut self,
        header: &InHeader,
        _: &GetattrIn,
        reply: ReplyAttr<'a>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + 'a>> {
        match header.nodeid {
            Nodeid::ROOT => Box::pin(reply.ok(root_attr().into())),
            _ => Box::pin(reply.err(libc::ENOENT)),
        }
    }

    fn setattr<'a>(
        &mut self,
        header: &InHeader,
        _: &SetattrIn,
        reply: ReplyAttr<'a>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + 'a>> {
        match header.nodeid {
            Nodeid::ROOT => Box::pin(reply.ok(root_attr().into())),
            _ => Box::pin(reply.err(libc::ENOENT)),
        }
    }

    fn open<'a>(
        &mut self,
        header: &InHeader,
        _: &OpenIn,
        reply: ReplyOpen<'a>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + 'a>> {
        match header.nodeid {
            Nodeid::ROOT => Box::pin(reply.ok(OpenOut::default())),
            _ => Box::pin(reply.err(libc::ENOENT)),
        }
    }

    fn read<'a>(
        &mut self,
        header: &InHeader,
        _: &ReadIn,
        reply: ReplyData<'a>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + 'a>> {
        match header.nodeid {
            Nodeid::ROOT => Box::pin(reply.ok(&[])),
            _ => Box::pin(reply.err(libc::ENOENT)),
        }
    }

    fn write<'a>(
        &mut self,
        header: &InHeader,
        _: &WriteIn,
        data: &'d [u8],
        reply: ReplyWrite<'a>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + 'a>> {
        match header.nodeid {
            Nodeid::ROOT => {
                let mut out = WriteOut::default();
                out.size = data.len() as u32;
                Box::pin(reply.ok(out))
            }
            _ => Box::pin(reply.err(libc::ENOENT)),
        }
    }

    fn release<'a>(
        &mut self,
        header: &InHeader,
        _: &ReleaseIn,
        reply: ReplyUnit<'a>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + 'a>> {
        match header.nodeid {
            Nodeid::ROOT => Box::pin(reply.ok()),
            _ => Box::pin(reply.err(libc::ENOENT)),
        }
    }
}

fn root_attr() -> FileAttr {
    let mut attr: libc::stat = unsafe { std::mem::zeroed() };
    attr.st_mode = libc::S_IFREG | 0o644;
    attr.st_nlink = 1;
    attr.st_uid = unsafe { libc::getuid() };
    attr.st_gid = unsafe { libc::getgid() };
    attr.try_into().unwrap()
}
