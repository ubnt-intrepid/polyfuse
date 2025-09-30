#![allow(clippy::unnecessary_mut_passed, clippy::rc_buffer)]
#![deny(clippy::unimplemented)]
#![forbid(unsafe_code)]

use polyfuse::{
    fs::{self, Daemon, Filesystem},
    op::{self, SetxattrFlags},
    reply::{AttrOut, EntryOut, OpenOut, OpenOutFlags, ReaddirOut, WriteOut, XattrOut},
    types::{FileAttr, FileID, FileMode, FilePermissions, FileType, NodeID},
};

use anyhow::{ensure, Context as _, Result};
use dashmap::DashMap;
use rustix::io::Errno;
use slab::Slab;
use std::{
    borrow::Cow,
    collections::hash_map::{Entry, HashMap},
    ffi::{OsStr, OsString},
    io,
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

    let daemon = Daemon::mount(mountpoint, Default::default(), Default::default()).await?;
    daemon.run(Arc::new(MemFS::new()), None).await?;

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

    fn make_node<F>(&self, parent: NodeID, name: &OsStr, f: F) -> fs::Result<EntryOut>
    where
        F: FnOnce(&VacantEntry<'_>) -> INode,
    {
        let mut parent = self.inodes.get_mut(parent).ok_or(Errno::NOENT)?;
        let parent = match parent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return Err(Errno::NOTDIR.into()),
        };

        let map_entry = match parent.children.entry(name.into()) {
            Entry::Occupied(..) => return Err(Errno::EXIST.into()),
            Entry::Vacant(map_entry) => map_entry,
        };
        let inode_entry = self.inodes.vacant_entry().expect("inode number conflict");
        let inode = f(&inode_entry);

        let out = EntryOut {
            ino: *inode_entry.key(),
            attr: Cow::Owned(inode.attr.clone()),
            entry_valid: Some(self.ttl),
            attr_valid: None,
            generation: 0,
        };

        map_entry.insert(*inode_entry.key());
        inode_entry.insert(inode);

        Ok(out)
    }

    fn unlink_node(&self, parent: NodeID, name: &OsStr) -> fs::Result<()> {
        let mut parent = self.inodes.get_mut(parent).ok_or(Errno::NOENT)?;
        let parent = parent.as_dir_mut().ok_or(Errno::NOTDIR)?;

        let ino = parent.children.get(name).copied().ok_or(Errno::NOENT)?;

        let mut inode = self.inodes.get_mut(ino).unwrap_or_else(|| unreachable!());
        if let Some(dir) = inode.as_dir_mut() {
            if !dir.children.is_empty() {
                return Err(Errno::NOTEMPTY.into());
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
    async fn lookup(self: &Arc<Self>, req: fs::Request<'_>, op: op::Lookup<'_>) -> fs::Result {
        let parent = self.inodes.get(op.parent).ok_or(Errno::NOENT)?;
        let parent = parent.as_dir().ok_or(Errno::NOTDIR)?;

        let child_ino = parent.children.get(op.name).copied().ok_or(Errno::NOENT)?;
        let mut child = self
            .inodes
            .get_mut(child_ino)
            .expect("should not be panic here");
        child.refcount += 1;

        req.reply(EntryOut {
            ino: child_ino,
            generation: 0,
            attr: Cow::Borrowed(&child.attr),
            entry_valid: Some(self.ttl),
            attr_valid: Some(self.ttl),
        })
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

    async fn getattr(self: &Arc<Self>, req: fs::Request<'_>, op: op::Getattr<'_>) -> fs::Result {
        let inode = self.inodes.get(op.ino).ok_or(Errno::NOENT)?;

        req.reply(AttrOut {
            attr: Cow::Borrowed(&inode.attr),
            valid: Some(self.ttl),
        })
    }

    async fn setattr(self: &Arc<Self>, req: fs::Request<'_>, op: op::Setattr<'_>) -> fs::Result {
        let mut inode = self.inodes.get_mut(op.ino).ok_or(Errno::NOENT)?;

        fn to_duration(t: op::SetAttrTime) -> Duration {
            match t {
                op::SetAttrTime::Timespec(ts) => ts,
                _ => SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap(),
            }
        }

        if let Some(mode) = op.mode {
            inode.attr.mode = mode;
        }
        if let Some(uid) = op.uid {
            inode.attr.uid = uid;
        }
        if let Some(gid) = op.gid {
            inode.attr.gid = gid;
        }
        if let Some(size) = op.size {
            inode.attr.size = size;
        }
        if let Some(atime) = op.atime {
            inode.attr.atime = to_duration(atime);
        }
        if let Some(mtime) = op.mtime {
            inode.attr.mtime = to_duration(mtime);
        }
        if let Some(ctime) = op.ctime {
            inode.attr.ctime = ctime;
        }

        req.reply(AttrOut {
            attr: Cow::Borrowed(&inode.attr),
            valid: Some(self.ttl),
        })
    }

    async fn readlink(self: &Arc<Self>, req: fs::Request<'_>, op: op::Readlink<'_>) -> fs::Result {
        let inode = self.inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let link = inode.as_symlink().ok_or(Errno::INVAL)?;
        req.reply(link)
    }

    async fn opendir(self: &Arc<Self>, req: fs::Request<'_>, op: op::Opendir<'_>) -> fs::Result {
        let inode = self.inodes.get(op.ino).ok_or(Errno::NOENT)?;
        if inode.attr.nlink == 0 {
            return Err(Errno::NOENT.into());
        }
        let dir = inode.as_dir().ok_or(Errno::NOTDIR)?;

        let dir_handles = &mut *self.dir_handles.lock().await;
        let key = dir_handles.insert(DirHandle {
            entries: dir.collect_entries(&inode.attr),
            offset: AtomicUsize::new(0),
        });

        req.reply(OpenOut {
            fh: FileID::from_raw(key as u64),
            open_flags: OpenOutFlags::empty(),
        })
    }

    async fn readdir(self: &Arc<Self>, req: fs::Request<'_>, op: op::Readdir<'_>) -> fs::Result {
        if op.mode == op::ReaddirMode::Plus {
            Err(Errno::NOSYS)?;
        }

        let dir_handles = &mut *self.dir_handles.lock().await;
        let dir = dir_handles
            .get(op.fh.into_raw() as usize)
            .ok_or(Errno::INVAL)?;

        let mut buf = ReaddirOut::new(op.size as usize);

        for entry in dir.entries.iter().skip(op.offset as usize) {
            if buf.push_entry(&entry.name, entry.ino, entry.typ, entry.off) {
                break;
            }
            dir.offset.fetch_add(1, Ordering::SeqCst);
        }

        req.reply(buf)
    }

    async fn releasedir(
        self: &Arc<Self>,
        req: fs::Request<'_>,
        op: op::Releasedir<'_>,
    ) -> fs::Result {
        let dir_handles = &mut *self.dir_handles.lock().await;
        dir_handles.remove(op.fh.into_raw() as usize);
        req.reply(())
    }

    async fn mknod(self: &Arc<Self>, req: fs::Request<'_>, op: op::Mknod<'_>) -> fs::Result {
        match op.mode.file_type() {
            Some(FileType::Regular) => (),
            _ => Err(Errno::NOTSUP)?,
        }

        let out = self.make_node(op.parent, op.name, |entry| INode {
            attr: {
                let mut attr = FileAttr::new();
                attr.ino = *entry.key();
                attr.nlink = 1;
                attr.mode = op.mode;
                attr
            },
            xattrs: HashMap::new(),
            refcount: 1,
            links: 1,
            kind: INodeKind::RegularFile(vec![]),
        })?;

        req.reply(out)
    }

    async fn mkdir(self: &Arc<Self>, req: fs::Request<'_>, op: op::Mkdir<'_>) -> fs::Result {
        let out = self.make_node(op.parent, op.name, |entry| INode {
            attr: {
                let mut attr = FileAttr::new();
                attr.ino = *entry.key();
                attr.nlink = 2;
                attr.mode = FileMode::new(FileType::Directory, op.permissions);
                attr
            },
            xattrs: HashMap::new(),
            refcount: 1,
            links: 1,
            kind: INodeKind::Directory(Directory {
                children: HashMap::new(),
                parent: Some(op.parent),
            }),
        })?;

        req.reply(out)
    }

    async fn symlink(self: &Arc<Self>, req: fs::Request<'_>, op: op::Symlink<'_>) -> fs::Result {
        let out = self.make_node(op.parent, op.name, |entry| INode {
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
            kind: INodeKind::Symlink(Arc::new(op.link.into())),
        })?;

        req.reply(out)
    }

    async fn link(self: &Arc<Self>, req: fs::Request<'_>, op: op::Link<'_>) -> fs::Result {
        let mut inode = self.inodes.get_mut(op.ino).ok_or(Errno::NOENT)?;

        debug_assert!(op.ino != op.newparent);
        let mut newparent = self.inodes.get_mut(op.newparent).ok_or(Errno::NOENT)?;
        let newparent = newparent.as_dir_mut().ok_or(Errno::NOTDIR)?;

        match newparent.children.entry(op.newname.into()) {
            Entry::Occupied(..) => return Err(Errno::EXIST.into()),
            Entry::Vacant(entry) => {
                entry.insert(op.ino);
                inode.links += 1;
                inode.attr.nlink += 1;
                inode.refcount += 1;
            }
        }

        req.reply(EntryOut {
            ino: op.ino,
            attr: Cow::Borrowed(&inode.attr),
            entry_valid: Some(self.ttl),
            attr_valid: None,
            generation: 0,
        })
    }

    async fn unlink(self: &Arc<Self>, req: fs::Request<'_>, op: op::Unlink<'_>) -> fs::Result {
        self.unlink_node(op.parent, op.name)?;
        req.reply(())
    }

    async fn rmdir(self: &Arc<Self>, req: fs::Request<'_>, op: op::Rmdir<'_>) -> fs::Result {
        self.unlink_node(op.parent, op.name)?;
        req.reply(())
    }

    async fn rename(self: &Arc<Self>, req: fs::Request<'_>, op: op::Rename<'_>) -> fs::Result {
        if !op.flags.is_empty() {
            // TODO: handle RENAME_NOREPLACE and RENAME_EXCHANGE.
            Err(Errno::INVAL)?;
        }

        let mut parent = self.inodes.get_mut(op.parent).ok_or(Errno::NOENT)?;
        let parent = parent.as_dir_mut().ok_or(Errno::NOTDIR)?;

        match op.newparent {
            newparent if newparent == op.parent => {
                let ino = match parent.children.get(op.name) {
                    Some(&ino) => ino,
                    None => return Err(Errno::NOENT.into()),
                };

                match parent.children.entry(op.newname.into()) {
                    Entry::Occupied(..) => return Err(Errno::EXIST.into()),
                    Entry::Vacant(entry) => {
                        entry.insert(ino);
                    }
                }
                parent
                    .children
                    .remove(op.name)
                    .unwrap_or_else(|| unreachable!());
            }

            newparent => {
                let mut newparent = self.inodes.get_mut(newparent).ok_or(Errno::NOENT)?;
                let newparent = newparent.as_dir_mut().ok_or(Errno::NOTDIR)?;

                let entry = match newparent.children.entry(op.newname.into()) {
                    Entry::Occupied(..) => return Err(Errno::EXIST.into()),
                    Entry::Vacant(entry) => entry,
                };
                let ino = parent.children.remove(op.name).ok_or(Errno::NOENT)?;
                entry.insert(ino);
            }
        }

        req.reply(())
    }

    async fn getxattr(self: &Arc<Self>, req: fs::Request<'_>, op: op::Getxattr<'_>) -> fs::Result {
        let inode = self.inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let value = inode.xattrs.get(op.name).ok_or(Errno::NODATA)?;
        match op.size {
            0 => req.reply(XattrOut::new(value.len() as u32)),
            size => {
                if value.len() as u32 > size {
                    return Err(Errno::RANGE.into());
                }
                req.reply(value)
            }
        }
    }

    async fn setxattr(self: &Arc<Self>, req: fs::Request<'_>, op: op::Setxattr<'_>) -> fs::Result {
        let create = op.flags.contains(SetxattrFlags::CREATE);
        let replace = op.flags.contains(SetxattrFlags::REPLACE);
        if create && replace {
            return Err(Errno::INVAL.into());
        }

        let mut inode = self.inodes.get_mut(op.ino).ok_or(Errno::NOENT)?;

        match inode.xattrs.entry(op.name.into()) {
            Entry::Occupied(entry) => {
                if create {
                    return Err(Errno::EXIST.into());
                }
                let value = Arc::make_mut(entry.into_mut());
                *value = op.value.into();
            }
            Entry::Vacant(entry) => {
                if replace {
                    return Err(Errno::NODATA.into());
                }
                if create {
                    entry.insert(Arc::default());
                } else {
                    entry.insert(Arc::new(op.value.into()));
                }
            }
        }

        req.reply(())
    }

    async fn listxattr(
        self: &Arc<Self>,
        req: fs::Request<'_>,
        op: op::Listxattr<'_>,
    ) -> fs::Result {
        let inode = self.inodes.get(op.ino).ok_or(Errno::NOENT)?;

        match op.size {
            0 => {
                let total_len = inode.xattrs.keys().map(|name| name.len() as u32 + 1).sum();
                req.reply(XattrOut::new(total_len))
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
                    return Err(Errno::RANGE.into());
                }

                req.reply(names)
            }
        }
    }

    async fn removexattr(
        self: &Arc<Self>,
        req: fs::Request<'_>,
        op: op::Removexattr<'_>,
    ) -> fs::Result {
        let mut inode = self.inodes.get_mut(op.ino).ok_or(Errno::NOENT)?;

        match inode.xattrs.entry(op.name.into()) {
            Entry::Occupied(entry) => {
                entry.remove();
            }
            Entry::Vacant(..) => return Err(Errno::NODATA.into()),
        }

        req.reply(())
    }

    async fn read(self: &Arc<Self>, req: fs::Request<'_>, op: op::Read<'_>) -> fs::Result {
        let inode = self.inodes.get(op.ino).ok_or(Errno::NOENT)?;

        let content = inode.as_file().ok_or(Errno::INVAL)?;

        let offset = op.offset as usize;
        let size = op.size as usize;

        let content = content.get(offset..).unwrap_or(&[]);
        let content = &content[..std::cmp::min(content.len(), size)];

        req.reply(content)
    }

    async fn write(
        self: &Arc<Self>,
        req: fs::Request<'_>,
        op: op::Write<'_>,
        mut data: impl io::Read + Send,
    ) -> fs::Result {
        let mut inode = self.inodes.get_mut(op.ino).ok_or(Errno::NOENT)?;

        let content = inode.as_file_mut().ok_or(Errno::INVAL)?;

        let offset = op.offset as usize;
        let size = op.size as usize;

        content.resize(std::cmp::max(content.len(), offset + size), 0);

        data.read_exact(&mut content[offset..offset + size])?;

        inode.attr.size = (offset + size) as u64;

        req.reply(WriteOut::new(op.size))
    }
}
