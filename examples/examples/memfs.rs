#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

use crossbeam::atomic::AtomicCell;
use futures::io::AsyncReadExt;
use polyfuse::{
    op,
    reply::{ReplyAttr, ReplyEntry, ReplyWrite},
    DirEntry, FileAttr,
};
use polyfuse_examples::prelude::*;
use std::{
    any::TypeId,
    collections::hash_map::{Entry, HashMap},
    fs::Metadata,
    io,
    os::unix::fs::MetadataExt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Weak,
    },
    time::SystemTime,
};
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mountpoint = examples::get_mountpoint()?;
    anyhow::ensure!(mountpoint.is_dir(), "the mountpoint must be a directory");

    let metadata = tokio::fs::metadata(&mountpoint).await?;
    let memfs = MemFS {
        inodes: INodeTable::new(&metadata),
        ttl_entry: Duration::from_secs(u64::max_value()),
        ttl_attr: Duration::from_secs(u64::max_value()),
    };

    polyfuse_tokio::mount(memfs, mountpoint, &[]).await?;

    Ok(())
}

struct MemFS {
    inodes: INodeTable,
    ttl_entry: Duration,
    ttl_attr: Duration,
}

impl MemFS {
    fn make_attr(&self, uid: u32, gid: u32, mode: u32) -> FileAttr {
        let now = SystemTime::now();

        let mut attr = FileAttr::default();
        attr.set_nlink(1);
        attr.set_mode(mode);
        attr.set_uid(uid);
        attr.set_gid(gid);
        attr.set_atime(now);
        attr.set_mtime(now);
        attr.set_ctime(now);
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
                op.reply(cx, {
                    ReplyEntry::default()
                        .ino(attr.ino())
                        .attr(attr)
                        .ttl_entry(self.ttl_entry)
                        .ttl_attr(self.ttl_attr)
                })
                .await
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
        op.reply(cx, {
            ReplyAttr::new(attr) //
                .ttl_attr(self.ttl_attr)
        })
        .await
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
        if let Some(atime) = op.atime() {
            attr.set_atime(atime);
        }
        if let Some(mtime) = op.mtime() {
            attr.set_mtime(mtime);
        }
        if let Some(ctime) = op.ctime() {
            attr.set_ctime(ctime);
        }

        inode.store_attr(attr);

        op.reply(cx, {
            ReplyAttr::new(attr) //
                .ttl_attr(self.ttl_attr)
        })
        .await
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
                        op.reply(cx, {
                            ReplyEntry::default()
                                .ino(attr.ino())
                                .attr(attr)
                                .ttl_entry(self.ttl_entry)
                                .ttl_attr(self.ttl_attr)
                        })
                        .await
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
                op.reply(cx, {
                    ReplyEntry::default()
                        .ino(attr.ino())
                        .attr(attr)
                        .ttl_entry(self.ttl_entry)
                        .ttl_attr(self.ttl_attr)
                })
                .await
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

// ==== INodeTable ====

struct INodeTable {
    inodes: Mutex<HashMap<u64, Arc<dyn INode>>>,
    next_id: AtomicU64,
}

impl INodeTable {
    fn new(metadata: &Metadata) -> Self {
        let mut attr = FileAttr::default();
        attr.set_ino(1);
        attr.set_nlink(2);
        attr.set_mode(metadata.mode());
        attr.set_uid(metadata.uid());
        attr.set_gid(metadata.gid());
        attr.set_atime_raw((metadata.atime() as u64, metadata.atime_nsec() as u32));
        attr.set_mtime_raw((metadata.mtime() as u64, metadata.mtime_nsec() as u32));
        attr.set_ctime_raw((metadata.ctime() as u64, metadata.ctime_nsec() as u32));

        let mut inodes = HashMap::new();
        inodes.insert(1, Arc::new(Directory::new(1, attr, None)) as Arc<dyn INode>);

        Self {
            inodes: Mutex::new(inodes),
            next_id: AtomicU64::new(2), // ino=1 is used by the root inode.
        }
    }

    async fn get(&self, ino: u64) -> Option<Arc<dyn INode>> {
        let inodes = self.inodes.lock().await;
        inodes.get(&ino).cloned()
    }

    async fn lookup(&self, parent: u64, name: &OsStr) -> Option<Arc<dyn INode>> {
        let parent = self.get(parent).await?;
        match parent.downcast_ref::<Directory>() {
            Some(parent) => parent.get(name).await?.upgrade(),
            None => None,
        }
    }

    async fn insert_file(
        &self,
        parent: u64,
        name: &OsStr,
        attr: FileAttr,
        data: Vec<u8>,
    ) -> Result<Arc<dyn INode>, libc::c_int> {
        let mut inodes = self.inodes.lock().await;

        let parent = inodes.get(&parent).ok_or_else(|| libc::ENOENT)?;
        let parent = parent
            .downcast_ref::<Directory>()
            .ok_or_else(|| libc::ENOTDIR)?;

        let ino = self.next_id.load(Ordering::SeqCst);
        let inode: Arc<dyn INode> = Arc::new(File::new(ino, attr, data));
        parent.insert(name, Arc::downgrade(&inode)).await?;
        inodes.insert(ino, inode.clone());
        self.next_id.fetch_add(1, Ordering::SeqCst);

        Ok(inode)
    }

    async fn insert_dir(
        &self,
        parent: u64,
        name: &OsStr,
        attr: FileAttr,
    ) -> Result<Arc<dyn INode>, libc::c_int> {
        let mut inodes = self.inodes.lock().await;

        let parent = inodes.get(&parent).ok_or_else(|| libc::ENOENT)?;
        let parent_handle = Arc::downgrade(parent);

        let parent = parent
            .downcast_ref::<Directory>()
            .ok_or_else(|| libc::ENOTDIR)?;

        let ino = self.next_id.load(Ordering::SeqCst);
        let inode: Arc<dyn INode> = Arc::new(Directory::new(ino, attr, Some(parent_handle)));
        parent.insert(name, Arc::downgrade(&inode)).await?;
        inodes.insert(ino, inode.clone());
        self.next_id.fetch_add(1, Ordering::SeqCst);

        Ok(inode)
    }

    async fn remove(&self, parent: u64, name: &OsStr) -> Result<(), libc::c_int> {
        let parent = self.get(parent).await.ok_or_else(|| libc::ENOENT)?;
        match parent.downcast_ref::<Directory>() {
            Some(parent) => {
                let inode = parent.remove(name).await?.upgrade().unwrap();

                let mut inodes = self.inodes.lock().await;
                if let Entry::Occupied(entry) = inodes.entry(inode.ino()) {
                    drop(entry.remove());
                }

                Ok(())
            }
            _ => Err(libc::ENOTDIR),
        }
    }
}

// ==== INode ====

trait INode: Send + Sync + 'static {
    fn ino(&self) -> u64;
    fn load_attr(&self) -> FileAttr;
    fn store_attr(&self, attr: FileAttr);

    #[doc(hidden)] // private API
    fn __private_type_id__(&self) -> TypeId {
        TypeId::of::<Self>()
    }
}

impl dyn INode {
    fn downcast_ref<T: INode>(&self) -> Option<&T> {
        if self.__private_type_id__() == TypeId::of::<T>() {
            Some(unsafe { &*(self as *const dyn INode as *const T) })
        } else {
            None
        }
    }
}

struct Directory {
    ino: u64,
    attr: AtomicCell<FileAttr>,
    parent: Option<Weak<dyn INode>>,
    children: Mutex<HashMap<OsString, Weak<dyn INode>>>,
}

impl INode for Directory {
    fn ino(&self) -> u64 {
        self.ino
    }

