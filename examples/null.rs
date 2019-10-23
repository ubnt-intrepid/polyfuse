#![warn(clippy::unimplemented)]
#![allow(clippy::needless_lifetimes)]

use polyfuse::{
    reply::{ReplyAttr, ReplyData, ReplyEmpty, ReplyOpen, ReplyWrite},
    AttrSet, //
    Context,
    FileAttr,
    MountOptions,
    Nodeid,
    Operations,
};
use std::{
    convert::TryInto, //
    env,
    future::Future,
    io,
    path::PathBuf,
    pin::Pin,
};

#[tokio::main]
async fn main() -> io::Result<()> {
    env::set_var("RUST_LOG", "polyfuse=debug");
    pretty_env_logger::init();

    let mountpoint = env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing mountpoint"))?;
    if !mountpoint.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "the mountpoint must be a regular file",
        ));
    }

    polyfuse::tokio::mount(
        Null {}, //
        mountpoint,
        MountOptions::default(),
    )
    .await?;
    Ok(())
}

struct Null {}

impl<T> Operations<T> for Null {
    fn getattr<'a>(
        &mut self,
        _cx: &Context,
        ino: Nodeid,
        _fh: Option<u64>,
        reply: ReplyAttr<'a>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + 'a>> {
        Box::pin(async move {
            match ino {
                Nodeid::ROOT => reply.attr(root_attr()).await,
                _ => reply.err(libc::ENOENT).await,
            }
        })
    }

    fn setattr<'a>(
        &mut self,
        _cx: &Context,
        ino: Nodeid,
        _fh: Option<u64>,
        _attr: AttrSet,
        reply: ReplyAttr<'a>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + 'a>> {
        Box::pin(async move {
            match ino {
                Nodeid::ROOT => reply.attr(root_attr()).await,
                _ => reply.err(libc::ENOENT).await,
            }
        })
    }

    fn open<'a>(
        &mut self,
        _cx: &Context,
        ino: Nodeid,
        _flags: u32,
        reply: ReplyOpen<'a>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + 'a>> {
        Box::pin(async move {
            match ino {
                Nodeid::ROOT => reply.open(0).await,
                _ => reply.err(libc::ENOENT).await,
            }
        })
    }

    fn read<'a>(
        &mut self,
        _cx: &Context,
        ino: Nodeid,
        _fh: u64,
        _offset: u64,
        _flags: u32,
        _lock_owner: Option<u64>,
        reply: ReplyData<'a>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + 'a>> {
        Box::pin(async move {
            match ino {
                Nodeid::ROOT => reply.data(&[]).await,
                _ => reply.err(libc::ENOENT).await,
            }
        })
    }

    fn write<'a>(
        &mut self,
        _cx: &Context,
        ino: Nodeid,
        _fh: u64,
        _offset: u64,
        _data: T,
        size: u32,
        _flags: u32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite<'a>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + 'a>> {
        Box::pin(async move {
            match ino {
                Nodeid::ROOT => reply.write(size).await,
                _ => reply.err(libc::ENOENT).await,
            }
        })
    }

    fn release<'a>(
        &mut self,
        _cx: &Context,
        ino: Nodeid,
        _fh: u64,
        _flags: u32,
        _lock_owner: Option<u64>,
        _flush: bool,
        _flock_release: bool,
        reply: ReplyEmpty<'a>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + 'a>> {
        Box::pin(async move {
            match ino {
                Nodeid::ROOT => reply.ok().await,
                _ => reply.err(libc::ENOENT).await,
            }
        })
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
