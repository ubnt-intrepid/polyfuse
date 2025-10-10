#![allow(clippy::unnecessary_mut_passed)]
#![warn(clippy::unimplemented, clippy::todo)]

mod nix;

use polyfuse::{
    fs::{self, Daemon, Filesystem},
    mount::{MountFlags, MountOptions},
    op::{self, OpenFlags},
    reply::{
        self, AttrOut, EntryOut, OpenOut, OpenOutFlags, ReaddirOut, StatfsOut, WriteOut, XattrOut,
    },
    session::{KernelConfig, KernelFlags},
    types::{DeviceID, FileID, FileMode, FilePermissions, FileType, NodeID},
};

use crate::nix::{FileDesc, ReadDir};
use anyhow::{ensure, Context as _, Result};
use libc::{
    AT_REMOVEDIR, AT_SYMLINK_FOLLOW, AT_SYMLINK_NOFOLLOW, O_NOFOLLOW, O_PATH, S_IFLNK, S_IFMT,
    UTIME_NOW, UTIME_OMIT,
};
use pico_args::Arguments;
use rustix::io::Errno;
use slab::Slab;
use std::{
    borrow::Cow,
    collections::hash_map::{Entry, HashMap},
    ffi::OsStr,
    fs::{File, OpenOptions},
    io::{self, prelude::*},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = Arguments::from_env();

    let source: PathBuf = args
        .opt_value_from_str(["-s", "--source"])?
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    ensure!(source.is_dir(), "the source path must be a directory");

    let timeout = if args.contains("--no-cache") {
        None
    } else {
        Some(Duration::from_secs(60 * 60 * 24)) // one day
    };

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_dir(), "mountpoint must be a directory");

    let mut mountopts = MountOptions::new();
    mountopts.flags |= MountFlags::DEFAULT_PERMISSIONS;
    mountopts.fsname = Some("passthrough".into());

    let mut config = KernelConfig::default();
    config.flags |= KernelFlags::EXPORT_SUPPORT;
    config.flags |= KernelFlags::FLOCK_LOCKS;
    config.flags |= KernelFlags::WRITEBACK_CACHE;

    let daemon = Daemon::mount(mountpoint, mountopts, config)?;

    let fs = Passthrough::new(source, timeout)?;
    daemon.run(Arc::new(fs), None)?;

    Ok(())
}

type SrcId = (u64, libc::dev_t);

struct Passthrough {
    inodes: Mutex<INodeTable>,
    opened_dirs: HandlePool<Mutex<ReadDir>>,
    opened_files: HandlePool<Mutex<File>>,
    timeout: Option<Duration>,
}

impl Passthrough {
    fn new(source: PathBuf, timeout: Option<Duration>) -> io::Result<Self> {
        let source = source.canonicalize()?;
        tracing::debug!("source={:?}", source);
        let fd = FileDesc::open(&source, O_PATH)?;
        let stat = fd.fstatat("", AT_SYMLINK_NOFOLLOW)?;

        let mut inodes = INodeTable::new();
        let entry = inodes.vacant_entry();
        debug_assert_eq!(entry.ino(), NodeID::ROOT);
        entry.insert(INode {
            ino: NodeID::ROOT,
            fd,
            refcount: u64::max_value() / 2, // the root node's cache is never removed.
            src_id: (stat.st_ino, stat.st_dev),
            is_symlink: false,
        });

        Ok(Self {
            inodes: Mutex::new(inodes),
            opened_dirs: HandlePool::default(),
            opened_files: HandlePool::default(),
            timeout,
        })
    }

