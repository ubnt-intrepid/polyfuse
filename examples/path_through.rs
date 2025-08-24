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

use libc::{EINVAL, ENOENT, ENOSYS, ERANGE};
use polyfuse::{
    fs::{self, Filesystem},
    mount::MountOptions,
    op::{self, Forget},
    reply::{AttrOut, EntryOut, FileAttr, OpenOut, ReaddirOut, WriteOut},
    KernelConfig,
};

use anyhow::{ensure, Context as _, Result};
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap},
    ffi::OsString,
    fs::{File, Metadata, OpenOptions, ReadDir},
    io::{self, prelude::*, BufRead, BufReader},
    os::unix::prelude::*,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let source: PathBuf = args
        .opt_value_from_str(["-s", "--source"])?
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    ensure!(source.is_dir(), "the source path must be a directory");

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_dir(), "mountpoint must be a directory");

    let fs = PathThrough::new(source)?;

    polyfuse::fs::run(
        fs,
        mountpoint,
        MountOptions::default(),
        KernelConfig::default(),
    )?;

    Ok(())
}

type Ino = u64;

struct INode {
    ino: Ino,
    path: PathBuf,
    refcount: u64,
}

struct INodeTable {
    map: HashMap<Ino, INode>,
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

    fn get(&self, ino: Ino) -> Option<&INode> {
        self.map.get(&ino)
    }

    fn get_by_path_mut(&mut self, path: &Path) -> Option<&mut INode> {
        let ino = self.path_to_ino.get(path).copied()?;
        self.map.get_mut(&ino)
    }
}

struct VacantEntry<'a> {
    table: &'a mut INodeTable,
    ino: Ino,
}

impl VacantEntry<'_> {
    fn insert(self, inode: INode) {
        let path = inode.path.clone();
        self.table.map.insert(self.ino, inode);
        self.table.path_to_ino.insert(path, self.ino);
        self.table.next_ino += 1;
    }
}

struct PathThrough {
    source: PathBuf,
    inodes: Mutex<INodeTable>,
    dirs: Mutex<Slab<DirHandle>>,
    files: Mutex<Slab<FileHandle>>,
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
            dirs: Mutex::new(Slab::new()),
            files: Mutex::new(Slab::new()),
        })
    }
}

