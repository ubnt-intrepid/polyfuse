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
    fs::{self, Daemon, Filesystem},
    op::{self, Forget, OpenFlags},
    reply::{self, AttrOut, EntryOut, OpenOut, OpenOutFlags, ReaddirOut, WriteOut},
    types::{FileID, FileType, NodeID},
};

use anyhow::{ensure, Context as _, Result};
use rustix::io::Errno;
use slab::Slab;
use std::{
    borrow::Cow,
    collections::hash_map::{Entry, HashMap},
    ffi::OsString,
    fs::{File, OpenOptions, ReadDir},
    io::{self, prelude::*, BufRead, BufReader},
    os::unix::prelude::*,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
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

    let daemon = Daemon::mount(mountpoint, Default::default(), Default::default())?;

    let fs = PathThrough::new(source)?;
    daemon.run(Arc::new(fs), None)?;

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
        let ino = NodeID::from_raw(self.next_ino).unwrap();
        VacantEntry { table: self, ino }
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
    fn lookup(self: &Arc<Self>, req: fs::Request<'_>, op: op::Lookup<'_>) -> fs::Result {
        let inodes = &mut *self.inodes.lock().unwrap();
        let parent = inodes.get(op.parent).ok_or(Errno::NOENT)?;
        let path = parent.path.join(op.name);

        let metadata = std::fs::symlink_metadata(self.source.join(&path))?;

        let attr = metadata.try_into().unwrap();

        let ino = match inodes.get_by_path_mut(&path) {
            Some(inode) => {
                inode.refcount += 1;
                inode.ino
            }
            None => {
                let entry = inodes.vacant_entry();
                let ino = entry.ino;
                let inode = INode {
                    ino,
                    path,
                    refcount: 1,
                };
                entry.insert(inode);
                ino
            }
        };

        req.reply(EntryOut {
            ino: Some(ino),
            attr: Cow::Owned(attr),
            generation: 0,
            attr_valid: None,
            entry_valid: None,
        })
    }

    fn forget(self: &Arc<Self>, forgets: &[Forget]) {
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

    fn getattr(self: &Arc<Self>, req: fs::Request<'_>, op: op::Getattr<'_>) -> fs::Result {
        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let metadata = std::fs::symlink_metadata(self.source.join(&inode.path))?;

        req.reply(AttrOut {
            attr: Cow::Owned(metadata.try_into().map_err(|_| Errno::INVAL)?),
            valid: None,
        })
    }

    fn setattr(self: &Arc<Self>, req: fs::Request<'_>, op: op::Setattr<'_>) -> fs::Result {
        let fh = op.fh.ok_or(Errno::NOENT)?;
        let files = &mut *self.files.lock().unwrap();
        let file = files.get(fh.into_raw() as usize).ok_or(Errno::INVAL)?;

        file.file.sync_all()?;

        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let path = self.source.join(&inode.path);

        // chmod
        if let Some(mode) = op.mode {
            file.file.set_permissions(mode.permissions().into())?;
        }

        // truncate
        if let Some(size) = op.size {
            file.file.set_len(size)?;
        }

        // chown
        match (op.uid, op.gid) {
            (None, None) => (),
            (uid, gid) => {
                rustix::fs::chown(&*path, uid, gid)?;
            }
        }

        // TODO: utimes

        let metadata = std::fs::symlink_metadata(self.source.join(&inode.path))?;

        req.reply(AttrOut {
            attr: Cow::Owned(metadata.try_into().map_err(|_| Errno::INVAL)?),
            valid: None,
        })
    }

    fn readlink(self: &Arc<Self>, req: fs::Request<'_>, op: op::Readlink<'_>) -> fs::Result {
        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;
        let path = std::fs::read_link(self.source.join(&inode.path))?;
        req.reply(reply::Raw(path.as_os_str()))
    }

    fn opendir(self: &Arc<Self>, req: fs::Request<'_>, op: op::Opendir<'_>) -> fs::Result {
        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;

        let dirs = &mut *self.dirs.lock().unwrap();
        let fh = dirs.insert(DirHandle {
            read_dir: std::fs::read_dir(self.source.join(&inode.path))?,
            last_entry: None,
            offset: 1,
        }) as u64;

        req.reply(OpenOut {
            fh: FileID::from_raw(fh),
            open_flags: OpenOutFlags::empty(),
        })
    }

    fn readdir(self: &Arc<Self>, req: fs::Request<'_>, op: op::Readdir<'_>) -> fs::Result {
        if op.mode == op::ReaddirMode::Plus {
            return Err(Errno::NOSYS.into());
        }

        let dirs = &mut *self.dirs.lock().unwrap();
        let dir = Slab::get_mut(dirs, op.fh.into_raw() as usize).ok_or(Errno::INVAL)?;

        let mut at_least_one_entry = false;

        let mut buf = ReaddirOut::new(op.size as usize);
        if let Some(entry) = dir.last_entry.take() {
            let full = buf.push_entry(entry.name.as_ref(), entry.ino, entry.typ, dir.offset);
            if full {
                dir.last_entry.replace(entry);
                return Err(Errno::RANGE.into());
            }
            at_least_one_entry = true;
            dir.offset += 1;
        }

        while let Some(entry) = dir.read_dir.next().transpose()? {
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

            let full = buf.push_entry(
                &entry.file_name(),
                NodeID::from_raw(metadata.ino()).unwrap(),
                typ,
                dir.offset,
            );
            if full {
                dir.last_entry.replace(DirEntry {
                    name: entry.file_name(),
                    ino: NodeID::from_raw(metadata.ino()).unwrap(),
                    typ,
                });
                if !at_least_one_entry {
                    return Err(Errno::RANGE.into());
                }
                break;
            }

            at_least_one_entry = true;
            dir.offset += 1;
        }

        req.reply(buf)
    }

    fn releasedir(self: &Arc<Self>, req: fs::Request<'_>, op: op::Releasedir<'_>) -> fs::Result {
        let dirs = &mut *self.dirs.lock().unwrap();
        let _dir = dirs.remove(op.fh.into_raw() as usize);
        req.reply(())
    }

    fn open(self: &Arc<Self>, req: fs::Request<'_>, op: op::Open<'_>) -> fs::Result {
        let inodes = &mut *self.inodes.lock().unwrap();
        let inode = inodes.get(op.ino).ok_or(Errno::NOENT)?;

        let options: OpenOptions = {
            let mut options = op.options;
            options.set_flags(options.flags() & !OpenFlags::NOFOLLOW);
            options.into()
        };

        let files = &mut *self.files.lock().unwrap();
        let fh = files.insert(FileHandle {
            file: options.open(self.source.join(&inode.path))?,
        }) as u64;

        req.reply(OpenOut {
            fh: FileID::from_raw(fh),
            open_flags: OpenOutFlags::DIRECT_IO,
        })
    }

    fn read(self: &Arc<Self>, req: fs::Request<'_>, op: op::Read<'_>) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let file = Slab::get_mut(files, op.fh.into_raw() as usize).ok_or(Errno::INVAL)?;
        let buf = file.read(op.offset, op.size as usize)?;
        req.reply(reply::Raw(buf))
    }

    fn write(
        self: &Arc<Self>,
        req: fs::Request<'_>,
        op: op::Write<'_>,
        data: impl io::Read + Unpin,
    ) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let file = Slab::get_mut(files, op.fh.into_raw() as usize).ok_or(Errno::INVAL)?;
        let offset = op.offset;
        let size = op.size;
        let written = file.write(BufReader::new(data).take(size as u64), offset)?;

        req.reply(WriteOut::new(written as u32))
    }

    fn flush(self: &Arc<Self>, req: fs::Request<'_>, op: op::Flush<'_>) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let file = Slab::get_mut(files, op.fh.into_raw() as usize).ok_or(Errno::INVAL)?;
        file.fsync(false)?;
        req.reply(())
    }

    fn fsync(self: &Arc<Self>, req: fs::Request<'_>, op: op::Fsync<'_>) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let file = Slab::get_mut(files, op.fh.into_raw() as usize).ok_or(Errno::INVAL)?;
        file.fsync(op.datasync)?;
        req.reply(())
    }

    fn release(self: &Arc<Self>, req: fs::Request<'_>, op: op::Release<'_>) -> fs::Result {
        let files = &mut *self.files.lock().unwrap();
        let _file = files.remove(op.fh.into_raw() as usize);
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
