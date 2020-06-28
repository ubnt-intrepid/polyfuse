#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

use polyfuse::{
    io::{Reader, Writer},
    op,
    reply::{Collector, Reply, ReplyAttr, ReplyEntry, ReplyOpen, ReplyWrite, ReplyXattr},
    Context, DirEntry, FileAttr, Filesystem, Forget, Operation,
};
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap},
    ffi::{OsStr, OsString},
    fmt::Debug,
    io,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use tracing_futures::Instrument;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args
        .free_from_str()?
        .ok_or_else(|| anyhow::anyhow!("missing mountpoint"))?;
    anyhow::ensure!(mountpoint.is_dir(), "the mountpoint must be a directory");

    let memfs = MemFS::new();
    polyfuse_tokio::mount(memfs, mountpoint, &[]).await?;

    Ok(())
}

type Ino = u64;

struct INodeTable {
    map: HashMap<Ino, Arc<Mutex<INode>>>,
    next_ino: Ino,
}

impl INodeTable {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            next_ino: 1, // a
        }
    }

    fn vacant_entry(&mut self) -> VacantEntry<'_> {
        let ino = self.next_ino;
        VacantEntry { table: self, ino }
    }

    fn get(&self, ino: Ino) -> Option<Arc<Mutex<INode>>> {
        self.map.get(&ino).cloned()
    }

    fn remove(&mut self, ino: Ino) -> Option<Arc<Mutex<INode>>> {
        self.map.remove(&ino)
    }
}

struct VacantEntry<'a> {
    table: &'a mut INodeTable,
    ino: u64,
}

impl VacantEntry<'_> {
    fn ino(&self) -> Ino {
        self.ino
    }

    fn insert(self, inode: INode) {
        let Self { table, ino } = self;
        table.map.insert(ino, Arc::new(Mutex::new(inode)));
        table.next_ino += 1;
    }
}

struct INode {
    attr: FileAttr,
    xattrs: HashMap<OsString, Arc<Vec<u8>>>,
    refcount: u64,
    links: u64,
    kind: INodeKind,
}

enum INodeKind {
    RegularFile(Vec<u8>),
    Directory(Directory),
    Symlink(Arc<OsString>),
}

struct Directory {
    children: HashMap<OsString, Ino>,
    parent: Option<Ino>,
}

impl Directory {
    fn collect_entries(&self, attr: &FileAttr) -> Vec<Arc<DirEntry>> {
        let mut entries = Vec::with_capacity(self.children.len() + 2);
        let mut offset: u64 = 1;

        entries.push(Arc::new(DirEntry::dir(".", attr.ino(), offset)));
        offset += 1;

        entries.push(Arc::new(DirEntry::dir(
            "..",
            self.parent.unwrap_or_else(|| attr.ino()),
            offset,
        )));
        offset += 1;

        for (name, &ino) in &self.children {
            entries.push(Arc::new(DirEntry::new(name, ino, offset)));
            offset += 1;
        }

        entries
    }
}

struct DirHandle {
    entries: Vec<Arc<DirEntry>>,
}

struct MemFS {
    inodes: Mutex<INodeTable>,
    ttl: Duration,
    dir_handles: Mutex<Slab<Arc<Mutex<DirHandle>>>>,
}

impl MemFS {
    fn new() -> Self {
        let mut inodes = INodeTable::new();
        inodes.vacant_entry().insert(INode {
            attr: {
                let mut attr = FileAttr::default();
                attr.set_ino(1);
                attr.set_nlink(2);
                attr.set_mode(libc::S_IFDIR | 0o755);
                attr
            },
            xattrs: HashMap::new(),
            refcount: u64::max_value() / 2,
            links: u64::max_value() / 2,
            kind: INodeKind::Directory(Directory {
                children: HashMap::new(),
                parent: None,
            }),
        });
        Self {
            inodes: Mutex::new(inodes),
            dir_handles: Mutex::default(),
            ttl: Duration::from_secs(60 * 60 * 24),
        }
    }