impl Filesystem for PathThrough {
    fn lookup(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Lookup<'_>>) -> fs::Result {
        let inodes = &mut *self.inodes.lock().unwrap();
        let parent = inodes.get(req.arg().parent()).ok_or(ENOENT)?;
        let path = parent.path.join(req.arg().name());

        let metadata = std::fs::symlink_metadata(self.source.join(&path))?;

        let mut out = EntryOut::default();
        fill_attr(&metadata, out.attr());

        match inodes.get_by_path_mut(&path) {
            Some(inode) => {
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

        req.reply(out)
    }

    fn forget(&self, _: fs::Context<'_, '_>, forgets: &[Forget]) {
        let inodes = &mut *self.inodes.lock().unwrap();
        for forget in forgets {
            if let Entry::Occupied(mut entry) = inodes.map.entry(forget.ino()) {
                let refcount = {
                    let inode = entry.get_mut();
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
        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let metadata = std::fs::symlink_metadata(self.source.join(&inode.path))?;

        let mut out = AttrOut::default();
        fill_attr(&metadata, out.attr());

        req.reply(out)
    }

    fn setattr(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Setattr<'_>>) -> fs::Result {
        let fh = req.arg().fh().ok_or(ENOENT)?;
        let files = &mut *self.files.lock().unwrap();
        let file = files.get(fh as usize).ok_or(EINVAL)?;

        file.file.sync_all()?;

        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let path = self.source.join(&inode.path);

        // chmod
        if let Some(mode) = req.arg().mode() {
            let perm = std::fs::Permissions::from_mode(mode);
            file.file.set_permissions(perm)?;
        }

        // truncate
        if let Some(size) = req.arg().size() {
            file.file.set_len(size)?;
        }

        // chown
        match (req.arg().uid(), req.arg().gid()) {
            (None, None) => (),
            (uid, gid) => {
                let uid = uid.map(nix::unistd::Uid::from_raw);
                let gid = gid.map(nix::unistd::Gid::from_raw);
                nix::unistd::chown(&*path, uid, gid).map_err(|err| err as i32)?;
            }
        }

        // TODO: utimes

        let metadata = std::fs::symlink_metadata(self.source.join(&inode.path))?;

        let mut out = AttrOut::default();
        fill_attr(&metadata, out.attr());

        req.reply(out)
    }

    fn readlink(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Readlink<'_>>,
    ) -> fs::Result {
        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;
        let path = std::fs::read_link(self.source.join(&inode.path))?;
        req.reply(path.as_os_str())
    }

    fn opendir(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Opendir<'_>>) -> fs::Result {
        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;

        let dirs = &mut *self.dirs.lock().unwrap();
        let fh = dirs.insert(DirHandle {
            read_dir: std::fs::read_dir(self.source.join(&inode.path))?,
            last_entry: None,
            offset: 1,
        }) as u64;

        let mut out = OpenOut::default();
        out.fh(fh);

        req.reply(out)
    }

    fn readdir(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Readdir<'_>>) -> fs::Result {
        if req.arg().mode() == op::ReaddirMode::Plus {
            return Err(ENOSYS.into());
        }

        let dirs = &mut *self.dirs.lock().unwrap();
        let dir = Slab::get_mut(dirs, req.arg().fh() as usize).ok_or(EINVAL)?;

        let mut out = ReaddirOut::new(req.arg().size() as usize);
        let mut at_least_one_entry = false;

        if let Some(entry) = dir.last_entry.take() {
            let full = out.entry(entry.name.as_ref(), entry.ino, entry.typ, dir.offset);
            if full {
                dir.last_entry.replace(entry);
                return Err(ERANGE.into());
            }
            at_least_one_entry = true;
            dir.offset += 1;
        }

        #[allow(clippy::while_let_on_iterator)]
        while let Some(entry) = dir.read_dir.next() {
            let entry = entry?;
            match entry.file_name() {
                name if name.as_bytes() == b"." || name.as_bytes() == b".." => continue,
                _ => (),
            }

            let metadata = entry.metadata()?;
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
                    return Err(ERANGE.into());
                }
                break;
            }

            at_least_one_entry = true;
            dir.offset += 1;
        }

        req.reply(out)
    }

    fn releasedir(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::Releasedir<'_>>,
    ) -> fs::Result {
        let dirs = &mut *self.dirs.lock().unwrap();
        let _dir = dirs.remove(req.arg().fh() as usize);
        req.reply(())
    }

    fn open(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Open<'_>>) -> fs::Result {
        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(req.arg().ino()).ok_or(ENOENT)?;

        let mut options = OpenOptions::new();
        match req.arg().flags() as i32 & libc::O_ACCMODE {
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
        options.custom_flags(req.arg().flags() as i32 & !libc::O_NOFOLLOW);

        let files = &mut *self.files.lock().unwrap();
        let fh = files.insert(FileHandle {
            file: options.open(self.source.join(&inode.path))?,
        }) as u64;

        let mut out = OpenOut::default();
        out.fh(fh);

        req.reply(out)
    }

    fn read(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Read<'_>>) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let file = Slab::get_mut(files, req.arg().fh() as usize).ok_or(EINVAL)?;
        let buf = file.read(req.arg().offset(), req.arg().size() as usize)?;
        req.reply(buf)
    }

    fn write(&self, _: fs::Context<'_, '_>, mut req: fs::Request<'_, op::Write<'_>>) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let file = Slab::get_mut(files, req.arg().fh() as usize).ok_or(EINVAL)?;
        let offset = req.arg().offset();
        let size = req.arg().size();
        let written = file.write(BufReader::new(&mut req).take(size as u64), offset)?;

        let mut out = WriteOut::default();
        out.size(written as u32);
        req.reply(out)
    }

    fn flush(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Flush<'_>>) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let file = Slab::get_mut(files, req.arg().fh() as usize).ok_or(EINVAL)?;
        file.fsync(false)?;
        req.reply(())
    }

    fn fsync(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Fsync<'_>>) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let file = Slab::get_mut(files, req.arg().fh() as usize).ok_or(EINVAL)?;
        file.fsync(req.arg().datasync())?;
        req.reply(())
    }

    fn release(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Release<'_>>) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let _file = files.remove(req.arg().fh() as usize);
        req.reply(())
    }
}

// ==== Dir ====

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

// ==== file ====

struct FileHandle {
    file: File,
}

impl FileHandle {
    fn read(&mut self, offset: u64, size: usize) -> io::Result<Vec<u8>> {
        self.file.seek(io::SeekFrom::Start(offset))?;

        let mut buf = Vec::<u8>::with_capacity(size);
        (&mut self.file).take(size as u64).read_to_end(&mut buf)?;

        Ok(buf)
    }

    fn write<T>(&mut self, mut data: T, offset: u64) -> io::Result<usize>
    where
        T: BufRead + Unpin,
    {
        self.file.seek(io::SeekFrom::Start(offset))?;

        let mut written = 0;
        loop {
            let chunk = data.fill_buf()?;
            if chunk.is_empty() {
                break;
            }
            let n = self.file.write(chunk)?;
            written += n;
        }

        Ok(written)
    }

    fn fsync(&mut self, datasync: bool) -> io::Result<()> {
        if datasync {
            self.file.sync_data()?;
        } else {
            self.file.sync_all()?;
        }
        Ok(())
    }
}

fn fill_attr(metadata: &Metadata, attr: &mut FileAttr) {
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
}
