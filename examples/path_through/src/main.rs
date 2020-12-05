#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented, clippy::todo)]

// This example is another version of `passthrough.rs` that uses the
// path strings instead of the file descriptors with `O_PATH` flag
// for referencing the underlying file entries.
// It has the advantage of being able to use straightforward the
// *standard* filesystem APIs, but also the additional path resolution
// cost for each operation.
//
// This example is inteded to be used as a templete for implementing
// the path based filesystems such as libfuse's highlevel API.

use polyfuse::{
    op::{self, Forget},
    reply::{AttrOut, EntryOut, FileAttr, OpenOut, ReaddirOut, WriteOut},
    Config, Connection, MountOptions, Operation, Session,
};

use anyhow::Context as _;
use async_io::Async;
use async_std::{
    fs::{File, OpenOptions, ReadDir},
    sync::Mutex,
};
use futures::{io::AsyncBufRead, prelude::*};
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap},
    ffi::OsString,
    io,
    os::unix::prelude::*,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[async_std::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let source: PathBuf = args
        .opt_value_from_str(["-s", "--source"])?
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    anyhow::ensure!(source.is_dir(), "the source path must be a directory");

    let mountpoint: PathBuf = args.free_from_str()?.context("missing mountpoint")?;
    anyhow::ensure!(mountpoint.is_dir(), "the mountpoint must be a directory");

    let conn = async_std::task::spawn_blocking(move || {
        Connection::open(mountpoint, MountOptions::default())
    })
    .await?;
    let conn = Async::new(conn)?;

    let session = Session::start(&conn, conn.get_ref(), Config::default()).await?;

    let fs = PathThrough::new(source)?;

    while let Some(req) = session.next_request(&conn).await? {
        let op = req.operation()?;
        tracing::debug!("handle operation: {:#?}", op);

        macro_rules! try_reply {
            ($e:expr) => {
                match ($e).await {
                    Ok(reply) => req.reply(conn.get_ref(), reply)?,
                    Err(err) => {
                        req.reply_error(conn.get_ref(), err.raw_os_error().unwrap_or(libc::EIO))?
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
            Operation::Open(op) => try_reply!(fs.do_open(&op)),
            Operation::Read(op) => try_reply!(fs.do_read(&op)),
            Operation::Write(op, mut data) => {
                let res = fs.do_write(&op, &mut data).await;
                try_reply!(async { res })
            }
            Operation::Flush(op) => try_reply!(fs.do_flush(&op)),
            Operation::Fsync(op) => try_reply!(fs.do_fsync(&op)),
            Operation::Release(op) => try_reply!(fs.do_release(&op)),

            _ => req.reply_error(conn.get_ref(), libc::ENOSYS)?,
        }
    }

    Ok(())
}

type Ino = u64;

struct INode {
    ino: Ino,
    path: PathBuf,
    refcount: u64,
}

struct INodeTable {
    map: HashMap<Ino, Arc<Mutex<INode>>>,
    path_to_ino: HashMap<PathBuf, Ino>,
    next_ino: u64,
}

impl INodeTable {
    fn new() -> Self {
        INodeTable {
            map: HashMap::new(),
            path_to_ino: HashMap::new(),
            next_ino: 1, // the inode number is started with 1 and the first node is root.
        }
    }

    fn vacant_entry(&mut self) -> VacantEntry<'_> {
        let ino = self.next_ino;
        VacantEntry { table: self, ino }
    }

    fn get(&self, ino: Ino) -> Option<Arc<Mutex<INode>>> {
        self.map.get(&ino).cloned()
    }

    fn get_path(&self, path: &Path) -> Option<Arc<Mutex<INode>>> {
        let ino = self.path_to_ino.get(path).copied()?;
        self.get(ino)
    }
}

struct VacantEntry<'a> {
    table: &'a mut INodeTable,
    ino: Ino,
}

impl VacantEntry<'_> {
    fn insert(mut self, inode: INode) {
        let path = inode.path.clone();
        self.table.map.insert(self.ino, Arc::new(Mutex::new(inode)));
        self.table.path_to_ino.insert(path, self.ino);
        self.table.next_ino += 1;
    }
}

