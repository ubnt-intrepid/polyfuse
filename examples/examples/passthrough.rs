#![allow(clippy::unnecessary_mut_passed)]
#![warn(clippy::unimplemented, clippy::todo)]

use pico_args::Arguments;
use polyfuse::{
    io::{Reader, Writer},
    op,
    reply::{ReplyAttr, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, ReplyXattr},
    CapabilityFlags, Context, DirEntry, Filesystem, Operation,
};
use polyfuse_examples::fs::{self, FileDesc, ReadDir};
use polyfuse_examples::prelude::*;
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap},
    convert::TryInto,
    io,
    os::unix::prelude::*,
    sync::Arc,
    time::Duration,
};
use tokio::{
    fs::{File, OpenOptions},
    sync::Mutex,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = Arguments::from_env();

    let source: PathBuf = args
        .opt_value_from_str(["-s", "--source"])?
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    anyhow::ensure!(source.is_dir(), "the source path must be a directory");

    let timeout = if args.contains("--no-cache") {
        None
    } else {
        Some(Duration::from_secs(60 * 60 * 24)) // one day
    };

    let mountpoint: PathBuf = args
        .free_from_str()?
        .ok_or_else(|| anyhow::anyhow!("missing mountpoint"))?;
    anyhow::ensure!(mountpoint.is_dir(), "the mountpoint must be a directory");

    let mut server = polyfuse_tokio::Builder::default();
    *server.session().flags() |= CapabilityFlags::EXPORT_SUPPORT;
    *server.session().flags() |= CapabilityFlags::FLOCK_LOCKS;
    if timeout.is_some() {
        *server.session().flags() |= CapabilityFlags::WRITEBACK_CACHE;
    }
    // TODO: splice read/write

    let mut server = server
        .mount(
            mountpoint,
            &[
                "-o".as_ref(),
                "default_permissions,fsname=passthrough".as_ref(),
            ],
        )
        .await?;

    let fs = Passthrough::new(source, timeout)?;
    server.run(fs).await?;

    Ok(())
}

