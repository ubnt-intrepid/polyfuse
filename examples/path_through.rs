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
    mount::{mount, MountOptions},
    op::{self, Forget},
    reply::{AttrOut, EntryOut, FileAttr, OpenOut, ReaddirOut, WriteOut},
    Connection, KernelConfig, Operation, Request, Session,
};

use anyhow::{ensure, Context as _, Result};
use slab::Slab;
use std::{
    collections::hash_map::{Entry, HashMap},
    ffi::OsString,
    fs::{self, File, Metadata, OpenOptions, ReadDir},
    io::{self, prelude::*, BufRead, BufReader},
    os::unix::prelude::*,
    path::{Path, PathBuf},
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

    let (conn, fusermount) = mount(mountpoint, MountOptions::default())?;
    let mut conn = Connection::from(conn);
    let session = Session::init(&mut conn, KernelConfig::default())?;

    let mut fs = PathThrough::new(source)?;

    let mut req = session.new_request_buffer()?;
    while session.read_request(&mut conn, &mut req)? {
        let op = req.operation()?;
        tracing::debug!("handle operation: {:#?}", op);

        macro_rules! try_reply {
            ($e:expr) => {
                match $e {
                    Ok(data) => session.reply(&mut conn, &req, data)?,
                    Err(err) => session.reply_error(
                        &mut conn,
                        &req,
                        err.raw_os_error().unwrap_or(libc::EIO),
                    )?,
                }
            };
        }

        match op {
            Operation::Lookup(op) => try_reply!(fs.do_lookup(&op)),
            Operation::Forget(forgets) => {
                fs.do_forget(forgets.as_ref());
            }
            Operation::Getattr(op) => try_reply!(fs.do_getattr(&op)),
            Operation::Setattr(op) => try_reply!(fs.do_setattr(&op)),
            Operation::Readlink(op) => try_reply!(fs.do_readlink(&op)),
            Operation::Opendir(op) => try_reply!(fs.do_opendir(&op)),
            Operation::Readdir(op) => try_reply!(fs.do_readdir(&op)),
            Operation::Releasedir(op) => try_reply!(fs.do_releasedir(&op)),
            Operation::Open(op) => try_reply!(fs.do_open(&op)),
            Operation::Read(op) => try_reply!(fs.do_read(&op)),
            Operation::Write(op) => try_reply!(fs.do_write(&op, &req)),
            Operation::Flush(op) => try_reply!(fs.do_flush(&op)),
            Operation::Fsync(op) => try_reply!(fs.do_fsync(&op)),
            Operation::Release(op) => try_reply!(fs.do_release(&op)),

            _ => session.reply_error(&mut conn, &req, libc::ENOSYS)?,
        }
    }

    fusermount.unmount()?;

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
    inodes: INodeTable,
    dirs: Slab<DirHandle>,
    files: Slab<FileHandle>,
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
            inodes,
            dirs: Slab::new(),
            files: Slab::new(),
        })
    }

    fn do_lookup(&mut self, op: &op::Lookup<'_>) -> io::Result<EntryOut> {
        let parent = self.inodes.get(op.parent()).ok_or_else(no_entry)?;
        let path = parent.path.join(op.name());

        let metadata = fs::symlink_metadata(self.source.join(&path))?;

        let mut out = EntryOut::default();
        fill_attr(&metadata, out.attr());

        match self.inodes.get_by_path_mut(&path) {
            Some(inode) => {
                out.ino(inode.ino);
                inode.refcount += 1;
            }
            None => {
                let entry = self.inodes.vacant_entry();
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

    fn do_forget(&mut self, forgets: &[Forget]) {
        for forget in forgets {
            if let Entry::Occupied(mut entry) = self.inodes.map.entry(forget.ino()) {
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

    fn do_getattr(&mut self, op: &op::Getattr<'_>) -> io::Result<AttrOut> {
        let inode = self.inodes.get(op.ino()).ok_or_else(no_entry)?;
        let metadata = fs::symlink_metadata(self.source.join(&inode.path))?;

        let mut out = AttrOut::default();
        fill_attr(&metadata, out.attr());

        Ok(out)
    }

    fn do_setattr(&mut self, op: &op::Setattr<'_>) -> io::Result<AttrOut> {
        let fh = op.fh().ok_or_else(no_entry)?;
        let file = self
            .files
            .get(fh as usize)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;

        file.file.sync_all()?;

        let inode = self.inodes.get(op.ino()).ok_or_else(no_entry)?;
        let path = self.source.join(&inode.path);

        // chmod
        if let Some(mode) = op.mode() {
            let perm = std::fs::Permissions::from_mode(mode);
            file.file.set_permissions(perm)?;
        }

        // truncate
        if let Some(size) = op.size() {
            file.file.set_len(size)?;
        }

        // chown
        match (op.uid(), op.gid()) {
            (None, None) => (),
            (uid, gid) => {
                let uid = uid.map(nix::unistd::Uid::from_raw);
                let gid = gid.map(nix::unistd::Gid::from_raw);
                nix::unistd::chown(&*path, uid, gid)?;
            }
        }

        // TODO: utimes

        let metadata = fs::symlink_metadata(self.source.join(&inode.path))?;

        let mut out = AttrOut::default();
        fill_attr(&metadata, out.attr());

        Ok(out)
    }

    fn do_readlink(&mut self, op: &op::Readlink<'_>) -> io::Result<OsString> {
        let inode = self.inodes.get(op.ino()).ok_or_else(no_entry)?;
        let path = fs::read_link(self.source.join(&inode.path))?;
        Ok(path.into_os_string())
    }

    fn do_opendir(&mut self, op: &op::Opendir<'_>) -> io::Result<OpenOut> {
        let inode = self.inodes.get(op.ino()).ok_or_else(no_entry)?;

        let fh = self.dirs.insert(DirHandle {
            read_dir: fs::read_dir(self.source.join(&inode.path))?,
            last_entry: None,
            offset: 1,
        }) as u64;

        let mut out = OpenOut::default();
        out.fh(fh);

        Ok(out)
    }

    fn do_readdir(&mut self, op: &op::Readdir<'_>) -> io::Result<ReaddirOut> {
        if op.mode() == op::ReaddirMode::Plus {
            return Err(io::Error::from_raw_os_error(libc::ENOSYS));
        }

        let dir = Slab::get_mut(&mut self.dirs, op.fh() as usize).ok_or_else(invalid_handle)?;

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
                    return Err(io::Error::from_raw_os_error(libc::ERANGE));
                }
                break;
            }

            at_least_one_entry = true;
            dir.offset += 1;
        }

        Ok(out)
    }

    fn do_releasedir(&mut self, op: &op::Releasedir<'_>) -> io::Result<()> {
        let _dir = self.dirs.remove(op.fh() as usize);
        Ok(())
    }

    fn do_open(&mut self, op: &op::Open<'_>) -> io::Result<OpenOut> {
        let inode = self.inodes.get(op.ino()).ok_or_else(no_entry)?;

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

        let fh = self.files.insert(FileHandle {
            file: options.open(self.source.join(&inode.path))?,
        }) as u64;

        let mut out = OpenOut::default();
        out.fh(fh);

        Ok(out)
    }

    fn do_read(&mut self, op: &op::Read<'_>) -> io::Result<Vec<u8>> {
        let file = Slab::get_mut(&mut self.files, op.fh() as usize).ok_or_else(invalid_handle)?;
        let buf = file.read(op.offset(), op.size() as usize)?;
        Ok(buf)
    }

    fn do_write(&mut self, op: &op::Write<'_>, req: &Request) -> io::Result<WriteOut> {
        let file = Slab::get_mut(&mut self.files, op.fh() as usize).ok_or_else(invalid_handle)?;
        let written = file.write(BufReader::new(req).take(op.size() as u64), op.offset())?;

        let mut out = WriteOut::default();
        out.size(written as u32);
        Ok(out)
    }

    fn do_flush(&mut self, op: &op::Flush<'_>) -> io::Result<()> {
        let file = Slab::get_mut(&mut self.files, op.fh() as usize).ok_or_else(invalid_handle)?;
        file.fsync(false)?;
        Ok(())
    }

    fn do_fsync(&mut self, op: &op::Fsync<'_>) -> io::Result<()> {
        let file = Slab::get_mut(&mut self.files, op.fh() as usize).ok_or_else(invalid_handle)?;
        file.fsync(op.datasync())?;
        Ok(())
    }

    fn do_release(&mut self, op: &op::Release<'_>) -> io::Result<()> {
        let _file = self.files.remove(op.fh() as usize);
        Ok(())
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

// ==== utils ====

#[inline]
fn no_entry() -> io::Error {
    io::Error::from_raw_os_error(libc::ENOENT)
}

#[inline]
fn invalid_handle() -> io::Error {
    io::Error::from_raw_os_error(libc::EINVAL)
}