    fn load_attr(&self) -> FileAttr {
        self.attr.load()
    }

    fn store_attr(&self, attr: FileAttr) {
        self.attr.store(attr);
    }
}

impl Directory {
    fn new(ino: u64, mut attr: FileAttr, parent: Option<Weak<dyn INode>>) -> Self {
        attr.set_ino(ino);
        attr.set_mode((attr.mode() & libc::S_IFMT) | libc::S_IFDIR);

        Self {
            ino,
            attr: AtomicCell::new(attr),
            parent,
            children: Mutex::new(HashMap::new()),
        }
    }

    async fn is_empty(&self) -> bool {
        self.children.lock().await.is_empty()
    }

    async fn get(&self, name: &OsStr) -> Option<Weak<dyn INode>> {
        self.children.lock().await.get(name).cloned()
    }

    async fn insert(&self, name: &OsStr, inode: Weak<dyn INode>) -> Result<(), libc::c_int> {
        let mut children = self.children.lock().await;
        match children.entry(name.into()) {
            Entry::Occupied(..) => Err(libc::EEXIST),
            Entry::Vacant(entry) => {
                entry.insert(inode);
                Ok(())
            }
        }
    }

    async fn remove(&self, name: &OsStr) -> Result<Weak<dyn INode>, libc::c_int> {
        let mut children = self.children.lock().await;
        match children.entry(name.into()) {
            Entry::Occupied(entry) => {
                let inode = entry.get().upgrade().unwrap();
                if let Some(dir) = inode.downcast_ref::<Directory>() {
                    if !dir.is_empty().await {
                        return Err(libc::ENOTEMPTY);
                    }
                }

                Ok(entry.remove())
            }
            Entry::Vacant(..) => Err(libc::ENOENT),
        }
    }

    async fn entries(&self) -> Vec<DirEntry> {
        let mut entries = vec![DirEntry::dir(".", self.ino, 1)];
        let mut offset = 2;

        if let Some(ref parent) = self.parent {
            let parent = parent.upgrade().unwrap();
            entries.push(DirEntry::dir("..", parent.ino(), 2));
            offset += 1;
        }

        let children = self.children.lock().await;
        entries.extend(children.iter().enumerate().map(|(i, (name, inode))| {
            let ino = inode.upgrade().unwrap().ino();
            DirEntry::new(name, ino, i as u64 + offset)
        }));

        entries
    }
}

#[derive(Debug)]
struct File {
    ino: u64,
    attr: AtomicCell<FileAttr>,
    data: Mutex<Vec<u8>>,
}

impl INode for File {
    fn ino(&self) -> u64 {
        self.ino
    }

    fn load_attr(&self) -> FileAttr {
        self.attr.load()
    }

    fn store_attr(&self, attr: FileAttr) {
        self.attr.store(attr);
    }
}

impl File {
    fn new(ino: u64, mut attr: FileAttr, data: Vec<u8>) -> Self {
        attr.set_ino(ino);
        attr.set_mode((attr.mode() & libc::S_IFMT) | libc::S_IFREG);
        attr.set_size(data.len() as u64);

        Self {
            ino,
            attr: AtomicCell::new(attr),
            data: Mutex::new(data),
        }
    }

    async fn read(&self, offset: usize, bufsize: usize) -> Option<Vec<u8>> {
        let data = self.data.lock().await;

        if offset >= data.len() {
            return None;
        }

        let data = &data[offset..];
        Some(data[..std::cmp::min(data.len(), bufsize)].to_vec())
    }

    async fn write(&self, offset: usize, data: &[u8]) {
        let mut orig_data = self.data.lock().await;

        orig_data.resize(offset + data.len(), 0);

        let out = &mut orig_data[offset..offset + data.len()];
        out.copy_from_slice(data);
    }
}