    fn make_entry_reply(&self, ino: Ino, attr: FileAttr) -> ReplyEntry {
        let mut reply = ReplyEntry::default();
        reply.ino(ino);
        reply.attr(attr);
        reply.ttl_entry(self.ttl);
        reply
    }

    async fn lookup_inode(&self, parent: Ino, name: &OsStr) -> io::Result<ReplyEntry> {
        let inodes = self.inodes.lock().await;

        let parent = inodes.get(parent).ok_or_else(no_entry)?;
        let parent = parent.lock().await;

        let parent = match parent.kind {
            INodeKind::Directory(ref dir) => dir,
            _ => return Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
        };

        let child_ino = parent.children.get(&*name).copied().ok_or_else(no_entry)?;
        let child = inodes.get(child_ino).unwrap_or_else(|| unreachable!());
        let mut child = child.lock().await;
        child.refcount += 1;

        Ok(self.make_entry_reply(child_ino, child.attr))
    }

    async fn make_node<F>(&self, parent: Ino, name: &OsStr, f: F) -> io::Result<ReplyEntry>
    where
        F: FnOnce(&VacantEntry<'_>) -> INode,
    {
        let mut inodes = self.inodes.lock().await;

        let parent = inodes.get(parent).ok_or_else(no_entry)?;
        let mut parent = parent.lock().await;
        let parent = match parent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
        };

        match parent.children.entry(name.into()) {
            Entry::Occupied(..) => Err(io::Error::from_raw_os_error(libc::EEXIST)),
            Entry::Vacant(map_entry) => {
                let inode_entry = inodes.vacant_entry();
                let inode = f(&inode_entry);

                let reply = self.make_entry_reply(inode_entry.ino(), inode.attr);
                map_entry.insert(inode_entry.ino());
                inode_entry.insert(inode);

                Ok(reply)
            }
        }
    }

    async fn link_node(&self, ino: u64, newparent: Ino, newname: &OsStr) -> io::Result<ReplyEntry> {
        {
            let inodes = self.inodes.lock().await;

            let inode = inodes.get(ino).ok_or_else(no_entry)?;

            let newparent = inodes.get(newparent).ok_or_else(no_entry)?;
            let mut newparent = newparent.lock().await;
            let newparent = match newparent.kind {
                INodeKind::Directory(ref mut dir) => dir,
                _ => return Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
            };

            match newparent.children.entry(newname.into()) {
                Entry::Occupied(..) => return Err(io::Error::from_raw_os_error(libc::EEXIST)),
                Entry::Vacant(entry) => {
                    entry.insert(ino);
                    let mut inode = inode.lock().await;
                    let inode = &mut *inode;
                    inode.links += 1;
                    inode.attr.set_nlink(inode.attr.nlink() + 1);
                }
            }
        }

        self.lookup_inode(newparent, newname).await
    }

    async fn unlink_node(&self, parent: Ino, name: &OsStr) -> io::Result<()> {
        let inodes = self.inodes.lock().await;

        let parent = inodes.get(parent).ok_or_else(no_entry)?;
        let mut parent = parent.lock().await;
        let parent = match parent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
        };

        let &ino = parent.children.get(name).ok_or_else(no_entry)?;

        let inode = inodes.get(ino).unwrap_or_else(|| unreachable!());
        let mut inode = inode.lock().await;
        let inode = &mut *inode;

        match inode.kind {
            INodeKind::Directory(ref dir) if !dir.children.is_empty() => {
                return Err(io::Error::from_raw_os_error(libc::ENOTEMPTY));
            }
            _ => (),
        }

        parent
            .children
            .remove(name)
            .unwrap_or_else(|| unreachable!());

        inode.links = inode.links.saturating_sub(1);
        inode.attr.set_nlink(inode.attr.nlink().saturating_sub(1));

