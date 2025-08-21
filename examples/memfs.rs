#![allow(clippy::unnecessary_mut_passed, clippy::rc_buffer)]
#![deny(clippy::unimplemented)]

use polyfuse::{
    mount::{mount, MountOptions},
    op,
    reply::{AttrOut, EntryOut, FileAttr, OpenOut, ReaddirOut, WriteOut, XattrOut},
    Connection, KernelConfig, Operation, Request, Session,
};

use anyhow::{ensure, Context as _, Result};
use dashmap::DashMap;
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap, RandomState},
    ffi::{OsStr, OsString},
    io::{self, Read},
    mem,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_dir(), "mountpoint must be a directory");

    let (conn, fusermount) = mount(mountpoint, MountOptions::default())?;
    let mut conn = Connection::from(conn);
    let session = Session::init(&mut conn, KernelConfig::default())?;

    let mut fs = MemFS::new();

    let mut req = session.new_request_buffer()?;
    while session.read_request(&mut conn, &mut req)? {
        let span = tracing::debug_span!("handle_request", unique = req.unique());
        let _enter = span.enter();

        let op = req.operation()?;
        tracing::debug!(?op);

        let cx = &mut Context {
            session: &session,
            conn: &mut conn,
            request: &req,
        };

        match op {
            Operation::Lookup(op) => fs.do_lookup(cx, op)?,
            Operation::Forget(forgets) => {
                fs.do_forget(forgets.as_ref());
            }
            Operation::Getattr(op) => fs.do_getattr(cx, op)?,
            Operation::Setattr(op) => fs.do_setattr(cx, op)?,
            Operation::Readlink(op) => fs.do_readlink(cx, op)?,

            Operation::Opendir(op) => fs.do_opendir(cx, op)?,
            Operation::Readdir(op) => fs.do_readdir(cx, op)?,
            Operation::Releasedir(op) => fs.do_releasedir(cx, op)?,

            Operation::Mknod(op) => fs.do_mknod(cx, op)?,
            Operation::Mkdir(op) => fs.do_mkdir(cx, op)?,
            Operation::Symlink(op) => fs.do_symlink(cx, op)?,
            Operation::Link(op) => fs.do_link(cx, op)?,
            Operation::Unlink(op) => fs.do_unlink(cx, op)?,
            Operation::Rmdir(op) => fs.do_rmdir(cx, op)?,
            Operation::Rename(op) => fs.do_rename(cx, op)?,

            Operation::Getxattr(op) => fs.do_getxattr(cx, op)?,
            Operation::Setxattr(op) => fs.do_setxattr(cx, op)?,
            Operation::Listxattr(op) => fs.do_listxattr(cx, op)?,
            Operation::Removexattr(op) => fs.do_removexattr(cx, op)?,

            Operation::Read(op) => fs.do_read(cx, op)?,
            Operation::Write(op) => fs.do_write(cx, op)?,

            _ => {
                tracing::debug!("NOSYS");
                session.reply_error(&mut conn, &req, libc::ENOSYS)?;
            }
        }
    }

    fusermount.unmount()?;

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

struct Context<'req> {
    session: &'req Session,
    conn: &'req mut Connection,
    request: &'req Request,
}

impl Context<'_> {
    fn reply<T>(&mut self, arg: T) -> io::Result<()>
    where
        T: polyfuse::bytes::Bytes,
    {
        self.session.reply(&mut *self.conn, self.request, arg)
    }

    fn reply_error(&mut self, code: i32) -> io::Result<()> {
        self.session
            .reply_error(&mut *self.conn, self.request, code)
    }
}