struct DirHandle {
    read_dir: ReadDir,
    last_entry: Option<DirEntry>,
    offset: u64,
}

struct DirEntry {
    name: OsString,
    ino: Ino,
    typ: u32,
}

struct FileHandle {
    file: File,
}

struct PathThrough {
    source: PathBuf,
    inodes: Mutex<INodeTable>,
    dirs: Mutex<Slab<Arc<Mutex<DirHandle>>>>,
    files: Mutex<Slab<Arc<Mutex<FileHandle>>>>,
}

impl PathThrough {
    fn new(source: PathBuf) -> io::Result<Self> {
        let source = source.canonicalize()?;

        let mut inodes = INodeTable::new();
        inodes.vacant_entry().insert(INode {
            ino: 1,
            path: PathBuf::new(),
            refcount: u64::max_value() / 2,
        });

        Ok(Self {
            source,
            inodes: Mutex::new(inodes),
            dirs: Mutex::default(),
            files: Mutex::default(),
        })
    }

    async fn fill_attr(&self, path: impl AsRef<Path>, attr: &mut FileAttr) -> io::Result<()> {
        let metadata = async_std::fs::symlink_metadata(self.source.join(path)).await?;

        attr.ino(metadata.ino());
        attr.size(metadata.size());
        attr.mode(metadata.mode());
        attr.nlink(metadata.nlink() as u32);
        attr.uid(metadata.uid());
        attr.gid(metadata.gid());
        attr.rdev(metadata.rdev() as u32);
        attr.blksize(metadata.blksize() as u32);
        attr.blocks(metadata.blocks());
        attr.atime(Duration::new(
            metadata.atime() as u64,
            metadata.atime_nsec() as u32,
        ));
        attr.mtime(Duration::new(
            metadata.mtime() as u64,
            metadata.mtime_nsec() as u32,
        ));
        attr.ctime(Duration::new(
            metadata.ctime() as u64,
            metadata.ctime_nsec() as u32,
        ));

        Ok(())
    }

    async fn do_lookup(&self, op: &op::Lookup<'_>) -> io::Result<EntryOut> {
        let mut inodes = self.inodes.lock().await;

        let parent = inodes.get(op.parent()).ok_or_else(no_entry)?;
        let parent = parent.lock().await;

        let path = parent.path.join(op.name());

        let mut out = EntryOut::default();
        self.fill_attr(&path, out.attr()).await?;

        match inodes.get_path(&path) {
            Some(inode) => {
                let mut inode = inode.lock().await;
                out.ino(inode.ino);
                inode.refcount += 1;
            }
            None => {
                let entry = inodes.vacant_entry();
                out.ino(entry.ino);
                let inode = INode {
                    ino: entry.ino,
                    path,
                    refcount: 1,
                };
                entry.insert(inode);
            }
        }

        Ok(out)
    }

