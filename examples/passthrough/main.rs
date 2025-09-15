#![allow(clippy::unnecessary_mut_passed)]
#![warn(clippy::unimplemented, clippy::todo)]

mod nix;

use polyfuse::{
    fs::{
        self,
        reply::{
            self, ReplyAttr, ReplyData, ReplyDir, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyUnit,
            ReplyWrite, ReplyXattr,
        },
        Daemon, Filesystem,
    },
    mount::MountOptions,
    op::{self, OpenFlags},
    types::{DeviceID, FileID, FileMode, FilePermissions, FileType, NodeID, GID, UID},
    KernelConfig, KernelFlags,
};

use crate::nix::{FileDesc, ReadDir};
use anyhow::{ensure, Context as _, Result};
use libc::{
    AT_REMOVEDIR, AT_SYMLINK_FOLLOW, AT_SYMLINK_NOFOLLOW, EINVAL, ENOENT, ENOSYS, ENOTSUP,
    EOPNOTSUPP, EPERM, O_NOFOLLOW, O_PATH, S_IFLNK, S_IFMT, UTIME_NOW, UTIME_OMIT,
};
use pico_args::Arguments;
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap},
    ffi::OsStr,
    fs::{File, OpenOptions},
    io::{self, prelude::*},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{sync::Mutex, task};

