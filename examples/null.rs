#![warn(clippy::unimplemented)]
#![allow(clippy::needless_lifetimes)]

use polyfuse::{Context, Filesystem, Operation, Server};
use std::{env, io, path::PathBuf};

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

    Server::mount(mountpoint, Default::default())?
        .run(Null {})
        .await?;

    Ok(())
}

struct Null {}

#[async_trait::async_trait(?Send)]
impl<T> Filesystem<T> for Null {
    async fn call(&self, cx: &mut Context<'_>, op: Operation<'_, T>) -> io::Result<()>
    where
        T: 'async_trait, // https://github.com/dtolnay/async-trait/issues/8
    {
        match op {
            Operation::Getattr { ino, reply, .. } => match ino {
                1 => reply.attr(cx, root_attr()).await,
                _ => cx.reply_err(libc::ENOENT).await,
            },
            Operation::Setattr { ino, reply, .. } => match ino {
                1 => reply.attr(cx, root_attr()).await,
                _ => cx.reply_err(libc::ENOENT).await,
            },
            Operation::Open { ino, reply, .. } => match ino {
                1 => reply.open(cx, 0).await,
                _ => cx.reply_err(libc::ENOENT).await,
            },
            Operation::Read { ino, reply, .. } => match ino {
                1 => reply.data(cx, &[]).await,
                _ => cx.reply_err(libc::ENOENT).await,
            },
            Operation::Write {
                ino, size, reply, ..
            } => match ino {
                1 => reply.write(cx, size).await,
                _ => cx.reply_err(libc::ENOENT).await,
            },
            Operation::Release { ino, reply, .. } => match ino {
                1 => reply.ok(cx).await,
                _ => cx.reply_err(libc::ENOENT).await,
            },
            _ => cx.reply_err(libc::ENOSYS).await,
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
