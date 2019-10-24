#![warn(clippy::unimplemented)]
#![allow(clippy::needless_lifetimes)]

use polyfuse::{
    fs::{Context, Filesystem, Operation}, //
    FileAttr,
    MountOptions,
    Nodeid,
};
use std::{convert::TryInto, env, io, path::PathBuf};

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

    polyfuse::mount(
        Null {}, //
        mountpoint,
        MountOptions::default(),
    )
    .await?;
    Ok(())
}

struct Null {}

#[async_trait::async_trait(?Send)]
impl<T> Filesystem<T> for Null {
    async fn call(&mut self, _cx: &mut Context<'_>, op: Operation<'_, T>) -> io::Result<()>
    where
        T: 'async_trait, // https://github.com/dtolnay/async-trait/issues/8
    {
        match op {
            Operation::Getattr { ino, reply, .. } => match ino {
                Nodeid::ROOT => reply.attr(root_attr()).await,
                _ => reply.err(libc::ENOENT).await,
            },
            Operation::Setattr { ino, reply, .. } => match ino {
                Nodeid::ROOT => reply.attr(root_attr()).await,
                _ => reply.err(libc::ENOENT).await,
            },
            Operation::Open { ino, reply, .. } => match ino {
                Nodeid::ROOT => reply.open(0).await,
                _ => reply.err(libc::ENOENT).await,
            },
            Operation::Read { ino, reply, .. } => match ino {
                Nodeid::ROOT => reply.data(&[]).await,
                _ => reply.err(libc::ENOENT).await,
            },
            Operation::Write {
                ino, size, reply, ..
            } => match ino {
                Nodeid::ROOT => reply.write(size).await,
                _ => reply.err(libc::ENOENT).await,
            },
            Operation::Release { ino, reply, .. } => match ino {
                Nodeid::ROOT => reply.ok().await,
                _ => reply.err(libc::ENOENT).await,
            },
            op => op.reply_default().await,
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
