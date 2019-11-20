#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

use polyfuse_examples::prelude::*;

use futures::select;
use polyfuse::{DirEntry, FileAttr, Interrupt};
use std::io;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    examples::init_tracing()?;

    let mountpoint = examples::get_mountpoint()?;
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

    fn lookup(&self, parent: u64, name: &OsStr) -> Option<FileAttr> {
        match parent {
            1 if name.as_bytes() == self.filename.as_bytes() => Some(self.hello_attr()),
            _ => None,
        }
    }

    fn getattr(&self, ino: u64) -> Option<FileAttr> {
        match ino {
            1 => Some(self.root_attr()),
            2 => Some(self.hello_attr()),
            _ => None,
        }
    }

    async fn read(
        &self,
        ino: u64,
        offset: usize,
        size: usize,
        mut intr: Interrupt,
    ) -> Result<&[u8], libc::c_int> {
        match ino {
            1 => Err(libc::EISDIR),
            2 => {
                let mut fetch_task = Box::pin(async {
                    tokio::time::delay_for(std::time::Duration::from_secs(10)).await;
                })
                .fuse();
                select! {
                    _ = fetch_task => (),
                    _ = intr => return Err(libc::EINTR),
                }

                if offset >= self.content.len() {
                    return Ok(&[] as &[u8]);
                }
                let data = &self.content.as_bytes()[offset..];
                let data = &data[..std::cmp::min(data.len(), size)];
                Ok(data)
            }
            _ => Err(libc::ENOENT),
        }
    }

    fn readdir(&self, ino: u64, offset: u64, size: usize) -> Result<Vec<&[u8]>, libc::c_int> {
        if ino != 1 {
            return Err(libc::ENOENT);
        }

        let mut entries = Vec::with_capacity(3);
        let mut total_len = 0usize;
        for entry in self.dir_entries.iter().skip(offset as usize) {
            let entry = entry.as_ref();
            if total_len + entry.len() > size {
                break;
            }
            entries.push(entry);
            total_len += entry.len();
        }

        Ok(entries)
    }
}

#[async_trait]
impl<T> Filesystem<T> for Hello {
    async fn call<W: ?Sized>(&self, cx: &mut Context<'_, W>, op: Operation<'_, T>) -> io::Result<()>
    where
        T: Send + 'async_trait,
        W: AsyncWrite + Unpin + Send,
    {
        match op {
            Operation::Lookup(op) => match self.lookup(op.parent(), op.name()) {
                Some(attr) => {
                    let mut reply = op.reply();
                    reply.attr_valid(u64::max_value(), u32::max_value());
                    reply.entry_valid(u64::max_value(), u32::max_value());
                    reply.entry(cx, attr, 0).await?;
                }
                None => cx.reply_err(libc::ENOENT).await?,
            },

            Operation::Getattr(op) => match self.getattr(op.ino()) {
                Some(attr) => {
                    let mut reply = op.reply();
                    reply.attr_valid(u64::max_value(), u32::max_value());
                    reply.attr(cx, attr).await?;
                }
                None => cx.reply_err(libc::ENOENT).await?,
            },

            Operation::Read(op) => {
                let intr = cx.on_interrupt().await;
                match self
                    .read(op.ino(), op.offset() as usize, op.size() as usize, intr)
                    .await
                {
                    Ok(data) => op.reply().data(cx, data).await?,
                    Err(errno) => cx.reply_err(errno).await?,
                }
            }

            Operation::Readdir(op) => match self.readdir(op.ino(), op.offset(), op.size() as usize)
            {
                Ok(entries) => op.reply().data_vectored(cx, &*entries).await?,
                Err(errno) => cx.reply_err(errno).await?,
            },

            _ => (),
        }

        Ok(())
    }
}
