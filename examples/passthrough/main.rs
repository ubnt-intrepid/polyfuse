#![allow(clippy::unnecessary_mut_passed)]
#![warn(clippy::unimplemented, clippy::todo)]

mod nix;

use polyfuse::{
    fs::{self, Filesystem},
    mount::MountOptions,
    op,
    reply::{
        AttrOut, EntryOut, FileAttr, OpenOut, ReaddirOut, Statfs, StatfsOut, WriteOut, XattrOut,
    },
    types::{FileID, NodeID, GID, UID},
    KernelConfig, KernelFlags,
};

use crate::nix::{FileDesc, ReadDir};
use anyhow::{ensure, Context as _, Result};
use libc::{
    AT_REMOVEDIR, AT_SYMLINK_FOLLOW, AT_SYMLINK_NOFOLLOW, EINVAL, ENOENT, ENOSYS, ENOTSUP,
    EOPNOTSUPP, EPERM, O_NOFOLLOW, O_PATH, O_RDONLY, O_RDWR, O_WRONLY, S_IFDIR, S_IFLNK, S_IFMT,
    UTIME_NOW, UTIME_OMIT,
};
use pico_args::Arguments;
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap},
    ffi::OsStr,
    fs::{File, OpenOptions},
    io::{self, prelude::*},
    os::unix::prelude::*,
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

    let mountopts = MountOptions {
        default_permissions: true,
        fsname: Some("passthrough".into()),
        ..Default::default()
    };

    let mut config = KernelConfig::default();
    config.flags |= KernelFlags::EXPORT_SUPPORT;
    config.flags |= KernelFlags::FLOCK_LOCKS;
    config.flags |= KernelFlags::WRITEBACK_CACHE;

    let fs = Passthrough::new(source, timeout)?;

    polyfuse::fs::run(fs, mountpoint, mountopts, config)?;

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

    fn make_entry_param(&self, ino: NodeID, attr: libc::stat) -> EntryOut {
        let mut reply = EntryOut::default();
        reply.ino(ino);
        fill_attr(reply.attr(), &attr);
        if let Some(timeout) = self.timeout {
            reply.ttl_entry(timeout);
            reply.ttl_attr(timeout);
        };
        reply
    }

    fn do_lookup(&self, parent: NodeID, name: &OsStr) -> fs::Result<EntryOut> {
        let mut inodes = self.inodes.lock().unwrap();
        let inodes = &mut *inodes;

        let parent = inodes.get(parent).ok_or(ENOENT)?;
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

        Ok(self.make_entry_param(ino, stat))
    }

    fn make_node(
        &self,
        parent: NodeID,
        name: &OsStr,
        mode: u32,
        rdev: Option<u32>,
        link: Option<&OsStr>,
    ) -> fs::Result<EntryOut> {
        {
            let inodes = self.inodes.lock().unwrap();
            let parent = inodes.get(parent).ok_or(ENOENT)?;
            let parent = parent.lock().unwrap();

            match mode & S_IFMT {
                S_IFDIR => {
                    parent.fd.mkdirat(name, mode)?;
                }
                S_IFLNK => {
                    let link = link.expect("missing 'link'");
                    parent.fd.symlinkat(name, link)?;
                }
                _ => {
                    parent
                        .fd
                        .mknodat(name, mode, rdev.unwrap_or(0) as libc::dev_t)?;
                }
            }
        }
        self.do_lookup(parent, name)
    }
}

impl Filesystem for Passthrough {
    fn lookup(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Lookup<'_>>) -> fs::Result {
        let out = self.do_lookup(req.arg().parent(), req.arg().name())?;
        req.reply(out)
    }

