use crate::{
    bytes::{Bytes, ToBytes},
    conn::Connection,
    mount::{Mount, MountOptions},
    op::{self, Forget, Operation},
    request::{FallbackBuf, RequestBuf, RequestHeader, SpliceBuf},
    session::{KernelConfig, KernelFlags, Session},
    types::{NodeID, NotifyID, PollWakeupID},
};
use polyfuse_kernel::*;
use rustix::{
    fs::{Gid, Uid},
    io::Errno,
    process::Pid,
};
use std::{
    borrow::Cow,
    ffi::OsStr,
    io,
    num::NonZeroUsize,
    panic,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Weak,
    },
    thread,
};
use zerocopy::IntoBytes as _;

/// The kind of errors during the handling of each operation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("operation failed with error code {}", _0)]
    Code(Errno),

    #[error("fatal error: {}", _0)]
    Fatal(#[source] io::Error),
}

impl From<Errno> for Error {
    fn from(errno: Errno) -> Self {
        Self::Code(errno)
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Code(Errno::from_io_error(&err).unwrap_or(Errno::IO))
    }
}

pub type Result<T = Replied, E = Error> = std::result::Result<T, E>;

macro_rules! define_ops {
    ($( $name:ident: $Arg:ident ),*$(,)*) => {$(
        #[allow(unused_variables)]
        fn $name(
            self: &Arc<Self>,
            req: Request<'_>,
            op: op::$Arg<'_>,
        ) -> Result {
            Err(Error::Code(Errno::NOSYS))
        }
    )*};
}

pub trait Filesystem {
    define_ops! {
        lookup: Lookup,
        getattr: Getattr,
        setattr: Setattr,
        readlink: Readlink,
        symlink: Symlink,
        mknod: Mknod,
        mkdir: Mkdir,
        unlink: Unlink,
        rmdir: Rmdir,
        rename: Rename,
        link: Link,
        open: Open,
        read: Read,
        release: Release,
        statfs: Statfs,
        fsync: Fsync,
        setxattr: Setxattr,
        getxattr: Getxattr,
        listxattr: Listxattr,
        removexattr: Removexattr,
        flush: Flush,
        opendir: Opendir,
        readdir: Readdir,
        releasedir: Releasedir,
        fsyncdir: Fsyncdir,
        getlk: Getlk,
        setlk: Setlk,
        flock: Flock,
        access: Access,
        create: Create,
        bmap: Bmap,
        fallocate: Fallocate,
        copy_file_range: CopyFileRange,
        poll: Poll,
        lseek: Lseek,
    }

    #[allow(unused_variables)]
    fn write(
        self: &Arc<Self>,
        req: Request<'_>,
        op: op::Write<'_>,
        data: impl io::Read + Unpin,
    ) -> Result {
        Err(Error::Code(Errno::NOSYS))
    }

    #[allow(unused_variables)]
    fn forget(self: &Arc<Self>, forgets: &[Forget]) {}

    #[allow(unused_variables)]
    fn notify_reply(
        self: &Arc<Self>,
        op: op::NotifyReply<'_>,
        data: impl io::Read + Unpin,
    ) -> io::Result<()> {
        Ok(())
    }
}

struct Global {
    session: Session,
    conn: Connection,
    notify_unique: AtomicU64,
}

impl Global {
    fn notifier(self: &Arc<Self>) -> Notifier {
        Notifier {
            global: Arc::downgrade(self),
        }
    }

    fn send_notify<B>(&self, code: fuse_notify_code, arg: B) -> io::Result<()>
    where
        B: Bytes,
    {
        self.session.send_notify(&self.conn, code, arg)
    }
}

pub struct Notifier {
    global: Weak<Global>,
}

