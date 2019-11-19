mod inode;

use crate::prelude::*;
use inode::INodeTable;
use polyfuse::{
    reply::{ReplyAttr, ReplyData, ReplyEntry, ReplyWrite},
    FileAttr,
};
use std::{io, time::SystemTime};

/// An in-memory filesystem.
pub struct MemFS {
    inodes: INodeTable,
    entry_valid: (u64, u32),
    attr_valid: (u64, u32),
}

impl MemFS {
    /// Create a new `MemFS` mounted on the specified directory.
    pub fn new(mountpoint: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self {
            inodes: INodeTable::new(mountpoint)?,
            entry_valid: (u64::max_value(), u32::max_value()),
            attr_valid: (u64::max_value(), u32::max_value()),
        })
    }

    fn make_attr<W: ?Sized>(&self, cx: &Context<'_, W>, mode: u32) -> FileAttr {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        let sec = now.as_secs();
        let nsec = now.subsec_nanos();

        let mut attr = FileAttr::default();
        attr.set_nlink(1);
        attr.set_mode(mode);
        attr.set_uid(cx.uid());
        attr.set_gid(cx.gid());
        attr.set_atime(sec, nsec);
        attr.set_mtime(sec, nsec);
        attr.set_ctime(sec, nsec);
        attr
    }

    async fn do_lookup<W: ?Sized>(
        &self,
        cx: &mut Context<'_, W>,
        parent: u64,
        name: &OsStr,
        mut reply: ReplyEntry,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        match self.inodes.lookup(parent, name).await {
            Some(inode) => {
                let attr = inode.load_attr();
                reply.entry_valid(self.entry_valid.0, self.entry_valid.1);
                reply.attr_valid(self.attr_valid.0, self.attr_valid.1);
                reply.entry(cx, attr, 0).await
            }
            None => cx.reply_err(libc::ENOENT).await,
        }
    }

    async fn do_getattr<W: ?Sized>(
        &self,
        cx: &mut Context<'_, W>,
        ino: u64,
        mut reply: ReplyAttr,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let inode = match self.inodes.get(ino).await {
            Some(inode) => inode,
            None => return cx.reply_err(libc::ENOENT).await,
        };

        let attr = inode.load_attr();

        reply.attr_valid(self.attr_valid.0, self.attr_valid.1);
        reply.attr(cx, attr).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn do_setattr<W: ?Sized>(
        &self,
        cx: &mut Context<'_, W>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<(u64, u32, bool)>,
        mtime: Option<(u64, u32, bool)>,
        ctime: Option<(u64, u32)>,
        mut reply: ReplyAttr,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let inode = match self.inodes.get(ino).await {
            Some(inode) => inode,
            None => return cx.reply_err(libc::ENOENT).await,
        };

        let mut attr = inode.load_attr();
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();

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
                attr.set_atime(now.as_secs() as u64, now.subsec_nanos());
            } else {
                attr.set_atime(s, ns);
            }
        }
        if let Some((s, ns, is_now)) = mtime {
            if is_now {
                attr.set_mtime(now.as_secs() as u64, now.subsec_nanos());
            } else {
                attr.set_mtime(s, ns);
            }
        }
        if let Some((s, ns)) = ctime {
            attr.set_ctime(s, ns);
        }

        inode.store_attr(attr);

        reply.attr_valid(self.attr_valid.0, self.attr_valid.1);
        reply.attr(cx, attr).await
    }

    async fn do_read<W: ?Sized>(
        &self,
        cx: &mut Context<'_, W>,
        ino: u64,
        offset: u64,
        reply: ReplyData,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let inode = match self.inodes.get(ino).await {
            Some(inode) => inode,
            None => return cx.reply_err(libc::ENOENT).await,
        };

        let file = match inode.as_file() {
            Some(file) => file,
            None => return cx.reply_err(libc::EPERM).await,
        };

        match file.read(offset as usize, reply.size() as usize).await {
            Some(data) => reply.data(cx, &data).await,
            None => reply.data(cx, &[]).await,
        }
    }

    async fn do_write<W: ?Sized, T>(
        &self,
        cx: &mut Context<'_, W>,
        ino: u64,
        offset: u64,
        data: T,
        reply: ReplyWrite,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
        T: AsRef<[u8]>,
    {
        let inode = match self.inodes.get(ino).await {
            Some(inode) => inode,
            None => return cx.reply_err(libc::ENOENT).await,
        };

        let file = match inode.as_file() {
            Some(file) => file,
            None => return cx.reply_err(libc::EPERM).await,
        };

        let data = data.as_ref();
        file.write(offset as usize, data).await;

        let mut attr = inode.load_attr();
        attr.set_size(offset + data.len() as u64);
        inode.store_attr(attr);

        reply.write(cx, data.len() as u32).await
    }

    async fn do_readdir<W: ?Sized>(
        &self,
        cx: &mut Context<'_, W>,
        ino: u64,
        offset: u64,
        reply: ReplyData,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let inode = match self.inodes.get(ino).await {
            Some(inode) => inode,
            None => return cx.reply_err(libc::ENOENT).await,
        };

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

        reply.data_vectored(cx, &*entries).await
    }

    async fn do_mknod<W: ?Sized>(
        &self,
        cx: &mut Context<'_, W>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        mut reply: ReplyEntry,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        match mode & libc::S_IFMT {
            libc::S_IFREG => {
                let attr = self.make_attr(cx, mode);
                match self.inodes.insert_file(parent, name, attr, vec![]).await {
                    Ok(attr) => {
                        reply.entry_valid(self.entry_valid.0, self.entry_valid.1);
                        reply.attr_valid(self.attr_valid.0, self.attr_valid.1);
                        reply.entry(cx, attr, 0).await
                    }
                    Err(errno) => cx.reply_err(errno).await,
                }
            }
            _ => cx.reply_err(libc::ENOTSUP).await,
        }
    }

    async fn do_mkdir<W: ?Sized>(
        &self,
        cx: &mut Context<'_, W>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        mut reply: ReplyEntry,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let attr = self.make_attr(cx, mode);
        match self.inodes.insert_dir(parent, name, attr).await {
            Ok(attr) => {
                reply.entry_valid(self.entry_valid.0, self.entry_valid.1);
                reply.attr_valid(self.attr_valid.0, self.attr_valid.1);
                reply.entry(cx, attr, 0).await
            }
            Err(errno) => cx.reply_err(errno).await,
        }
    }
}