type Ino = u64;
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
        let fd = FileDesc::open(&source, libc::O_PATH)?;
        let stat = fd.fstatat("", libc::AT_SYMLINK_NOFOLLOW)?;

        let mut inodes = INodeTable::new();
        let entry = inodes.vacant_entry();
        debug_assert_eq!(entry.ino(), 1);
        entry.insert(INode {
            ino: 1,
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

    fn make_entry_param(&self, ino: u64, attr: libc::stat) -> ReplyEntry {
        let mut reply = ReplyEntry::default();
        reply.ino(ino);
        reply.attr(attr.try_into().unwrap());
        if let Some(timeout) = self.timeout {
            reply.ttl_entry(timeout);
            reply.ttl_attr(timeout);
        };
        reply
    }

    async fn do_lookup(&self, parent: Ino, name: &OsStr) -> io::Result<ReplyEntry> {
        let mut inodes = self.inodes.lock().await;
        let inodes = &mut *inodes;

        let parent = inodes.get(parent).ok_or_else(no_entry)?;
        let parent = parent.lock().await;

        let fd = parent.fd.openat(name, libc::O_PATH | libc::O_NOFOLLOW)?;

        let stat = fd.fstatat("", libc::AT_SYMLINK_NOFOLLOW)?;
        let src_id = (stat.st_ino, stat.st_dev);
        let is_symlink = stat.st_mode & libc::S_IFMT == libc::S_IFLNK;

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

        Ok(self.make_entry_param(ino, stat))
    }

    async fn forget_one(&self, ino: Ino, nlookup: u64) {
        let mut inodes = self.inodes.lock().await;

        if let Entry::Occupied(mut entry) = inodes.map.entry(ino) {
            let refcount = {
                let mut inode = entry.get_mut().lock().await;
                inode.refcount = inode.refcount.saturating_sub(nlookup);
                inode.refcount
            };

            if refcount == 0 {
                tracing::debug!("remove ino={}", entry.key());
                drop(entry.remove());
            }
        }
    }

    async fn do_getattr(&self, op: &op::Getattr<'_>) -> io::Result<ReplyAttr> {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        let stat = inode.fd.fstatat("", libc::AT_SYMLINK_NOFOLLOW)?;
        let mut attr = ReplyAttr::new(stat.try_into().unwrap());
        if let Some(timeout) = self.timeout {
            attr.ttl_attr(timeout);
        };

        Ok(attr)
    }

    #[allow(clippy::cognitive_complexity)]
    async fn do_setattr(&self, op: &op::Setattr<'_>) -> io::Result<ReplyAttr> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;
        let fd = &inode.fd;

        let mut file = if let Some(fh) = op.fh() {
            Some(self.opened_files.get(fh).await.ok_or_else(no_entry)?)
        } else {
            None
        };
        let mut file = if let Some(ref mut file) = file {
            Some(file.lock().await)
        } else {
            None
        };

        // chmod
        if let Some(mode) = op.mode() {
            if let Some(file) = file.as_mut() {
                fs::fchmod(&**file, mode)?;
            } else {
                fs::chmod(fd.procname(), mode)?;
            }
        }

        // chown
        match (op.uid(), op.gid()) {
            (None, None) => (),
            (uid, gid) => {
                fd.fchownat("", uid, gid, libc::AT_SYMLINK_NOFOLLOW)?;
            }
        }

        // truncate
        if let Some(size) = op.size() {
            if let Some(file) = file.as_mut() {
                fs::ftruncate(&**file, size as libc::off_t)?;
            } else {
                fs::truncate(fd.procname(), size as libc::off_t)?;
            }
        }

        // utimens
        fn make_timespec(t: Option<(u64, u32, bool)>) -> libc::timespec {
            match t {
                Some((_, _, true)) => libc::timespec {
                    tv_sec: 0,
                    tv_nsec: libc::UTIME_NOW,
                },
                Some((sec, nsec, false)) => libc::timespec {
                    tv_sec: sec as i64,
                    tv_nsec: nsec as u64 as i64,
                },
                None => libc::timespec {
                    tv_sec: 0,
                    tv_nsec: libc::UTIME_OMIT,
                },
            }
        }
        match (op.atime_raw(), op.mtime_raw()) {
            (None, None) => (),
            (atime, mtime) => {
                let tv = [make_timespec(atime), make_timespec(mtime)];
                if let Some(file) = file.as_mut() {
                    fs::futimens(&**file, tv)?;
                } else if inode.is_symlink {
                    // According to libfuse/examples/passthrough_hp.cc, it does not work on
                    // the current kernels, but may in the future.
                    fd.futimensat("", tv, libc::AT_SYMLINK_NOFOLLOW)
                        .map_err(|err| match err.raw_os_error() {
                            Some(libc::EINVAL) => io::Error::from_raw_os_error(libc::EPERM),
                            _ => err,
                        })?;
                } else {
                    fs::utimens(fd.procname(), tv)?;
                }
            }
        }

        // finally, acquiring the latest metadata from the source filesystem.
        let stat = fd.fstatat("", libc::AT_SYMLINK_NOFOLLOW)?;
        let mut attr = ReplyAttr::new(stat.try_into().unwrap());
        if let Some(timeout) = self.timeout {
            attr.ttl_attr(timeout);
        };

        Ok(attr)
    }

    async fn do_readlink(&self, op: &op::Readlink<'_>) -> io::Result<OsString> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;
        inode.fd.readlink("")
    }

    async fn do_link(&self, op: &op::Link<'_>) -> io::Result<ReplyEntry> {
        let inodes = self.inodes.lock().await;

        let source = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let mut source = source.lock().await;

        let parent = inodes.get(op.newparent()).ok_or_else(no_entry)?;
        let parent = parent.lock().await;

        if source.is_symlink {
            source
                .fd
                .linkat("", &parent.fd, op.newname(), 0)
                .map_err(|err| match err.raw_os_error() {
                    Some(libc::ENOENT) | Some(libc::EINVAL) => {
                        // no race-free way to hard-link a symlink.
                        io::Error::from_raw_os_error(libc::EOPNOTSUPP)
                    }
                    _ => err,
                })?;
        } else {
            fs::link(
                source.fd.procname(),
                &parent.fd,
                op.newname(),
                libc::AT_SYMLINK_FOLLOW,
            )?;
        }

        let stat = source.fd.fstatat("", libc::AT_SYMLINK_NOFOLLOW)?;
        let entry = self.make_entry_param(source.ino, stat);

        source.refcount += 1;

        Ok(entry)
    }

    async fn make_node(
        &self,
        parent: Ino,
        name: &OsStr,
        mode: u32,
        rdev: Option<u32>,
        link: Option<&OsStr>,
    ) -> io::Result<ReplyEntry> {
        {
            let inodes = self.inodes.lock().await;
            let parent = inodes.get(parent).ok_or_else(no_entry)?;
            let parent = parent.lock().await;

            match mode & libc::S_IFMT {
                libc::S_IFDIR => {
                    parent.fd.mkdirat(name, mode)?;
                }
                libc::S_IFLNK => {
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
        self.do_lookup(parent, name).await
    }

    async fn do_unlink(&self, op: &op::Unlink<'_>) -> io::Result<()> {
        let inodes = self.inodes.lock().await;
        let parent = inodes.get(op.parent()).ok_or_else(no_entry)?;
        let parent = parent.lock().await;
        parent.fd.unlinkat(op.name(), 0)?;
        Ok(())
    }

    async fn do_rmdir(&self, op: &op::Rmdir<'_>) -> io::Result<()> {
        let inodes = self.inodes.lock().await;
        let parent = inodes.get(op.parent()).ok_or_else(no_entry)?;
        let parent = parent.lock().await;
        parent.fd.unlinkat(op.name(), libc::AT_REMOVEDIR)?;
        Ok(())
    }

    async fn do_rename(&self, op: &op::Rename<'_>) -> io::Result<()> {
        if op.flags() != 0 {
            // rename2 is not supported.
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }

        let inodes = self.inodes.lock().await;

        let parent = inodes.get(op.parent()).ok_or_else(no_entry)?;
        let newparent = inodes.get(op.newparent()).ok_or_else(no_entry)?;

        let parent = parent.lock().await;
        if op.parent() == op.newparent() {
            parent
                .fd
                .renameat(op.name(), None::<&FileDesc>, op.newname())?;
        } else {
            let newparent = newparent.lock().await;
            parent
                .fd
                .renameat(op.name(), Some(&newparent.fd), op.newname())?;
        }

        Ok(())
    }

    async fn do_opendir(&self, op: &op::Opendir<'_>) -> io::Result<ReplyOpen> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;
        let dir = inode.fd.read_dir()?;
        let fh = self.opened_dirs.insert(Mutex::new(dir)).await;

        Ok(ReplyOpen::new(fh))
    }

    async fn do_readdir(&self, op: &op::Readdir<'_>) -> io::Result<Vec<DirEntry>> {
        let read_dir = self
            .opened_dirs
            .get(op.fh())
            .await
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        let mut read_dir = read_dir.lock().await;
        let read_dir = &mut *read_dir;

        read_dir.seek(op.offset());

        let mut entries = vec![];
        let mut total_len = 0;
        for entry in read_dir {
            let entry = entry?;
            if total_len + entry.as_ref().len() > op.size() as usize {
                break;
            }
            total_len += entry.as_ref().len();
            entries.push(entry);
        }

        Ok(entries)
    }

    async fn do_fsyncdir(&self, op: &op::Fsyncdir<'_>) -> io::Result<()> {
        let read_dir = self.opened_dirs.get(op.fh()).await.ok_or_else(no_entry)?;
        let read_dir = read_dir.lock().await;

        if op.datasync() {
            read_dir.sync_data()?;
        } else {
            read_dir.sync_all()?;
        }

        Ok(())
    }

    async fn do_releasedir(&self, op: &op::Releasedir<'_>) -> io::Result<()> {
        let _dir = self.opened_dirs.remove(op.fh()).await;
        Ok(())
    }

    async fn do_open(&self, op: &op::Open<'_>) -> io::Result<ReplyOpen> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        let options = OpenOptions::from({
            let mut options = std::fs::OpenOptions::new();
            match (op.flags() & 0x03) as i32 {
                libc::O_RDONLY => {
                    options.read(true);
                }
                libc::O_WRONLY => {
                    options.write(true);
                }
                libc::O_RDWR => {
                    options.read(true).write(true);
                }
                _ => (),
            }
            options.custom_flags(op.flags() as i32 & !libc::O_NOFOLLOW);
            options
        });
        let file = options.open(&inode.fd.procname()).await?;
        let fh = self.opened_files.insert(Mutex::new(file)).await;

        Ok(ReplyOpen::new(fh))
    }

    async fn do_read(&self, op: &op::Read<'_>) -> io::Result<Vec<u8>> {
        let file = self.opened_files.get(op.fh()).await.ok_or_else(no_entry)?;
        let mut file = file.lock().await;
        let file = &mut *file;

        file.seek(io::SeekFrom::Start(op.offset())).await?;

        use tokio::io::AsyncReadExt;
        let mut buf = Vec::<u8>::with_capacity(op.size() as usize);
        tokio::io::copy(&mut file.take(op.size() as u64), &mut buf).await?;

        Ok(buf)
    }

    async fn do_write<T: ?Sized>(
        &self,
        cx: &mut Context<'_, T>,
        op: &op::Write<'_>,
    ) -> io::Result<ReplyWrite>
    where
        T: Reader + Unpin,
    {
        let file = self.opened_files.get(op.fh()).await.ok_or_else(no_entry)?;
        let mut file = file.lock().await;
        let file = &mut *file;

        file.seek(io::SeekFrom::Start(op.offset())).await?;

        // At here, the data is transferred via the temporary buffer due to
        // the incompatibility between the I/O abstraction in `futures` and
        // `tokio`.
        //
        // In order to efficiently transfer the large files, both of zero
        // copying support in `polyfuse` and resolution of impedance mismatch
        // between `futures::io` and `tokio::io` are required.
        let mut buf = Vec::with_capacity(op.size() as usize);
        {
            use futures::io::AsyncReadExt;
            let mut reader = cx.reader();
            reader.read_to_end(&mut buf).await?;
        }

        use tokio::io::AsyncReadExt;
        let mut buf = &buf[..];
        let mut buf = (&mut buf).take(op.size() as u64);
        let written = tokio::io::copy(&mut buf, &mut *file).await?;

        Ok(ReplyWrite::new(written as u32))
    }

    async fn do_flush(&self, op: &op::Flush<'_>) -> io::Result<()> {
        let file = self.opened_files.get(op.fh()).await.ok_or_else(no_entry)?;
        let file = file.lock().await;

        file.try_clone().await?;

        Ok(())
    }

    async fn do_fsync(&self, op: &op::Fsync<'_>) -> io::Result<()> {
        let file = self.opened_files.get(op.fh()).await.ok_or_else(no_entry)?;
        let mut file = file.lock().await;

        if op.datasync() {
            file.sync_data().await?;
        } else {
            file.sync_all().await?;
        }

        Ok(())
    }

    async fn do_flock(&self, op: &op::Flock<'_>) -> io::Result<()> {
        let file = self.opened_files.get(op.fh()).await.ok_or_else(no_entry)?;
        let file = file.lock().await;

        let op = op.op().expect("invalid lock operation") as i32;

        fs::flock(&*file, op)?;

        Ok(())
    }

    async fn do_fallocate(&self, op: &op::Fallocate<'_>) -> io::Result<()> {
        if op.mode() != 0 {
            return Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP));
        }

        let file = self.opened_files.get(op.fh()).await.ok_or_else(no_entry)?;
        let file = file.lock().await;

        fs::posix_fallocate(&*file, op.offset() as i64, op.length() as i64)?;

        Ok(())
    }

    async fn do_release(&self, op: &op::Release<'_>) -> io::Result<()> {
        let _file = self.opened_files.remove(op.fh()).await;
        Ok(())
    }

    async fn do_getxattr_size(&self, op: &op::Getxattr<'_>) -> io::Result<ReplyXattr> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(io::Error::from_raw_os_error(libc::ENOTSUP));
        }

        let size = fs::getxattr(inode.fd.procname(), op.name(), None)?;

        Ok(ReplyXattr::new(size as u32))
    }

    async fn do_getxattr_value(&self, op: &op::Getxattr<'_>, size: u32) -> io::Result<Vec<u8>> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(io::Error::from_raw_os_error(libc::ENOTSUP));
        }

        let mut value = vec![0u8; size as usize];
        let n = fs::getxattr(inode.fd.procname(), op.name(), Some(&mut value[..]))?;
        value.resize(n as usize, 0);

        Ok(value)
    }

    async fn do_listxattr_size(&self, op: &op::Listxattr<'_>) -> io::Result<ReplyXattr> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(io::Error::from_raw_os_error(libc::ENOTSUP));
        }

        let size = fs::listxattr(inode.fd.procname(), None)?;

        Ok(ReplyXattr::new(size as u32))
    }

    async fn do_listxattr_value(&self, op: &op::Listxattr<'_>, size: u32) -> io::Result<Vec<u8>> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(io::Error::from_raw_os_error(libc::ENOTSUP));
        }

        let mut value = vec![0u8; size as usize];
        let n = fs::listxattr(inode.fd.procname(), Some(&mut value[..]))?;
        value.resize(n as usize, 0);

        Ok(value)
    }

    async fn do_setxattr(&self, op: &op::Setxattr<'_>) -> io::Result<()> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(io::Error::from_raw_os_error(libc::ENOTSUP));
        }

        fs::setxattr(
            inode.fd.procname(),
            op.name(),
            op.value(),
            op.flags() as libc::c_int,
        )?;

        Ok(())
    }

    async fn do_removexattr(&self, op: &op::Removexattr<'_>) -> io::Result<()> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        if inode.is_symlink {
            // no race-free way to getxattr on symlink.
            return Err(io::Error::from_raw_os_error(libc::ENOTSUP));
        }

        fs::removexattr(inode.fd.procname(), op.name())?;

        Ok(())
    }

    async fn do_statfs(&self, op: &op::Statfs<'_>) -> io::Result<ReplyStatfs> {
        let inodes = self.inodes.lock().await;
        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        let st = fs::fstatvfs(&inode.fd)?.try_into().unwrap();

        Ok(ReplyStatfs::new(st))
    }
}