    fn forget(&self, _: fs::Context<'_, '_>, forgets: &[op::Forget]) {
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

    fn getattr(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Getattr<'_>>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();

        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let inode = inode.lock().unwrap();

        let stat = inode.fd.fstatat("", AT_SYMLINK_NOFOLLOW)?;

        let mut out = AttrOut::default();
        fill_attr(out.attr(), &stat);
        if let Some(timeout) = self.timeout {
            out.ttl(timeout);
        };

        req.reply(out)
    }

    #[allow(clippy::cognitive_complexity)]
    fn setattr(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Setattr<'_>>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let inode = inode.lock().unwrap();
        let fd = &inode.fd;

        let mut file = if let Some(fh) = req.arg().fh() {
            Some(self.opened_files.get(fh).ok_or(ENOENT)?)
        } else {
            None
        };
        let mut file = file.as_mut().map(|file| file.lock().unwrap());

        // chmod
        if let Some(mode) = req.arg().mode() {
            if let Some(file) = file.as_mut() {
                nix::fchmod(&**file, mode)?;
            } else {
                nix::chmod(fd.procname(), mode)?;
            }
        }

        // chown
        match (req.arg().uid(), req.arg().gid()) {
            (None, None) => (),
            (uid, gid) => {
                fd.fchownat(
                    "",
                    uid.map(UID::into_raw),
                    gid.map(GID::into_raw),
                    AT_SYMLINK_NOFOLLOW,
                )?;
            }
        }

        // truncate
        if let Some(size) = req.arg().size() {
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
        match (req.arg().atime(), req.arg().mtime()) {
            (None, None) => (),
            (atime, mtime) => {
                let tv = [make_timespec(atime), make_timespec(mtime)];
                if let Some(file) = file.as_mut() {
                    nix::futimens(&**file, tv)?;
                } else if inode.is_symlink {
                    // According to libfuse/examples/passthrough_hp.cc, it does not work on
                    // the current kernels, but may in the future.
                    fd.futimensat("", tv, AT_SYMLINK_NOFOLLOW).map_err(|err| {
                        match err.raw_os_error() {
                            Some(EINVAL) => io::Error::from_raw_os_error(EPERM),
                            _ => err,
                        }
                    })?;
                } else {
                    nix::utimens(fd.procname(), tv)?;
                }
            }
        }

        // finally, acquiring the latest metadata from the source filesystem.
        let stat = fd.fstatat("", AT_SYMLINK_NOFOLLOW)?;

        let mut out = AttrOut::default();
        fill_attr(out.attr(), &stat);
        if let Some(timeout) = self.timeout {
            out.ttl(timeout);
        };

        req.reply(out)
    }

    fn readlink(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Readlink<'_>>,
    ) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let inode = inode.lock().unwrap();
        let link = inode.fd.readlinkat("")?;
        req.reply(link)
    }

    fn link(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Link<'_>>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();

        let source = inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let mut source = source.lock().unwrap();

        let parent = inodes.get(req.arg().newparent()).ok_or(ENOENT)?;
        let parent = parent.lock().unwrap();

        if source.is_symlink {
            source
                .fd
                .linkat("", &parent.fd, req.arg().newname(), 0)
                .map_err(|err| match err.raw_os_error() {
                    Some(ENOENT) | Some(EINVAL) => {
                        // no race-free way to hard-link a symlink.
                        io::Error::from_raw_os_error(EOPNOTSUPP)
                    }
                    _ => err,
                })?;
        } else {
            nix::link(
                source.fd.procname(),
                &parent.fd,
                req.arg().newname(),
                AT_SYMLINK_FOLLOW,
            )?;
        }

        let stat = source.fd.fstatat("", AT_SYMLINK_NOFOLLOW)?;
        let entry = self.make_entry_param(source.ino, stat);

        source.refcount += 1;

        req.reply(entry)
    }

    fn mknod(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Mknod<'_>>) -> fs::Result {
        let out = self.make_node(
            req.arg().parent(),
            req.arg().name(),
            req.arg().mode(),
            Some(req.arg().rdev()),
            None,
        )?;
        req.reply(out)
    }

    fn mkdir(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Mkdir<'_>>) -> fs::Result {
        let out = self.make_node(
            req.arg().parent(),
            req.arg().name(),
            S_IFDIR | req.arg().mode(),
            None,
            None,
        )?;
        req.reply(out)
    }

    fn symlink(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Symlink<'_>>) -> fs::Result {
        let out = self.make_node(
            req.arg().parent(),
            req.arg().name(),
            S_IFLNK,
            None,
            Some(req.arg().link()),
        )?;
        req.reply(out)
    }

    fn unlink(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Unlink<'_>>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let parent = inodes.get(req.arg().parent()).ok_or(ENOENT)?;
        let parent = parent.lock().unwrap();
        parent.fd.unlinkat(req.arg().name(), 0)?;
        req.reply(())
    }

    fn rmdir(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Rmdir<'_>>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let parent = inodes.get(req.arg().parent()).ok_or(ENOENT)?;
        let parent = parent.lock().unwrap();
        parent.fd.unlinkat(req.arg().name(), AT_REMOVEDIR)?;
        req.reply(())
    }

    fn rename(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Rename<'_>>) -> fs::Result {
        if req.arg().flags() != 0 {
            // rename2 is not supported.
            return Err(EINVAL.into());
        }

        let inodes = self.inodes.lock().unwrap();

        let parent = inodes.get(req.arg().parent()).ok_or(ENOENT)?;
        let newparent = inodes.get(req.arg().newparent()).ok_or(ENOENT)?;

        let parent = parent.lock().unwrap();
        if req.arg().parent() == req.arg().newparent() {
            parent
                .fd
                .renameat(req.arg().name(), None::<&FileDesc>, req.arg().newname())?;
        } else {
            let newparent = newparent.lock().unwrap();
            parent
                .fd
                .renameat(req.arg().name(), Some(&newparent.fd), req.arg().newname())?;
        }

        req.reply(())
    }

    fn opendir(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Opendir<'_>>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let inode = inode.lock().unwrap();
        let dir = inode.fd.read_dir()?;
        let fh = self.opened_dirs.insert(Mutex::new(dir));

        let mut out = OpenOut::default();
        out.fh(fh);

        req.reply(out)
    }

    fn readdir(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Readdir<'_>>) -> fs::Result {
        if req.arg().mode() == op::ReaddirMode::Plus {
            return Err(ENOSYS.into());
        }

        let read_dir = self
            .opened_dirs
            .get(req.arg().fh())
            .ok_or_else(|| io::Error::from_raw_os_error(ENOENT))?;
        let read_dir = &mut *read_dir.lock().unwrap();
        read_dir.seek(req.arg().offset());

        let mut out = ReaddirOut::new(req.arg().size() as usize);
        for entry in read_dir {
            let entry = entry?;
            if out.entry(
                &entry.name,
                NodeID::from_raw(entry.ino),
                entry.typ,
                entry.off,
            ) {
                break;
            }
        }

        req.reply(out)
    }

    fn fsyncdir(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Fsyncdir<'_>>,
    ) -> fs::Result {
        let read_dir = self.opened_dirs.get(req.arg().fh()).ok_or(ENOENT)?;
        let read_dir = read_dir.lock().unwrap();

        if req.arg().datasync() {
            read_dir.sync_data()?;
        } else {
            read_dir.sync_all()?;
        }

        req.reply(())
    }

    fn releasedir(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Releasedir<'_>>,
    ) -> fs::Result {
        let _dir = self.opened_dirs.remove(req.arg().fh());
        req.reply(())
    }

    fn open(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Open<'_>>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let inode = inode.lock().unwrap();

        let mut options = OpenOptions::new();
        match (req.arg().flags() & 0x03) as i32 {
            O_RDONLY => {
                options.read(true);
            }
            O_WRONLY => {
                options.write(true);
            }
            O_RDWR => {
                options.read(true).write(true);
            }
            _ => (),
        }
        options.custom_flags(req.arg().flags() as i32 & !O_NOFOLLOW);

        let file = options.open(inode.fd.procname())?;
        let fh = self.opened_files.insert(Mutex::new(file));

        let mut out = OpenOut::default();
        out.fh(fh);

        req.reply(out)
    }

    fn read(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Read<'_>>) -> fs::Result {
        let file = self.opened_files.get(req.arg().fh()).ok_or(ENOENT)?;
        let mut file = file.lock().unwrap();
        let file = &mut *file;

        file.seek(io::SeekFrom::Start(req.arg().offset()))?;

        let mut buf = Vec::<u8>::with_capacity(req.arg().size() as usize);
        file.take(req.arg().size() as u64).read_to_end(&mut buf)?;

        req.reply(buf)
    }

    fn write(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Write<'_>>,
        mut data: fs::Data<'_>,
    ) -> fs::Result {
        let file = self.opened_files.get(req.arg().fh()).ok_or(ENOENT)?;
        let mut file = file.lock().unwrap();
        let file = &mut *file;

        file.seek(io::SeekFrom::Start(req.arg().offset()))?;

        // At here, the data is transferred via the temporary buffer due to
        // the incompatibility between the I/O abstraction in `futures` and
        // `tokio`.
        //
        // In order to efficiently transfer the large files, both of zero
        // copying support in `polyfuse` and resolution of impedance mismatch
        // between `futures::io` and `tokio::io` are required.
        let mut buf = Vec::with_capacity(req.arg().size() as usize);
        data.read_to_end(&mut buf)?;

        let mut buf = &buf[..];
        let mut buf = Read::take(&mut buf, req.arg().size() as u64);
        let written = std::io::copy(&mut buf, &mut *file)?;

        let mut out = WriteOut::default();
        out.size(written as u32);

        req.reply(out)
    }

    fn flush(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Flush<'_>>) -> fs::Result {
        let file = self.opened_files.get(req.arg().fh()).ok_or(ENOENT)?;
        let file = file.lock().unwrap();

        file.sync_all()?;

        req.reply(())
    }

    fn fsync(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Fsync<'_>>) -> fs::Result {
        let file = self.opened_files.get(req.arg().fh()).ok_or(ENOENT)?;
        let file = file.lock().unwrap();

        if req.arg().datasync() {
            file.sync_data()?;
        } else {
            file.sync_all()?;
        }

        req.reply(())
    }

    fn flock(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Flock<'_>>) -> fs::Result {
        let file = self.opened_files.get(req.arg().fh()).ok_or(ENOENT)?;
        let file = file.lock().unwrap();

        let op = req.arg().op().expect("invalid lock operation") as i32;

        nix::flock(&*file, op)?;

        req.reply(())
    }

    fn fallocate(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Fallocate<'_>>,
    ) -> fs::Result {
        if req.arg().mode() != 0 {
            return Err(EOPNOTSUPP.into());
        }

        let file = self.opened_files.get(req.arg().fh()).ok_or(ENOENT)?;
        let file = file.lock().unwrap();

        nix::posix_fallocate(&*file, req.arg().offset() as i64, req.arg().length() as i64)?;

        req.reply(())
    }

    fn release(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Release<'_>>) -> fs::Result {
        let _file = self.opened_files.remove(req.arg().fh());
        req.reply(())
    }

    fn getxattr(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Getxattr<'_>>,
    ) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let inode = inode.lock().unwrap();

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(ENOTSUP.into());
        }