        Ok(())
    }

    async fn rename_node(&self, parent: Ino, name: &OsStr, newname: &OsStr) -> io::Result<()> {
        let inodes = self.inodes.lock().await;

        let parent = inodes.get(parent).ok_or_else(no_entry)?;
        let mut parent = parent.lock().await;
        let parent = match parent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
        };

        let &ino = parent.children.get(name).ok_or_else(no_entry)?;

        match parent.children.entry(newname.into()) {
            Entry::Occupied(..) => Err(io::Error::from_raw_os_error(libc::EEXIST)),
            Entry::Vacant(entry) => {
                entry.insert(ino);
                parent
                    .children
                    .remove(name)
                    .unwrap_or_else(|| unreachable!());
                Ok(())
            }
        }
    }

    async fn graft_node(
        &self,
        parent: Ino,
        name: &OsStr,
        newparent: Ino,
        newname: &OsStr,
    ) -> io::Result<()> {
        debug_assert_ne!(parent, newparent);

        let inodes = self.inodes.lock().await;

        let parent = inodes.get(parent).ok_or_else(no_entry)?;
        let mut parent = parent.lock().await;
        let parent = match parent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
        };

        let newparent = inodes.get(newparent).ok_or_else(no_entry)?;
        let mut newparent = newparent.lock().await;
        let newparent = match newparent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
        };

        match newparent.children.entry(newname.into()) {
            Entry::Occupied(..) => Err(io::Error::from_raw_os_error(libc::EEXIST)),
            Entry::Vacant(entry) => {
                let ino = parent.children.remove(name).ok_or_else(no_entry)?;
                entry.insert(ino);
                Ok(())
            }
        }
    }

    async fn do_lookup(&self, op: &op::Lookup<'_>) -> io::Result<ReplyEntry> {
        self.lookup_inode(op.parent(), op.name()).await
    }

    async fn do_forget(&self, forgets: &[Forget]) {
        let mut inodes = self.inodes.lock().await;

        for forget in forgets {
            if let Some(inode) = inodes.get(forget.ino()) {
                let mut inode = inode.lock().await;
                inode.refcount = inode.refcount.saturating_sub(forget.nlookup());
                if inode.refcount == 0 && inode.links == 0 {
                    inodes.remove(forget.ino());
                }
            }
        }
    }

    async fn do_getattr(&self, op: &op::Getattr<'_>) -> io::Result<ReplyAttr> {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        let mut reply = ReplyAttr::new(inode.attr);
        reply.ttl_attr(self.ttl);

        Ok(reply)
    }

    async fn do_setattr(&self, op: &op::Setattr<'_>) -> io::Result<ReplyAttr> {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let mut inode = inode.lock().await;

        if let Some(mode) = op.mode() {
            inode.attr.set_mode(mode);
        }
        if let Some(uid) = op.uid() {
            inode.attr.set_uid(uid);
        }
        if let Some(gid) = op.gid() {
            inode.attr.set_gid(gid);
        }
        if let Some(size) = op.size() {
            inode.attr.set_size(size);
        }
        if let Some(atime) = op.atime() {
            inode.attr.set_atime(atime);
        }
        if let Some(mtime) = op.mtime() {
            inode.attr.set_mtime(mtime);
        }
        if let Some(ctime) = op.ctime() {
            inode.attr.set_ctime(ctime);
        }

        let mut reply = ReplyAttr::new(inode.attr);
        reply.ttl_attr(self.ttl);

        Ok(reply)
    }

    async fn do_readlink(&self, op: &op::Readlink<'_>) -> io::Result<Arc<OsString>> {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        match inode.kind {
            INodeKind::Symlink(ref link) => Ok(link.clone()),
            _ => Err(io::Error::from_raw_os_error(libc::EINVAL)),
        }
    }

    async fn do_opendir(&self, op: &op::Opendir<'_>) -> io::Result<ReplyOpen> {
        let inodes = self.inodes.lock().await;
        let mut dirs = self.dir_handles.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;
        if inode.attr.nlink() == 0 {
            return Err(no_entry());
        }
        let dir = match inode.kind {
            INodeKind::Directory(ref dir) => dir,
            _ => return Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
        };

        let key = dirs.insert(Arc::new(Mutex::new(DirHandle {
            entries: dir.collect_entries(&inode.attr),
        })));

        Ok(ReplyOpen::new(key as u64))
    }

    async fn do_readdir(&self, op: &op::Readdir<'_>) -> io::Result<impl Reply + Debug> {
        let dirs = self.dir_handles.lock().await;

        let dir = dirs
            .get(op.fh() as usize)
            .cloned()
            .ok_or_else(unknown_error)?;
        let dir = dir.lock().await;

        let mut total_len = 0;
        let entries: Vec<_> = dir
            .entries
            .iter()
            .skip(op.offset() as usize)
            .take_while(|entry| {
                let entry: &DirEntry = &*entry;
                total_len += entry.as_ref().len() as u32;
                total_len < op.size()
            })
            .cloned()
            .collect();

        Ok(entries)
    }

    async fn do_releasedir(&self, op: &op::Releasedir<'_>) -> io::Result<()> {
        let mut dirs = self.dir_handles.lock().await;

        let dir = dirs.remove(op.fh() as usize);
        drop(dir);

        Ok(())
    }

    async fn do_mknod(&self, op: &op::Mknod<'_>) -> io::Result<ReplyEntry> {
        match op.mode() & libc::S_IFMT {
            libc::S_IFREG => (),
            _ => return Err(io::Error::from_raw_os_error(libc::ENOTSUP)),
        }

        self.make_node(op.parent(), op.name(), |entry| INode {
            attr: {
                let mut attr = FileAttr::default();
                attr.set_ino(entry.ino());
                attr.set_nlink(1);
                attr.set_mode(op.mode());
                attr
            },
            xattrs: HashMap::new(),
            refcount: 1,
            links: 1,
            kind: INodeKind::RegularFile(vec![]),
        })
        .await
    }

    async fn do_mkdir(&self, op: &op::Mkdir<'_>) -> io::Result<ReplyEntry> {
        self.make_node(op.parent(), op.name(), |entry| INode {
            attr: {
                let mut attr = FileAttr::default();
                attr.set_ino(entry.ino());
                attr.set_nlink(2);
                attr.set_mode(op.mode() | libc::S_IFDIR);
                attr
            },
            xattrs: HashMap::new(),
            refcount: 1,
            links: 1,
            kind: INodeKind::Directory(Directory {
                children: HashMap::new(),
                parent: Some(op.parent()),
            }),
        })
        .await
    }

    async fn do_symlink(&self, op: &op::Symlink<'_>) -> io::Result<ReplyEntry> {
        self.make_node(op.parent(), op.name(), |entry| INode {
            attr: {
                let mut attr = FileAttr::default();
                attr.set_ino(entry.ino());
                attr.set_nlink(1);
                attr.set_mode(libc::S_IFLNK | 0o777);
                attr
            },
            xattrs: HashMap::new(),
            refcount: 1,
            links: 1,
            kind: INodeKind::Symlink(Arc::new(op.link().into())),
        })
        .await
    }

    async fn do_link(&self, op: &op::Link<'_>) -> io::Result<ReplyEntry> {
        self.link_node(op.ino(), op.newparent(), op.newname()).await
    }

    async fn do_unlink(&self, op: &op::Unlink<'_>) -> io::Result<()> {
        self.unlink_node(op.parent(), op.name()).await
    }

    async fn do_rmdir(&self, op: &op::Rmdir<'_>) -> io::Result<()> {
        self.unlink_node(op.parent(), op.name()).await
    }

    async fn do_rename(&self, op: &op::Rename<'_>) -> io::Result<()> {
        if op.flags() != 0 {
            // TODO: handle RENAME_NOREPLACE and RENAME_EXCHANGE.
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }

        match (op.parent(), op.newparent()) {
            (parent, newparent) if parent == newparent => {
                self.rename_node(parent, op.name(), op.newname()).await
            }
            (parent, newparent) => {
                self.graft_node(parent, op.name(), newparent, op.newname())
                    .await
            }
        }
    }

    async fn do_getxattr(&self, op: &op::Getxattr<'_>) -> io::Result<impl Reply + Debug> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        let value = inode
            .xattrs
            .get(op.name())
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENODATA))?;

        match op.size() {
            0 => Ok(Either::Left(ReplyXattr::new(value.len() as u32))),
            size => {
                if value.len() as u32 > size {
                    return Err(io::Error::from_raw_os_error(libc::ERANGE));
                }
                Ok(Either::Right(value.clone()))
            }
        }
    }

    async fn do_setxattr(&self, op: &op::Setxattr<'_>) -> io::Result<()> {
        let create = op.flags() as i32 & libc::XATTR_CREATE != 0;
        let replace = op.flags() as i32 & libc::XATTR_REPLACE != 0;
        if create && replace {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }

        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let mut inode = inode.lock().await;

        match inode.xattrs.entry(op.name().into()) {
            Entry::Occupied(..) if create => Err(io::Error::from_raw_os_error(libc::EEXIST)),
            Entry::Occupied(entry) => {
                let value = Arc::make_mut(entry.into_mut());
                *value = op.value().into();
                Ok(())
            }
            Entry::Vacant(..) if replace => Err(io::Error::from_raw_os_error(libc::ENODATA)),
            Entry::Vacant(entry) => {
                if create {
                    entry.insert(Arc::default());
                } else {
                    entry.insert(Arc::new(op.value().into()));
                }
                Ok(())
            }
        }
    }

    async fn do_listxattr(&self, op: &op::Listxattr<'_>) -> io::Result<impl Reply + Debug> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        match op.size() {
            0 => {
                let total_len = inode.xattrs.keys().map(|name| name.len() as u32 + 1).sum();
                Ok(Either::Left(ReplyXattr::new(total_len)))
            }
            size => {
                let mut total_len = 0;
                let names = inode.xattrs.keys().fold(OsString::new(), |mut acc, name| {
                    acc.push(name);
                    acc.push("\0");
                    total_len += name.len() as u32 + 1;
                    acc
                });

                if total_len > size {
                    return Err(io::Error::from_raw_os_error(libc::ERANGE));
                }

                Ok(Either::Right(names))
            }
        }
    }

    async fn do_removexattr(&self, op: &op::Removexattr<'_>) -> io::Result<()> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let mut inode = inode.lock().await;

        match inode.xattrs.entry(op.name().into()) {
            Entry::Occupied(entry) => {
                entry.remove();
                Ok(())
            }
            Entry::Vacant(..) => Err(io::Error::from_raw_os_error(libc::ENODATA)),
        }
    }

    async fn do_read(&self, op: &op::Read<'_>) -> io::Result<impl Reply + Debug> {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        let content = match inode.kind {
            INodeKind::RegularFile(ref content) => content,
            _ => return Err(io::Error::from_raw_os_error(libc::EINVAL)),
        };

        let offset = op.offset() as usize;
        let size = op.size() as usize;

        let content = content.get(offset..).unwrap_or(&[]);
        let content = &content[..std::cmp::min(content.len(), size)];

        Ok(content.to_vec())
    }

    async fn do_write<R: ?Sized>(
        &self,
        op: &op::Write<'_>,
        reader: &mut R,
    ) -> io::Result<ReplyWrite>
    where
        R: Reader + Unpin,
    {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let mut inode = inode.lock().await;
        let inode = &mut *inode;

        let content = match inode.kind {
            INodeKind::RegularFile(ref mut content) => content,
            _ => return Err(io::Error::from_raw_os_error(libc::EINVAL)),
        };

        let offset = op.offset() as usize;
        let size = op.size() as usize;

        content.resize(std::cmp::max(content.len(), offset + size), 0);

        use futures::io::AsyncReadExt;
        reader
            .read_exact(&mut content[offset..offset + size])
            .await?;

        inode.attr.set_size(content.len() as u64);

        Ok(ReplyWrite::new(op.size()))
    }
}

