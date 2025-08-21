#![allow(clippy::unnecessary_mut_passed, clippy::rc_buffer)]
#![deny(clippy::unimplemented)]

use libc::{EEXIST, EINVAL, ENODATA, ENOENT, ENOSYS, ENOTDIR, ENOTEMPTY, ENOTSUP, ERANGE};
use polyfuse::{
    fs::{self, Filesystem},
    mount::MountOptions,
    op,
    reply::{AttrOut, EntryOut, FileAttr, OpenOut, ReaddirOut, WriteOut, XattrOut},
    KernelConfig,
};

use anyhow::{ensure, Context as _, Result};
use dashmap::DashMap;
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap, RandomState},
    ffi::{OsStr, OsString},
    io::prelude::*,
    mem,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime},
};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_dir(), "mountpoint must be a directory");

    polyfuse::fs::run(
        MemFS::new(),
        mountpoint,
        MountOptions::default(),
        KernelConfig::default(),
    )?;

    Ok(())
}

type Ino = u64;

struct INodeTable {
    map: DashMap<Ino, INode, RandomState>,
    next_ino: AtomicU64,
}

impl INodeTable {
    fn new() -> Self {
        Self {
            map: DashMap::with_hasher(RandomState::new()),
            next_ino: AtomicU64::new(1), // ino=1 is reserved by root node
        }
    }

    fn get(&self, ino: Ino) -> Option<INodeRef<'_>> {
        self.map.get(&ino).map(|ref_| INodeRef { ref_ })
    }

    fn get_mut(&self, ino: Ino) -> Option<INodeRefMut<'_>> {
        self.map
            .get_mut(&ino)
            .map(|ref_mut| INodeRefMut { ref_mut })
    }

    fn occupied_entry(&self, ino: Ino) -> Option<OccupiedEntry<'_>> {
        match self.map.entry(ino) {
            dashmap::mapref::entry::Entry::Occupied(entry) => Some(OccupiedEntry { entry }),
            dashmap::mapref::entry::Entry::Vacant(..) => None,
        }
    }

    fn vacant_entry(&self) -> Option<VacantEntry<'_>> {
        // TODO: choose appropriate atomic ordering.
        let ino = self.next_ino.fetch_add(1, Ordering::SeqCst);

        match self.map.entry(ino) {
            dashmap::mapref::entry::Entry::Occupied(..) => None,
            dashmap::mapref::entry::Entry::Vacant(entry) => Some(VacantEntry { entry }),
        }
    }
}

struct INodeRef<'a> {
    ref_: dashmap::mapref::one::Ref<'a, Ino, INode>,
}
impl std::ops::Deref for INodeRef<'_> {
    type Target = INode;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.ref_
    }
}

struct INodeRefMut<'a> {
    ref_mut: dashmap::mapref::one::RefMut<'a, Ino, INode>,
}
impl std::ops::Deref for INodeRefMut<'_> {
    type Target = INode;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.ref_mut
    }
}
impl std::ops::DerefMut for INodeRefMut<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ref_mut
    }
}

struct OccupiedEntry<'a> {
    entry: dashmap::mapref::entry::OccupiedEntry<'a, Ino, INode>,
}
impl<'a> OccupiedEntry<'a> {
    fn get(&self) -> &INode {
        self.entry.get()
    }

    fn get_mut(&mut self) -> &mut INode {
        self.entry.get_mut()
    }

    fn remove(self) -> INode {
        self.entry.remove()
    }
}

struct VacantEntry<'a> {
    entry: dashmap::mapref::entry::VacantEntry<'a, Ino, INode>,
}
impl<'a> VacantEntry<'a> {
    fn ino(&self) -> Ino {
        *self.entry.key()
    }

    fn insert(self, inode: INode) -> INodeRefMut<'a> {
        INodeRefMut {
            ref_mut: self.entry.insert(inode),
        }
    }
}

struct INode {
    attr: libc::stat,
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
    children: HashMap<OsString, Ino>,
    parent: Option<Ino>,
}

struct DirEntry {
    name: OsString,
    ino: u64,
    typ: u32,
    off: u64,
}