        match req.arg().size() {
            0 => {
                let size = nix::getxattr(inode.fd.procname(), req.arg().name(), None)?;
                let mut out = XattrOut::default();
                out.size(size as u32);
                req.reply(out)
            }
            size => {
                let mut value = vec![0u8; size as usize];
                let n = nix::getxattr(inode.fd.procname(), req.arg().name(), Some(&mut value[..]))?;
                value.resize(n as usize, 0);
                req.reply(value)
            }
        }
    }

    fn listxattr(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Listxattr<'_>>,
    ) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let inode = inode.lock().unwrap();

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(ENOTSUP.into());
        }

        match req.arg().size() {
            0 => {
                let size = nix::listxattr(inode.fd.procname(), None)?;
                let mut out = XattrOut::default();
                out.size(size as u32);
                req.reply(out)
            }
            size => {
                let mut value = vec![0u8; size as usize];
                let n = nix::listxattr(inode.fd.procname(), Some(&mut value[..]))?;
                value.resize(n as usize, 0);
                req.reply(value)
            }
        }
    }

    fn setxattr(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Setxattr<'_>>,
    ) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let inode = inode.lock().unwrap();

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(ENOTSUP.into());
        }

        nix::setxattr(
            inode.fd.procname(),
            req.arg().name(),
            req.arg().value(),
            req.arg().flags() as libc::c_int,
        )?;

        req.reply(())
    }

    fn removexattr(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Removexattr<'_>>,
    ) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let inode = inode.lock().unwrap();

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(ENOTSUP.into());
        }

        nix::removexattr(inode.fd.procname(), req.arg().name())?;

        req.reply(())
    }

    fn statfs(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Statfs<'_>>) -> fs::Result {
        let inodes = self.inodes.lock().unwrap();
        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let inode = inode.lock().unwrap();

        let st = nix::fstatvfs(&inode.fd)?;

        let mut out = StatfsOut::default();
        fill_statfs(out.statfs(), &st);

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
    attr.rdev(st.st_rdev as u32);
    attr.blksize(st.st_blksize as u32);
    attr.blocks(st.st_blocks as u64);
    attr.atime(Duration::new(st.st_atime as u64, st.st_atime_nsec as u32));
    attr.mtime(Duration::new(st.st_mtime as u64, st.st_mtime_nsec as u32));
    attr.ctime(Duration::new(st.st_ctime as u64, st.st_ctime_nsec as u32));
}

fn fill_statfs(statfs: &mut Statfs, st: &libc::statvfs) {
    statfs.bsize(st.f_bsize as u32);
    statfs.frsize(st.f_frsize as u32);
    statfs.blocks(st.f_blocks);
    statfs.bfree(st.f_bfree);
    statfs.bavail(st.f_bavail);
    statfs.files(st.f_files);
    statfs.ffree(st.f_ffree);
    statfs.namelen(st.f_namemax as u32);
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
        let ino = self.next_ino;
        VacantEntry {
            table: self,
            ino: NodeID::from_raw(ino),
        }
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
