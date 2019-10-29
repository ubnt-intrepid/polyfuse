#![warn(clippy::unimplemented)]

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use polyfuse::{Context, DirEntry, Filesystem, Operation, Server};
use std::{env, io, os::unix::ffi::OsStrExt, path::PathBuf};

#[tokio::main]
async fn main() -> Result<()> {
    env::set_var("RUST_LOG", "polyfuse=debug");
    pretty_env_logger::init();

    let mountpoint = env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("missing mountpoint"))?;
    anyhow::ensure!(mountpoint.is_dir(), "the mountpoint must be a directory");

    let filename = "hello.txt".to_string();
    let content = "Hello, World!\n".to_string();
    let dir_entries = {
        let mut entries = Vec::with_capacity(3);
        entries.push(DirEntry::new(".", 1, 1, libc::DT_DIR.into()));
        entries.push(DirEntry::new("..", 1, 2, libc::DT_DIR.into()));
        entries.push(DirEntry::new(&filename, 2, 3, libc::DT_REG.into()));
        entries
    };
    let hello = Hello {
        filename,
        content,
        dir_entries,
    };

    Server::mount(mountpoint, Default::default())?
        .run(hello)
        .await?;

    Ok(())
}

struct Hello {
    filename: String,
    content: String,
    dir_entries: Vec<DirEntry>,
}

impl Hello {
    fn root_attr(&self) -> libc::stat {
        let mut attr: libc::stat = unsafe { std::mem::zeroed() };
        attr.st_mode = libc::S_IFDIR | 0o555;
        attr.st_ino = 1;
        attr.st_nlink = 2;
        attr.st_uid = unsafe { libc::getuid() };
        attr.st_gid = unsafe { libc::getgid() };
        attr
    }

    fn hello_attr(&self) -> libc::stat {
        let mut attr: libc::stat = unsafe { std::mem::zeroed() };
        attr.st_mode = libc::S_IFREG | 0o444;
        attr.st_ino = 2;
        attr.st_nlink = 1;
        attr.st_size = self.content.len() as i64;
        attr.st_uid = unsafe { libc::getuid() };
        attr.st_gid = unsafe { libc::getgid() };
        attr
    }
}

#[async_trait]
impl<T: Send> Filesystem<T> for Hello {
    async fn call(&self, cx: &mut Context<'_>, op: Operation<'_, T>) -> io::Result<()>
    where
        T: 'async_trait, // https://github.com/dtolnay/async-trait/issues/8
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
                        reply.attr_valid(std::u64::MAX, std::u32::MAX);
                        reply.entry_valid(std::u64::MAX, std::u32::MAX);
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
                reply.attr_valid(std::u64::MAX, std::u32::MAX);
                reply.attr(cx, attr).await
            }

            Operation::Read {
                ino, reply, offset, ..
            } => match ino {
                1 => cx.reply_err(libc::EISDIR).await,
                2 => {
                    let offset = offset as usize;
                    let size = reply.size() as usize;
                    if offset >= self.content.len() {
                        return reply.data(cx, &[]).await;
                    }

                    let data = &self.content.as_bytes()[offset..];
                    let data = &data[..std::cmp::min(data.len(), size)];
                    reply.data(cx, data).await
                }
                _ => cx.reply_err(libc::ENOENT).await,
            },

            Operation::Readdir {
                ino, reply, offset, ..
            } => {
                if ino != 1 {
                    return cx.reply_err(libc::ENOENT).await;
                }

                let mut entries = smallvec::SmallVec::<[_; 3]>::new();
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

            _ => cx.reply_err(libc::ENOSYS).await,
        }
    }
}
