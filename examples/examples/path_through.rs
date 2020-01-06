#![allow(clippy::unnecessary_mut_passed)]
#![warn(clippy::unimplemented, clippy::todo)]

// This example is another version of `passthrough.rs` that uses the
// path strings instead of the file descriptors with `O_PATH` flag
// for referencing the underlying file entries.
// It has the advantage of being able to use straightforward the
// *standard* filesystem APIs, but also the additional path resolution
// cost for each operation.
//
// This example is inteded to be used as a templete for implementing
// the path based filesystems such as libfuse's highlevel API.

use pico_args::Arguments;
use polyfuse::{
    op,
    reply::{ReplyAttr, ReplyEntry, ReplyOpen},
    DirEntry, FileAttr, Forget,
};
use polyfuse_examples::prelude::*;
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap},
    io,
    os::unix::prelude::*,
    sync::Arc,
};
use tokio::{
    fs::{File, OpenOptions, ReadDir},
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

    let mountpoint: PathBuf = args
        .free_from_str()?
        .ok_or_else(|| anyhow::anyhow!("missing mountpoint"))?;
    anyhow::ensure!(mountpoint.is_dir(), "the mountpoint must be a directory");

    let fs = PathThrough::new(source)?;
    polyfuse_tokio::mount(fs, mountpoint, &[]).await?;

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

    fn make_entry_out(&self, ino: Ino, attr: FileAttr) -> io::Result<ReplyEntry> {
        let mut reply = ReplyEntry::default();
        reply.ino(ino);
        reply.attr(attr);
        Ok(reply)
    }

    async fn get_attr(&self, path: impl AsRef<Path>) -> io::Result<FileAttr> {
        let metadata = tokio::fs::symlink_metadata(self.source.join(path)).await?;

        let mut attr = FileAttr::default();
        attr.set_ino(metadata.ino());
        attr.set_size(metadata.size());
        attr.set_mode(metadata.mode());
        attr.set_nlink(metadata.nlink() as u32);
        attr.set_uid(metadata.uid());
        attr.set_gid(metadata.gid());
        attr.set_rdev(metadata.rdev() as u32);
        attr.set_blksize(metadata.blksize() as u32);
        attr.set_blocks(metadata.blocks());
        attr.set_atime_raw((metadata.atime() as u64, metadata.atime_nsec() as u32));
        attr.set_mtime_raw((metadata.mtime() as u64, metadata.mtime_nsec() as u32));
        attr.set_ctime_raw((metadata.ctime() as u64, metadata.ctime_nsec() as u32));

        Ok(attr)
    }

    async fn do_lookup(&self, op: &op::Lookup<'_>) -> io::Result<ReplyEntry> {
        let mut inodes = self.inodes.lock().await;

        let parent = inodes.get(op.parent()).ok_or_else(no_entry)?;
        let parent = parent.lock().await;

        let path = parent.path.join(op.name());
        let metadata = self.get_attr(&path).await?;

        let ino;
        match inodes.get_path(&path) {
            Some(inode) => {
                let mut inode = inode.lock().await;
                ino = inode.ino;
                inode.refcount += 1;
            }
            None => {
                let entry = inodes.vacant_entry();
                ino = entry.ino;
                entry.insert(INode {
                    ino,
                    path,
                    refcount: 1,
                })
            }
        }

        self.make_entry_out(ino, metadata)
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

    async fn do_getattr(&self, op: &op::Getattr<'_>) -> io::Result<ReplyAttr> {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        let attr = self.get_attr(&inode.path).await?;

        Ok(ReplyAttr::new(attr))
    }

    async fn do_readlink(&self, op: &op::Readlink<'_>) -> io::Result<OsString> {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        let link = tokio::fs::read_link(self.source.join(&inode.path)).await?;

        // TODO: impl ScatteredBytes for PathBuf
        Ok(link.into_os_string())
    }

    async fn do_opendir(&self, op: &op::Opendir<'_>) -> io::Result<ReplyOpen> {
        let inodes = self.inodes.lock().await;

        let inode = inodes.get(op.ino()).ok_or_else(no_entry)?;
        let inode = inode.lock().await;

        let dir = DirHandle {
            read_dir: tokio::fs::read_dir(self.source.join(&inode.path)).await?,
            last_entry: None,
            offset: 1,
        };

        let mut dirs = self.dirs.lock().await;
        let key = dirs.insert(Arc::new(Mutex::new(dir)));

        Ok(ReplyOpen::new(key as u64))
    }

    async fn do_readdir(&self, op: &op::Readdir<'_>) -> io::Result<impl ScatteredBytes> {
        let dirs = self.dirs.lock().await;

        let dir = dirs
            .get(op.fh() as usize)
            .cloned()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EIO))?;
        let mut dir = dir.lock().await;
        let dir = &mut *dir;

        let mut entries = vec![];
        let mut total_len = 0;

        if let Some(mut entry) = dir.last_entry.take() {
            if total_len + entry.as_ref().len() > op.size() as usize {
                return Err(io::Error::from_raw_os_error(libc::ERANGE));
            }
            entry.set_offset(dir.offset);
            total_len += entry.as_ref().len();
            dir.offset += 1;
            entries.push(entry);
        }

        while let Some(entry) = dir.read_dir.next_entry().await? {
            match entry.file_name() {
                name if name.as_bytes() == b"." || name.as_bytes() == b".." => continue,
                _ => (),
            }

            let metadata = entry.metadata().await?;
            let mut entry = DirEntry::new(entry.file_name(), metadata.ino(), 0);

            if total_len + entry.as_ref().len() <= op.size() as usize {
                entry.set_offset(dir.offset);
                total_len += entry.as_ref().len();
                dir.offset += 1;
                entries.push(entry);
            } else {
                dir.last_entry.replace(entry);
            }
        }

        Ok(entries)
    }

    async fn do_releasedir(&self, op: &op::Releasedir<'_>) -> io::Result<()> {
        let mut dirs = self.dirs.lock().await;

        let dir = dirs.remove(op.fh() as usize);
        drop(dir);

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

        let file = FileHandle {
            file: options.open(self.source.join(&inode.path)).await?,
        };

        let mut files = self.files.lock().await;
        let key = files.insert(Arc::new(Mutex::new(file)));

        Ok(ReplyOpen::new(key as u64))
    }

    async fn do_read(&self, op: &op::Read<'_>) -> io::Result<impl ScatteredBytes> {
        let files = self.files.lock().await;

        let file = files
            .get(op.fh() as usize)
            .cloned()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EIO))?;
        let mut file = file.lock().await;
        let file = &mut file.file;

        file.seek(io::SeekFrom::Start(op.offset())).await?;

        let mut buf = Vec::<u8>::with_capacity(op.size() as usize);

        use tokio::io::AsyncReadExt;
        tokio::io::copy(&mut file.take(op.size() as u64), &mut buf).await?;

        Ok(buf)
    }

    async fn do_flush(&self, op: &op::Flush<'_>) -> io::Result<()> {
        let files = self.files.lock().await;

        let file = files
            .get(op.fh() as usize)
            .cloned()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EIO))?;
        let file = file.lock().await;

        file.file.try_clone().await?;

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

