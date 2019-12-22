mod inode;
mod table;

pub use inode::{Directory, File, INode};
pub use table::INodeTable;

use crate::prelude::*;
use futures::io::AsyncReadExt;
use polyfuse::{
    op,
    reply::{ReplyAttr, ReplyEntry, ReplyWrite},
    FileAttr,
};
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

    fn make_attr(&self, uid: u32, gid: u32, mode: u32) -> FileAttr {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        let sec = now.as_secs();
        let nsec = now.subsec_nanos();

        let mut attr = FileAttr::default();
        attr.set_nlink(1);
        attr.set_mode(mode);
        attr.set_uid(uid);
        attr.set_gid(gid);
        attr.set_atime(sec, nsec);
        attr.set_mtime(sec, nsec);
        attr.set_ctime(sec, nsec);
        attr
    }

    async fn do_lookup<T: ?Sized>(
        &self,
        cx: &mut Context<'_, T>,
        op: op::Lookup<'_>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        match self.inodes.lookup(op.parent(), op.name()).await {
            Some(inode) => {
                let attr = inode.load_attr();
                let mut entry = ReplyEntry::new(attr);
                entry.entry_valid(self.entry_valid.0, self.entry_valid.1);
                entry.attr_valid(self.attr_valid.0, self.attr_valid.1);
                op.reply(cx, entry).await
            }
            None => cx.reply_err(libc::ENOENT).await,
        }
    }

    async fn do_getattr<T: ?Sized>(
        &self,
        cx: &mut Context<'_, T>,
        op: op::Getattr<'_>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let inode = match self.inodes.get(op.ino()).await {
            Some(inode) => inode,
            None => return cx.reply_err(libc::ENOENT).await,
        };

        let attr = inode.load_attr();

        let mut attr = ReplyAttr::new(attr);
        attr.attr_valid(self.attr_valid.0, self.attr_valid.1);
        op.reply(cx, attr).await
    }

    async fn do_setattr<T: ?Sized>(
        &self,
        cx: &mut Context<'_, T>,
        op: polyfuse::op::Setattr<'_>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
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

        let mut attr = ReplyAttr::new(attr);
        attr.attr_valid(self.attr_valid.0, self.attr_valid.1);
        op.reply(cx, attr).await
    }

    async fn do_read<T: ?Sized>(&self, cx: &mut Context<'_, T>, op: op::Read<'_>) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let inode = match self.inodes.get(op.ino()).await {
            Some(inode) => inode,
            None => return cx.reply_err(libc::ENOENT).await,
        };

        let file = match inode.downcast_ref::<File>() {
            Some(file) => file,
            None => return cx.reply_err(libc::EPERM).await,
        };

        match file.read(op.offset() as usize, op.size() as usize).await {
            Some(data) => op.reply(cx, &data).await,
            None => op.reply(cx, &[]).await,
        }
    }

    async fn do_write<T: ?Sized>(
        &self,
        cx: &mut Context<'_, T>,
        op: op::Write<'_>,
    ) -> io::Result<()>
    where
        T: Reader + Writer + Unpin,
    {
        let inode = match self.inodes.get(op.ino()).await {
            Some(inode) => inode,
            None => return cx.reply_err(libc::ENOENT).await,
        };

        let file = match inode.downcast_ref::<File>() {
            Some(file) => file,
            None => return cx.reply_err(libc::EPERM).await,
        };

        let mut data = vec![0u8; op.size() as usize];
        cx.reader().read_exact(&mut data).await?;
        let offset = op.offset();

        file.write(offset as usize, &data).await;

        let mut attr = inode.load_attr();
        attr.set_size(offset + data.len() as u64);
        inode.store_attr(attr);

        op.reply(cx, ReplyWrite::new(data.len() as u32)).await
    }

    async fn do_readdir<T: ?Sized>(
        &self,
        cx: &mut Context<'_, T>,
        op: op::Readdir<'_>,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
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

        op.reply_vectored(cx, &*entries).await
    }

    async fn do_mknod<T: ?Sized>(
        &self,
        cx: &mut Context<'_, T>,
        op: op::Mknod<'_>,
        uid: u32,
        gid: u32,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        match op.mode() & libc::S_IFMT {
            libc::S_IFREG => {
                let attr = self.make_attr(uid, gid, op.mode());
                match self
                    .inodes
                    .insert_file(op.parent(), op.name(), attr, vec![])
                    .await
                {
                    Ok(inode) => {
                        let attr = inode.load_attr();
                        let mut entry = ReplyEntry::new(attr);
                        entry.entry_valid(self.entry_valid.0, self.entry_valid.1);
                        entry.attr_valid(self.attr_valid.0, self.attr_valid.1);
                        op.reply(cx, entry).await
                    }
                    Err(errno) => cx.reply_err(errno).await,
                }
            }
            _ => cx.reply_err(libc::ENOTSUP).await,
        }
    }

    async fn do_mkdir<T: ?Sized>(
        &self,
        cx: &mut Context<'_, T>,
        op: op::Mkdir<'_>,
        uid: u32,
        gid: u32,
    ) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        let attr = self.make_attr(uid, gid, op.mode());
        match self.inodes.insert_dir(op.parent(), op.name(), attr).await {
            Ok(inode) => {
                let attr = inode.load_attr();
                let mut entry = ReplyEntry::new(attr);
                entry.entry_valid(self.entry_valid.0, self.entry_valid.1);
                entry.attr_valid(self.attr_valid.0, self.attr_valid.1);
                op.reply(cx, entry).await
            }
            Err(errno) => cx.reply_err(errno).await,
        }
    }
}

#[async_trait]
impl Filesystem for MemFS {
    async fn call<'a, 'cx, T: ?Sized>(
        &'a self,
        cx: &'a mut Context<'cx, T>,
        op: Operation<'cx>,
    ) -> io::Result<()>
    where
        T: Reader + Writer + Send + Unpin,
    {
        let uid = cx.uid();
        let gid = cx.gid();
        match op {
            Operation::Lookup(op) => self.do_lookup(cx, op).await?,
            Operation::Getattr(op) => self.do_getattr(cx, op).await?,
            Operation::Setattr(op) => self.do_setattr(cx, op).await?,
            Operation::Read(op) => self.do_read(cx, op).await?,
            Operation::Write(op) => self.do_write(cx, op).await?,
            Operation::Readdir(op) => self.do_readdir(cx, op).await?,
            Operation::Mknod(op) => self.do_mknod(cx, op, uid, gid).await?,
            Operation::Mkdir(op) => self.do_mkdir(cx, op, uid, gid).await?,
            Operation::Unlink(op) => match self.inodes.remove(op.parent(), op.name()).await {
                Ok(()) => op.reply(cx).await?,
                Err(errno) => cx.reply_err(errno).await?,
            },
            Operation::Rmdir(op) => match self.inodes.remove(op.parent(), op.name()).await {
                Ok(()) => op.reply(cx).await?,
                Err(errno) => cx.reply_err(errno).await?,
            },

            _ => (),
        }

        Ok(())
    }
}