impl Notifier {
    fn global(&self) -> io::Result<Arc<Global>> {
        self.global.upgrade().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "The daemon has already been destroyed",
            )
        })
    }

    /// Notify the cache invalidation about an inode to the kernel.
    pub fn inval_inode(&self, ino: NodeID, off: i64, len: i64) -> io::Result<()> {
        self.global()?.send_notify(
            fuse_notify_code::FUSE_NOTIFY_INVAL_INODE,
            fuse_notify_inval_inode_out {
                ino: ino.into_raw(),
                off,
                len,
            }
            .as_bytes(),
        )
    }

    /// Notify the invalidation about a directory entry to the kernel.
    pub fn inval_entry(&self, parent: NodeID, name: &OsStr) -> io::Result<()> {
        let namelen = name.len().try_into().expect("provided name is too long");
        self.global()?.send_notify(
            fuse_notify_code::FUSE_NOTIFY_INVAL_ENTRY,
            (
                fuse_notify_inval_entry_out {
                    parent: parent.into_raw(),
                    namelen,
                    padding: 0,
                }
                .as_bytes(),
                name,
            ),
        )
    }

    /// Notify the invalidation about a directory entry to the kernel.
    ///
    /// The role of this notification is similar to `InvalEntry`.
    /// Additionally, when the provided `child` inode matches the inode
    /// in the dentry cache, the inotify will inform the deletion to
    /// watchers if exists.
    pub fn delete(&self, parent: NodeID, child: NodeID, name: &OsStr) -> io::Result<()> {
        let namelen = name.len().try_into().expect("provided name is too long");
        self.global()?.send_notify(
            fuse_notify_code::FUSE_NOTIFY_DELETE,
            (
                fuse_notify_delete_out {
                    parent: parent.into_raw(),
                    child: child.into_raw(),
                    namelen,
                    padding: 0,
                }
                .as_bytes(),
                name,
            ),
        )
    }

    /// Push the data in an inode for updating the kernel cache.
    pub fn store<B>(&self, ino: NodeID, offset: u64, data: B) -> io::Result<()>
    where
        B: Bytes,
    {
        let size = data.size().try_into().expect("provided data is too large");
        self.global()?.send_notify(
            fuse_notify_code::FUSE_NOTIFY_STORE,
            (
                fuse_notify_store_out {
                    nodeid: ino.into_raw(),
                    offset,
                    size,
                    padding: 0,
                }
                .as_bytes(),
                data,
            ),
        )
    }

    /// Retrieve data in an inode from the kernel cache.
    pub fn retrieve(&self, ino: NodeID, offset: u64, size: u32) -> io::Result<NotifyID> {
        let global = self.global()?;
        let unique = global.notify_unique.fetch_add(1, Ordering::AcqRel);
        global.session.send_notify(
            &global.conn,
            fuse_notify_code::FUSE_NOTIFY_RETRIEVE,
            fuse_notify_retrieve_out {
                notify_unique: unique,
                nodeid: ino.into_raw(),
                offset,
                size,
                padding: 0,
            }
            .as_bytes(),
        )?;
        Ok(NotifyID::from_raw(unique))
    }

    /// Send I/O readiness to the kernel.
    pub fn poll_wakeup(&self, kh: PollWakeupID) -> io::Result<()> {
        self.global()?.send_notify(
            fuse_notify_code::FUSE_NOTIFY_POLL,
            fuse_notify_poll_wakeup_out { kh: kh.into_raw() }.as_bytes(),
        )
    }
}

pub struct Spawner<'a> {
    join_set: &'a mut Vec<thread::JoinHandle<io::Result<()>>>,
}

impl Spawner<'_> {
    pub fn spawn<F>(&mut self, f: F)
    where
        F: FnOnce() -> io::Result<()> + Send + 'static,
    {
        self.join_set.push(thread::spawn(f));
    }
}

/// The context for a single FUSE request used by the filesystem.
pub struct Request<'req> {
    global: &'req Arc<Global>,
    conn: &'req Connection,
    header: &'req RequestHeader,
    join_set: &'req mut Vec<thread::JoinHandle<io::Result<()>>>,
}

