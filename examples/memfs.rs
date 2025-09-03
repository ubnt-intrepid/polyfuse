#![allow(clippy::unnecessary_mut_passed, clippy::rc_buffer)]
#![deny(clippy::unimplemented)]

use polyfuse::{
    fs::{self, Filesystem},
    mount::MountOptions,
    op,
    reply::{AttrOut, EntryOut, FileAttr, OpenOut, ReaddirOut, WriteOut, XattrOut},
    types::{DeviceID, FileID, NodeID, GID, UID},
    KernelConfig,
};

use anyhow::{ensure, Context as _, Result};
use dashmap::DashMap;
use libc::{
    DT_DIR, DT_UNKNOWN, EEXIST, EINVAL, ENODATA, ENOENT, ENOSYS, ENOTDIR, ENOTEMPTY, ENOTSUP,
    ERANGE, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG, XATTR_CREATE, XATTR_REPLACE,
};
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap},
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
    children: HashMap<OsString, NodeID>,
    parent: Option<NodeID>,
}

struct DirEntry {
    name: OsString,
    ino: NodeID,
    typ: u32,
    off: u64,
}

impl Directory {
    fn collect_entries(&self, attr: &libc::stat) -> Vec<Arc<DirEntry>> {
        let mut entries = Vec::with_capacity(self.children.len() + 2);
        let mut offset: u64 = 1;

        entries.push(Arc::new(DirEntry {
            name: ".".into(),
            ino: NodeID::from_raw(attr.st_ino),
            typ: DT_DIR as u32,
            off: offset,
        }));
        offset += 1;

        entries.push(Arc::new(DirEntry {
            name: "..".into(),
            ino: self.parent.unwrap_or(NodeID::from_raw(attr.st_ino)),
            typ: DT_DIR as u32,
            off: offset,
        }));
        offset += 1;

        for (name, &ino) in &self.children {
            entries.push(Arc::new(DirEntry {
                name: name.into(),
                ino,
                typ: DT_UNKNOWN as u32,
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
                attr.st_mode = S_IFDIR | 0o755;
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
        out.ino(*inode_entry.key());
        fill_attr(out.attr(), &inode.attr);
        out.ttl_entry(self.ttl);

        map_entry.insert(*inode_entry.key());
        inode_entry.insert(inode);

        Ok(out)
    }

    fn unlink_node(&self, parent: NodeID, name: &OsStr) -> fs::Result<()> {
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

        Ok(())
    }
}

impl Filesystem for MemFS {
    fn lookup(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Lookup<'_>>) -> fs::Result {
        let parent = self.inodes.get(req.arg().parent()).ok_or(ENOENT)?;
        let parent = parent.as_dir().ok_or(ENOTDIR)?;

        let child_ino = parent
            .children
            .get(req.arg().name())
            .copied()
            .ok_or(ENOENT)?;
        let mut child = self
            .inodes
            .get_mut(child_ino)
            .expect("should not be panic here");
        child.refcount += 1;

        let mut out = EntryOut::default();
        out.ino(child_ino);
        fill_attr(out.attr(), &child.attr);
        out.ttl_entry(self.ttl);

        req.reply(out)
    }

    fn forget(&self, _: fs::Context<'_, '_>, forgets: &[op::Forget]) {
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

    fn getattr(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Getattr<'_>>) -> fs::Result {
        let inode = self.inodes.get(req.arg().ino()).ok_or(ENOENT)?;

        let mut out = AttrOut::default();
        fill_attr(out.attr(), &inode.attr);
        out.ttl(self.ttl);

        req.reply(out)
    }

    fn setattr(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Setattr<'_>>) -> fs::Result {
        let mut inode = self.inodes.get_mut(req.arg().ino()).ok_or(ENOENT)?;

        fn to_duration(t: op::SetAttrTime) -> Duration {
            match t {
                op::SetAttrTime::Timespec(ts) => ts,
                _ => SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap(),
            }
        }

        if let Some(mode) = req.arg().mode() {
            inode.attr.st_mode = mode;
        }
        if let Some(uid) = req.arg().uid() {
            inode.attr.st_uid = uid.into_raw();
        }
        if let Some(gid) = req.arg().gid() {
            inode.attr.st_gid = gid.into_raw();
        }
        if let Some(size) = req.arg().size() {
            inode.attr.st_size = size as libc::off_t;
        }
        if let Some(atime) = req.arg().atime() {
            let atime = to_duration(atime);
            inode.attr.st_atime = atime.as_secs() as i64;
            inode.attr.st_atime_nsec = atime.subsec_nanos() as u64 as i64;
        }
        if let Some(mtime) = req.arg().mtime() {
            let mtime = to_duration(mtime);
            inode.attr.st_mtime = mtime.as_secs() as i64;
            inode.attr.st_mtime_nsec = mtime.subsec_nanos() as u64 as i64;
        }
        if let Some(ctime) = req.arg().ctime() {
            inode.attr.st_ctime = ctime.as_secs() as i64;
            inode.attr.st_ctime_nsec = ctime.subsec_nanos() as u64 as i64;
        }

        let mut out = AttrOut::default();
        fill_attr(out.attr(), &inode.attr);
        out.ttl(self.ttl);

        req.reply(out)
    }

    fn readlink(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Readlink<'_>>,
    ) -> fs::Result {
        let inode = self.inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let link = inode.as_symlink().ok_or(EINVAL)?;
        req.reply(link)
    }

    fn opendir(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Opendir<'_>>) -> fs::Result {
        let inode = self.inodes.get(req.arg().ino()).ok_or(ENOENT)?;
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
        out.fh(FileID::from_raw(key as u64));

        req.reply(out)
    }

    fn readdir(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Readdir<'_>>) -> fs::Result {
        if req.arg().mode() == op::ReaddirMode::Plus {
            Err(ENOSYS)?;
        }

        let dir_handles = &mut *self.dir_handles.lock().unwrap();
        let dir = dir_handles
            .get(req.arg().fh().into_raw() as usize)
            .ok_or(EINVAL)?;

        let mut out = ReaddirOut::new(req.arg().size() as usize);

        for entry in dir.entries.iter().skip(req.arg().offset() as usize) {
            if out.entry(&entry.name, entry.ino, entry.typ, entry.off) {
                break;
            }
            dir.offset.fetch_add(1, Ordering::SeqCst);
        }

        req.reply(out)
    }

    fn releasedir(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Releasedir<'_>>,
    ) -> fs::Result {
        let dir_handles = &mut *self.dir_handles.lock().unwrap();
        dir_handles.remove(req.arg().fh().into_raw() as usize);
        req.reply(())
    }

    fn mknod(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Mknod<'_>>) -> fs::Result {
        match req.arg().mode() & S_IFMT {
            S_IFREG => (),
            _ => Err(ENOTSUP)?,
        }

        let out = self.make_node(req.arg().parent(), req.arg().name(), |entry| INode {
            attr: {
                let mut attr = unsafe { mem::zeroed::<libc::stat>() };
                attr.st_ino = entry.key().into_raw();
                attr.st_nlink = 1;
                attr.st_mode = req.arg().mode();
                attr
            },
            xattrs: HashMap::new(),
            refcount: 1,
            links: 1,
            kind: INodeKind::RegularFile(vec![]),
        })?;
        req.reply(out)
    }

    fn mkdir(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Mkdir<'_>>) -> fs::Result {
        let out = self.make_node(req.arg().parent(), req.arg().name(), |entry| INode {
            attr: {
                let mut attr = unsafe { mem::zeroed::<libc::stat>() };
                attr.st_ino = entry.key().into_raw();
                attr.st_nlink = 2;
                attr.st_mode = req.arg().mode() | S_IFDIR;
                attr
            },
            xattrs: HashMap::new(),
            refcount: 1,
            links: 1,
            kind: INodeKind::Directory(Directory {
                children: HashMap::new(),
                parent: Some(req.arg().parent()),
            }),
        })?;
        req.reply(out)
    }

    fn symlink(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Symlink<'_>>) -> fs::Result {
        let out = self.make_node(req.arg().parent(), req.arg().name(), |entry| INode {
            attr: {
                let mut attr = unsafe { mem::zeroed::<libc::stat>() };
                attr.st_ino = entry.key().into_raw();
                attr.st_nlink = 1;
                attr.st_mode = S_IFLNK | 0o777;
                attr
            },
            xattrs: HashMap::new(),
            refcount: 1,
            links: 1,
            kind: INodeKind::Symlink(Arc::new(req.arg().link().into())),
        })?;
        req.reply(out)
    }

    fn link(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Link<'_>>) -> fs::Result {
        let mut inode = self.inodes.get_mut(req.arg().ino()).ok_or(ENOENT)?;

        debug_assert!(req.arg().ino() != req.arg().newparent());
        let mut newparent = self.inodes.get_mut(req.arg().newparent()).ok_or(ENOENT)?;
        let newparent = newparent.as_dir_mut().ok_or(ENOTDIR)?;

        match newparent.children.entry(req.arg().newname().into()) {
            Entry::Occupied(..) => return Err(EEXIST.into()),
            Entry::Vacant(entry) => {
                entry.insert(req.arg().ino());
                inode.links += 1;
                inode.attr.st_nlink += 1;
                inode.refcount += 1;
            }
        }

        let mut out = EntryOut::default();
        out.ino(req.arg().ino());
        fill_attr(out.attr(), &inode.attr);
        out.ttl_entry(self.ttl);

        req.reply(out)
    }

    fn unlink(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Unlink<'_>>) -> fs::Result {
        self.unlink_node(req.arg().parent(), req.arg().name())?;
        req.reply(())
    }

    fn rmdir(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Rmdir<'_>>) -> fs::Result {
        self.unlink_node(req.arg().parent(), req.arg().name())?;
        req.reply(())
    }

    fn rename(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Rename<'_>>) -> fs::Result {
        if req.arg().flags() != 0 {
            // TODO: handle RENAME_NOREPLACE and RENAME_EXCHANGE.
            Err(EINVAL)?;
        }

        let mut parent = self.inodes.get_mut(req.arg().parent()).ok_or(ENOENT)?;
        let parent = parent.as_dir_mut().ok_or(ENOTDIR)?;

        match req.arg().newparent() {
            newparent if newparent == req.arg().parent() => {
                let ino = match parent.children.get(req.arg().name()) {
                    Some(&ino) => ino,
                    None => return Err(ENOENT.into()),
                };

                match parent.children.entry(req.arg().newname().into()) {
                    Entry::Occupied(..) => return Err(EEXIST.into()),
                    Entry::Vacant(entry) => {
                        entry.insert(ino);
                    }
                }
                parent
                    .children
                    .remove(req.arg().name())
                    .unwrap_or_else(|| unreachable!());
            }

            newparent => {
                let mut newparent = self.inodes.get_mut(newparent).ok_or(ENOENT)?;
                let newparent = newparent.as_dir_mut().ok_or(ENOTDIR)?;

                let entry = match newparent.children.entry(req.arg().newname().into()) {
                    Entry::Occupied(..) => return Err(EEXIST.into()),
                    Entry::Vacant(entry) => entry,
                };
                let ino = parent.children.remove(req.arg().name()).ok_or(ENOENT)?;
                entry.insert(ino);
            }
        }

        req.reply(())
    }

    fn getxattr(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Getxattr<'_>>,
    ) -> fs::Result {
        let inode = self.inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let value = inode.xattrs.get(req.arg().name()).ok_or(ENODATA)?;
        match req.arg().size() {
            0 => {
                let mut out = XattrOut::default();
                out.size(value.len() as u32);
                req.reply(out)
            }
            size => {
                if value.len() as u32 > size {
                    return Err(ERANGE.into());
                }
                req.reply(value)
            }
        }
    }

    fn setxattr(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Setxattr<'_>>,
    ) -> fs::Result {
        let create = req.arg().flags() as i32 & XATTR_CREATE != 0;
        let replace = req.arg().flags() as i32 & XATTR_REPLACE != 0;
        if create && replace {
            return Err(EINVAL.into());
        }

        let mut inode = self.inodes.get_mut(req.arg().ino()).ok_or(ENOENT)?;

        match inode.xattrs.entry(req.arg().name().into()) {
            Entry::Occupied(entry) => {
                if create {
                    return Err(EEXIST.into());
                }
                let value = Arc::make_mut(entry.into_mut());
                *value = req.arg().value().into();
            }
            Entry::Vacant(entry) => {
                if replace {
                    return Err(ENODATA.into());
                }
                if create {
                    entry.insert(Arc::default());
                } else {
                    entry.insert(Arc::new(req.arg().value().into()));
                }
            }
        }

        req.reply(())
    }

    fn listxattr(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Listxattr<'_>>,
    ) -> fs::Result {
        let inode = self.inodes.get(req.arg().ino()).ok_or(ENOENT)?;

        match req.arg().size() {
            0 => {
                let total_len = inode.xattrs.keys().map(|name| name.len() as u32 + 1).sum();
                let mut out = XattrOut::default();
                out.size(total_len);
                req.reply(out)
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

                req.reply(names)
            }
        }
    }

    fn removexattr(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Removexattr<'_>>,
    ) -> fs::Result {
        let mut inode = self.inodes.get_mut(req.arg().ino()).ok_or(ENOENT)?;

        match inode.xattrs.entry(req.arg().name().into()) {
            Entry::Occupied(entry) => {
                entry.remove();
            }
            Entry::Vacant(..) => return Err(ENODATA.into()),
        }

        req.reply(())
    }

    fn read(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Read<'_>>) -> fs::Result {
        let inode = self.inodes.get(req.arg().ino()).ok_or(ENOENT)?;

        let content = inode.as_file().ok_or(EINVAL)?;

        let offset = req.arg().offset() as usize;
        let size = req.arg().size() as usize;

        let content = content.get(offset..).unwrap_or(&[]);
        let content = &content[..std::cmp::min(content.len(), size)];

        req.reply(content)
    }

    fn write(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Write<'_>>,
        mut data: fs::Data<'_>,
    ) -> fs::Result {
        let mut inode = self.inodes.get_mut(req.arg().ino()).ok_or(ENOENT)?;

        let content = inode.as_file_mut().ok_or(EINVAL)?;

        let offset = req.arg().offset() as usize;
        let size = req.arg().size() as usize;

        content.resize(std::cmp::max(content.len(), offset + size), 0);

        data.read_exact(&mut content[offset..offset + size])?;

        inode.attr.st_size = (offset + size) as libc::off_t;

        let mut out = WriteOut::default();
        out.size(req.arg().size());

        req.reply(out)
    }
}

fn fill_attr(attr: &mut FileAttr, st: &libc::stat) {
    attr.ino(NodeID::from_raw(st.st_ino));
    attr.size(st.st_size as u64);
    attr.mode(st.st_mode);
    attr.nlink(st.st_nlink as u32);
    attr.uid(UID::from_raw(st.st_uid));
    attr.gid(GID::from_raw(st.st_gid));
    attr.rdev(DeviceID::from_userspace_dev(st.st_rdev));
    attr.blksize(st.st_blksize as u32);
    attr.blocks(st.st_blocks as u64);
    attr.atime(Duration::new(st.st_atime as u64, st.st_atime_nsec as u32));
    attr.mtime(Duration::new(st.st_mtime as u64, st.st_mtime_nsec as u32));
    attr.ctime(Duration::new(st.st_ctime as u64, st.st_ctime_nsec as u32));
}
