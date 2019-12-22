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

    async fn do_lookup<W: ?Sized>(
        &self,
        writer: &mut ReplyWriter<'_, W>,
        op: op::Lookup<'_>,
    ) -> io::Result<()>
    where
        W: Writer + Unpin,
    {
        match self.inodes.lookup(op.parent(), op.name()).await {
            Some(inode) => {
                let attr = inode.load_attr();
                let mut entry = ReplyEntry::new(attr);
                entry.entry_valid(self.entry_valid.0, self.entry_valid.1);
                entry.attr_valid(self.attr_valid.0, self.attr_valid.1);
                op.reply(writer, entry).await
            }
            None => writer.reply_err(libc::ENOENT).await,
        }
    }

    async fn do_getattr<W: ?Sized>(
        &self,
        writer: &mut ReplyWriter<'_, W>,
        op: op::Getattr<'_>,
    ) -> io::Result<()>
    where
        W: Writer + Unpin,
    {
        let inode = match self.inodes.get(op.ino()).await {
            Some(inode) => inode,
            None => return writer.reply_err(libc::ENOENT).await,
        };

        let attr = inode.load_attr();

        let mut attr = ReplyAttr::new(attr);
        attr.attr_valid(self.attr_valid.0, self.attr_valid.1);
        op.reply(writer, attr).await
    }

    async fn do_setattr<W: ?Sized>(
        &self,
        writer: &mut ReplyWriter<'_, W>,
        op: polyfuse::op::Setattr<'_>,
    ) -> io::Result<()>
    where
        W: Writer + Unpin,
    {
        let inode = match self.inodes.get(op.ino()).await {
            Some(inode) => inode,
            None => return writer.reply_err(libc::ENOENT).await,
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
        op.reply(writer, attr).await
    }

    async fn do_read<W: ?Sized>(
        &self,
        writer: &mut ReplyWriter<'_, W>,
        op: op::Read<'_>,
    ) -> io::Result<()>
    where
        W: Writer + Unpin,
    {
        let inode = match self.inodes.get(op.ino()).await {
            Some(inode) => inode,
            None => return writer.reply_err(libc::ENOENT).await,
        };

        let file = match inode.downcast_ref::<File>() {
            Some(file) => file,
            None => return writer.reply_err(libc::EPERM).await,
        };

        match file.read(op.offset() as usize, op.size() as usize).await {
            Some(data) => op.reply(writer, &data).await,
            None => op.reply(writer, &[]).await,
        }
    }

    async fn do_write<R: ?Sized, W: ?Sized>(
        &self,
        reader: &mut R,
        writer: &mut ReplyWriter<'_, W>,
        op: op::Write<'_>,
    ) -> io::Result<()>
    where
        R: AsyncRead + Unpin,
        W: Writer + Unpin,
    {
        let inode = match self.inodes.get(op.ino()).await {
            Some(inode) => inode,
            None => return writer.reply_err(libc::ENOENT).await,
        };

        let file = match inode.downcast_ref::<File>() {
            Some(file) => file,
            None => return writer.reply_err(libc::EPERM).await,
        };

        let mut data = vec![0u8; op.size() as usize];
        reader.read_exact(&mut data).await?;
        let offset = op.offset();

        file.write(offset as usize, &data).await;

        let mut attr = inode.load_attr();
        attr.set_size(offset + data.len() as u64);
        inode.store_attr(attr);

        op.reply(writer, ReplyWrite::new(data.len() as u32)).await
    }

    async fn do_readdir<W: ?Sized>(
        &self,
        writer: &mut ReplyWriter<'_, W>,
        op: op::Readdir<'_>,
    ) -> io::Result<()>
    where
        W: Writer + Unpin,
    {
        let inode = match self.inodes.get(op.ino()).await {
            Some(inode) => inode,
            None => return writer.reply_err(libc::ENOENT).await,
        };

        let dir = match inode.downcast_ref::<Directory>() {
            Some(dir) => dir,
            None => return writer.reply_err(libc::ENOTDIR).await,
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

        op.reply_vectored(writer, &*entries).await
    }

    async fn do_mknod<W: ?Sized>(
        &self,
        writer: &mut ReplyWriter<'_, W>,
        op: op::Mknod<'_>,
        uid: u32,
        gid: u32,
    ) -> io::Result<()>
    where
        W: Writer + Unpin,
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
                        op.reply(writer, entry).await
                    }
                    Err(errno) => writer.reply_err(errno).await,
                }
            }
            _ => writer.reply_err(libc::ENOTSUP).await,
        }
    }

    async fn do_mkdir<W: ?Sized>(
        &self,
        writer: &mut ReplyWriter<'_, W>,
        op: op::Mkdir<'_>,
        uid: u32,
        gid: u32,
    ) -> io::Result<()>
    where
        W: Writer + Unpin,
    {
        let attr = self.make_attr(uid, gid, op.mode());
        match self.inodes.insert_dir(op.parent(), op.name(), attr).await {
            Ok(inode) => {
                let attr = inode.load_attr();
                let mut entry = ReplyEntry::new(attr);
                entry.entry_valid(self.entry_valid.0, self.entry_valid.1);
                entry.attr_valid(self.attr_valid.0, self.attr_valid.1);
                op.reply(writer, entry).await
            }
            Err(errno) => writer.reply_err(errno).await,
        }
    }
}

#[async_trait]
impl Filesystem for MemFS {
    async fn reply<'a, 'cx, 'w, R: ?Sized, W: ?Sized>(
        &'a self,
        op: Operation<'cx>,
        reader: &'a mut R,
        writer: &'a mut ReplyWriter<'w, W>,
    ) -> io::Result<()>
    where
        R: AsyncRead + Send + Unpin,
        W: Writer + Send + Unpin,
    {
        let uid = op.uid();
        let gid = op.gid();
        match op {
            Operation::Lookup(op) => self.do_lookup(writer, op).await?,
            Operation::Getattr(op) => self.do_getattr(writer, op).await?,
            Operation::Setattr(op) => self.do_setattr(writer, op).await?,
            Operation::Read(op) => self.do_read(writer, op).await?,
            Operation::Write(op) => self.do_write(reader, writer, op).await?,
            Operation::Readdir(op) => self.do_readdir(writer, op).await?,
            Operation::Mknod(op) => self.do_mknod(writer, op, uid, gid).await?,
            Operation::Mkdir(op) => self.do_mkdir(writer, op, uid, gid).await?,
            Operation::Unlink(op) => match self.inodes.remove(op.parent(), op.name()).await {
                Ok(()) => op.reply(writer).await?,
                Err(errno) => writer.reply_err(errno).await?,
            },
            Operation::Rmdir(op) => match self.inodes.remove(op.parent(), op.name()).await {
                Ok(()) => op.reply(writer).await?,
                Err(errno) => writer.reply_err(errno).await?,
            },

            _ => (),
        }

        Ok(())
    }
}