impl Request<'_> {
    pub fn uid(&self) -> Uid {
        self.header.uid()
    }

    pub fn gid(&self) -> Gid {
        self.header.gid()
    }

    pub fn pid(&self) -> Option<Pid> {
        self.header.pid()
    }

    pub fn notifier(&self) -> Notifier {
        self.global.notifier()
    }

    pub fn spawner(&mut self) -> Spawner<'_> {
        Spawner {
            join_set: self.join_set,
        }
    }

    pub fn reply<B>(self, arg: B) -> Result
    where
        B: ToBytes,
    {
        self.global
            .session
            .send_reply(self.conn, self.header.unique(), None, arg)
            .map_err(Error::Fatal)?;
        Ok(Replied { _priv: () })
    }
}

pub struct Replied {
    _priv: (),
}

pub struct Daemon {
    global: Arc<Global>,
    fusermount: Mount,
    join_set: Vec<thread::JoinHandle<io::Result<()>>>,
}

impl Daemon {
    pub fn mount(
        mountpoint: impl Into<Cow<'static, Path>>,
        mountopts: MountOptions,
        config: KernelConfig,
    ) -> io::Result<Self> {
        let (conn, fusermount) = crate::mount::mount(&mountpoint.into(), &mountopts)?;

        let session = Session::init(
            &conn,
            FallbackBuf::new(FUSE_MIN_READ_BUFFER as usize),
            config,
        )?;

        Ok(Self {
            global: Arc::new(Global {
                session,
                conn,
                notify_unique: AtomicU64::new(0),
            }),
            fusermount,
            join_set: vec![],
        })
    }

    pub fn config(&self) -> &KernelConfig {
        self.global.session.config()
    }

    pub fn notifier(&self) -> Notifier {
        self.global.notifier()
    }

    pub fn spawner(&mut self) -> Spawner<'_> {
        Spawner {
            join_set: &mut self.join_set,
        }
    }

    fn new_worker(&self, _i: usize) -> io::Result<Worker> {
        let conn = self.global.conn.try_ioc_clone()?;
        Ok(Worker {
            global: self.global.clone(),
            conn,
            join_set: vec![],
        })
    }

    pub fn run<T>(mut self, fs: Arc<T>, num_workers: Option<NonZeroUsize>) -> io::Result<()>
    where
        T: Filesystem + Send + Sync + 'static,
    {
        let num_workers = num_workers.map(|n| n.get()).unwrap_or_else(num_cpus::get);

        for i in 0..num_workers {
            let worker = self.new_worker(i)?;
            let fs = fs.clone();
            let bufsize = self.global.session.request_buffer_size();
            if self.config().flags.contains(KernelFlags::SPLICE_READ) {
                let buf = SpliceBuf::new(bufsize)?;
                self.join_set
                    .push(thread::spawn(move || worker.run(buf, fs)));
            } else {
                let buf = FallbackBuf::new(bufsize);
                self.join_set
                    .push(thread::spawn(move || worker.run(buf, fs)));
            }
        }

        for handle in self.join_set.drain(..) {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    tracing::error!("A worker thread is exited with error: {}", err);
                }
                Err(err) => panic::resume_unwind(err),
            }
        }

        self.fusermount.unmount()?;

        Ok(())
    }
}

struct Worker {
    global: Arc<Global>,
    conn: Connection,
    join_set: Vec<thread::JoinHandle<io::Result<()>>>,
}