impl Directory {
    fn collect_entries(&self, attr: &libc::stat) -> Vec<Arc<DirEntry>> {
        let mut entries = Vec::with_capacity(self.children.len() + 2);
        let mut offset: u64 = 1;

        entries.push(Arc::new(DirEntry {
            name: ".".into(),
            ino: attr.st_ino,
            typ: libc::DT_DIR as u32,
            off: offset,
        }));
        offset += 1;

        entries.push(Arc::new(DirEntry {
            name: "..".into(),
            ino: self.parent.unwrap_or(attr.st_ino),
            typ: libc::DT_DIR as u32,
            off: offset,
        }));
        offset += 1;

        for (name, &ino) in &self.children {
            entries.push(Arc::new(DirEntry {
                name: name.into(),
                ino,
                typ: libc::DT_UNKNOWN as u32,
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
                let mut attr = unsafe { mem::zeroed::<libc::stat>() };
                attr.st_ino = 1;
                attr.st_nlink = 2;
                attr.st_mode = libc::S_IFDIR | 0o755;
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

    fn make_node<F>(&self, cx: fs::Context<'_>, parent: Ino, name: &OsStr, f: F) -> fs::Result
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

        let mut out = EntryOut::default();
        out.ino(inode_entry.ino());
        fill_attr(out.attr(), &inode.attr);
        out.ttl_entry(self.ttl);
        let replied = cx.reply(out)?;

        map_entry.insert(inode_entry.ino());
        inode_entry.insert(inode);

        Ok(replied)
    }

    fn unlink_node(&self, cx: fs::Context<'_>, parent: Ino, name: &OsStr) -> fs::Result {
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
        inode.attr.st_nlink = inode.attr.st_nlink.saturating_sub(1);

        cx.reply(())
    }
}

impl Filesystem for MemFS {
    fn lookup(&self, cx: fs::Context<'_>, op: op::Lookup<'_>) -> fs::Result {
        let parent = self.inodes.get(op.parent()).ok_or(ENOENT)?;
        let parent = parent.as_dir().ok_or(ENOTDIR)?;

        let child_ino = parent.children.get(op.name()).copied().ok_or(ENOENT)?;
        let mut child = self
            .inodes
            .get_mut(child_ino)
            .expect("should not be panic here");
        child.refcount += 1;

        let mut out = EntryOut::default();
        out.ino(child_ino);
        fill_attr(out.attr(), &child.attr);
        out.ttl_entry(self.ttl);

        cx.reply(out)
    }

    fn forget(&self, forgets: &[op::Forget]) {
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

    fn getattr(&self, cx: fs::Context<'_>, op: op::Getattr<'_>) -> fs::Result {
        let inode = self.inodes.get(op.ino()).ok_or(ENOENT)?;

        let mut out = AttrOut::default();
        fill_attr(out.attr(), &inode.attr);
        out.ttl(self.ttl);

        cx.reply(out)
    }

    fn setattr(&self, cx: fs::Context<'_>, op: op::Setattr<'_>) -> fs::Result {
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
            inode.attr.st_mode = mode;
        }
        if let Some(uid) = op.uid() {
            inode.attr.st_uid = uid;
        }
        if let Some(gid) = op.gid() {
            inode.attr.st_gid = gid;
        }
        if let Some(size) = op.size() {
            inode.attr.st_size = size as libc::off_t;
        }
        if let Some(atime) = op.atime() {
            let atime = to_duration(atime);
            inode.attr.st_atime = atime.as_secs() as i64;
            inode.attr.st_atime_nsec = atime.subsec_nanos() as u64 as i64;
        }
        if let Some(mtime) = op.mtime() {
            let mtime = to_duration(mtime);
            inode.attr.st_mtime = mtime.as_secs() as i64;
            inode.attr.st_mtime_nsec = mtime.subsec_nanos() as u64 as i64;
        }
        if let Some(ctime) = op.ctime() {
            inode.attr.st_ctime = ctime.as_secs() as i64;
            inode.attr.st_ctime_nsec = ctime.subsec_nanos() as u64 as i64;
        }

        let mut out = AttrOut::default();
        fill_attr(out.attr(), &inode.attr);
        out.ttl(self.ttl);

        cx.reply(out)
    }

    fn readlink(&self, cx: fs::Context<'_>, op: op::Readlink<'_>) -> fs::Result {
        let inode = self.inodes.get(op.ino()).ok_or(ENOENT)?;
        let link = inode.as_symlink().ok_or(EINVAL)?;
        cx.reply(link)
    }

    fn opendir(&self, cx: fs::Context<'_>, op: op::Opendir<'_>) -> fs::Result {
        let inode = self.inodes.get(op.ino()).ok_or(ENOENT)?;
        if inode.attr.st_nlink == 0 {
            return Err(ENOENT.into());
        }
        let dir = inode.as_dir().ok_or(ENOTDIR)?;

        let dir_handles = &mut *self.dir_handles.lock().unwrap();
        let key = dir_handles.insert(DirHandle {
            entries: dir.collect_entries(&inode.attr),
            offset: AtomicUsize::new(0),
        });

        let mut out = OpenOut::default();
        out.fh(key as u64);

        cx.reply(out)
    }

    fn readdir(&self, cx: fs::Context<'_>, op: op::Readdir<'_>) -> fs::Result {
        if op.mode() == op::ReaddirMode::Plus {
            return Err(ENOSYS.into());
        }

        let dir_handles = &mut *self.dir_handles.lock().unwrap();
        let dir = dir_handles.get(op.fh() as usize).ok_or(EINVAL)?;

        let mut out = ReaddirOut::new(op.size() as usize);

        for entry in dir.entries.iter().skip(op.offset() as usize) {
            if out.entry(&entry.name, entry.ino, entry.typ, entry.off) {
                break;
            }
            dir.offset.fetch_add(1, Ordering::SeqCst);
        }

        cx.reply(out)
    }

    fn releasedir(&self, cx: fs::Context<'_>, op: op::Releasedir<'_>) -> fs::Result {
        let dir_handles = &mut *self.dir_handles.lock().unwrap();
        dir_handles.remove(op.fh() as usize);
        cx.reply(())
    }

    fn mknod(&self, cx: fs::Context<'_>, op: op::Mknod<'_>) -> fs::Result {
        match op.mode() & libc::S_IFMT {
            libc::S_IFREG => (),
            _ => return Err(ENOTSUP.into()),
        }

        self.make_node(cx, op.parent(), op.name(), |entry| INode {
            attr: {
                let mut attr = unsafe { mem::zeroed::<libc::stat>() };
                attr.st_ino = entry.ino();
                attr.st_nlink = 1;
                attr.st_mode = op.mode();
                attr
            },
            xattrs: HashMap::new(),
            refcount: 1,
            links: 1,
            kind: INodeKind::RegularFile(vec![]),
        })
    }

    fn mkdir(&self, cx: fs::Context<'_>, op: op::Mkdir<'_>) -> fs::Result {
        self.make_node(cx, op.parent(), op.name(), |entry| INode {
            attr: {
                let mut attr = unsafe { mem::zeroed::<libc::stat>() };
                attr.st_ino = entry.ino();
                attr.st_nlink = 2;
                attr.st_mode = op.mode() | libc::S_IFDIR;
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

    fn symlink(&self, cx: fs::Context<'_>, op: op::Symlink<'_>) -> fs::Result {
        self.make_node(cx, op.parent(), op.name(), |entry| INode {
            attr: {
                let mut attr = unsafe { mem::zeroed::<libc::stat>() };
                attr.st_ino = entry.ino();
                attr.st_nlink = 1;
                attr.st_mode = libc::S_IFLNK | 0o777;
                attr
            },
            xattrs: HashMap::new(),
            refcount: 1,
            links: 1,
            kind: INodeKind::Symlink(Arc::new(op.link().into())),
        })
    }

    fn link(&self, cx: fs::Context<'_>, op: op::Link<'_>) -> fs::Result {
        let mut inode = self.inodes.get_mut(op.ino()).ok_or(ENOENT)?;

        debug_assert!(op.ino() != op.newparent());
        let mut newparent = self.inodes.get_mut(op.newparent()).ok_or(ENOENT)?;
        let newparent = newparent.as_dir_mut().ok_or(ENOTDIR)?;

        match newparent.children.entry(op.newname().into()) {
            Entry::Occupied(..) => return Err(EEXIST.into()),
            Entry::Vacant(entry) => {
                entry.insert(op.ino());
                inode.links += 1;
                inode.attr.st_nlink += 1;
                inode.refcount += 1;
            }
        }

        let mut out = EntryOut::default();
        out.ino(op.ino());
        fill_attr(out.attr(), &inode.attr);
        out.ttl_entry(self.ttl);

        cx.reply(out)
    }

    fn unlink(&self, cx: fs::Context<'_>, op: op::Unlink<'_>) -> fs::Result {
        self.unlink_node(cx, op.parent(), op.name())
    }

    fn rmdir(&self, cx: fs::Context<'_>, op: op::Rmdir<'_>) -> fs::Result {
        self.unlink_node(cx, op.parent(), op.name())
    }

    fn rename(&self, cx: fs::Context<'_>, op: op::Rename<'_>) -> fs::Result {
        if op.flags() != 0 {
            // TODO: handle RENAME_NOREPLACE and RENAME_EXCHANGE.
            return Err(EINVAL.into());
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

        cx.reply(())
    }

    fn getxattr(&self, cx: fs::Context<'_>, op: op::Getxattr<'_>) -> fs::Result {
        let inode = self.inodes.get(op.ino()).ok_or(ENOENT)?;
        let value = inode.xattrs.get(op.name()).ok_or(ENODATA)?;
        match op.size() {
            0 => {
                let mut out = XattrOut::default();
                out.size(value.len() as u32);
                cx.reply(out)
            }
            size => {
                if value.len() as u32 > size {
                    return Err(ERANGE.into());
                }
                cx.reply(value)
            }
        }
    }

    fn setxattr(&self, cx: fs::Context<'_>, op: op::Setxattr<'_>) -> fs::Result {
        let create = op.flags() as i32 & libc::XATTR_CREATE != 0;
        let replace = op.flags() as i32 & libc::XATTR_REPLACE != 0;
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

        cx.reply(())
    }

    fn listxattr(&self, cx: fs::Context<'_>, op: op::Listxattr<'_>) -> fs::Result {
        let inode = self.inodes.get(op.ino()).ok_or(ENOENT)?;

        match op.size() {
            0 => {
                let total_len = inode.xattrs.keys().map(|name| name.len() as u32 + 1).sum();
                let mut out = XattrOut::default();
                out.size(total_len);
                cx.reply(out)
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

                cx.reply(names)
            }
        }
    }

    fn removexattr(&self, cx: fs::Context<'_>, op: op::Removexattr<'_>) -> fs::Result {
        let mut inode = self.inodes.get_mut(op.ino()).ok_or(ENOENT)?;

        match inode.xattrs.entry(op.name().into()) {
            Entry::Occupied(entry) => {
                entry.remove();
            }
            Entry::Vacant(..) => return Err(ENODATA.into()),
        }

        cx.reply(())
    }

    fn read(&self, cx: fs::Context<'_>, op: op::Read<'_>) -> fs::Result {
        let inode = self.inodes.get(op.ino()).ok_or(ENOENT)?;

        let content = inode.as_file().ok_or(EINVAL)?;

        let offset = op.offset() as usize;
        let size = op.size() as usize;

        let content = content.get(offset..).unwrap_or(&[]);
        let content = &content[..std::cmp::min(content.len(), size)];

        cx.reply(content)
    }

    fn write(&self, mut cx: fs::Context<'_>, op: op::Write<'_>) -> fs::Result {
        let mut inode = self.inodes.get_mut(op.ino()).ok_or(ENOENT)?;

        let content = inode.as_file_mut().ok_or(EINVAL)?;

        let offset = op.offset() as usize;
        let size = op.size() as usize;

        content.resize(std::cmp::max(content.len(), offset + size), 0);

        cx.read_exact(&mut content[offset..offset + size])?;

        inode.attr.st_size = (offset + size) as libc::off_t;

        let mut out = WriteOut::default();
        out.size(op.size());

        cx.reply(out)
    }
}

fn fill_attr(attr: &mut FileAttr, st: &libc::stat) {
    attr.ino(st.st_ino);
    attr.size(st.st_size as u64);
    attr.mode(st.st_mode);
    attr.nlink(st.st_nlink as u32);
    attr.uid(st.st_uid);
    attr.gid(st.st_gid);
    attr.rdev(st.st_rdev as u32);
    attr.blksize(st.st_blksize as u32);
    attr.blocks(st.st_blocks as u64);
    attr.atime(Duration::new(st.st_atime as u64, st.st_atime_nsec as u32));
    attr.mtime(Duration::new(st.st_mtime as u64, st.st_mtime_nsec as u32));
    attr.ctime(Duration::new(st.st_ctime as u64, st.st_ctime_nsec as u32));
}
