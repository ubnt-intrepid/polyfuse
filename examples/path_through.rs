#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented, clippy::todo)]
#![forbid(unsafe_code)]

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
    fs::{self, Filesystem},
    mount::MountOptions,
    op::{self, Forget, OpenFlags},
    types::{FileID, FileType, NodeID},
    KernelConfig,
};

use anyhow::{ensure, Context as _, Result};
use libc::{EINVAL, ENOENT, ENOSYS, ERANGE};
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap},
    ffi::OsString,
    fs::{File, OpenOptions, ReadDir},
    io::{self, prelude::*, BufRead, BufReader},
    os::unix::prelude::*,
    path::{Path, PathBuf},
    sync::Mutex,
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

struct INode {
    ino: NodeID,
    path: PathBuf,
    refcount: u64,
}

struct INodeTable {
    map: HashMap<NodeID, INode>,
    path_to_ino: HashMap<PathBuf, NodeID>,
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
        VacantEntry {
            table: self,
            ino: NodeID::from_raw(ino),
        }
    }

    fn get(&self, ino: NodeID) -> Option<&INode> {
        self.map.get(&ino)
    }

    fn get_by_path_mut(&mut self, path: &Path) -> Option<&mut INode> {
        let ino = self.path_to_ino.get(path).copied()?;
        self.map.get_mut(&ino)
    }
}

