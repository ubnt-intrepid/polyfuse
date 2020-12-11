#![allow(clippy::unnecessary_mut_passed, clippy::rc_buffer)]
#![deny(clippy::unimplemented)]

use polyfuse::{
    op,
    reply::{AttrOut, EntryOut, FileAttr, OpenOut, ReaddirOut, Reply, WriteOut, XattrOut},
    Config, MountOptions, Operation, Session,
};
use polyfuse_example_async_std_support::AsyncConnection;

use async_std::{prelude::*, sync::Mutex};
use either::Either;
use futures::io::AsyncBufRead;
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap},
    ffi::{OsStr, OsString},
    fmt::Debug,
    io, mem,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tracing::Instrument as _;

#[async_std::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args
        .free_from_str()?
        .ok_or_else(|| anyhow::anyhow!("missing mountpoint"))?;
    anyhow::ensure!(mountpoint.is_dir(), "the mountpoint must be a directory");

    let conn = AsyncConnection::open(mountpoint, MountOptions::default()).await?;
    let session = Session::start(&conn, &conn, Config::default()).await?;

    let fs = MemFS::new();

    while let Some(req) = session.next_request(&conn).await? {
        let span = tracing::debug_span!("MemFS::call", unique = req.unique());

        let op = req.operation()?;
        span.in_scope(|| tracing::debug!(?op));

        macro_rules! try_reply {
            ($e:expr) => {
                match ($e).instrument(span.clone()).await {
                    Ok(data) => {
                        span.in_scope(|| tracing::debug!(data=?data));
                        polyfuse::bytes::write_bytes(
                            &conn,
                            Reply::new(req.unique(), 0, data)
                        )?;
                    }
                    Err(err) => {
                        let errno = err.raw_os_error().unwrap_or(libc::EIO);
                        span.in_scope(|| tracing::debug!(errno=errno));
                        polyfuse::bytes::write_bytes(
                            &conn,
                            Reply::new(req.unique(), errno, ())
                        )?;
                    }
                }
            };
        }

        match op {
            Operation::Lookup(op) => try_reply!(fs.do_lookup(&op)),
            Operation::Forget(forgets) => {
                fs.do_forget(forgets.as_ref()).await;
            }
            Operation::Getattr(op) => try_reply!(fs.do_getattr(&op)),
            Operation::Setattr(op) => try_reply!(fs.do_setattr(&op)),
            Operation::Readlink(op) => try_reply!(fs.do_readlink(&op)),

            Operation::Opendir(op) => try_reply!(fs.do_opendir(&op)),
            Operation::Readdir(op) => try_reply!(fs.do_readdir(&op)),
            Operation::Releasedir(op) => try_reply!(fs.do_releasedir(&op)),

            Operation::Mknod(op) => try_reply!(fs.do_mknod(&op)),
            Operation::Mkdir(op) => try_reply!(fs.do_mkdir(&op)),
            Operation::Symlink(op) => try_reply!(fs.do_symlink(&op)),
            Operation::Unlink(op) => try_reply!(fs.do_unlink(&op)),
            Operation::Rmdir(op) => try_reply!(fs.do_rmdir(&op)),
            Operation::Link(op) => try_reply!(fs.do_link(&op)),
            Operation::Rename(op) => try_reply!(fs.do_rename(&op)),

            Operation::Getxattr(op) => try_reply!(fs.do_getxattr(&op)),
            Operation::Setxattr(op) => try_reply!(fs.do_setxattr(&op)),
            Operation::Listxattr(op) => try_reply!(fs.do_listxattr(&op)),
            Operation::Removexattr(op) => try_reply!(fs.do_removexattr(&op)),

            Operation::Read(op) => try_reply!(fs.do_read(&op)),
            Operation::Write(op, data) => try_reply!(fs.do_write(&op, data)),

            _ => {
                span.in_scope(|| tracing::debug!("NOSYS"));
                polyfuse::bytes::write_bytes(&conn, Reply::new(req.unique(), libc::ENOSYS, ()))?;
            }
        }
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

    fn make_entry_out(&self, ino: Ino, st: &libc::stat) -> EntryOut {
        let mut out = EntryOut::default();
        out.ino(ino);
        fill_attr(out.attr(), st);
        out.ttl_entry(self.ttl);
        out
    }

    async fn lookup_inode(&self, parent: Ino, name: &OsStr) -> io::Result<EntryOut> {
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

        Ok(self.make_entry_out(child_ino, &child.attr))
    }

    async fn make_node<F>(&self, parent: Ino, name: &OsStr, f: F) -> io::Result<EntryOut>
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

                let reply = self.make_entry_out(inode_entry.ino(), &inode.attr);
                map_entry.insert(inode_entry.ino());
                inode_entry.insert(inode);

                Ok(reply)
            }
        }
    }

    async fn link_node(&self, ino: u64, newparent: Ino, newname: &OsStr) -> io::Result<EntryOut> {
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
                    inode.attr.st_nlink += 1;
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
        inode.attr.st_nlink = inode.attr.st_nlink.saturating_sub(1);

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

    async fn do_lookup(&self, op: &op::Lookup<'_>) -> io::Result<EntryOut> {
        self.lookup_inode(op.parent(), op.name()).await
    }

    async fn do_forget(&self, forgets: &[op::Forget]) {
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

    async fn do_getattr(&self, op: &op::Getattr<'_>) -> io::Result<AttrOut> {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        let mut out = AttrOut::default();
        fill_attr(out.attr(), &inode.attr);
        out.ttl(self.ttl);

        Ok(out)
    }

    async fn do_setattr(&self, op: &op::Setattr<'_>) -> io::Result<AttrOut> {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let mut inode = inode.lock().await;

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

        Ok(out)
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

    async fn do_opendir(&self, op: &op::Opendir<'_>) -> io::Result<OpenOut> {
        let inodes = self.inodes.lock().await;
        let mut dirs = self.dir_handles.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;
        if inode.attr.st_nlink == 0 {
            return Err(no_entry());
        }
        let dir = match inode.kind {
            INodeKind::Directory(ref dir) => dir,
            _ => return Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
        };

        let key = dirs.insert(Arc::new(Mutex::new(DirHandle {
            entries: dir.collect_entries(&inode.attr),
        })));

        let mut out = OpenOut::default();
        out.fh(key as u64);

        Ok(out)
    }

    async fn do_readdir(&self, op: &op::Readdir<'_>) -> io::Result<ReaddirOut> {
        if op.mode() == op::ReaddirMode::Plus {
            return Err(io::Error::from_raw_os_error(libc::ENOSYS));
        }

        let dirs = self.dir_handles.lock().await;

        let dir = dirs
            .get(op.fh() as usize)
            .cloned()
            .ok_or_else(unknown_error)?;
        let dir = dir.lock().await;

        let mut out = ReaddirOut::new(op.size() as usize);

        for entry in dir.entries.iter().skip(op.offset() as usize) {
            if out.entry(&entry.name, entry.ino, entry.typ, entry.off) {
                break;
            }
        }

        Ok(out)
    }

    async fn do_releasedir(&self, op: &op::Releasedir<'_>) -> io::Result<()> {
        let mut dirs = self.dir_handles.lock().await;

        let dir = dirs.remove(op.fh() as usize);
        drop(dir);

        Ok(())
    }

    async fn do_mknod(&self, op: &op::Mknod<'_>) -> io::Result<EntryOut> {
        match op.mode() & libc::S_IFMT {
            libc::S_IFREG => (),
            _ => return Err(io::Error::from_raw_os_error(libc::ENOTSUP)),
        }

        self.make_node(op.parent(), op.name(), |entry| INode {
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
        .await
    }

    async fn do_mkdir(&self, op: &op::Mkdir<'_>) -> io::Result<EntryOut> {
        self.make_node(op.parent(), op.name(), |entry| INode {
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
        .await
    }

    async fn do_symlink(&self, op: &op::Symlink<'_>) -> io::Result<EntryOut> {
        self.make_node(op.parent(), op.name(), |entry| INode {
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
        .await
    }

    async fn do_link(&self, op: &op::Link<'_>) -> io::Result<EntryOut> {
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

    async fn do_getxattr(
        &self,
        op: &op::Getxattr<'_>,
    ) -> io::Result<impl polyfuse::bytes::Bytes + Debug> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        let value = inode
            .xattrs
            .get(op.name())
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENODATA))?;

        match op.size() {
            0 => {
                let mut out = XattrOut::default();
                out.size(value.len() as u32);
                Ok(Either::Left(out))
            }
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

    async fn do_listxattr(
        &self,
        op: &op::Listxattr<'_>,
    ) -> io::Result<impl polyfuse::bytes::Bytes + Debug> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        match op.size() {
            0 => {
                let total_len = inode.xattrs.keys().map(|name| name.len() as u32 + 1).sum();
                let mut out = XattrOut::default();
                out.size(total_len);
                Ok(Either::Left(out))
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

    async fn do_read(&self, op: &op::Read<'_>) -> io::Result<impl polyfuse::bytes::Bytes + Debug> {
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

    async fn do_write<T>(&self, op: &op::Write<'_>, mut data: T) -> io::Result<WriteOut>
    where
        T: AsyncBufRead + Unpin,
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

        data.read_exact(&mut content[offset..offset + size]).await?;

        inode.attr.st_size = (offset + size) as libc::off_t;

        let mut out = WriteOut::default();
        out.size(op.size());

        Ok(out)
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

fn no_entry() -> io::Error {
    io::Error::from_raw_os_error(libc::ENOENT)
}

fn unknown_error() -> io::Error {
    io::Error::from_raw_os_error(libc::EIO)
}
