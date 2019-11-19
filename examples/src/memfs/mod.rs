mod inode;

use crate::prelude::*;
use polyfuse::FileAttr;

use inode::INodeTable;
use std::io;

pub struct MemFs {
    inodes: INodeTable,
    entry_valid: (u64, u32),
    attr_valid: (u64, u32),
}

impl Default for MemFs {
    fn default() -> Self {
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };

        Self {
            inodes: INodeTable::new(uid, gid),
            entry_valid: (u64::max_value(), u32::max_value()),
            attr_valid: (u64::max_value(), u32::max_value()),
        }
    }
}

impl MemFs {
    fn make_attr(&self, mode: u32, uid: u32, gid: u32) -> FileAttr {
        let mut attr = FileAttr::default();
        attr.set_nlink(1);
        attr.set_mode(mode);
        attr.set_uid(uid);
        attr.set_gid(gid);
        attr
    }
}

#[async_trait]
impl<T: AsRef<[u8]>> Filesystem<T> for MemFs {
    #[allow(clippy::cognitive_complexity)]
    async fn call<W: ?Sized>(&self, cx: &mut Context<'_, W>, op: Operation<'_, T>) -> io::Result<()>
    where
        W: AsyncWrite + Send + Unpin + 'async_trait,
        T: Send + 'async_trait,
    {
        match op {
            Operation::Lookup {
                parent,
                name,
                mut reply,
                ..
            } => match self.inodes.lookup(parent, name).await {
                Some(inode) => {
                    let attr = inode.attr().await;
                    reply.entry_valid(self.entry_valid.0, self.entry_valid.1);
                    reply.attr_valid(self.attr_valid.0, self.attr_valid.1);
                    reply.entry(cx, attr, 0).await?;
                }
                None => cx.reply_err(libc::ENOENT).await?,
            },

            Operation::Getattr { ino, mut reply, .. } => match self.inodes.get(ino).await {
                Some(inode) => {
                    let attr = inode.attr().await;
                    reply.attr_valid(self.attr_valid.0, self.attr_valid.1);
                    reply.attr(cx, attr).await?;
                }
                None => cx.reply_err(libc::ENOENT).await?,
            },

            Operation::Setattr {
                ino,
                mode,
                uid,
                gid,
                size,
                atime,
                mtime,
                ctime,
                reply,
                ..
            } => {
                let inode = match self.inodes.get(ino).await {
                    Some(inode) => inode,
                    None => return cx.reply_err(libc::ENOENT).await,
                };

                let mut attr = inode.attr().await;
                let now = chrono::Local::now();

                if let Some(mode) = mode {
                    attr.set_mode(mode);
                }
                if let Some(uid) = uid {
                    attr.set_uid(uid);
                }
                if let Some(gid) = gid {
                    attr.set_gid(gid);
                }
                if let Some(size) = size {
                    attr.set_size(size);
                }
                if let Some((s, ns, is_now)) = atime {
                    if is_now {
                        attr.set_atime(now.timestamp() as u64, now.timestamp_subsec_nanos());
                    } else {
                        attr.set_atime(s, ns);
                    }
                }
                if let Some((s, ns, is_now)) = mtime {
                    if is_now {
                        attr.set_mtime(now.timestamp() as u64, now.timestamp_subsec_nanos());
                    } else {
                        attr.set_mtime(s, ns);
                    }
                }
                if let Some((s, ns)) = ctime {
                    attr.set_ctime(s, ns);
                }

                inode.set_attr(attr).await;
                reply.attr(cx, attr).await?;
            }

            Operation::Read {
                ino, offset, reply, ..
            } => match self.inodes.get(ino).await {
                Some(inode) => {
                    let file = match inode.as_file() {
                        Some(file) => file,
                        None => return cx.reply_err(libc::EPERM).await,
                    };

                    match file.read(offset as usize, reply.size() as usize).await {
                        Some(data) => reply.data(cx, &data).await?,
                        None => reply.data(cx, &[]).await?,
                    }
                }
                None => cx.reply_err(libc::ENOENT).await?,
            },

            Operation::Write {
                ino,
                offset,
                data,
                reply,
                ..
            } => match self.inodes.get(ino).await {
                Some(inode) => {
                    let file = match inode.as_file() {
                        Some(file) => file,
                        None => return cx.reply_err(libc::EPERM).await,
                    };

                    let data = data.as_ref();
                    file.write(offset as usize, data.as_ref()).await;

                    let mut attr = inode.attr().await;
                    attr.set_size(offset + data.len() as u64);
                    inode.set_attr(attr).await;

                    reply.write(cx, data.len() as u32).await?;
                }
                None => cx.reply_err(libc::ENOENT).await?,
            },

            Operation::Readdir {
                ino, offset, reply, ..
            } => match self.inodes.get(ino).await {
                Some(inode) => {
                    let dir = match inode.as_dir() {
                        Some(dir) => dir,
                        None => return cx.reply_err(libc::ENOTDIR).await,
                    };

                    // FIXME: polish
                    let mut totallen = 0;
                    let entries: Vec<_> = dir
                        .entries()
                        .await
                        .into_iter()
                        .skip(offset as usize)
                        .take_while(|entry| {
                            totallen += entry.as_ref().len();
                            totallen <= reply.size() as usize
                        })
                        .collect();
                    let entries: Vec<_> = entries.iter().map(|entry| entry.as_ref()).collect();

                    reply.data_vectored(cx, &*entries).await?;
                }
                None => cx.reply_err(libc::ENOENT).await?,
            },

            Operation::Mknod {
                parent,
                name,
                mode,
                reply,
                ..
            } => match mode & libc::S_IFMT {
                libc::S_IFREG => {
                    let attr = self.make_attr(mode, cx.uid(), cx.gid());
                    match self.inodes.insert_file(parent, name, attr, vec![]).await {
                        Ok(attr) => reply.entry(cx, attr, 0).await?,
                        Err(errno) => cx.reply_err(errno).await?,
                    }
                }
                _ => cx.reply_err(libc::ENOTSUP).await?,
            },

            Operation::Mkdir {
                parent,
                name,
                mode,
                reply,
                ..
            } => {
                let attr = self.make_attr(mode, cx.uid(), cx.gid());
                match self.inodes.insert_dir(parent, name, attr).await {
                    Ok(attr) => reply.entry(cx, attr, 0).await?,
                    Err(errno) => cx.reply_err(errno).await?,
                }
            }

            Operation::Unlink {
                parent,
                name,
                reply,
                ..
            } => match self.inodes.remove(parent, name).await {
                Ok(()) => reply.ok(cx).await?,
                Err(errno) => cx.reply_err(errno).await?,
            },

            Operation::Rmdir {
                parent,
                name,
                reply,
                ..
            } => match self.inodes.remove(parent, name).await {
                Ok(()) => reply.ok(cx).await?,
                Err(errno) => cx.reply_err(errno).await?,
            },

            _ => (),
        }

        Ok(())
    }
}
