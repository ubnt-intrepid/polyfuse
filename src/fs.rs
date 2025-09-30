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
    future::Future,
    io,
    num::NonZeroUsize,
    panic,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Weak,
    },
};
use tokio::{
    io::unix::AsyncFd,
    task::{AbortHandle, JoinSet},
};
use zerocopy::IntoBytes as _;

const NUM_WORKERS_PER_THREAD: usize = 4;

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
    ($( $name:ident: { $Arg:ident, $Reply:ident } ),*$(,)*) => {$(
        #[allow(unused_variables)]
        fn $name(
            self: &Arc<Self>,
            req: Request<'_>,
            op: op::$Arg<'_>,
        ) -> impl Future<Output = Result> + Send {
            async {
                Err(Error::Code(Errno::NOSYS))
            }
        }
    )*};
}

pub trait Filesystem {
    define_ops! {
        lookup: { Lookup, ReplyEntry },
        getattr: { Getattr, ReplyAttr },
        setattr: { Setattr, ReplyAttr },
        readlink: { Readlink, ReplyData },
        symlink: { Symlink, ReplyEntry },
        mknod: { Mknod, ReplyEntry },
        mkdir: { Mkdir, ReplyEntry },
        unlink: { Unlink, ReplyUnit },
        rmdir: { Rmdir, ReplyUnit },
        rename: { Rename, ReplyUnit },
        link: { Link, ReplyEntry },
        open: { Open, ReplyOpen },
        read: { Read, ReplyData },
        release: { Release, ReplyUnit },
        statfs: { Statfs, ReplyStatfs },
        fsync: { Fsync, ReplyUnit },
        setxattr: { Setxattr, ReplyUnit },
        getxattr: { Getxattr, ReplyXattr },
        listxattr: { Listxattr, ReplyXattr },
        removexattr: { Removexattr, ReplyUnit },
        flush: { Flush, ReplyUnit },
        opendir: { Opendir, ReplyOpen },
        readdir: { Readdir, ReplyDir },
        releasedir: { Releasedir, ReplyUnit },
        fsyncdir: { Fsyncdir, ReplyUnit },
        getlk: { Getlk, ReplyLock },
        setlk: { Setlk, ReplyUnit },
        flock: { Flock, ReplyUnit },
        access: { Access, ReplyUnit },
        create: { Create, ReplyCreate },
        bmap: { Bmap, ReplyBmap },
        fallocate: { Fallocate, ReplyUnit },
        copy_file_range: { CopyFileRange, ReplyWrite },
        poll: { Poll, ReplyPoll },
        lseek: { Lseek, ReplyLseek },
    }

    #[allow(unused_variables)]
    fn write(
        self: &Arc<Self>,
        req: Request<'_>,
        op: op::Write<'_>,
        data: impl io::Read + Send + Unpin,
    ) -> impl Future<Output = Result> + Send {
        async { Err(Error::Code(Errno::NOSYS)) }
    }

    #[allow(unused_variables)]
    fn forget(self: &Arc<Self>, forgets: &[Forget]) -> impl Future<Output = ()> + Send {
        async {}
    }

    #[allow(unused_variables)]
    fn notify_reply(
        self: &Arc<Self>,
        op: op::NotifyReply<'_>,
        data: impl io::Read + Send + Unpin,
    ) -> impl Future<Output = io::Result<()>> + Send {
        async { Ok(()) }
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
    join_set: &'a mut JoinSet<io::Result<()>>,
}

impl Spawner<'_> {
    pub fn spawn<F>(&mut self, future: F) -> AbortHandle
    where
        F: Future<Output = io::Result<()>> + Send + 'static,
    {
        self.join_set.spawn(future)
    }

    pub fn spawn_blocking<F>(&mut self, f: F) -> AbortHandle
    where
        F: FnOnce() -> io::Result<()> + Send + 'static,
    {
        self.join_set.spawn_blocking(f)
    }
}

/// The context for a single FUSE request used by the filesystem.
pub struct Request<'req> {
    global: &'req Arc<Global>,
    conn: &'req Connection,
    header: &'req RequestHeader,
    join_set: &'req mut JoinSet<io::Result<()>>,
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
    join_set: JoinSet<io::Result<()>>,
}

impl Daemon {
    pub async fn mount(
        mountpoint: impl Into<Cow<'static, Path>>,
        mountopts: MountOptions,
        config: KernelConfig,
    ) -> io::Result<Self> {
        let (conn, fusermount) = crate::mount::mount(&mountpoint.into(), &mountopts)?;

        let session = Session::init(&conn, config)?;

        Ok(Self {
            global: Arc::new(Global {
                session,
                conn,
                notify_unique: AtomicU64::new(0),
            }),
            fusermount,
            join_set: JoinSet::new(),
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
            conn: AsyncFd::new(conn)?,
            join_set: JoinSet::new(),
        })
    }