    fn do_lookup(&self, parent: NodeID, name: &OsStr) -> fs::Result<EntryOut> {
        let mut inodes = self.inodes.lock().unwrap();
        let inodes = &mut *inodes;

        let parent = inodes.get(parent).ok_or(Errno::NOENT)?;
        let parent = parent.lock().unwrap();

        let fd = parent.fd.openat(name, O_PATH | O_NOFOLLOW)?;

        let stat = fd.fstatat("", AT_SYMLINK_NOFOLLOW)?;
        let src_id = (stat.st_ino, stat.st_dev);
        let is_symlink = stat.st_mode & S_IFMT == S_IFLNK;

        let ino;
        match inodes.get_src(src_id) {
            Some(inode) => {
                let mut inode = inode.lock().unwrap();
                ino = inode.ino;
                inode.refcount += 1;
                tracing::debug!(
                    "update the lookup count: ino={}, refcount={}",
                    inode.ino,
                    inode.refcount
                );
            }
            None => {
                let entry = inodes.vacant_entry();
                ino = entry.ino();
                tracing::debug!("create a new inode cache: ino={}", ino);
                entry.insert(INode {
                    ino,
                    fd,
                    refcount: 1,
                    src_id,
                    is_symlink,
                });
            }
        }

        Ok(EntryOut {
            ino: Some(ino),
            attr: Cow::Owned(stat.try_into().map_err(|_| Errno::INVAL)?),
            generation: 0,
            entry_valid: self.timeout,
            attr_valid: self.timeout,
        })
    }

    fn make_node(
        &self,
        parent: NodeID,
        name: &OsStr,
        mode: FileMode,
        rdev: Option<DeviceID>,
        link: Option<&OsStr>,
    ) -> fs::Result<EntryOut> {
        {
            let inodes = self.inodes.lock().unwrap();
            let parent = inodes.get(parent).ok_or(Errno::NOENT)?;
            let parent = parent.lock().unwrap();

            match mode.file_type() {
                Some(FileType::Directory) => {
                    parent.fd.mkdirat(name, mode.permissions().bits())?;
                }
                Some(FileType::SymbolicLink) => {
                    let link = link.expect("missing 'link'");
                    parent.fd.symlinkat(name, link)?;
                }
                _ => {
                    parent.fd.mknodat(
                        name,
                        mode.into_raw(),
                        rdev.map_or(0, DeviceID::into_userspace_dev),
                    )?;
                }
            }
        }
        self.do_lookup(parent, name)
    }
}

impl Filesystem for Passthrough {
    fn lookup(self: &Arc<Self>, req: fs::Request<'_>, op: op::Lookup<'_>) -> fs::Result {
        let out = self.do_lookup(op.parent, op.name)?;
        req.reply(out)
    }

    fn forget(self: &Arc<Self>, forgets: &[op::Forget]) {
        let mut inodes = self.inodes.lock().unwrap();

        for forget in forgets {
            if let Entry::Occupied(mut entry) = inodes.map.entry(forget.ino()) {
                let refcount = {
                    let mut inode = entry.get_mut().lock().unwrap();
                    inode.refcount = inode.refcount.saturating_sub(forget.nlookup());
                    inode.refcount
                };

                if refcount == 0 {
                    tracing::debug!("remove ino={}", entry.key());
                    drop(entry.remove());
                }
            }
        }
    }

    fn getattr(self: &Arc<Self>, req: fs::Request<'_>, op: op::Getattr<'_>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();

        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let inode = inode.lock().unwrap();

        let stat = inode.fd.fstatat("", AT_SYMLINK_NOFOLLOW)?;

        req.reply(AttrOut {
            attr: Cow::Owned(stat.try_into().map_err(|_| Errno::INVAL)?),
            valid: self.timeout,
        })
    }

