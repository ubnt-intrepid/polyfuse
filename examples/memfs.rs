#![allow(clippy::unnecessary_mut_passed, clippy::rc_buffer)]
#![deny(clippy::unimplemented)]
#![forbid(unsafe_code)]

use polyfuse::{
    fs::{
        self,
        reply::{
            self, ReplyAttr, ReplyData, ReplyDir, ReplyEntry, ReplyOpen, ReplyUnit, ReplyWrite,
            ReplyXattr,
        },
        Filesystem,
    },
    mount::MountOptions,
    op::{self, SetxattrFlags},
    types::{FileAttr, FileID, FileMode, FilePermissions, FileType, NodeID},
    KernelConfig,
};

use anyhow::{ensure, Context as _, Result};
use dashmap::DashMap;
use libc::{EEXIST, EINVAL, ENODATA, ENOENT, ENOSYS, ENOTDIR, ENOTEMPTY, ENOTSUP, ERANGE};
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap},
    ffi::{OsStr, OsString},
    io::prelude::*,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_dir(), "mountpoint must be a directory");

    polyfuse::fs::run(
        MemFS::new(),
        mountpoint,
        MountOptions::default(),
        KernelConfig::default(),
    )
    .await?;

    Ok(())
}

struct INodeTable {
    map: DashMap<NodeID, INode>,
    next_ino: AtomicU64,
}

impl INodeTable {
    fn new() -> Self {
        Self {
            map: DashMap::new(),
            next_ino: AtomicU64::new(1), // ino=1 is reserved by root node
        }
    }

    fn get(&self, ino: NodeID) -> Option<INodeRef<'_>> {
        self.map.get(&ino)
    }

    fn get_mut(&self, ino: NodeID) -> Option<INodeRefMut<'_>> {
        self.map.get_mut(&ino)
    }

    fn occupied_entry(&self, ino: NodeID) -> Option<OccupiedEntry<'_>> {
        match self.map.entry(ino) {
            dashmap::mapref::entry::Entry::Occupied(entry) => Some(entry),
            dashmap::mapref::entry::Entry::Vacant(..) => None,
        }
    }

    fn vacant_entry(&self) -> Option<VacantEntry<'_>> {
        // TODO: choose appropriate atomic ordering.
        let ino = self.next_ino.fetch_add(1, Ordering::SeqCst);

        match self.map.entry(NodeID::from_raw(ino)) {
            dashmap::mapref::entry::Entry::Occupied(..) => None,
            dashmap::mapref::entry::Entry::Vacant(entry) => Some(entry),
        }
    }
}

type INodeRef<'a> = dashmap::mapref::one::Ref<'a, NodeID, INode>;
type INodeRefMut<'a> = dashmap::mapref::one::RefMut<'a, NodeID, INode>;
type OccupiedEntry<'a> = dashmap::mapref::entry::OccupiedEntry<'a, NodeID, INode>;
type VacantEntry<'a> = dashmap::mapref::entry::VacantEntry<'a, NodeID, INode>;

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

impl INode {
    fn as_file(&self) -> Option<&Vec<u8>> {
        match &self.kind {
            INodeKind::RegularFile(contents) => Some(contents),
            _ => None,
        }
    }

    fn as_file_mut(&mut self) -> Option<&mut Vec<u8>> {
        match &mut self.kind {
            INodeKind::RegularFile(contents) => Some(contents),
            _ => None,
        }
    }

    fn as_dir(&self) -> Option<&Directory> {
        match &self.kind {
            INodeKind::Directory(dir) => Some(dir),
            _ => None,
        }
    }

    fn as_dir_mut(&mut self) -> Option<&mut Directory> {
        match &mut self.kind {
            INodeKind::Directory(dir) => Some(dir),
            _ => None,
        }
    }

    fn as_symlink(&self) -> Option<&OsStr> {
        match &self.kind {
            INodeKind::Symlink(link) => Some(&**link),
            _ => None,
        }
    }
}