struct VacantEntry<'a> {
    table: &'a mut INodeTable,
    ino: NodeID,
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
            ino: NodeID::ROOT,
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
    fn lookup(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Lookup<'_>,
        mut reply: fs::ReplyEntry<'_>,
    ) -> fs::Result {
        let inodes = &mut *self.inodes.lock().unwrap();
        let parent = inodes.get(op.parent()).ok_or(ENOENT)?;
        let path = parent.path.join(op.name());

        let metadata = std::fs::symlink_metadata(self.source.join(&path))?;

        reply.out().attr(metadata.try_into().expect("unreachable"));

        match inodes.get_by_path_mut(&path) {
            Some(inode) => {
                reply.out().ino(inode.ino);
                inode.refcount += 1;
            }
            None => {
                let entry = inodes.vacant_entry();
                reply.out().ino(entry.ino);
                let inode = INode {
                    ino: entry.ino,
                    path,
                    refcount: 1,
                };
                entry.insert(inode);
            }
        }

        reply.send()
    }

    fn forget(&self, _: fs::Env<'_, '_>, forgets: &[Forget]) {
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

    fn getattr(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Getattr<'_>,
        mut reply: fs::ReplyAttr<'_>,
    ) -> fs::Result {
        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;
        let metadata = std::fs::symlink_metadata(self.source.join(&inode.path))?;

        reply.out().attr(metadata.try_into().expect("unreachable"));
        reply.send()
    }

    fn setattr(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Setattr<'_>,
        mut reply: fs::ReplyAttr<'_>,
    ) -> fs::Result {
        let fh = op.fh().ok_or(ENOENT)?;
        let files = &mut *self.files.lock().unwrap();
        let file = files.get(fh.into_raw() as usize).ok_or(EINVAL)?;

        file.file.sync_all()?;

        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;
        let path = self.source.join(&inode.path);

        // chmod
        if let Some(mode) = op.mode() {
            file.file.set_permissions(mode.permissions().into())?;
        }

        // truncate
        if let Some(size) = op.size() {
            file.file.set_len(size)?;
        }

        // chown
        match (op.uid(), op.gid()) {
            (None, None) => (),
            (uid, gid) => {
                let uid = uid.map(|id| nix::unistd::Uid::from_raw(id.into_raw()));
                let gid = gid.map(|id| nix::unistd::Gid::from_raw(id.into_raw()));
                nix::unistd::chown(&*path, uid, gid).map_err(|err| err as i32)?;
            }
        }

        // TODO: utimes

        let metadata = std::fs::symlink_metadata(self.source.join(&inode.path))?;

        reply.out().attr(metadata.try_into().expect("unreachable"));
        reply.send()
    }

    fn readlink(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Readlink<'_>,
        reply: fs::ReplyData<'_>,
    ) -> fs::Result {
        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;
        let path = std::fs::read_link(self.source.join(&inode.path))?;
        reply.send(path.as_os_str())
    }

    fn opendir(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Opendir<'_>,
        mut reply: fs::ReplyOpen<'_>,
    ) -> fs::Result {
        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;

        let dirs = &mut *self.dirs.lock().unwrap();
        let fh = dirs.insert(DirHandle {
            read_dir: std::fs::read_dir(self.source.join(&inode.path))?,
            last_entry: None,
            offset: 1,
        }) as u64;

        reply.out().fh(FileID::from_raw(fh));
        reply.send()
    }

    fn readdir(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Readdir<'_>,
        mut reply: fs::ReplyDir<'_>,
    ) -> fs::Result {
        if op.mode() == op::ReaddirMode::Plus {
            return Err(ENOSYS.into());
        }

        let dirs = &mut *self.dirs.lock().unwrap();
        let dir = Slab::get_mut(dirs, op.fh().into_raw() as usize).ok_or(EINVAL)?;

        let mut at_least_one_entry = false;

        if let Some(entry) = dir.last_entry.take() {
            let full = reply.push_entry(entry.name.as_ref(), entry.ino, entry.typ, dir.offset);
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
                Some(FileType::Regular)
            } else if file_type.is_dir() {
                Some(FileType::Directory)
            } else if file_type.is_symlink() {
                Some(FileType::SymbolicLink)
            } else {
                None
            };

            let full = reply.push_entry(
                &entry.file_name(),
                NodeID::from_raw(metadata.ino()),
                typ,
                dir.offset,
            );
            if full {
                dir.last_entry.replace(DirEntry {
                    name: entry.file_name(),
                    ino: NodeID::from_raw(metadata.ino()),
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

        reply.send()
    }

    fn releasedir(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Releasedir<'_>,
        reply: fs::ReplyUnit,
    ) -> fs::Result {
        let dirs = &mut *self.dirs.lock().unwrap();
        let _dir = dirs.remove(op.fh().into_raw() as usize);
        reply.send()
    }

    fn open(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Open<'_>,
        mut reply: fs::ReplyOpen<'_>,
    ) -> fs::Result {
        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino()).ok_or(ENOENT)?;

        let options: OpenOptions = op.options().remove(OpenFlags::NOFOLLOW).into();

        let files = &mut *self.files.lock().unwrap();
        let fh = files.insert(FileHandle {
            file: options.open(self.source.join(&inode.path))?,
        }) as u64;

        reply.out().fh(FileID::from_raw(fh));
        reply.send()
    }

    fn read(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Read<'_>,
        reply: fs::ReplyData<'_>,
    ) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let file = Slab::get_mut(files, op.fh().into_raw() as usize).ok_or(EINVAL)?;
        let buf = file.read(op.offset(), op.size() as usize)?;
        reply.send(buf)
    }

    fn write(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Write<'_>,
        data: fs::Data<'_>,
        reply: fs::ReplyWrite<'_>,
    ) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let file = Slab::get_mut(files, op.fh().into_raw() as usize).ok_or(EINVAL)?;
        let offset = op.offset();
        let size = op.size();
        let written = file.write(BufReader::new(data).take(size as u64), offset)?;

        reply.send(written as u32)
    }

    fn flush(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Flush<'_>,
        reply: fs::ReplyUnit<'_>,
    ) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let file = Slab::get_mut(files, op.fh().into_raw() as usize).ok_or(EINVAL)?;
        file.fsync(false)?;
        reply.send()
    }

    fn fsync(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Fsync<'_>,
        reply: fs::ReplyUnit<'_>,
    ) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let file = Slab::get_mut(files, op.fh().into_raw() as usize).ok_or(EINVAL)?;
        file.fsync(op.datasync())?;
        reply.send()
    }

    fn release(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Release<'_>,
        reply: fs::ReplyUnit<'_>,
    ) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let _file = files.remove(op.fh().into_raw() as usize);
        reply.send()
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
    ino: NodeID,
    typ: Option<FileType>,
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