    #[allow(clippy::cognitive_complexity)]
    fn setattr(self: &Arc<Self>, req: fs::Request<'_>, op: op::Setattr<'_>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let inode = inode.lock().unwrap();
        let fd = &inode.fd;

        let mut file = if let Some(fh) = op.fh {
            Some(self.opened_files.get(fh).ok_or(Errno::NOENT)?)
        } else {
            None
        };
        let mut file = file.as_mut().map(|file| file.lock().unwrap());

        // chmod
        if let Some(mode) = op.mode {
            if let Some(file) = file.as_mut() {
                nix::fchmod(&**file, mode.into_raw())?;
            } else {
                nix::chmod(fd.procname(), mode.into_raw())?;
            }
        }

        // chown
        match (op.uid, op.gid) {
            (None, None) => (),
            (uid, gid) => {
                fd.fchownat(
                    "",
                    uid.map(|id| id.as_raw()),
                    gid.map(|id| id.as_raw()),
                    AT_SYMLINK_NOFOLLOW,
                )?;
            }
        }

        // truncate
        if let Some(size) = op.size {
            if let Some(file) = file.as_mut() {
                nix::ftruncate(&**file, size as libc::off_t)?;
            } else {
                nix::truncate(fd.procname(), size as libc::off_t)?;
            }
        }

        // utimens
        fn make_timespec(t: Option<op::SetAttrTime>) -> libc::timespec {
            match t {
                Some(op::SetAttrTime::Now) => libc::timespec {
                    tv_sec: 0,
                    tv_nsec: UTIME_NOW,
                },
                Some(op::SetAttrTime::Timespec(ts)) => libc::timespec {
                    tv_sec: ts.as_secs() as i64,
                    tv_nsec: ts.subsec_nanos() as u64 as i64,
                },
                _ => libc::timespec {
                    tv_sec: 0,
                    tv_nsec: UTIME_OMIT,
                },
            }
        }
        match (op.atime, op.mtime) {
            (None, None) => (),
            (atime, mtime) => {
                let tv = [make_timespec(atime), make_timespec(mtime)];
                if let Some(file) = file.as_mut() {
                    nix::futimens(&**file, tv)?;
                } else if inode.is_symlink {
                    // According to libfuse/examples/passthrough_hp.cc, it does not work on
                    // the current kernels, but may in the future.
                    fd.futimensat("", tv, AT_SYMLINK_NOFOLLOW) //
                        .map_err(|err| match Errno::from_io_error(&err) {
                            Some(Errno::INVAL) => Errno::PERM.into(),
                            _ => err,
                        })?;
                } else {
                    nix::utimens(fd.procname(), tv)?;
                }
            }
        }

        // finally, acquiring the latest metadata from the source filesystem.
        let stat = fd.fstatat("", AT_SYMLINK_NOFOLLOW)?;

        req.reply(AttrOut {
            attr: Cow::Owned(stat.try_into().map_err(|_| Errno::INVAL)?),
            valid: self.timeout,
        })
    }

    fn readlink(self: &Arc<Self>, req: fs::Request<'_>, op: op::Readlink<'_>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let inode = inode.lock().unwrap();
        let link = inode.fd.readlinkat("")?;
        req.reply(reply::Raw(link))
    }

    fn link(self: &Arc<Self>, req: fs::Request<'_>, op: op::Link<'_>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();

        let source = inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let mut source = source.lock().unwrap();

        let parent = inodes.get(op.newparent).ok_or(Errno::NOENT)?;
        let parent = parent.lock().unwrap();

        if source.is_symlink {
            source
                .fd
                .linkat("", &parent.fd, op.newname, 0) //
                .map_err(|err| match Errno::from_io_error(&err) {
                    Some(Errno::NOENT) | Some(Errno::INVAL) => {
                        // no race-free way to hard-link a symlink.
                        Errno::OPNOTSUPP.into()
                    }
                    _ => err,
                })?;
        } else {
            nix::link(
                source.fd.procname(),
                &parent.fd,
                op.newname,
                AT_SYMLINK_FOLLOW,
            )?;
        }

        let stat = source.fd.fstatat("", AT_SYMLINK_NOFOLLOW)?;

        let replied = req.reply(EntryOut {
            ino: Some(source.ino),
            attr: Cow::Owned(stat.try_into().map_err(|_| Errno::INVAL)?),
            attr_valid: self.timeout,
            entry_valid: self.timeout,
            generation: 0,
        })?;

        source.refcount += 1;

        Ok(replied)
    }

    fn mknod(self: &Arc<Self>, req: fs::Request<'_>, op: op::Mknod<'_>) -> fs::Result {
        let out = self.make_node(op.parent, op.name, op.mode, Some(op.rdev), None)?;
        req.reply(out)
    }

