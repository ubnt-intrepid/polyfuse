#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures::{future::FutureExt, io::AsyncWrite, select};
use polyfuse::{Context, DirEntry, FileAttr, Filesystem, Operation};
use std::{env, io, os::unix::ffi::OsStrExt, path::PathBuf};
use tracing::Level;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_max_level(Level::DEBUG)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let mountpoint = env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("missing mountpoint"))?;
    anyhow::ensure!(mountpoint.is_dir(), "the mountpoint must be a directory");

    let filename = "hello.txt".to_string();
    let content = "Hello, World!\n".to_string();
    let dir_entries = {
        let mut entries = Vec::with_capacity(3);
        entries.push(DirEntry::dir(".", 1, 1));
        entries.push(DirEntry::dir("..", 1, 2));
        entries.push(DirEntry::file(&filename, 2, 3));
        entries
    };
    let hello = Hello {
        filename,
        content,
        dir_entries,
    };

    polyfuse_tokio::run(hello, mountpoint).await?;

    Ok(())
}

struct Hello {
    filename: String,
    content: String,
    dir_entries: Vec<DirEntry>,
}

impl Hello {
    fn root_attr(&self) -> FileAttr {
        let mut attr = FileAttr::default();
        attr.set_mode(libc::S_IFDIR as u32 | 0o555);
        attr.set_ino(1);
        attr.set_nlink(2);
        attr.set_uid(unsafe { libc::getuid() });
        attr.set_gid(unsafe { libc::getgid() });
        attr
    }

    fn hello_attr(&self) -> FileAttr {
        let mut attr = FileAttr::default();
        attr.set_mode(libc::S_IFREG as u32 | 0o444);
        attr.set_ino(2);
        attr.set_nlink(1);
        attr.set_size(self.content.len() as u64);
        attr.set_uid(unsafe { libc::getuid() });
        attr.set_gid(unsafe { libc::getgid() });
        attr
    }
}

async fn expensive_task() -> io::Result<()> {
    tokio::time::delay_for(std::time::Duration::from_secs(10)).await;
    Ok(())
}

#[async_trait]
impl<T> Filesystem<T> for Hello {
    async fn call<W: ?Sized>(&self, cx: &mut Context<'_, W>, op: Operation<'_, T>) -> io::Result<()>
    where
        T: Send + 'async_trait,
        W: AsyncWrite + Unpin + Send,
    {
        match op {
            Operation::Lookup {
                parent,
                name,
                mut reply,
                ..
            } => match parent {
                1 => {
                    if name.as_bytes() == self.filename.as_bytes() {
                        reply.attr_valid(u64::max_value(), u32::max_value());
                        reply.entry_valid(u64::max_value(), u32::max_value());
                        reply.entry(cx, self.hello_attr(), 0).await
                    } else {
                        cx.reply_err(libc::ENOENT).await
                    }
                }
                _ => cx.reply_err(libc::ENOENT).await,
            },

            Operation::Getattr { ino, mut reply, .. } => {
                let attr = match ino {
                    1 => self.root_attr(),
                    2 => self.hello_attr(),
                    _ => return cx.reply_err(libc::ENOENT).await,
                };
                reply.attr_valid(u64::max_value(), u32::max_value());
                reply.attr(cx, attr).await
            }

            Operation::Read {
                ino, reply, offset, ..
            } => match ino {
                1 => cx.reply_err(libc::EISDIR).await,
                2 => {
                    let mut task = Box::pin(expensive_task()).fuse();
                    let mut intr = cx.on_interrupt().await;
                    let this = self;
                    select! {
                        res = task => {
                            res?;
                            let offset = offset as usize;
                            let size = reply.size() as usize;
                            if offset >= this.content.len() {
                                return reply.data(cx, &[]).await;
                            }

                            let data = &this.content.as_bytes()[offset..];
                            let data = &data[..std::cmp::min(data.len(), size)];
                            reply.data(cx, data).await
                        },
                        _ = intr => cx.reply_err(libc::EINTR).await,
                    }
                }
                _ => cx.reply_err(libc::ENOENT).await,
            },

            Operation::Readdir {
                ino, reply, offset, ..
            } => {
                if ino != 1 {
                    return cx.reply_err(libc::ENOENT).await;
                }

                let mut entries = Vec::with_capacity(3);
                let mut total_len = 0usize;
                for entry in self.dir_entries.iter().skip(offset as usize) {
                    let entry = entry.as_ref();
                    if total_len + entry.len() > reply.size() as usize {
                        break;
                    }
                    entries.push(entry);
                    total_len += entry.len();
                }

                reply.data_vectored(cx, &*entries).await
            }

            _ => Ok(()),
        }
    }
}