struct MemFS {
    inodes: INodeTable,
    dir_handles: Slab<DirHandle>,
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
            dir_handles: Slab::new(),
            ttl: Duration::from_secs(60 * 60 * 24),
        }
    }

    fn do_lookup(&mut self, cx: &mut Context<'_>, op: op::Lookup<'_>) -> io::Result<()> {
        let parent = match self.inodes.get(op.parent()) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };

        let parent = match parent.kind {
            INodeKind::Directory(ref dir) => dir,
            _ => return cx.reply_error(libc::ENOTDIR),
        };

        let child_ino = match parent.children.get(op.name()) {
            Some(&ino) => ino,
            None => return cx.reply_error(libc::ENOENT),
        };
        let mut child = self
            .inodes
            .get_mut(child_ino)
            .unwrap_or_else(|| unreachable!());
        child.refcount += 1;

        let mut out = EntryOut::default();
        out.ino(child_ino);
        fill_attr(out.attr(), &child.attr);
        out.ttl_entry(self.ttl);

        cx.reply(out)
    }

    fn do_forget(&self, forgets: &[op::Forget]) {
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

    fn do_getattr(&mut self, cx: &mut Context<'_>, op: op::Getattr<'_>) -> io::Result<()> {
        let inode = match self.inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };

        let mut out = AttrOut::default();
        fill_attr(out.attr(), &inode.attr);
        out.ttl(self.ttl);

        cx.reply(out)
    }

    fn do_setattr(&mut self, cx: &mut Context<'_>, op: op::Setattr<'_>) -> io::Result<()> {
        let mut inode = match self.inodes.get_mut(op.ino()) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };

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

    fn do_readlink(&mut self, cx: &mut Context<'_>, op: op::Readlink<'_>) -> io::Result<()> {
        let inode = match self.inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };

        let link = match inode.kind {
            INodeKind::Symlink(ref link) => link,
            _ => return cx.reply_error(libc::EINVAL),
        };

        cx.reply(link)
    }

    fn do_opendir(&mut self, cx: &mut Context<'_>, op: op::Opendir<'_>) -> io::Result<()> {
        let inode = match self.inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };
        if inode.attr.st_nlink == 0 {
            return cx.reply_error(libc::ENOENT);
        }
        let dir = match inode.kind {
            INodeKind::Directory(ref dir) => dir,
            _ => return cx.reply_error(libc::ENOTDIR),
        };

        let key = self.dir_handles.insert(DirHandle {
            entries: dir.collect_entries(&inode.attr),
            offset: AtomicUsize::new(0),
        });

        let mut out = OpenOut::default();
        out.fh(key as u64);

        cx.reply(out)
    }

    fn do_readdir(&mut self, cx: &mut Context<'_>, op: op::Readdir<'_>) -> io::Result<()> {
        if op.mode() == op::ReaddirMode::Plus {
            return cx.reply_error(libc::ENOSYS);
        }

        let dir = match self.dir_handles.get(op.fh() as usize) {
            Some(dir) => dir,
            None => return cx.reply_error(libc::EINVAL),
        };

        let mut out = ReaddirOut::new(op.size() as usize);

        for entry in dir.entries.iter().skip(op.offset() as usize) {
            if out.entry(&entry.name, entry.ino, entry.typ, entry.off) {
                break;
            }
            dir.offset.fetch_add(1, Ordering::SeqCst);
        }

        cx.reply(out)
    }

    fn do_releasedir(&mut self, cx: &mut Context<'_>, op: op::Releasedir<'_>) -> io::Result<()> {
        self.dir_handles.remove(op.fh() as usize);
        cx.reply(())
    }

    fn do_mknod(&mut self, cx: &mut Context<'_>, op: op::Mknod<'_>) -> io::Result<()> {
        match op.mode() & libc::S_IFMT {
            libc::S_IFREG => (),
            _ => return cx.reply_error(libc::ENOTSUP),
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

    fn do_mkdir(&mut self, cx: &mut Context<'_>, op: op::Mkdir<'_>) -> io::Result<()> {
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

    fn do_symlink(&mut self, cx: &mut Context<'_>, op: op::Symlink<'_>) -> io::Result<()> {
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

    fn make_node<F>(
        &mut self,
        cx: &mut Context<'_>,
        parent: Ino,
        name: &OsStr,
        f: F,
    ) -> io::Result<()>
    where
        F: FnOnce(&VacantEntry<'_>) -> INode,
    {
        let mut parent = match self.inodes.get_mut(parent) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };
        let parent = match parent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return cx.reply_error(libc::ENOTDIR),
        };

        let map_entry = match parent.children.entry(name.into()) {
            Entry::Occupied(..) => return cx.reply_error(libc::EEXIST),
            Entry::Vacant(map_entry) => map_entry,
        };
        let inode_entry = self.inodes.vacant_entry().expect("inode number conflict");
        let inode = f(&inode_entry);

        let mut out = EntryOut::default();
        out.ino(inode_entry.ino());
        fill_attr(out.attr(), &inode.attr);
        out.ttl_entry(self.ttl);
        cx.reply(out)?;

        map_entry.insert(inode_entry.ino());
        inode_entry.insert(inode);

        Ok(())
    }

    fn do_link(&mut self, cx: &mut Context<'_>, op: op::Link<'_>) -> io::Result<()> {
        let mut inode = match self.inodes.get_mut(op.ino()) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };

        debug_assert!(op.ino() != op.newparent());
        let mut newparent = match self.inodes.get_mut(op.newparent()) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };
        let newparent = match newparent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return cx.reply_error(libc::ENOTDIR),
        };

        match newparent.children.entry(op.newname().into()) {
            Entry::Occupied(..) => return cx.reply_error(libc::EEXIST),
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

    fn do_unlink(&mut self, cx: &mut Context<'_>, op: op::Unlink<'_>) -> io::Result<()> {
        self.unlink_node(cx, op.parent(), op.name())
    }

    fn do_rmdir(&mut self, cx: &mut Context<'_>, op: op::Rmdir<'_>) -> io::Result<()> {
        self.unlink_node(cx, op.parent(), op.name())
    }

    fn unlink_node(&mut self, cx: &mut Context<'_>, parent: Ino, name: &OsStr) -> io::Result<()> {
        let mut parent = match self.inodes.get_mut(parent) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };
        let parent: &mut Directory = match parent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return cx.reply_error(libc::ENOTDIR),
        };

        let ino = match parent.children.get(name) {
            Some(&ino) => ino,
            None => return cx.reply_error(libc::ENOENT),
        };

        let mut inode = self.inodes.get_mut(ino).unwrap_or_else(|| unreachable!());
        match inode.kind {
            INodeKind::Directory(ref dir) if !dir.children.is_empty() => {
                return cx.reply_error(libc::ENOTEMPTY);
            }
            _ => (),
        }

        parent
            .children
            .remove(name)
            .unwrap_or_else(|| unreachable!());

        inode.links = inode.links.saturating_sub(1);
        inode.attr.st_nlink = inode.attr.st_nlink.saturating_sub(1);

        cx.reply(())
    }

    fn do_rename(&mut self, cx: &mut Context<'_>, op: op::Rename<'_>) -> io::Result<()> {
        if op.flags() != 0 {
            // TODO: handle RENAME_NOREPLACE and RENAME_EXCHANGE.
            return cx.reply_error(libc::EINVAL);
        }

        let mut parent = match self.inodes.get_mut(op.parent()) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };
        let parent = match parent.kind {
            INodeKind::Directory(ref mut dir) => dir,
            _ => return cx.reply_error(libc::ENOTDIR),
        };

        match op.newparent() {
            newparent if newparent == op.parent() => {
                let ino = match parent.children.get(op.name()) {
                    Some(&ino) => ino,
                    None => return cx.reply_error(libc::ENOENT),
                };

                match parent.children.entry(op.newname().into()) {
                    Entry::Occupied(..) => return cx.reply_error(libc::EEXIST),
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
                let mut newparent = match self.inodes.get_mut(newparent) {
                    Some(inode) => inode,
                    None => return cx.reply_error(libc::ENOENT),
                };
                let newparent = match newparent.kind {
                    INodeKind::Directory(ref mut dir) => dir,
                    _ => return cx.reply_error(libc::ENOTDIR),
                };

                let entry = match newparent.children.entry(op.newname().into()) {
                    Entry::Occupied(..) => return cx.reply_error(libc::EEXIST),
                    Entry::Vacant(entry) => entry,
                };
                let ino = match parent.children.remove(op.name()) {
                    Some(ino) => ino,
                    None => return cx.reply_error(libc::ENOENT),
                };
                entry.insert(ino);
            }
        }

        cx.reply(())
    }

    fn do_getxattr(&mut self, cx: &mut Context<'_>, op: op::Getxattr<'_>) -> io::Result<()> {
        let inode = match self.inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };

        let value = match inode.xattrs.get(op.name()) {
            Some(value) => value,
            None => return cx.reply_error(libc::ENODATA),
        };

        match op.size() {
            0 => {
                let mut out = XattrOut::default();
                out.size(value.len() as u32);
                cx.reply(out)
            }
            size => {
                if value.len() as u32 > size {
                    return cx.reply_error(libc::ERANGE);
                }
                cx.reply(value)
            }
        }
    }

    fn do_setxattr(&mut self, cx: &mut Context<'_>, op: op::Setxattr<'_>) -> io::Result<()> {
        let create = op.flags() as i32 & libc::XATTR_CREATE != 0;
        let replace = op.flags() as i32 & libc::XATTR_REPLACE != 0;
        if create && replace {
            return cx.reply_error(libc::EINVAL);
        }

        let mut inode = match self.inodes.get_mut(op.ino()) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };

        match inode.xattrs.entry(op.name().into()) {
            Entry::Occupied(entry) => {
                if create {
                    return cx.reply_error(libc::EEXIST);
                }
                let value = Arc::make_mut(entry.into_mut());
                *value = op.value().into();
            }
            Entry::Vacant(entry) => {
                if replace {
                    return cx.reply_error(libc::ENODATA);
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

    fn do_listxattr(&mut self, cx: &mut Context<'_>, op: op::Listxattr<'_>) -> io::Result<()> {
        let inode = match self.inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };

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
                    return cx.reply_error(libc::ERANGE);
                }

                cx.reply(names)
            }
        }
    }

    fn do_removexattr(&mut self, cx: &mut Context<'_>, op: op::Removexattr<'_>) -> io::Result<()> {
        let mut inode = match self.inodes.get_mut(op.ino()) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };

        match inode.xattrs.entry(op.name().into()) {
            Entry::Occupied(entry) => {
                entry.remove();
            }
            Entry::Vacant(..) => return cx.reply_error(libc::ENODATA),
        }

        cx.reply(())
    }

    fn do_read(&mut self, cx: &mut Context<'_>, op: op::Read<'_>) -> io::Result<()> {
        let inode = match self.inodes.get(op.ino()) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };

        let content = match inode.kind {
            INodeKind::RegularFile(ref content) => content,
            _ => return cx.reply_error(libc::EINVAL),
        };

        let offset = op.offset() as usize;
        let size = op.size() as usize;

        let content = content.get(offset..).unwrap_or(&[]);
        let content = &content[..std::cmp::min(content.len(), size)];

        cx.reply(content)
    }

    fn do_write(&mut self, cx: &mut Context<'_>, op: op::Write<'_>) -> io::Result<()> {
        let mut inode = match self.inodes.get_mut(op.ino()) {
            Some(inode) => inode,
            None => return cx.reply_error(libc::ENOENT),
        };

        let content = match inode.kind {
            INodeKind::RegularFile(ref mut content) => content,
            _ => return cx.reply_error(libc::EINVAL),
        };

        let offset = op.offset() as usize;
        let size = op.size() as usize;

        content.resize(std::cmp::max(content.len(), offset + size), 0);

        cx.request.read_exact(&mut content[offset..offset + size])?;

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
