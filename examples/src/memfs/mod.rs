mod inode;
mod table;

pub use inode::{Directory, File, INode};
pub use table::INodeTable;

use crate::prelude::*;
use polyfuse::{op, FileAttr};
use std::{fs::Metadata, io, time::SystemTime};

/// An in-memory filesystem.
pub struct MemFS {
    inodes: INodeTable,
    entry_valid: (u64, u32),
    attr_valid: (u64, u32),
}

impl MemFS {
    /// Create a new `MemFS` mounted on the specified directory.
    pub fn new(metadata: &Metadata) -> Self {
        Self {
            inodes: INodeTable::new(metadata),
            entry_valid: (u64::max_value(), u32::max_value()),
            attr_valid: (u64::max_value(), u32::max_value()),
        }
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
        op: op::Lookup<'_>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        match self.inodes.lookup(op.parent(), op.name()).await {
            Some(inode) => {
                let attr = inode.load_attr();
                let mut reply = op.reply();
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
        op: op::Getattr<'_>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let inode = match self.inodes.get(op.ino()).await {
            Some(inode) => inode,
            None => return cx.reply_err(libc::ENOENT).await,
        };

        let attr = inode.load_attr();

        let mut reply = op.reply();
        reply.attr_valid(self.attr_valid.0, self.attr_valid.1);
        reply.attr(cx, attr).await
    }

    async fn do_setattr<W: ?Sized>(
        &self,
        cx: &mut Context<'_, W>,
        op: polyfuse::op::Setattr<'_>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let inode = match self.inodes.get(op.ino()).await {
            Some(inode) => inode,
            None => return cx.reply_err(libc::ENOENT).await,
        };

        let mut attr = inode.load_attr();
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();

        if let Some(mode) = op.mode() {
            attr.set_mode(mode);
        }
        if let Some(uid) = op.uid() {
            attr.set_uid(uid);
        }
        if let Some(gid) = op.gid() {
            attr.set_gid(gid);
        }
        if let Some(size) = op.size() {
            attr.set_size(size);
        }
        if let Some((s, ns, is_now)) = op.atime() {
            if is_now {
                attr.set_atime(now.as_secs() as u64, now.subsec_nanos());
            } else {
                attr.set_atime(s, ns);
            }
        }
        if let Some((s, ns, is_now)) = op.mtime() {
            if is_now {
                attr.set_mtime(now.as_secs() as u64, now.subsec_nanos());
            } else {
                attr.set_mtime(s, ns);
            }
        }
        if let Some((s, ns)) = op.ctime() {
            attr.set_ctime(s, ns);
        }

        inode.store_attr(attr);

        let mut reply = op.reply();
        reply.attr_valid(self.attr_valid.0, self.attr_valid.1);
        reply.attr(cx, attr).await
    }

    async fn do_read<W: ?Sized>(&self, cx: &mut Context<'_, W>, op: op::Read<'_>) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let inode = match self.inodes.get(op.ino()).await {
            Some(inode) => inode,
            None => return cx.reply_err(libc::ENOENT).await,
        };

        let file = match inode.downcast_ref::<File>() {
            Some(file) => file,
            None => return cx.reply_err(libc::EPERM).await,
        };

        let reply = op.reply();
        match file.read(op.offset() as usize, op.size() as usize).await {
            Some(data) => reply.data(cx, &data).await,
            None => reply.data(cx, &[]).await,
        }
    }

    async fn do_write<W: ?Sized, T>(
        &self,
        cx: &mut Context<'_, W>,
        op: op::Write<'_>,
        data: T,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
        T: AsRef<[u8]>,
    {
        let inode = match self.inodes.get(op.ino()).await {
            Some(inode) => inode,
            None => return cx.reply_err(libc::ENOENT).await,
        };

        let file = match inode.downcast_ref::<File>() {
            Some(file) => file,
            None => return cx.reply_err(libc::EPERM).await,
        };

        let data = data.as_ref();
        let offset = op.offset();

        file.write(offset as usize, data).await;

        let mut attr = inode.load_attr();
        attr.set_size(offset + data.len() as u64);
        inode.store_attr(attr);

        let reply = op.reply();
        reply.write(cx, data.len() as u32).await
    }

    async fn do_readdir<W: ?Sized>(
        &self,
        cx: &mut Context<'_, W>,
        op: op::Readdir<'_>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let inode = match self.inodes.get(op.ino()).await {
            Some(inode) => inode,
            None => return cx.reply_err(libc::ENOENT).await,
        };

        let dir = match inode.downcast_ref::<Directory>() {
            Some(dir) => dir,
            None => return cx.reply_err(libc::ENOTDIR).await,
        };

        // FIXME: polish
        let mut totallen = 0;
        let entries: Vec<_> = dir
            .entries()
            .await
            .into_iter()
            .skip(op.offset() as usize)
            .take_while(|entry| {
                totallen += entry.as_ref().len();
                totallen <= op.size() as usize
            })
            .collect();
        let entries: Vec<_> = entries.iter().map(|entry| entry.as_ref()).collect();

        let reply = op.reply();
        reply.data_vectored(cx, &*entries).await
    }

    async fn do_mknod<W: ?Sized>(
        &self,
        cx: &mut Context<'_, W>,
        op: op::Mknod<'_>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        match op.mode() & libc::S_IFMT {
            libc::S_IFREG => {
                let attr = self.make_attr(cx, op.mode());
                match self
                    .inodes
                    .insert_file(op.parent(), op.name(), attr, vec![])
                    .await
                {
                    Ok(inode) => {
                        let attr = inode.load_attr();
                        let mut reply = op.reply();
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
        op: op::Mkdir<'_>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let attr = self.make_attr(cx, op.mode());
        match self.inodes.insert_dir(op.parent(), op.name(), attr).await {
            Ok(inode) => {
                let attr = inode.load_attr();
                let mut reply = op.reply();
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
            Operation::Lookup(op) => self.do_lookup(cx, op).await?,
            Operation::Getattr(op) => self.do_getattr(cx, op).await?,
            Operation::Setattr(op) => self.do_setattr(cx, op).await?,
            Operation::Read(op) => self.do_read(cx, op).await?,
            Operation::Write(op, data) => self.do_write(cx, op, data).await?,
            Operation::Readdir(op) => self.do_readdir(cx, op).await?,
            Operation::Mknod(op) => self.do_mknod(cx, op).await?,
            Operation::Mkdir(op) => self.do_mkdir(cx, op).await?,
            Operation::Unlink(op) => match self.inodes.remove(op.parent(), op.name()).await {
                Ok(()) => op.reply().ok(cx).await?,
                Err(errno) => cx.reply_err(errno).await?,
            },
            Operation::Rmdir(op) => match self.inodes.remove(op.parent(), op.name()).await {
                Ok(()) => op.reply().ok(cx).await?,
                Err(errno) => cx.reply_err(errno).await?,
            },

            _ => (),
        }

        Ok(())
    }
}
