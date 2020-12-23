#![allow(clippy::unnecessary_mut_passed, clippy::rc_buffer)]
#![deny(clippy::unimplemented)]

use polyfuse::{
    op,
    reply::{AttrOut, EntryOut, FileAttr, OpenOut, ReaddirOut, WriteOut, XattrOut},
    KernelConfig, Operation, Request, Session,
};

use anyhow::{ensure, Context as _, Result};
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap},
    ffi::{OsStr, OsString},
    io::{self, BufRead},
    mem,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_dir(), "mountpoint must be a directory");

    let session = Session::mount(mountpoint, KernelConfig::default())?;

    let fs = Arc::new(MemFS::new());

    while let Some(req) = session.next_request()? {
        let fs = fs.clone();

        std::thread::spawn(move || -> Result<()> {
            let span = tracing::debug_span!("handle_request", unique = req.unique());
            let _enter = span.enter();

            let op = req.operation()?;
            tracing::debug!(?op);

            match op {
                Operation::Lookup(op) => fs.do_lookup(&req, op)?,
                Operation::Forget(forgets) => {
                    fs.do_forget(forgets.as_ref());
                }
                Operation::Getattr(op) => fs.do_getattr(&req, op)?,
                Operation::Setattr(op) => fs.do_setattr(&req, op)?,
                Operation::Readlink(op) => fs.do_readlink(&req, op)?,

                Operation::Opendir(op) => fs.do_opendir(&req, op)?,
                Operation::Readdir(op) => fs.do_readdir(&req, op)?,
                Operation::Releasedir(op) => fs.do_releasedir(&req, op)?,

                Operation::Mknod(op) => fs.do_mknod(&req, op)?,
                Operation::Mkdir(op) => fs.do_mkdir(&req, op)?,
                Operation::Symlink(op) => fs.do_symlink(&req, op)?,
                Operation::Link(op) => fs.do_link(&req, op)?,
                Operation::Unlink(op) => fs.do_unlink(&req, op)?,
                Operation::Rmdir(op) => fs.do_rmdir(&req, op)?,
                Operation::Rename(op) => fs.do_rename(&req, op)?,

                Operation::Getxattr(op) => fs.do_getxattr(&req, op)?,
                Operation::Setxattr(op) => fs.do_setxattr(&req, op)?,
                Operation::Listxattr(op) => fs.do_listxattr(&req, op)?,
                Operation::Removexattr(op) => fs.do_removexattr(&req, op)?,

                Operation::Read(op) => fs.do_read(&req, op)?,
                Operation::Write(op, data) => fs.do_write(&req, op, data)?,

                _ => {
                    tracing::debug!("NOSYS");
                    req.reply_error(libc::ENOSYS)?;
                }
            }

            Ok(())
        });
    }

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
            inodes: Mutex::new(inodes),
            dir_handles: Mutex::default(),
            ttl: Duration::from_secs(60 * 60 * 24),
        }
    }

    fn do_lookup(&self, req: &Request, op: op::Lookup<'_>) -> io::Result<()> {
        let inodes = &*self.inodes.lock().unwrap();

        let parent = match inodes.get(op.parent()) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let parent = &*parent.lock().unwrap();

        let parent = match parent.kind {
            INodeKind::Directory(ref dir) => dir,
            _ => return req.reply_error(libc::ENOTDIR),
        };

        let child_ino = match parent.children.get(op.name()) {
            Some(&ino) => ino,
            None => return req.reply_error(libc::ENOENT),
        };
        let child = inodes.get(child_ino).unwrap_or_else(|| unreachable!());
        let child = &mut *child.lock().unwrap();
        child.refcount += 1;

        let mut out = EntryOut::default();
        out.ino(child_ino);
        fill_attr(out.attr(), &child.attr);
        out.ttl_entry(self.ttl);

        req.reply(out)
    }

    fn do_forget(&self, forgets: &[op::Forget]) {
        let inodes = &mut *self.inodes.lock().unwrap();

        for forget in forgets {
            if let Some(inode) = inodes.get(forget.ino()) {
                let inode = &mut *inode.lock().unwrap();
                inode.refcount = inode.refcount.saturating_sub(forget.nlookup());
                if inode.refcount == 0 && inode.links == 0 {
                    inodes.remove(forget.ino());
                }
            }
        }
    }

    fn do_getattr(&self, req: &Request, op: op::Getattr<'_>) -> io::Result<()> {
        let inodes = &*self.inodes.lock().unwrap();

        let inode = match inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let inode = &*inode.lock().unwrap();

        let mut out = AttrOut::default();
        fill_attr(out.attr(), &inode.attr);
        out.ttl(self.ttl);

        req.reply(out)
    }

    fn do_setattr(&self, req: &Request, op: op::Setattr<'_>) -> io::Result<()> {
        let inodes = &*self.inodes.lock().unwrap();

        let inode = match inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let inode = &mut *inode.lock().unwrap();

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

        req.reply(out)
    }

    fn do_readlink(&self, req: &Request, op: op::Readlink<'_>) -> io::Result<()> {
        let inodes = &*self.inodes.lock().unwrap();

        let inode = match inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let inode = &*inode.lock().unwrap();

        let link = match inode.kind {
            INodeKind::Symlink(ref link) => link,
            _ => return req.reply_error(libc::EINVAL),
        };

        req.reply(link)
    }

    fn do_opendir(&self, req: &Request, op: op::Opendir<'_>) -> io::Result<()> {
        let inodes = &*self.inodes.lock().unwrap();
        let dirs = &mut *self.dir_handles.lock().unwrap();

        let inode = match inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let inode = &*inode.lock().unwrap();
        if inode.attr.st_nlink == 0 {
            return req.reply_error(libc::ENOENT);
        }
        let dir = match inode.kind {
            INodeKind::Directory(ref dir) => dir,
            _ => return req.reply_error(libc::ENOTDIR),
        };

        let key = dirs.insert(Arc::new(Mutex::new(DirHandle {
            entries: dir.collect_entries(&inode.attr),
        })));

        let mut out = OpenOut::default();
        out.fh(key as u64);

        req.reply(out)
    }

    fn do_readdir(&self, req: &Request, op: op::Readdir<'_>) -> io::Result<()> {
        if op.mode() == op::ReaddirMode::Plus {
            return req.reply_error(libc::ENOSYS);
        }

        let dirs = &*self.dir_handles.lock().unwrap();

        let dir = match dirs.get(op.fh() as usize) {
            Some(dir) => dir.clone(),
            None => return req.reply_error(libc::EINVAL),
        };
        let dir = dir.lock().unwrap();

        let mut out = ReaddirOut::new(op.size() as usize);
        for entry in dir.entries.iter().skip(op.offset() as usize) {
            if out.entry(&entry.name, entry.ino, entry.typ, entry.off) {
                break;
            }
        }

        req.reply(out)
    }

    fn do_releasedir(&self, req: &Request, op: op::Releasedir<'_>) -> io::Result<()> {
        let dirs = &mut *self.dir_handles.lock().unwrap();

        let dir = dirs.remove(op.fh() as usize);
        drop(dir);

        req.reply(())
    }

    fn do_mknod(&self, req: &Request, op: op::Mknod<'_>) -> io::Result<()> {
        match op.mode() & libc::S_IFMT {
            libc::S_IFREG => (),
            _ => return req.reply_error(libc::ENOTSUP),
        }

        self.make_node(req, op.parent(), op.name(), |entry| INode {
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

    fn do_mkdir(&self, req: &Request, op: op::Mkdir<'_>) -> io::Result<()> {
        self.make_node(req, op.parent(), op.name(), |entry| INode {
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

    fn do_symlink(&self, req: &Request, op: op::Symlink<'_>) -> io::Result<()> {
        self.make_node(req, op.parent(), op.name(), |entry| INode {
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

    fn make_node<F>(&self, req: &Request, parent: Ino, name: &OsStr, f: F) -> io::Result<()>
    where
        F: FnOnce(&VacantEntry<'_>) -> INode,
    {
        let inodes = &mut *self.inodes.lock().unwrap();

        let parent = match inodes.get(parent) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let parent = &mut *parent.lock().unwrap();
        let parent = match parent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return req.reply_error(libc::ENOTDIR),
        };

        let map_entry = match parent.children.entry(name.into()) {
            Entry::Occupied(..) => return req.reply_error(libc::EEXIST),
            Entry::Vacant(map_entry) => map_entry,
        };
        let inode_entry = inodes.vacant_entry();
        let inode = f(&inode_entry);

        let mut out = EntryOut::default();
        out.ino(inode_entry.ino());
        fill_attr(out.attr(), &inode.attr);
        out.ttl_entry(self.ttl);
        req.reply(out)?;

        map_entry.insert(inode_entry.ino());
        inode_entry.insert(inode);

        Ok(())
    }

    fn do_link(&self, req: &Request, op: op::Link<'_>) -> io::Result<()> {
        let inodes = &*self.inodes.lock().unwrap();

        let inode = match inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };

        let newparent = match inodes.get(op.newparent()) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let newparent = &mut *newparent.lock().unwrap();
        let newparent = match newparent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return req.reply_error(libc::ENOTDIR),
        };

        match newparent.children.entry(op.newname().into()) {
            Entry::Occupied(..) => return req.reply_error(libc::EEXIST),
            Entry::Vacant(entry) => {
                entry.insert(op.ino());
                let inode = &mut *inode.lock().unwrap();
                inode.links += 1;
                inode.attr.st_nlink += 1;
            }
        }

        let child_ino = match newparent.children.get(op.newname()) {
            Some(&ino) => ino,
            None => return req.reply_error(libc::ENOENT),
        };
        let child = inodes.get(child_ino).unwrap_or_else(|| unreachable!());
        let child = &mut *child.lock().unwrap();
        child.refcount += 1;

        let mut out = EntryOut::default();
        out.ino(child_ino);
        fill_attr(out.attr(), &child.attr);
        out.ttl_entry(self.ttl);

        req.reply(out)
    }

    fn do_unlink(&self, req: &Request, op: op::Unlink<'_>) -> io::Result<()> {
        self.unlink_node(req, op.parent(), op.name())
    }

    fn do_rmdir(&self, req: &Request, op: op::Rmdir<'_>) -> io::Result<()> {
        self.unlink_node(req, op.parent(), op.name())
    }

    fn unlink_node(&self, req: &Request, parent: Ino, name: &OsStr) -> io::Result<()> {
        let inodes = &*self.inodes.lock().unwrap();

        let parent = match inodes.get(parent) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let parent = &mut *parent.lock().unwrap();
        let parent = match parent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return req.reply_error(libc::ENOTDIR),
        };

        let ino = match parent.children.get(name) {
            Some(&ino) => ino,
            None => return req.reply_error(libc::ENOENT),
        };

        let inode = inodes.get(ino).unwrap_or_else(|| unreachable!());
        let inode = &mut *inode.lock().unwrap();

        match inode.kind {
            INodeKind::Directory(ref dir) if !dir.children.is_empty() => {
                return req.reply_error(libc::ENOTEMPTY);
            }
            _ => (),
        }

        parent
            .children
            .remove(name)
            .unwrap_or_else(|| unreachable!());

        inode.links = inode.links.saturating_sub(1);
        inode.attr.st_nlink = inode.attr.st_nlink.saturating_sub(1);

        req.reply(())
    }

    fn do_rename(&self, req: &Request, op: op::Rename<'_>) -> io::Result<()> {
        if op.flags() != 0 {
            // TODO: handle RENAME_NOREPLACE and RENAME_EXCHANGE.
            return req.reply_error(libc::EINVAL);
        }

        let inodes = &*self.inodes.lock().unwrap();

        let parent = match inodes.get(op.parent()) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let parent = &mut *parent.lock().unwrap();
        let parent = match parent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return req.reply_error(libc::ENOTDIR),
        };

        match op.newparent() {
            newparent if newparent == op.parent() => {
                let ino = match parent.children.get(op.name()) {
                    Some(&ino) => ino,
                    None => return req.reply_error(libc::ENOENT),
                };

                match parent.children.entry(op.newname().into()) {
                    Entry::Occupied(..) => return req.reply_error(libc::EEXIST),
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
                let newparent = match inodes.get(newparent) {
                    Some(inode) => inode,
                    None => return req.reply_error(libc::ENOENT),
                };
                let newparent = &mut *newparent.lock().unwrap();
                let newparent = match newparent.kind {
                    INodeKind::Directory(ref mut dir) => dir,
                    _ => return req.reply_error(libc::ENOTDIR),
                };

                let entry = match newparent.children.entry(op.newname().into()) {
                    Entry::Occupied(..) => return req.reply_error(libc::EEXIST),
                    Entry::Vacant(entry) => entry,
                };
                let ino = match parent.children.remove(op.name()) {
                    Some(ino) => ino,
                    None => return req.reply_error(libc::ENOENT),
                };
                entry.insert(ino);
            }
        }

        req.reply(())
    }

    fn do_getxattr(&self, req: &Request, op: op::Getxattr<'_>) -> io::Result<()> {
        let inodes = &mut *self.inodes.lock().unwrap();

        let inode = match inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let inode = &*inode.lock().unwrap();

        let value = match inode.xattrs.get(op.name()) {
            Some(value) => value,
            None => return req.reply_error(libc::ENODATA),
        };

        match op.size() {
            0 => {
                let mut out = XattrOut::default();
                out.size(value.len() as u32);
                req.reply(out)
            }
            size => {
                if value.len() as u32 > size {
                    return req.reply_error(libc::ERANGE);
                }
                req.reply(value)
            }
        }
    }

    fn do_setxattr(&self, req: &Request, op: op::Setxattr<'_>) -> io::Result<()> {
        let create = op.flags() as i32 & libc::XATTR_CREATE != 0;
        let replace = op.flags() as i32 & libc::XATTR_REPLACE != 0;
        if create && replace {
            return req.reply_error(libc::EINVAL);
        }

        let inodes = &mut *self.inodes.lock().unwrap();

        let inode = match inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let inode = &mut *inode.lock().unwrap();

        match inode.xattrs.entry(op.name().into()) {
            Entry::Occupied(entry) => {
                if create {
                    return req.reply_error(libc::EEXIST);
                }
                let value = Arc::make_mut(entry.into_mut());
                *value = op.value().into();
            }
            Entry::Vacant(entry) => {
                if replace {
                    return req.reply_error(libc::ENODATA);
                }
                if create {
                    entry.insert(Arc::default());
                } else {
                    entry.insert(Arc::new(op.value().into()));
                }
            }
        }

        req.reply(())
    }

    fn do_listxattr(&self, req: &Request, op: op::Listxattr<'_>) -> io::Result<()> {
        let inodes = &mut *self.inodes.lock().unwrap();

        let inode = match inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let inode = &*inode.lock().unwrap();

        match op.size() {
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
                    return req.reply_error(libc::ERANGE);
                }

                req.reply(names)
            }
        }
    }

    fn do_removexattr(&self, req: &Request, op: op::Removexattr<'_>) -> io::Result<()> {
        let inodes = &mut *self.inodes.lock().unwrap();

        let inode = match inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let inode = &mut *inode.lock().unwrap();

        match inode.xattrs.entry(op.name().into()) {
            Entry::Occupied(entry) => {
                entry.remove();
            }
            Entry::Vacant(..) => return req.reply_error(libc::ENODATA),
        }

        req.reply(())
    }

    fn do_read(&self, req: &Request, op: op::Read<'_>) -> io::Result<()> {
        let inodes = &mut *self.inodes.lock().unwrap();

        let inode = match inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let inode = &*inode.lock().unwrap();

        let content = match inode.kind {
            INodeKind::RegularFile(ref content) => content,
            _ => return req.reply_error(libc::EINVAL),
        };

        let offset = op.offset() as usize;
        let size = op.size() as usize;

        let content = content.get(offset..).unwrap_or(&[]);
        let content = &content[..std::cmp::min(content.len(), size)];

        req.reply(content)
    }

    fn do_write<T>(&self, req: &Request, op: op::Write<'_>, mut data: T) -> io::Result<()>
    where
        T: BufRead + Unpin,
    {
        let inodes = &mut *self.inodes.lock().unwrap();

        let inode = match inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return req.reply_error(libc::ENOENT),
        };
        let inode = &mut *inode.lock().unwrap();

        let content = match inode.kind {
            INodeKind::RegularFile(ref mut content) => content,
            _ => return req.reply_error(libc::EINVAL),
        };

        let offset = op.offset() as usize;
        let size = op.size() as usize;

        content.resize(std::cmp::max(content.len(), offset + size), 0);

        data.read_exact(&mut content[offset..offset + size])?;

        inode.attr.st_size = (offset + size) as libc::off_t;

        let mut out = WriteOut::default();
        out.size(op.size());

        req.reply(out)
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