    async fn do_forget(&self, forgets: &[Forget]) {
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

    async fn do_getattr(&self, op: &op::Getattr<'_>) -> io::Result<AttrOut> {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        let mut out = AttrOut::default();
        self.fill_attr(&inode.path, out.attr()).await?;

        Ok(out)
    }

    async fn do_setattr(&self, op: &op::Setattr<'_>) -> io::Result<AttrOut> {
        let file = match op.fh() {
            Some(fh) => {
                let files = self.files.lock().await;
                files.get(fh as usize).cloned()
            }
            None => None,
        };
        let mut file = match file {
            Some(ref file) => {
                let file = file.lock().await;
                file.file.sync_all().await?;
                Some(file) // keep file lock
            }
            None => None,
        };

        let inode = {
            let inodes = self.inodes.lock().await;
            inodes.get(op.ino()).ok_or_else(no_entry)?
        };
        let inode = inode.lock().await;
        let path = Arc::new(self.source.join(&inode.path));

        enum FileRef<'a> {
            Borrowed(&'a mut File),
            Owned(File),
        }
        impl AsMut<File> for FileRef<'_> {
            fn as_mut(&mut self) -> &mut File {
                match self {
                    Self::Borrowed(file) => file,
                    Self::Owned(file) => file,
                }
            }
        }
        let mut file = match file {
            Some(ref mut file) => FileRef::Borrowed(&mut file.file),
            None => FileRef::Owned(File::open(&*path).await?),
        };

        // chmod
        if let Some(mode) = op.mode() {
            let perm = std::fs::Permissions::from_mode(mode);
            file.as_mut().set_permissions(perm).await?;
        }

        // truncate
        if let Some(size) = op.size() {
            file.as_mut().set_len(size).await?;
        }

        // chown
        match (op.uid(), op.gid()) {
            (None, None) => (),
            (uid, gid) => {
                let path = path.clone();
                let uid = uid.map(nix::unistd::Uid::from_raw);
                let gid = gid.map(nix::unistd::Gid::from_raw);
                async_std::task::spawn_blocking(move || nix::unistd::chown(&*path, uid, gid))
                    .await
                    .map_err(nix_to_io_error)?;
            }
        }

        // TODO: utimes

        let mut out = AttrOut::default();
        self.fill_attr(&inode.path, out.attr()).await?;

        Ok(out)
    }

    async fn do_readlink(&self, op: &op::Readlink<'_>) -> io::Result<OsString> {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        async_std::fs::read_link(self.source.join(&inode.path))
            .await
            .map(|path| path.into_os_string())
    }

    async fn do_opendir(&self, op: &op::Opendir<'_>) -> io::Result<OpenOut> {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        let dir = DirHandle {
            read_dir: async_std::fs::read_dir(self.source.join(&inode.path)).await?,
            last_entry: None,
            offset: 1,
        };

        let mut dirs = self.dirs.lock().await;
        let key = dirs.insert(Arc::new(Mutex::new(dir)));

        let mut out = OpenOut::default();
        out.fh(key as u64);