    fn mkdir(self: &Arc<Self>, req: fs::Request<'_>, op: op::Mkdir<'_>) -> fs::Result {
        let out = self.make_node(
            op.parent,
            op.name,
            FileMode::new(FileType::Directory, op.permissions),
            None,
            None,
        )?;
        req.reply(out)
    }

    fn symlink(self: &Arc<Self>, req: fs::Request<'_>, op: op::Symlink<'_>) -> fs::Result {
        let out = self.make_node(
            op.parent,
            op.name,
            FileMode::new(FileType::SymbolicLink, FilePermissions::empty()),
            None,
            Some(op.link),
        )?;
        req.reply(out)
    }

    fn unlink(self: &Arc<Self>, req: fs::Request<'_>, op: op::Unlink<'_>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let parent = inodes.get(op.parent).ok_or(Errno::NOENT)?;
        let parent = parent.lock().unwrap();
        parent.fd.unlinkat(op.name, 0)?;
        req.reply(())
    }

    fn rmdir(self: &Arc<Self>, req: fs::Request<'_>, op: op::Rmdir<'_>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let parent = inodes.get(op.parent).ok_or(Errno::NOENT)?;
        let parent = parent.lock().unwrap();
        parent.fd.unlinkat(op.name, AT_REMOVEDIR)?;
        req.reply(())
    }

    fn rename(self: &Arc<Self>, req: fs::Request<'_>, op: op::Rename<'_>) -> fs::Result {
        if !op.flags.is_empty() {
            // rename2 is not supported.
            return Err(Errno::INVAL.into());
        }

        let inodes = self.inodes.lock().unwrap();

        let parent = inodes.get(op.parent).ok_or(Errno::NOENT)?;
        let newparent = inodes.get(op.newparent).ok_or(Errno::NOENT)?;

        let parent = parent.lock().unwrap();
        if op.parent == op.newparent {
            parent.fd.renameat(op.name, None::<&FileDesc>, op.newname)?;
        } else {
            let newparent = newparent.lock().unwrap();
            parent
                .fd
                .renameat(op.name, Some(&newparent.fd), op.newname)?;
        }

        req.reply(())
    }

    fn opendir(self: &Arc<Self>, req: fs::Request<'_>, op: op::Opendir<'_>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let inode = inode.lock().unwrap();
        let dir = inode.fd.read_dir()?;
        let fh = self.opened_dirs.insert(Mutex::new(dir));

        req.reply(OpenOut {
            fh,
            open_flags: OpenOutFlags::empty(),
        })
    }

    fn readdir(self: &Arc<Self>, req: fs::Request<'_>, op: op::Readdir<'_>) -> fs::Result {
        if op.mode == op::ReaddirMode::Plus {
            return Err(Errno::NOSYS.into());
        }

        let read_dir = self.opened_dirs.get(op.fh).ok_or(Errno::NOENT)?;
        let read_dir = &mut *read_dir.lock().unwrap();
        read_dir.seek(op.offset);

        let mut out = ReaddirOut::new(op.size as usize);
        for entry in read_dir.by_ref() {
            let entry = entry?;
            if out.push_entry(
                &entry.name,
                entry.ino,
                FileType::from_dirent_type(entry.typ),
                entry.off,
            ) {
                break;
            }
        }

        req.reply(out)
    }

    fn fsyncdir(self: &Arc<Self>, req: fs::Request<'_>, op: op::Fsyncdir<'_>) -> fs::Result {
        let read_dir = self.opened_dirs.get(op.fh).ok_or(Errno::NOENT)?;
        let read_dir = read_dir.lock().unwrap();

        if op.datasync {
            read_dir.sync_data()?;
        } else {
            read_dir.sync_all()?;
        }

        req.reply(())
    }

    fn releasedir(self: &Arc<Self>, req: fs::Request<'_>, op: op::Releasedir<'_>) -> fs::Result {
        let _dir = self.opened_dirs.remove(op.fh);
        req.reply(())
    }