#[polyfuse::async_trait]
impl Filesystem for Passthrough {
    #[allow(clippy::cognitive_complexity)]
    async fn call<'a, 'cx, T: ?Sized>(
        &'a self,
        cx: &'a mut Context<'cx, T>,
        op: Operation<'cx>,
    ) -> io::Result<()>
    where
        T: Reader + Writer + Send + Unpin,
    {
        macro_rules! try_reply {
            ($e:expr) => {
                match ($e).await {
                    Ok(ret) => ret,
                    Err(err) => return cx.reply_err(io_to_errno(err)).await,
                }
            };
        }

        // TODOs:
        // * readdirplus
        // * create
        match op {
            Operation::Lookup(op) => {
                let entry = try_reply!(self.do_lookup(op.parent(), op.name()));
                op.reply(cx, entry).await
            }
            Operation::Forget(forgets) => {
                for forget in forgets.as_ref() {
                    self.forget_one(forget.ino(), forget.nlookup()).await;
                }
                Ok(())
            }
            Operation::Getattr(op) => {
                let attr = try_reply!(self.do_getattr(&op));
                op.reply(cx, attr).await
            }
            Operation::Setattr(op) => {
                let attr = try_reply!(self.do_setattr(&op));
                op.reply(cx, attr).await
            }
            Operation::Readlink(op) => {
                let link = try_reply!(self.do_readlink(&op));
                op.reply(cx, link).await
            }
            Operation::Link(op) => {
                let entry = try_reply!(self.do_link(&op));
                op.reply(cx, entry).await
            }

            Operation::Mknod(op) => {
                let entry = try_reply!(self.make_node(
                    op.parent(),
                    op.name(),
                    op.mode(),
                    Some(op.rdev()),
                    None
                ));
                op.reply(cx, entry).await
            }
            Operation::Mkdir(op) => {
                let entry = try_reply!(self.make_node(
                    op.parent(),
                    op.name(),
                    libc::S_IFDIR | op.mode(),
                    None,
                    None
                ));
                op.reply(cx, entry).await
            }
            Operation::Symlink(op) => {
                let entry = try_reply!(self.make_node(
                    op.parent(),
                    op.name(),
                    libc::S_IFLNK,
                    None,
                    Some(op.link())
                ));
                op.reply(cx, entry).await
            }

            Operation::Unlink(op) => {
                try_reply!(self.do_unlink(&op));
                op.reply(cx).await
            }
            Operation::Rmdir(op) => {
                try_reply!(self.do_rmdir(&op));
                op.reply(cx).await
            }
            Operation::Rename(op) => {
                try_reply!(self.do_rename(&op));
                op.reply(cx).await
            }

            Operation::Opendir(op) => {
                let open = try_reply!(self.do_opendir(&op));
                op.reply(cx, open).await
            }
            Operation::Readdir(op) => {
                let entries = try_reply!(self.do_readdir(&op));
                let entries: Vec<&[u8]> = entries.iter().map(|entry| entry.as_ref()).collect();
                op.reply_vectored(cx, &entries[..]).await
            }
            Operation::Fsyncdir(op) => {
                try_reply!(self.do_fsyncdir(&op));
                op.reply(cx).await
            }
            Operation::Releasedir(op) => {
                try_reply!(self.do_releasedir(&op));
                op.reply(cx).await
            }

            Operation::Open(op) => {
                let open = try_reply!(self.do_open(&op));
                op.reply(cx, open).await
            }
            Operation::Read(op) => {
                let data = try_reply!(self.do_read(&op));
                op.reply(cx, data).await
            }
            Operation::Write(op) => {
                let written = try_reply!(self.do_write(&mut *cx, &op));
                op.reply(cx, written).await
            }
            Operation::Flush(op) => {
                try_reply!(self.do_flush(&op));
                op.reply(cx).await
            }
            Operation::Fsync(op) => {
                try_reply!(self.do_fsync(&op));
                op.reply(cx).await
            }
            Operation::Flock(op) => {
                try_reply!(self.do_flock(&op));
                op.reply(cx).await
            }
            Operation::Fallocate(op) => {
                try_reply!(self.do_fallocate(&op));
                op.reply(cx).await
            }
            Operation::Release(op) => {
                try_reply!(self.do_release(&op));
                op.reply(cx).await
            }

            Operation::Getxattr(op) => match op.size() {
                0 => {
                    let size = try_reply!(self.do_getxattr_size(&op));
                    op.reply_size(cx, size).await
                }
                n => {
                    let value = try_reply!(self.do_getxattr_value(&op, n));
                    op.reply(cx, value).await
                }
            },
            Operation::Listxattr(op) => match op.size() {
                0 => {
                    let size = try_reply!(self.do_listxattr_size(&op));
                    op.reply_size(cx, size).await
                }
                n => {
                    let value = try_reply!(self.do_listxattr_value(&op, n));
                    op.reply(cx, value).await
                }
            },
            Operation::Setxattr(op) => {
                try_reply!(self.do_setxattr(&op));
                op.reply(cx).await
            }
            Operation::Removexattr(op) => {
                try_reply!(self.do_removexattr(&op));
                op.reply(cx).await
            }

            Operation::Statfs(op) => {
                let stat = try_reply!(self.do_statfs(&op));
                op.reply(cx, stat).await
            }

            _ => Ok(()),
        }
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
    async fn get(&self, fh: u64) -> Option<Arc<T>> {
        self.0.lock().await.get(fh as usize).cloned()
    }

    async fn remove(&self, fh: u64) -> Arc<T> {
        self.0.lock().await.remove(fh as usize)
    }

    async fn insert(&self, entry: T) -> u64 {
        self.0.lock().await.insert(Arc::new(entry)) as u64
    }
}

// ==== INode ====

struct INode {
    ino: Ino,
    src_id: SrcId,
    is_symlink: bool,
    fd: FileDesc,
    refcount: u64,
}

// ==== INodeTable ====

struct INodeTable {
    map: HashMap<Ino, Arc<Mutex<INode>>>,
    src_to_ino: HashMap<SrcId, Ino>,
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

    fn get(&self, ino: Ino) -> Option<Arc<Mutex<INode>>> {
        self.map.get(&ino).cloned()
    }

    fn get_src(&self, src_id: SrcId) -> Option<Arc<Mutex<INode>>> {
        let ino = self.src_to_ino.get(&src_id)?;
        self.map.get(ino).cloned()
    }

    fn vacant_entry(&mut self) -> VacantEntry<'_> {
        let ino = self.next_ino;
        VacantEntry { table: self, ino }
    }
}

struct VacantEntry<'a> {
    table: &'a mut INodeTable,
    ino: Ino,
}

impl VacantEntry<'_> {
    fn ino(&self) -> Ino {
        self.ino
    }

    fn insert(self, inode: INode) {
        let src_id = inode.src_id;
        self.table.map.insert(self.ino, Arc::new(Mutex::new(inode)));
        self.table.src_to_ino.insert(src_id, self.ino);
        self.table.next_ino += 1;
    }
}

#[inline]
fn no_entry() -> io::Error {
    io::Error::from_raw_os_error(libc::ENOENT)
}

#[inline]
fn io_to_errno(err: io::Error) -> i32 {
    err.raw_os_error().unwrap_or(libc::EIO)
}