        Ok(out)
    }

    async fn do_readdir(&self, op: &op::Readdir<'_>) -> io::Result<ReaddirOut> {
        if op.mode() == op::ReaddirMode::Plus {
            return Err(io::Error::from_raw_os_error(libc::ENOSYS));
        }

        let dir = {
            let dirs = self.dirs.lock().await;
            dirs.get(op.fh() as usize)
                .cloned()
                .ok_or_else(|| io::Error::from_raw_os_error(libc::EIO))?
        };
        let dir = &mut *dir.lock().await;

        let mut out = ReaddirOut::new(op.size() as usize);
        let mut at_least_one_entry = false;

        if let Some(entry) = dir.last_entry.take() {
            let full = out.entry(entry.name.as_ref(), entry.ino, entry.typ, dir.offset);
            if full {
                dir.last_entry.replace(entry);
                return Err(io::Error::from_raw_os_error(libc::ERANGE));
            }
            at_least_one_entry = true;
            dir.offset += 1;
        }

        while let Some(entry) = dir.read_dir.next().await {
            let entry = entry?;
            match entry.file_name() {
                name if name.as_bytes() == b"." || name.as_bytes() == b".." => continue,
                _ => (),
            }

            let metadata = entry.metadata().await?;
            let file_type = metadata.file_type();
            let typ = if file_type.is_file() {
                libc::DT_REG as u32
            } else if file_type.is_dir() {
                libc::DT_DIR as u32
            } else if file_type.is_symlink() {
                libc::DT_LNK as u32
            } else {
                libc::DT_UNKNOWN as u32
            };

            let full = out.entry(&entry.file_name(), metadata.ino(), typ, dir.offset);
            if full {
                dir.last_entry.replace(DirEntry {
                    name: entry.file_name(),
                    ino: metadata.ino(),
                    typ,
                });
                if !at_least_one_entry {
                    return Err(io::Error::from_raw_os_error(libc::ERANGE));
                }
                break;
            }

            at_least_one_entry = true;
            dir.offset += 1;
        }

        Ok(out)
    }

    async fn do_releasedir(&self, op: &op::Releasedir<'_>) -> io::Result<()> {
        let mut dirs = self.dirs.lock().await;

        let dir = dirs.remove(op.fh() as usize);
        drop(dir);

        Ok(())
    }

    async fn do_open(&self, op: &op::Open<'_>) -> io::Result<OpenOut> {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        let mut options = OpenOptions::new();
        match op.flags() as i32 & libc::O_ACCMODE {
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

        let file = FileHandle {
            file: options.open(self.source.join(&inode.path)).await?,
        };

        let mut files = self.files.lock().await;
        let key = files.insert(Arc::new(Mutex::new(file)));

        let mut out = OpenOut::default();
        out.fh(key as u64);

        Ok(out)
    }

    async fn do_read(&self, op: &op::Read<'_>) -> io::Result<Vec<u8>> {
        let files = self.files.lock().await;

        let file = files
            .get(op.fh() as usize)
            .cloned()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EIO))?;
        let file = &mut *file.lock().await;
        let file = &mut file.file;

        file.seek(io::SeekFrom::Start(op.offset())).await?;

        let mut buf = Vec::<u8>::with_capacity(op.size() as usize);
        file.take(op.size() as u64).read_to_end(&mut buf).await?;

        Ok(buf)
    }

    async fn do_write<T: ?Sized>(&self, op: &op::Write<'_>, data: &mut T) -> io::Result<WriteOut>
    where
        T: AsyncBufRead + Unpin,
    {
        let files = self.files.lock().await;

        let file = files
            .get(op.fh() as usize)
            .cloned()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EIO))?;
        let file = &mut *file.lock().await;
        let file = &mut file.file;

        file.seek(io::SeekFrom::Start(op.offset())).await?;

        let mut written = 0;
        let size = op.size() as usize;
        loop {
            let chunk = data.fill_buf().await?;
            let chunk = &chunk[..std::cmp::min(chunk.len(), size - written)];
            if chunk.is_empty() {
                // EOF
                break;
            }

            let n = file.write(chunk).await?;
            written += n;
        }

        let mut out = WriteOut::default();
        out.size(written as u32);
        Ok(out)
    }

    async fn do_flush(&self, op: &op::Flush<'_>) -> io::Result<()> {
        let files = self.files.lock().await;

        let file = files
            .get(op.fh() as usize)
            .cloned()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EIO))?;
        let file = &mut *file.lock().await;

        file.file.sync_all().await?;

        Ok(())
    }

    async fn do_fsync(&self, op: &op::Fsync<'_>) -> io::Result<()> {
        let files = self.files.lock().await;

        let file = files
            .get(op.fh() as usize)
            .cloned()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EIO))?;
        let mut file = file.lock().await;
        let file = &mut file.file;

        if op.datasync() {
            file.sync_data().await?;
        } else {
            file.sync_all().await?;
        }

        Ok(())
    }

    async fn do_release(&self, op: &op::Release<'_>) -> io::Result<()> {
        let mut files = self.files.lock().await;

        let file = files.remove(op.fh() as usize);
        drop(file);

        Ok(())
    }
}

#[inline]
fn no_entry() -> io::Error {
    io::Error::from_raw_os_error(libc::ENOENT)
}

fn nix_to_io_error(err: nix::Error) -> io::Error {
    let errno = err.as_errno().map_or(libc::EIO, |errno| errno as i32);
    io::Error::from_raw_os_error(errno)
}