#[async_trait]
impl<T> Filesystem<T> for MemFS
where
    T: AsRef<[u8]>,
{
    async fn call<W: ?Sized>(&self, cx: &mut Context<'_, W>, op: Operation<'_, T>) -> io::Result<()>
    where
        W: AsyncWrite + Send + Unpin + 'async_trait,
        T: Send + 'async_trait,
    {
        match op {
            Operation::Lookup {
                parent,
                name,
                reply,
                ..
            } => self.do_lookup(cx, parent, name, reply).await?,

            Operation::Getattr {
                ino, //
                reply,
                ..
            } => self.do_getattr(cx, ino, reply).await?,

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
                self.do_setattr(cx, ino, mode, uid, gid, size, atime, mtime, ctime, reply)
                    .await?
            }

            Operation::Read {
                ino, offset, reply, ..
            } => self.do_read(cx, ino, offset, reply).await?,

            Operation::Write {
                ino,
                offset,
                data,
                reply,
                ..
            } => self.do_write(cx, ino, offset, data, reply).await?,

            Operation::Readdir {
                ino, offset, reply, ..
            } => self.do_readdir(cx, ino, offset, reply).await?,

            Operation::Mknod {
                parent,
                name,
                mode,
                reply,
                ..
            } => self.do_mknod(cx, parent, name, mode, reply).await?,

            Operation::Mkdir {
                parent,
                name,
                mode,
                reply,
                ..
            } => self.do_mkdir(cx, parent, name, mode, reply).await?,

            Operation::Unlink {
                parent,
                name,
                reply,
                ..
            }
            | Operation::Rmdir {
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