    fn open(self: &Arc<Self>, req: fs::Request<'_>, op: op::Open<'_>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let inode = inode.lock().unwrap();

        let options: OpenOptions = {
            let mut options = op.options;
            options.set_flags(options.flags() & !OpenFlags::NOFOLLOW);
            options.into()
        };

        let file = options.open(inode.fd.procname())?;
        let fh = self.opened_files.insert(Mutex::new(file));

        req.reply(OpenOut {
            fh,
            open_flags: OpenOutFlags::DIRECT_IO,
        })
    }

    fn read(self: &Arc<Self>, req: fs::Request<'_>, op: op::Read<'_>) -> fs::Result {
        let file = self.opened_files.get(op.fh).ok_or(Errno::NOENT)?;
        let mut file = file.lock().unwrap();
        let file = &mut *file;

        file.seek(io::SeekFrom::Start(op.offset))?;

        let mut buf = Vec::<u8>::with_capacity(op.size as usize);
        file.take(op.size as u64).read_to_end(&mut buf)?;

        req.reply(reply::Raw(buf))
    }

    fn write(
        self: &Arc<Self>,
        req: fs::Request<'_>,
        op: op::Write<'_>,
        mut data: impl io::Read,
    ) -> fs::Result {
        let file = self.opened_files.get(op.fh).ok_or(Errno::NOENT)?;
        let mut file = file.lock().unwrap();
        let file = &mut *file;

        file.seek(io::SeekFrom::Start(op.offset))?;

        // At here, the data is transferred via the temporary buffer due to
        // the incompatibility between the I/O abstraction in `futures` and
        // `tokio`.
        //
        // In order to efficiently transfer the large files, both of zero
        // copying support in `polyfuse` and resolution of impedance mismatch
        // between `futures::io` and `tokio::io` are required.
        let mut buf = Vec::with_capacity(op.size as usize);
        data.read_to_end(&mut buf)?;

        let mut buf = &buf[..];
        let mut buf = Read::take(&mut buf, op.size as u64);
        let written = std::io::copy(&mut buf, &mut *file)?;

        req.reply(WriteOut::new(written as u32))
    }

    fn flush(self: &Arc<Self>, req: fs::Request<'_>, op: op::Flush<'_>) -> fs::Result {
        let file = self.opened_files.get(op.fh).ok_or(Errno::NOENT)?;
        let file = file.lock().unwrap();

        file.sync_all()?;

        req.reply(())
    }

    fn fsync(self: &Arc<Self>, req: fs::Request<'_>, op: op::Fsync<'_>) -> fs::Result {
        let file = self.opened_files.get(op.fh).ok_or(Errno::NOENT)?;
        let file = file.lock().unwrap();

        if op.datasync {
            file.sync_data()?;
        } else {
            file.sync_all()?;
        }

        req.reply(())
    }

    fn flock(self: &Arc<Self>, req: fs::Request<'_>, op: op::Flock<'_>) -> fs::Result {
        let file = self.opened_files.get(op.fh).ok_or(Errno::NOENT)?;
        let file = file.lock().unwrap();

        nix::flock(&*file, op.op.into_raw())?;

        req.reply(())
    }

    fn fallocate(self: &Arc<Self>, req: fs::Request<'_>, op: op::Fallocate<'_>) -> fs::Result {
        if !op.mode.is_empty() {
            return Err(Errno::OPNOTSUPP.into());
        }

        let file = self.opened_files.get(op.fh).ok_or(Errno::NOENT)?;
        let file = file.lock().unwrap();

        nix::posix_fallocate(&*file, op.offset as i64, op.length as i64)?;

        req.reply(())
    }

    fn release(self: &Arc<Self>, req: fs::Request<'_>, op: op::Release<'_>) -> fs::Result {
        let _file = self.opened_files.remove(op.fh);
        req.reply(())
    }

    fn getxattr(self: &Arc<Self>, req: fs::Request<'_>, op: op::Getxattr<'_>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let inode = inode.lock().unwrap();

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(Errno::NOTSUP.into());
        }