struct Directory {
    children: HashMap<OsString, NodeID>,
    parent: Option<NodeID>,
}

struct DirEntry {
    name: OsString,
    ino: NodeID,
    typ: Option<FileType>,
    off: u64,
}

impl Directory {
    fn collect_entries(&self, attr: &FileAttr) -> Vec<Arc<DirEntry>> {
        let mut entries = Vec::with_capacity(self.children.len() + 2);
        let mut offset: u64 = 1;

        entries.push(Arc::new(DirEntry {
            name: ".".into(),
            ino: attr.ino,
            typ: Some(FileType::Directory),
            off: offset,
        }));
        offset += 1;

        entries.push(Arc::new(DirEntry {
            name: "..".into(),
            ino: self.parent.unwrap_or(attr.ino),
            typ: Some(FileType::Directory),
            off: offset,
        }));
        offset += 1;

        for (name, &ino) in &self.children {
            entries.push(Arc::new(DirEntry {
                name: name.into(),
                ino,
                typ: None,
                off: offset,
            }));
            offset += 1;
        }

        entries
    }
}

struct DirHandle {
    entries: Vec<Arc<DirEntry>>,
    offset: AtomicUsize,
}

struct MemFS {
    inodes: INodeTable,
    dir_handles: Mutex<Slab<DirHandle>>,
    ttl: Duration,
}

