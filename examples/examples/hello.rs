#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

use polyfuse_examples::prelude::*;

use polyfuse::{
    reply::{ReplyAttr, ReplyEntry},
    DirEntry, FileAttr,
};
use std::io;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mountpoint = examples::get_mountpoint()?;
    ensure!(mountpoint.is_dir(), "the mountpoint must be a directory");

    polyfuse_tokio::mount(Hello::new(), mountpoint, &[]).await?;

    Ok(())
}

struct Hello {
    filename: String,
    content: String,
    dir_entries: Vec<DirEntry>,
}

impl Hello {
    fn new() -> Self {
        let filename = "hello.txt".to_string();
        let content = "Hello, World!\n".to_string();
        let dir_entries = {
            let mut entries = Vec::with_capacity(3);
            entries.push(DirEntry::dir(".", 1, 1));
            entries.push(DirEntry::dir("..", 1, 2));
            entries.push(DirEntry::file(&filename, 2, 3));
            entries
        };

        Self {
            filename,
            content,
            dir_entries,
        }
    }

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

    async fn read(&self, ino: u64, offset: usize, size: usize) -> Result<&[u8], libc::c_int> {
        match ino {
            1 => Err(libc::EISDIR),
            2 => {
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
impl Filesystem for Hello {
    async fn call<'a, 'cx, T: ?Sized>(
        &'a self,
        cx: &'a mut Context<'cx, T>,
        op: Operation<'cx>,
    ) -> io::Result<()>
    where
        T: Reader + Writer + Unpin + Send,
    {
        match op {
            Operation::Lookup(op) => match self.lookup(op.parent(), op.name()) {
                Some(attr) => {
                    op.reply(cx, {
                        ReplyEntry::new(attr)
                            .attr_valid(u64::max_value(), u32::max_value())
                            .entry_valid(u64::max_value(), u32::max_value())
                    })
                    .await?;
                }
                None => cx.reply_err(libc::ENOENT).await?,
            },

            Operation::Getattr(op) => match self.getattr(op.ino()) {
                Some(attr) => {
                    op.reply(cx, {
                        ReplyAttr::new(attr) //
                            .attr_valid(u64::max_value(), u32::max_value())
                    })
                    .await?;
                }
                None => cx.reply_err(libc::ENOENT).await?,
            },

            Operation::Read(op) => {
                match self
                    .read(op.ino(), op.offset() as usize, op.size() as usize)
                    .await
                {
                    Ok(data) => op.reply(cx, data).await?,
                    Err(errno) => cx.reply_err(errno).await?,
                }
            }

            Operation::Readdir(op) => match self.readdir(op.ino(), op.offset(), op.size() as usize)
            {
                Ok(entries) => op.reply_vectored(cx, &*entries).await?,
                Err(errno) => cx.reply_err(errno).await?,
            },

            _ => (),
        }

        Ok(())
    }
}