        match op.size {
            0 => {
                let size = nix::getxattr(inode.fd.procname(), op.name, None)?;
                req.reply(XattrOut::new(size as u32))
            }
            size => {
                let mut value = vec![0u8; size as usize];
                let n = nix::getxattr(inode.fd.procname(), op.name, Some(&mut value[..]))?;
                value.resize(n as usize, 0);
                req.reply(reply::Raw(value))
            }
        }
    }

    fn listxattr(self: &Arc<Self>, req: fs::Request<'_>, op: op::Listxattr<'_>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let inode = inode.lock().unwrap();

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(Errno::NOTSUP.into());
        }

        match op.size {
            0 => {
                let size = nix::listxattr(inode.fd.procname(), None)?;
                req.reply(XattrOut::new(size as u32))
            }
            size => {
                let mut value = vec![0u8; size as usize];
                let n = nix::listxattr(inode.fd.procname(), Some(&mut value[..]))?;
                value.resize(n as usize, 0);
                req.reply(reply::Raw(value))
            }
        }
    }

    fn setxattr(self: &Arc<Self>, req: fs::Request<'_>, op: op::Setxattr<'_>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let inode = inode.lock().unwrap();

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(Errno::NOTSUP.into());
        }

        nix::setxattr(
            inode.fd.procname(),
            op.name,
            op.value,
            op.flags.bits() as libc::c_int,
        )?;

        req.reply(())
    }

    fn removexattr(self: &Arc<Self>, req: fs::Request<'_>, op: op::Removexattr<'_>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let inode = inode.lock().unwrap();

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(Errno::NOTSUP.into());
        }

        nix::removexattr(inode.fd.procname(), op.name)?;

        req.reply(())
    }

    fn statfs(self: &Arc<Self>, req: fs::Request<'_>, op: op::Statfs<'_>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let inode = inode.lock().unwrap();

        let st = nix::fstatvfs(&inode.fd)?;
        let stat = st.try_into().map_err(|_| Errno::INVAL)?;

        req.reply(StatfsOut::new(&stat))
    }
}

// ==== HandlePool ====

struct HandlePool<T>(Mutex<Slab<Arc<T>>>);

impl<T> Default for HandlePool<T> {
    fn default() -> Self {
        Self(Mutex::default())
    }
}

impl<T> HandlePool<T> {
    fn get(&self, fh: FileID) -> Option<Arc<T>> {
        self.0.lock().unwrap().get(fh.into_raw() as usize).cloned()
    }

    fn remove(&self, fh: FileID) -> Arc<T> {
        self.0.lock().unwrap().remove(fh.into_raw() as usize)
    }

    fn insert(&self, entry: T) -> FileID {
        FileID::from_raw(self.0.lock().unwrap().insert(Arc::new(entry)) as u64)
    }
}

// ==== INode ====

struct INode {
    ino: NodeID,
    src_id: SrcId,
    is_symlink: bool,
    fd: FileDesc,
    refcount: u64,
}

// ==== INodeTable ====

struct INodeTable {
    map: HashMap<NodeID, Arc<Mutex<INode>>>,
    src_to_ino: HashMap<SrcId, NodeID>,
    next_ino: u64,
}

impl INodeTable {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            src_to_ino: HashMap::new(),
            next_ino: 1, // the ino is started with 1 and the first entry is mapped to the root.
        }
    }

    fn get(&self, ino: NodeID) -> Option<Arc<Mutex<INode>>> {
        self.map.get(&ino).cloned()
    }

    fn get_src(&self, src_id: SrcId) -> Option<Arc<Mutex<INode>>> {
        let ino = self.src_to_ino.get(&src_id)?;
        self.map.get(ino).cloned()
    }

    fn vacant_entry(&mut self) -> VacantEntry<'_> {
        let ino = NodeID::from_raw(self.next_ino).expect("invalid nodeid");
        VacantEntry { table: self, ino }
    }
}

struct VacantEntry<'a> {
    table: &'a mut INodeTable,
    ino: NodeID,
}

impl VacantEntry<'_> {
    fn ino(&self) -> NodeID {
        self.ino
    }

    fn insert(self, inode: INode) {
        let src_id = inode.src_id;
        self.table.map.insert(self.ino, Arc::new(Mutex::new(inode)));
        self.table.src_to_ino.insert(src_id, self.ino);
        self.table.next_ino += 1;
    }
}