    pub async fn run<T>(mut self, fs: Arc<T>, num_workers: Option<NonZeroUsize>) -> io::Result<()>
    where
        T: Filesystem + Send + Sync + 'static,
    {
        let num_workers = num_workers
            .map(|n| n.get())
            .unwrap_or_else(|| num_cpus::get() * NUM_WORKERS_PER_THREAD);

        for i in 0..num_workers {
            let worker = self.new_worker(i)?;
            let fs = fs.clone();
            let bufsize = self.global.session.request_buffer_size();
            if self.config().flags.contains(KernelFlags::SPLICE_READ) {
                let buf = SpliceBuf::new(bufsize)?;
                self.join_set
                    .spawn(async move { worker.run(buf, fs).await });
            } else {
                let buf = FallbackBuf::new(bufsize);
                self.join_set
                    .spawn(async move { worker.run(buf, fs).await });
            }
        }

        while let Some(result) = self.join_set.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    tracing::error!("A worker thread is exited with error: {}", err);
                }
                Err(err) if err.is_panic() => panic::resume_unwind(err.into_panic()),
                Err(_err) => (),
            }
        }

        self.fusermount.unmount()?;

        Ok(())
    }
}

struct Worker {
    global: Arc<Global>,
    conn: AsyncFd<Connection>,
    join_set: JoinSet<io::Result<()>>,
}

impl Worker {
    async fn run<T, B>(mut self, mut buf: B, fs: Arc<T>) -> io::Result<()>
    where
        T: Filesystem,
        B: RequestBuf,
        for<'req> B::RemainingData<'req>: Send + Unpin,
    {
        while self.read_request(&mut buf).await? {
            self.handle_request(&mut buf, &fs).await?;
        }
        self.join_set.shutdown().await;
        Ok(())
    }

    async fn read_request<B>(&self, buf: &mut B) -> io::Result<bool>
    where
        B: RequestBuf,
    {
        loop {
            let mut guard = self.conn.readable().await?;
            match guard.try_io(|conn| self.global.session.recv_request(conn.get_ref(), buf)) {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }

    async fn handle_request<T, B>(&mut self, buf: &mut B, fs: &Arc<T>) -> io::Result<()>
    where
        T: Filesystem,
        B: RequestBuf,
        for<'req> B::RemainingData<'req>: Send + Unpin,
    {
        let (header, arg, data) = buf.parts();

        let span = tracing::debug_span!("handle_request", unique = ?header.unique());
        let _enter = span.enter();

        let op = Operation::decode(header, arg)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        tracing::debug!(?op);

        let req = Request {
            global: &self.global,
            conn: self.conn.get_ref(),
            header,
            join_set: &mut self.join_set,
        };

        let result = match op {
            Operation::Lookup(op) => fs.lookup(req, op).await,
            Operation::Getattr(op) => fs.getattr(req, op).await,
            Operation::Setattr(op) => fs.setattr(req, op).await,
            Operation::Readlink(op) => fs.readlink(req, op).await,
            Operation::Symlink(op) => fs.symlink(req, op).await,
            Operation::Mknod(op) => fs.mknod(req, op).await,
            Operation::Mkdir(op) => fs.mkdir(req, op).await,
            Operation::Unlink(op) => fs.unlink(req, op).await,
            Operation::Rmdir(op) => fs.rmdir(req, op).await,
            Operation::Rename(op) => fs.rename(req, op).await,
            Operation::Link(op) => fs.link(req, op).await,
            Operation::Open(op) => fs.open(req, op).await,
            Operation::Read(op) => fs.read(req, op).await,
            Operation::Release(op) => fs.release(req, op).await,
            Operation::Statfs(op) => fs.statfs(req, op).await,
            Operation::Fsync(op) => fs.fsync(req, op).await,
            Operation::Setxattr(op) => fs.setxattr(req, op).await,
            Operation::Getxattr(op) => fs.getxattr(req, op).await,
            Operation::Listxattr(op) => fs.listxattr(req, op).await,
            Operation::Removexattr(op) => fs.removexattr(req, op).await,
            Operation::Flush(op) => fs.flush(req, op).await,
            Operation::Opendir(op) => fs.opendir(req, op).await,
            Operation::Readdir(op) => fs.readdir(req, op).await,
            Operation::Releasedir(op) => fs.releasedir(req, op).await,
            Operation::Fsyncdir(op) => fs.fsyncdir(req, op).await,
            Operation::Getlk(op) => fs.getlk(req, op).await,
            Operation::Setlk(op) => fs.setlk(req, op).await,
            Operation::Flock(op) => fs.flock(req, op).await,
            Operation::Access(op) => fs.access(req, op).await,
            Operation::Create(op) => fs.create(req, op).await,
            Operation::Bmap(op) => fs.bmap(req, op).await,
            Operation::Fallocate(op) => fs.fallocate(req, op).await,
            Operation::CopyFileRange(op) => fs.copy_file_range(req, op).await,
            Operation::Poll(op) => fs.poll(req, op).await,
            Operation::Lseek(op) => fs.lseek(req, op).await,
            Operation::Write(op) => fs.write(req, op, data).await,
            Operation::NotifyReply(op) => {
                fs.notify_reply(op, data).await?;
                return Ok(());
            }
            Operation::Forget(forgets) => {
                fs.forget(forgets.as_ref()).await;
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
            Err(Error::Code(errno)) => self.global.session.send_reply(
                self.conn.get_ref(),
                header.unique(),
                Some(errno),
                (),
            )?,
        }

        Ok(())
    }
}