#[polyfuse::async_trait]
impl Filesystem for MemFS {
    #[allow(clippy::cognitive_complexity)]
    async fn call<'a, 'cx, T: ?Sized>(
        &'a self,
        cx: &'a mut Context<'cx, T>,
        op: Operation<'cx>,
    ) -> io::Result<()>
    where
        T: Reader + Writer + Send + Unpin,
    {
        let span = tracing::debug_span!("MemFS::call", unique = cx.unique());
        span.in_scope(|| tracing::debug!(?op));

        macro_rules! try_reply {
            ($e:expr) => {
                match ($e).instrument(span.clone()).await {
                    Ok(reply) => {
                        span.in_scope(|| tracing::debug!(reply=?reply));
                        cx.reply(reply).await
                    }
                    Err(err) => {
                        let errno = err.raw_os_error().unwrap_or(libc::EIO);
                        span.in_scope(|| tracing::debug!(errno=errno));
                        cx.reply_err(errno).await
                    }
                }
            };
        }

        match op {
            Operation::Lookup(op) => try_reply!(self.do_lookup(&op)),
            Operation::Forget(forgets) => {
                self.do_forget(forgets.as_ref()).await;
                Ok(())
            }
            Operation::Getattr(op) => try_reply!(self.do_getattr(&op)),
            Operation::Setattr(op) => try_reply!(self.do_setattr(&op)),
            Operation::Readlink(op) => try_reply!(self.do_readlink(&op)),

            Operation::Opendir(op) => try_reply!(self.do_opendir(&op)),
            Operation::Readdir(op) => try_reply!(self.do_readdir(&op)),
            Operation::Releasedir(op) => try_reply!(self.do_releasedir(&op)),

            Operation::Mknod(op) => try_reply!(self.do_mknod(&op)),
            Operation::Mkdir(op) => try_reply!(self.do_mkdir(&op)),
            Operation::Symlink(op) => try_reply!(self.do_symlink(&op)),
            Operation::Unlink(op) => try_reply!(self.do_unlink(&op)),
            Operation::Rmdir(op) => try_reply!(self.do_rmdir(&op)),
            Operation::Link(op) => try_reply!(self.do_link(&op)),
            Operation::Rename(op) => try_reply!(self.do_rename(&op)),

            Operation::Getxattr(op) => try_reply!(self.do_getxattr(&op)),
            Operation::Setxattr(op) => try_reply!(self.do_setxattr(&op)),
            Operation::Listxattr(op) => try_reply!(self.do_listxattr(&op)),
            Operation::Removexattr(op) => try_reply!(self.do_removexattr(&op)),

            Operation::Read(op) => try_reply!(self.do_read(&op)),
            Operation::Write(op) => {
                let res = self
                    .do_write(&op, &mut cx.reader())
                    .instrument(span.clone())
                    .await;
                try_reply!(async { res })
            }

            _ => {
                span.in_scope(|| tracing::debug!("NOSYS"));
                Ok(())
            }
        }
    }
}

fn no_entry() -> io::Error {
    io::Error::from_raw_os_error(libc::ENOENT)
}

fn unknown_error() -> io::Error {
    io::Error::from_raw_os_error(libc::EIO)
}

// FIXME: use either crate.
#[derive(Debug)]
enum Either<L, R> {
    Left(L),
    Right(R),
}

impl<L, R> Reply for Either<L, R>
where
    L: Reply,
    R: Reply,
{
    #[inline]
    fn collect_bytes<'a, C: ?Sized>(&'a self, collector: &mut C)
    where
        C: Collector<'a>,
    {
        match self {
            Either::Left(l) => l.collect_bytes(collector),
            Either::Right(r) => r.collect_bytes(collector),
        }
    }
}