impl MemFS {
    fn new() -> Self {
        let inodes = INodeTable::new();
        inodes.vacant_entry().unwrap().insert(INode {
            attr: {
                let mut attr = FileAttr::new();
                attr.ino = NodeID::ROOT;
                attr.nlink = 2;
                attr.mode = FileMode::new(
                    FileType::Directory,
                    FilePermissions::READ | FilePermissions::EXEC | FilePermissions::WRITE_USER,
                );
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
            inodes,
            dir_handles: Mutex::new(Slab::new()),
            ttl: Duration::from_secs(60 * 60 * 24),
        }
    }

    fn make_node<F>(
        &self,
        mut reply: ReplyEntry<'_>,
        parent: NodeID,
        name: &OsStr,
        f: F,
    ) -> reply::Result
    where
        F: FnOnce(&VacantEntry<'_>) -> INode,
    {
        let mut parent = self.inodes.get_mut(parent).ok_or(ENOENT)?;
        let parent = match parent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return Err(ENOTDIR.into()),
        };

        let map_entry = match parent.children.entry(name.into()) {
            Entry::Occupied(..) => return Err(EEXIST.into()),
            Entry::Vacant(map_entry) => map_entry,
        };
        let inode_entry = self.inodes.vacant_entry().expect("inode number conflict");
        let inode = f(&inode_entry);

        reply.out().ino(*inode_entry.key());
        reply.out().attr(inode.attr.clone());
        reply.out().ttl_entry(self.ttl);

        map_entry.insert(*inode_entry.key());
        inode_entry.insert(inode);

        reply.send()
    }

    fn unlink_node(&self, parent: NodeID, name: &OsStr) -> reply::Result<()> {
        let mut parent = self.inodes.get_mut(parent).ok_or(ENOENT)?;
        let parent = parent.as_dir_mut().ok_or(ENOTDIR)?;

        let ino = parent.children.get(name).copied().ok_or(ENOENT)?;

        let mut inode = self.inodes.get_mut(ino).unwrap_or_else(|| unreachable!());
        if let Some(dir) = inode.as_dir_mut() {
            if !dir.children.is_empty() {
                return Err(ENOTEMPTY.into());
            }
        }

        parent
            .children
            .remove(name)
            .expect("should not be panic here");

        inode.links = inode.links.saturating_sub(1);
        inode.attr.nlink = inode.attr.nlink.saturating_sub(1);

        Ok(())
    }
}

impl Filesystem for MemFS {
    async fn lookup(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Lookup<'_>,
        mut reply: ReplyEntry<'_>,
    ) -> reply::Result {
        let parent = self.inodes.get(op.parent()).ok_or(ENOENT)?;
        let parent = parent.as_dir().ok_or(ENOTDIR)?;

        let child_ino = parent.children.get(op.name()).copied().ok_or(ENOENT)?;
        let mut child = self
            .inodes
            .get_mut(child_ino)
            .expect("should not be panic here");
        child.refcount += 1;

        reply.out().ino(child_ino);
        reply.out().attr(child.attr.clone());
        reply.out().ttl_entry(self.ttl);
        reply.send()
    }

    async fn forget(self: &Arc<Self>, forgets: &[op::Forget]) {
        for forget in forgets {
            if let Some(mut inode) = self.inodes.occupied_entry(forget.ino()) {
                inode.get_mut().refcount =
                    inode.get_mut().refcount.saturating_sub(forget.nlookup());

                if inode.get().refcount == 0 && inode.get().links == 0 {
                    inode.remove();
                }
            }
        }
    }

    async fn getattr(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Getattr<'_>,
        mut reply: ReplyAttr<'_>,
    ) -> reply::Result {
        let inode = self.inodes.get(op.ino()).ok_or(ENOENT)?;

        reply.out().attr(inode.attr.clone());
        reply.out().ttl(self.ttl);
        reply.send()
    }

    async fn setattr(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Setattr<'_>,
        mut reply: ReplyAttr<'_>,
    ) -> reply::Result {
        let mut inode = self.inodes.get_mut(op.ino()).ok_or(ENOENT)?;

        fn to_duration(t: op::SetAttrTime) -> Duration {
            match t {
                op::SetAttrTime::Timespec(ts) => ts,
                _ => SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap(),
            }
        }

        if let Some(mode) = op.mode() {
            inode.attr.mode = mode;
        }
        if let Some(uid) = op.uid() {
            inode.attr.uid = uid;
        }
        if let Some(gid) = op.gid() {
            inode.attr.gid = gid;
        }
        if let Some(size) = op.size() {
            inode.attr.size = size;
        }
        if let Some(atime) = op.atime() {
            inode.attr.atime = to_duration(atime);
        }
        if let Some(mtime) = op.mtime() {
            inode.attr.mtime = to_duration(mtime);
        }
        if let Some(ctime) = op.ctime() {
            inode.attr.ctime = ctime;
        }

        reply.out().attr(inode.attr.clone());
        reply.out().ttl(self.ttl);
        reply.send()
    }

    async fn readlink(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Readlink<'_>,
        reply: ReplyData<'_>,
    ) -> reply::Result {
        let inode = self.inodes.get(op.ino()).ok_or(ENOENT)?;
        let link = inode.as_symlink().ok_or(EINVAL)?;
        reply.send(link)
    }

    async fn opendir(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Opendir<'_>,
        mut reply: ReplyOpen<'_>,
    ) -> reply::Result {
        let inode = self.inodes.get(op.ino()).ok_or(ENOENT)?;
        if inode.attr.nlink == 0 {
            return Err(ENOENT.into());
        }
        let dir = inode.as_dir().ok_or(ENOTDIR)?;

        let dir_handles = &mut *self.dir_handles.lock().await;
        let key = dir_handles.insert(DirHandle {
            entries: dir.collect_entries(&inode.attr),
            offset: AtomicUsize::new(0),
        });

        reply.out().fh(FileID::from_raw(key as u64));
        reply.send()
    }

    async fn readdir(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Readdir<'_>,
        mut reply: ReplyDir<'_>,
    ) -> reply::Result {
        if op.mode() == op::ReaddirMode::Plus {
            Err(ENOSYS)?;
        }

        let dir_handles = &mut *self.dir_handles.lock().await;
        let dir = dir_handles.get(op.fh().into_raw() as usize).ok_or(EINVAL)?;

        for entry in dir.entries.iter().skip(op.offset() as usize) {
            if reply.push_entry(&entry.name, entry.ino, entry.typ, entry.off) {
                break;
            }
            dir.offset.fetch_add(1, Ordering::SeqCst);
        }

        reply.send()
    }

    async fn releasedir(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Releasedir<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        let dir_handles = &mut *self.dir_handles.lock().await;
        dir_handles.remove(op.fh().into_raw() as usize);
        reply.send()
    }

    async fn mknod(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Mknod<'_>,
        reply: ReplyEntry<'_>,
    ) -> reply::Result {
        match op.mode().file_type() {
            Some(FileType::Regular) => (),
            _ => Err(ENOTSUP)?,
        }

        self.make_node(reply, op.parent(), op.name(), |entry| INode {
            attr: {
                let mut attr = FileAttr::new();
                attr.ino = *entry.key();
                attr.nlink = 1;
                attr.mode = op.mode();
                attr
            },
            xattrs: HashMap::new(),
            refcount: 1,
            links: 1,
            kind: INodeKind::RegularFile(vec![]),
        })
    }

    async fn mkdir(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Mkdir<'_>,
        reply: ReplyEntry<'_>,
    ) -> reply::Result {
        self.make_node(reply, op.parent(), op.name(), |entry| INode {
            attr: {
                let mut attr = FileAttr::new();
                attr.ino = *entry.key();
                attr.nlink = 2;
                attr.mode = FileMode::new(FileType::Directory, op.permissions());
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
    }

    async fn symlink(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Symlink<'_>,
        reply: ReplyEntry<'_>,
    ) -> reply::Result {
        self.make_node(reply, op.parent(), op.name(), |entry| INode {
            attr: {
                let mut attr = FileAttr::new();
                attr.ino = *entry.key();
                attr.nlink = 1;
                attr.mode = FileMode::new(
                    FileType::SymbolicLink,
                    FilePermissions::READ | FilePermissions::WRITE | FilePermissions::EXEC,
                );
                attr
            },
            xattrs: HashMap::new(),
            refcount: 1,
            links: 1,
            kind: INodeKind::Symlink(Arc::new(op.link().into())),
        })
    }

    async fn link(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Link<'_>,
        mut reply: ReplyEntry<'_>,
    ) -> reply::Result {
        let mut inode = self.inodes.get_mut(op.ino()).ok_or(ENOENT)?;

        debug_assert!(op.ino() != op.newparent());
        let mut newparent = self.inodes.get_mut(op.newparent()).ok_or(ENOENT)?;
        let newparent = newparent.as_dir_mut().ok_or(ENOTDIR)?;

        match newparent.children.entry(op.newname().into()) {
            Entry::Occupied(..) => return Err(EEXIST.into()),
            Entry::Vacant(entry) => {
                entry.insert(op.ino());
                inode.links += 1;
                inode.attr.nlink += 1;
                inode.refcount += 1;
            }
        }

        reply.out().ino(op.ino());
        reply.out().attr(inode.attr.clone());
        reply.out().ttl_entry(self.ttl);
        reply.send()
    }

    async fn unlink(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Unlink<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        self.unlink_node(op.parent(), op.name())?;
        reply.send()
    }

    async fn rmdir(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Rmdir<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        self.unlink_node(op.parent(), op.name())?;
        reply.send()
    }

    async fn rename(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Rename<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        if !op.flags().is_empty() {
            // TODO: handle RENAME_NOREPLACE and RENAME_EXCHANGE.
            Err(EINVAL)?;
        }

        let mut parent = self.inodes.get_mut(op.parent()).ok_or(ENOENT)?;
        let parent = parent.as_dir_mut().ok_or(ENOTDIR)?;

        match op.newparent() {
            newparent if newparent == op.parent() => {
                let ino = match parent.children.get(op.name()) {
                    Some(&ino) => ino,
                    None => return Err(ENOENT.into()),
                };

                match parent.children.entry(op.newname().into()) {
                    Entry::Occupied(..) => return Err(EEXIST.into()),
                    Entry::Vacant(entry) => {
                        entry.insert(ino);
                    }
                }
                parent
                    .children
                    .remove(op.name())
                    .unwrap_or_else(|| unreachable!());
            }

            newparent => {
                let mut newparent = self.inodes.get_mut(newparent).ok_or(ENOENT)?;
                let newparent = newparent.as_dir_mut().ok_or(ENOTDIR)?;

                let entry = match newparent.children.entry(op.newname().into()) {
                    Entry::Occupied(..) => return Err(EEXIST.into()),
                    Entry::Vacant(entry) => entry,
                };
                let ino = parent.children.remove(op.name()).ok_or(ENOENT)?;
                entry.insert(ino);
            }
        }

        reply.send()
    }

    async fn getxattr(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Getxattr<'_>,
        reply: ReplyXattr<'_>,
    ) -> reply::Result {
        let inode = self.inodes.get(op.ino()).ok_or(ENOENT)?;
        let value = inode.xattrs.get(op.name()).ok_or(ENODATA)?;
        match op.size() {
            0 => reply.send_size(value.len() as u32),
            size => {
                if value.len() as u32 > size {
                    return Err(ERANGE.into());
                }
                reply.send_value(value)
            }
        }
    }

    async fn setxattr(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Setxattr<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        let create = op.flags().contains(SetxattrFlags::CREATE);
        let replace = op.flags().contains(SetxattrFlags::REPLACE);
        if create && replace {
            return Err(EINVAL.into());
        }

        let mut inode = self.inodes.get_mut(op.ino()).ok_or(ENOENT)?;

        match inode.xattrs.entry(op.name().into()) {
            Entry::Occupied(entry) => {
                if create {
                    return Err(EEXIST.into());
                }
                let value = Arc::make_mut(entry.into_mut());
                *value = op.value().into();
            }
            Entry::Vacant(entry) => {
                if replace {
                    return Err(ENODATA.into());
                }
                if create {
                    entry.insert(Arc::default());
                } else {
                    entry.insert(Arc::new(op.value().into()));
                }
            }
        }

        reply.send()
    }

    async fn listxattr(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Listxattr<'_>,
        reply: ReplyXattr<'_>,
    ) -> reply::Result {
        let inode = self.inodes.get(op.ino()).ok_or(ENOENT)?;

        match op.size() {
            0 => {
                let total_len = inode.xattrs.keys().map(|name| name.len() as u32 + 1).sum();
                reply.send_size(total_len)
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
                    return Err(ERANGE.into());
                }

                reply.send_value(names)
            }
        }
    }

    async fn removexattr(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Removexattr<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        let mut inode = self.inodes.get_mut(op.ino()).ok_or(ENOENT)?;

        match inode.xattrs.entry(op.name().into()) {
            Entry::Occupied(entry) => {
                entry.remove();
            }
            Entry::Vacant(..) => return Err(ENODATA.into()),
        }

        reply.send()
    }

    async fn read(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Read<'_>,
        reply: ReplyData<'_>,
    ) -> reply::Result {
        let inode = self.inodes.get(op.ino()).ok_or(ENOENT)?;

        let content = inode.as_file().ok_or(EINVAL)?;

        let offset = op.offset() as usize;
        let size = op.size() as usize;

        let content = content.get(offset..).unwrap_or(&[]);
        let content = &content[..std::cmp::min(content.len(), size)];

        reply.send(content)
    }

    async fn write(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Write<'_>,
        mut data: fs::Data<'_>,
        reply: ReplyWrite<'_>,
    ) -> reply::Result {
        let mut inode = self.inodes.get_mut(op.ino()).ok_or(ENOENT)?;

        let content = inode.as_file_mut().ok_or(EINVAL)?;

        let offset = op.offset() as usize;
        let size = op.size() as usize;

        content.resize(std::cmp::max(content.len(), offset + size), 0);

        data.read_exact(&mut content[offset..offset + size])?;

        inode.attr.size = (offset + size) as u64;

        reply.send(op.size())
    }
}