impl Worker {
    fn run<T, B>(mut self, mut buf: B, fs: Arc<T>) -> io::Result<()>
    where
        T: Filesystem,
        for<'a> B: RequestBuf<&'a Connection>,
        for<'a> B::RemainingData<'a>: io::Read + Unpin,
    {
        while self.read_request(&mut buf)? {
            self.handle_request(&mut buf, &fs)?;
        }

        for handle in self.join_set.drain(..) {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    tracing::error!("A worker thread is exited with error: {}", err);
                }
                Err(err) => panic::resume_unwind(err),
            }
        }

        Ok(())
    }

    fn read_request<B>(&self, buf: &mut B) -> io::Result<bool>
    where
        for<'a> B: RequestBuf<&'a Connection>,
    {
        self.global.session.recv_request(&self.conn, buf)
    }

    fn handle_request<T, B>(&mut self, buf: &mut B, fs: &Arc<T>) -> io::Result<()>
    where
        T: Filesystem,
        for<'a> B: RequestBuf<&'a Connection>,
        for<'a> B::RemainingData<'a>: io::Read + Unpin,
    {
        let (header, arg, data) = buf.to_request_parts();

        let span = tracing::debug_span!("handle_request", unique = ?header.unique());
        let _enter = span.enter();

        let op = Operation::decode(header, arg)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        tracing::debug!(?op);

        let req = Request {
            global: &self.global,
            conn: &self.conn,
            header,
            join_set: &mut self.join_set,
        };

        let result = match op {
            Operation::Lookup(op) => fs.lookup(req, op),
            Operation::Getattr(op) => fs.getattr(req, op),
            Operation::Setattr(op) => fs.setattr(req, op),
            Operation::Readlink(op) => fs.readlink(req, op),
            Operation::Symlink(op) => fs.symlink(req, op),
            Operation::Mknod(op) => fs.mknod(req, op),
            Operation::Mkdir(op) => fs.mkdir(req, op),
            Operation::Unlink(op) => fs.unlink(req, op),
            Operation::Rmdir(op) => fs.rmdir(req, op),
            Operation::Rename(op) => fs.rename(req, op),
            Operation::Link(op) => fs.link(req, op),
            Operation::Open(op) => fs.open(req, op),
            Operation::Read(op) => fs.read(req, op),
            Operation::Release(op) => fs.release(req, op),
            Operation::Statfs(op) => fs.statfs(req, op),
            Operation::Fsync(op) => fs.fsync(req, op),
            Operation::Setxattr(op) => fs.setxattr(req, op),
            Operation::Getxattr(op) => fs.getxattr(req, op),
            Operation::Listxattr(op) => fs.listxattr(req, op),
            Operation::Removexattr(op) => fs.removexattr(req, op),
            Operation::Flush(op) => fs.flush(req, op),
            Operation::Opendir(op) => fs.opendir(req, op),
            Operation::Readdir(op) => fs.readdir(req, op),
            Operation::Releasedir(op) => fs.releasedir(req, op),
            Operation::Fsyncdir(op) => fs.fsyncdir(req, op),
            Operation::Getlk(op) => fs.getlk(req, op),
            Operation::Setlk(op) => fs.setlk(req, op),
            Operation::Flock(op) => fs.flock(req, op),
            Operation::Access(op) => fs.access(req, op),
            Operation::Create(op) => fs.create(req, op),
            Operation::Bmap(op) => fs.bmap(req, op),
            Operation::Fallocate(op) => fs.fallocate(req, op),
            Operation::CopyFileRange(op) => fs.copy_file_range(req, op),
            Operation::Poll(op) => fs.poll(req, op),
            Operation::Lseek(op) => fs.lseek(req, op),
            Operation::Write(op) => fs.write(req, op, data),
            Operation::NotifyReply(op) => {
                fs.notify_reply(op, data)?;
                return Ok(());
            }
            Operation::Forget(forgets) => {
                fs.forget(forgets.as_ref());
                return Ok(());
            }
            Operation::Interrupt(op) => {
                tracing::warn!("interrupted(unique={})", op.unique);
                // TODO: handle interrupt requests.
                Err(Error::Code(Errno::NOSYS))
            }
        };

        match result {
            Ok(..) => {}
            Err(Error::Fatal(err)) => match Errno::from_io_error(&err) {
                Some(Errno::NOENT) => {
                    // missing in processing queue
                }
                _ => return Err(err),
            },
            Err(Error::Code(errno)) => {
                self.global
                    .session
                    .send_reply(&self.conn, header.unique(), Some(errno), ())?
            }
        }

        Ok(())
    }
}