#[polyfuse::async_trait]
impl Filesystem for PathThrough {
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
                    Ok(reply) => cx.reply_bytes(reply).await,
                    Err(err) => cx.reply_err(err.raw_os_error().unwrap_or(libc::EIO)).await,
                }
            };
        }

        match op {
            Operation::Lookup(op) => try_reply!(self.do_lookup(&op)),
            Operation::Forget(forgets) => {
                self.do_forget(forgets.as_ref()).await;
                Ok(())
            }
            Operation::Getattr(op) => try_reply!(self.do_getattr(&op)),
            Operation::Readlink(op) => try_reply!(self.do_readlink(&op)),
            Operation::Opendir(op) => try_reply!(self.do_opendir(&op)),
            Operation::Readdir(op) => try_reply!(self.do_readdir(&op)),
            Operation::Releasedir(op) => try_reply!(self.do_releasedir(&op)),
            Operation::Open(op) => try_reply!(self.do_open(&op)),
            Operation::Read(op) => try_reply!(self.do_read(&op)),
            Operation::Flush(op) => try_reply!(self.do_flush(&op)),
            Operation::Fsync(op) => try_reply!(self.do_fsync(&op)),
            Operation::Release(op) => try_reply!(self.do_release(&op)),
            _ => Ok(()),
        }
    }
}

#[inline]
fn no_entry() -> io::Error {
    io::Error::from_raw_os_error(libc::ENOENT)
}