#[tokio::main]
async fn main() -> Result<()> {
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

    let daemon = Daemon::mount(mountpoint, mountopts, config).await?;

    let fs = Passthrough::new(source, timeout)?;
    daemon.run(Arc::new(fs), None).await?;

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

    async fn do_lookup(
        &self,
        parent: NodeID,
        name: &OsStr,
        mut reply: ReplyEntry<'_>,
    ) -> reply::Result {
        let mut inodes = self.inodes.lock().await;
        let inodes = &mut *inodes;

        let parent = inodes.get(parent).ok_or(ENOENT)?;
        let parent = parent.lock().await;

        let fd = task::block_in_place(|| parent.fd.openat(name, O_PATH | O_NOFOLLOW))?;

        let stat = task::block_in_place(|| fd.fstatat("", AT_SYMLINK_NOFOLLOW))?;
        let src_id = (stat.st_ino, stat.st_dev);
        let is_symlink = stat.st_mode & S_IFMT == S_IFLNK;

        let ino;
        match inodes.get_src(src_id) {
            Some(inode) => {
                let mut inode = inode.lock().await;
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

        reply.ino(ino);
        reply.attr(&stat.try_into().unwrap());
        if let Some(timeout) = self.timeout {
            reply.ttl_entry(timeout);
            reply.ttl_attr(timeout);
        }
        reply.send()
    }

    async fn make_node(
        &self,
        parent: NodeID,
        name: &OsStr,
        mode: FileMode,
        rdev: Option<DeviceID>,
        link: Option<&OsStr>,
        reply: ReplyEntry<'_>,
    ) -> reply::Result {
        {
            let inodes = self.inodes.lock().await;
            let parent = inodes.get(parent).ok_or(ENOENT)?;
            let parent = parent.lock().await;

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
        self.do_lookup(parent, name, reply).await
    }
}

impl Filesystem for Passthrough {
    async fn lookup(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Lookup<'_>,
        reply: ReplyEntry<'_>,
    ) -> reply::Result {
        self.do_lookup(op.parent(), op.name(), reply).await
    }

    async fn forget(self: &Arc<Self>, forgets: &[op::Forget]) {
        let mut inodes = self.inodes.lock().await;

        for forget in forgets {
            if let Entry::Occupied(mut entry) = inodes.map.entry(forget.ino()) {
                let refcount = {
                    let mut inode = entry.get_mut().lock().await;
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

    async fn getattr(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Getattr<'_>,
        mut reply: ReplyAttr<'_>,
    ) -> reply::Result {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;
        let inode = inode.lock().await;

        let stat = inode.fd.fstatat("", AT_SYMLINK_NOFOLLOW)?;

        reply.attr(&stat.try_into().unwrap());
        if let Some(timeout) = self.timeout {
            reply.ttl(timeout);
        };

        reply.send()
    }

    #[allow(clippy::cognitive_complexity)]
    async fn setattr(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Setattr<'_>,
        mut reply: ReplyAttr<'_>,
    ) -> reply::Result {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;
        let inode = inode.lock().await;
        let fd = &inode.fd;

        let mut file = if let Some(fh) = op.fh() {
            Some(self.opened_files.get(fh).await.ok_or(ENOENT)?)
        } else {
            None
        };
        let mut file = match &mut file {
            Some(file) => Some(file.lock().await),
            None => None,
        };

        // chmod
        if let Some(mode) = op.mode() {
            if let Some(file) = file.as_mut() {
                task::block_in_place(|| nix::fchmod(&**file, mode.into_raw()))?;
            } else {
                task::block_in_place(|| nix::chmod(fd.procname(), mode.into_raw()))?;
            }
        }

        // chown
        match (op.uid(), op.gid()) {
            (None, None) => (),
            (uid, gid) => {
                task::block_in_place(|| {
                    fd.fchownat(
                        "",
                        uid.map(UID::into_raw),
                        gid.map(GID::into_raw),
                        AT_SYMLINK_NOFOLLOW,
                    )
                })?;
            }
        }

        // truncate
        if let Some(size) = op.size() {
            if let Some(file) = file.as_mut() {
                task::block_in_place(|| nix::ftruncate(&**file, size as libc::off_t))?;
            } else {
                task::block_in_place(|| nix::truncate(fd.procname(), size as libc::off_t))?;
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
        match (op.atime(), op.mtime()) {
            (None, None) => (),
            (atime, mtime) => {
                let tv = [make_timespec(atime), make_timespec(mtime)];
                if let Some(file) = file.as_mut() {
                    nix::futimens(&**file, tv)?;
                } else if inode.is_symlink {
                    // According to libfuse/examples/passthrough_hp.cc, it does not work on
                    // the current kernels, but may in the future.
                    task::block_in_place(|| fd.futimensat("", tv, AT_SYMLINK_NOFOLLOW)) //
                        .map_err(|err| match err.raw_os_error() {
                            Some(EINVAL) => io::Error::from_raw_os_error(EPERM),
                            _ => err,
                        })?;
                } else {
                    task::block_in_place(|| nix::utimens(fd.procname(), tv))?;
                }
            }
        }

        // finally, acquiring the latest metadata from the source filesystem.
        let stat = task::block_in_place(|| fd.fstatat("", AT_SYMLINK_NOFOLLOW))?;

        reply.attr(&stat.try_into().unwrap());
        if let Some(timeout) = self.timeout {
            reply.ttl(timeout);
        };

        reply.send()
    }

    async fn readlink(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Readlink<'_>,
        reply: ReplyData<'_>,
    ) -> reply::Result {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;
        let inode = inode.lock().await;
        let link = task::block_in_place(|| inode.fd.readlinkat(""))?;
        reply.send(link)
    }

    async fn link(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Link<'_>,
        mut reply: ReplyEntry<'_>,
    ) -> reply::Result {
        let inodes = self.inodes.lock().await;

        let source = inodes.get(op.ino()).ok_or(ENOENT)?;
        let mut source = source.lock().await;

        let parent = inodes.get(op.newparent()).ok_or(ENOENT)?;
        let parent = parent.lock().await;

        if source.is_symlink {
            task::block_in_place(|| source.fd.linkat("", &parent.fd, op.newname(), 0)) //
                .map_err(|err| match err.raw_os_error() {
                    Some(ENOENT) | Some(EINVAL) => {
                        // no race-free way to hard-link a symlink.
                        io::Error::from_raw_os_error(EOPNOTSUPP)
                    }
                    _ => err,
                })?;
        } else {
            task::block_in_place(|| {
                nix::link(
                    source.fd.procname(),
                    &parent.fd,
                    op.newname(),
                    AT_SYMLINK_FOLLOW,
                )
            })?;
        }

        let stat = task::block_in_place(|| source.fd.fstatat("", AT_SYMLINK_NOFOLLOW))?;

        reply.ino(source.ino);
        reply.attr(&stat.try_into().unwrap());
        if let Some(ttl) = self.timeout {
            reply.ttl_attr(ttl);
            reply.ttl_entry(ttl);
        }
        let replied = reply.send()?;

        source.refcount += 1;

        Ok(replied)
    }

    async fn mknod(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Mknod<'_>,
        reply: ReplyEntry<'_>,
    ) -> reply::Result {
        self.make_node(
            op.parent(),
            op.name(),
            op.mode(),
            Some(op.rdev()),
            None,
            reply,
        )
        .await
    }

    async fn mkdir(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Mkdir<'_>,
        reply: ReplyEntry<'_>,
    ) -> reply::Result {
        self.make_node(
            op.parent(),
            op.name(),
            FileMode::new(FileType::Directory, op.permissions()),
            None,
            None,
            reply,
        )
        .await
    }

    async fn symlink(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Symlink<'_>,
        reply: ReplyEntry<'_>,
    ) -> reply::Result {
        self.make_node(
            op.parent(),
            op.name(),
            FileMode::new(FileType::SymbolicLink, FilePermissions::empty()),
            None,
            Some(op.link()),
            reply,
        )
        .await
    }

    async fn unlink(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Unlink<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        let inodes = self.inodes.lock().await;
        let parent = inodes.get(op.parent()).ok_or(ENOENT)?;
        let parent = parent.lock().await;
        task::block_in_place(|| parent.fd.unlinkat(op.name(), 0))?;
        reply.send()
    }

    async fn rmdir(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Rmdir<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        let inodes = self.inodes.lock().await;
        let parent = inodes.get(op.parent()).ok_or(ENOENT)?;
        let parent = parent.lock().await;
        task::block_in_place(|| parent.fd.unlinkat(op.name(), AT_REMOVEDIR))?;
        reply.send()
    }

    async fn rename(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Rename<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        if !op.flags().is_empty() {
            // rename2 is not supported.
            return Err(EINVAL.into());
        }

        let inodes = self.inodes.lock().await;

        let parent = inodes.get(op.parent()).ok_or(ENOENT)?;
        let newparent = inodes.get(op.newparent()).ok_or(ENOENT)?;

        let parent = parent.lock().await;
        if op.parent() == op.newparent() {
            task::block_in_place(|| {
                parent
                    .fd
                    .renameat(op.name(), None::<&FileDesc>, op.newname())
            })?;
        } else {
            let newparent = newparent.lock().await;
            task::block_in_place(|| {
                parent
                    .fd
                    .renameat(op.name(), Some(&newparent.fd), op.newname())
            })?;
        }

        reply.send()
    }

    async fn opendir(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Opendir<'_>,
        mut reply: ReplyOpen<'_>,
    ) -> reply::Result {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;
        let inode = inode.lock().await;
        let dir = task::block_in_place(|| inode.fd.read_dir())?;
        let fh = self.opened_dirs.insert(Mutex::new(dir)).await;

        reply.fh(fh);
        reply.send()
    }

    async fn readdir(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Readdir<'_>,
        mut reply: ReplyDir<'_>,
    ) -> reply::Result {
        if op.mode() == op::ReaddirMode::Plus {
            return Err(ENOSYS.into());
        }

        let read_dir = self
            .opened_dirs
            .get(op.fh())
            .await
            .ok_or_else(|| io::Error::from_raw_os_error(ENOENT))?;
        let read_dir = &mut *read_dir.lock().await;
        task::block_in_place(|| read_dir.seek(op.offset()));

        while let Some(entry) = task::block_in_place(|| read_dir.next()) {
            let entry = entry?;
            if reply.push_entry(
                &entry.name,
                NodeID::from_raw(entry.ino),
                FileType::from_dirent_type(entry.typ),
                entry.off,
            ) {
                break;
            }
        }

        reply.send()
    }

    async fn fsyncdir(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Fsyncdir<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        let read_dir = self.opened_dirs.get(op.fh()).await.ok_or(ENOENT)?;
        let read_dir = read_dir.lock().await;

        if op.datasync() {
            task::block_in_place(|| read_dir.sync_data())?;
        } else {
            task::block_in_place(|| read_dir.sync_all())?;
        }

        reply.send()
    }

    async fn releasedir(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Releasedir<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        let _dir = self.opened_dirs.remove(op.fh()).await;
        reply.send()
    }

    async fn open(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Open<'_>,
        mut reply: ReplyOpen<'_>,
    ) -> reply::Result {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;
        let inode = inode.lock().await;

        let options: OpenOptions = op.options().remove(OpenFlags::NOFOLLOW).into();

        let file = task::block_in_place(|| options.open(inode.fd.procname()))?;
        let fh = self.opened_files.insert(Mutex::new(file)).await;

        reply.fh(fh);
        reply.send()
    }

    async fn read(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Read<'_>,
        reply: ReplyData<'_>,
    ) -> reply::Result {
        let file = self.opened_files.get(op.fh()).await.ok_or(ENOENT)?;
        let mut file = file.lock().await;
        let file = &mut *file;

        task::block_in_place(|| file.seek(io::SeekFrom::Start(op.offset())))?;

        let mut buf = Vec::<u8>::with_capacity(op.size() as usize);
        task::block_in_place(|| file.take(op.size() as u64).read_to_end(&mut buf))?;

        reply.send(buf)
    }

    async fn write(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Write<'_>,
        mut data: fs::Data<'_>,
        reply: ReplyWrite<'_>,
    ) -> reply::Result {
        let file = self.opened_files.get(op.fh()).await.ok_or(ENOENT)?;
        let mut file = file.lock().await;
        let file = &mut *file;

        task::block_in_place(|| file.seek(io::SeekFrom::Start(op.offset())))?;

        // At here, the data is transferred via the temporary buffer due to
        // the incompatibility between the I/O abstraction in `futures` and
        // `tokio`.
        //
        // In order to efficiently transfer the large files, both of zero
        // copying support in `polyfuse` and resolution of impedance mismatch
        // between `futures::io` and `tokio::io` are required.
        let mut buf = Vec::with_capacity(op.size() as usize);
        data.read_to_end(&mut buf)?;

        let mut buf = &buf[..];
        let mut buf = Read::take(&mut buf, op.size() as u64);
        let written = task::block_in_place(|| std::io::copy(&mut buf, &mut *file))?;

        reply.send(written as u32)
    }

    async fn flush(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Flush<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        let file = self.opened_files.get(op.fh()).await.ok_or(ENOENT)?;
        let file = file.lock().await;

        task::block_in_place(|| file.sync_all())?;

        reply.send()
    }

    async fn fsync(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Fsync<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        let file = self.opened_files.get(op.fh()).await.ok_or(ENOENT)?;
        let file = file.lock().await;

        if op.datasync() {
            task::block_in_place(|| file.sync_data())?;
        } else {
            task::block_in_place(|| file.sync_all())?;
        }

        reply.send()
    }

    async fn flock(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Flock<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        let file = self.opened_files.get(op.fh()).await.ok_or(ENOENT)?;
        let file = file.lock().await;

        let op = op.op().expect("invalid lock operation");

        task::block_in_place(|| nix::flock(&*file, op.into_raw()))?;

        reply.send()
    }

    async fn fallocate(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Fallocate<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        if !op.mode().is_empty() {
            return Err(EOPNOTSUPP.into());
        }

        let file = self.opened_files.get(op.fh()).await.ok_or(ENOENT)?;
        let file = file.lock().await;

        task::block_in_place(|| {
            nix::posix_fallocate(&*file, op.offset() as i64, op.length() as i64)
        })?;

        reply.send()
    }

    async fn release(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Release<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        let _file = self.opened_files.remove(op.fh()).await;
        reply.send()
    }

    async fn getxattr(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Getxattr<'_>,
        reply: ReplyXattr<'_>,
    ) -> reply::Result {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;
        let inode = inode.lock().await;

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(ENOTSUP.into());
        }

        match op.size() {
            0 => {
                let size =
                    task::block_in_place(|| nix::getxattr(inode.fd.procname(), op.name(), None))?;
                reply.send_size(size as u32)
            }
            size => {
                let mut value = vec![0u8; size as usize];
                let n = task::block_in_place(|| {
                    nix::getxattr(inode.fd.procname(), op.name(), Some(&mut value[..]))
                })?;
                value.resize(n as usize, 0);
                reply.send_value(value)
            }
        }
    }

    async fn listxattr(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Listxattr<'_>,
        reply: ReplyXattr<'_>,
    ) -> reply::Result {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;
        let inode = inode.lock().await;

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(ENOTSUP.into());
        }

        match op.size() {
            0 => {
                let size = task::block_in_place(|| nix::listxattr(inode.fd.procname(), None))?;
                reply.send_size(size as u32)
            }
            size => {
                let mut value = vec![0u8; size as usize];
                let n = task::block_in_place(|| {
                    nix::listxattr(inode.fd.procname(), Some(&mut value[..]))
                })?;
                value.resize(n as usize, 0);
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
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;
        let inode = inode.lock().await;

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(ENOTSUP.into());
        }

        task::block_in_place(|| {
            nix::setxattr(
                inode.fd.procname(),
                op.name(),
                op.value(),
                op.flags().bits() as libc::c_int,
            )
        })?;

        reply.send()
    }

    async fn removexattr(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Removexattr<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;
        let inode = inode.lock().await;

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(ENOTSUP.into());
        }

        task::block_in_place(|| nix::removexattr(inode.fd.procname(), op.name()))?;

        reply.send()
    }

    async fn statfs(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Statfs<'_>,
        reply: ReplyStatfs<'_>,
    ) -> reply::Result {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;
        let inode = inode.lock().await;

        let st = task::block_in_place(|| nix::fstatvfs(&inode.fd))?;

        reply.send(&st.try_into().unwrap())
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
    async fn get(&self, fh: FileID) -> Option<Arc<T>> {
        self.0.lock().await.get(fh.into_raw() as usize).cloned()
    }

    async fn remove(&self, fh: FileID) -> Arc<T> {
        self.0.lock().await.remove(fh.into_raw() as usize)
    }

    async fn insert(&self, entry: T) -> FileID {
        FileID::from_raw(self.0.lock().await.insert(Arc::new(entry)) as u64)
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
