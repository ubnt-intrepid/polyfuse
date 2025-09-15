use crate::{
    bytes::Bytes,
    op::{self, Forget, Operation},
    raw::{
        conn::Connection,
        mount::{mount, Fusermount, MountOptions},
        request::{RemainingData, RequestBuffer},
        session::{KernelConfig, KernelFlags, Session},
    },
    reply,
    types::{NodeID, NotifyID, PollWakeupID, GID, PID, UID},
};
use libc::{EIO, ENOENT, ENOSYS};
use polyfuse_kernel::*;
use std::{
    ffi::OsStr,
    future::Future,
    io,
    num::NonZeroUsize,
    panic,
    path::PathBuf,
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
    Code(i32),

    #[error("fatal error: {}", _0)]
    Fatal(#[source] io::Error),
}

impl From<i32> for Error {
    fn from(code: i32) -> Self {
        Self::Code(code)
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Code(err.raw_os_error().unwrap_or(EIO))
    }
}

pub type Result<T = _priv::Replied, E = Error> = std::result::Result<T, E>;

macro_rules! define_ops {
    ($( $name:ident: { $Arg:ident, $Reply:ident } ),*$(,)*) => {$(
        #[allow(unused_variables)]
        fn $name(
            self: &Arc<Self>,
            req: Request<'_>,
            op: op::$Arg<'_>,
            reply: $Reply<'_>,
        ) -> impl Future<Output = Result> + Send {
            async {
                Err(Error::Code(ENOSYS))
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
        data: Data<'_>,
        reply: ReplyWrite<'_>,
    ) -> impl Future<Output = Result> + Send {
        async { Err(Error::Code(ENOSYS)) }
    }

    #[allow(unused_variables)]
    fn forget(self: &Arc<Self>, forgets: &[Forget]) -> impl Future<Output = ()> + Send {
        async {}
    }

    #[allow(unused_variables)]
    fn notify_reply(
        self: &Arc<Self>,
        op: op::NotifyReply<'_>,
        data: Data<'_>,
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
    buf: &'req RequestBuffer,
    join_set: &'req mut JoinSet<io::Result<()>>,
}

impl Request<'_> {
    pub fn uid(&self) -> UID {
        self.buf.uid()
    }

    pub fn gid(&self) -> GID {
        self.buf.gid()
    }

    pub fn pid(&self) -> PID {
        self.buf.pid()
    }

    pub fn notifier(&self) -> Notifier {
        self.global.notifier()
    }

    pub fn spawner(&mut self) -> Spawner<'_> {
        Spawner {
            join_set: self.join_set,
        }
    }
}

pub struct Data<'req> {
    inner: RemainingData<'req>,
}

impl io::Read for Data<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        self.inner.read_vectored(bufs)
    }
}

pub struct ReplySender<'req> {
    session: &'req Session,
    conn: &'req Connection,
    buf: &'req RequestBuffer,
}

mod _priv {
    use super::*;

    pub struct Replied {
        _priv: (),
    }

    impl reply::ReplySender for ReplySender<'_> {
        type Ok = Replied;
        type Error = Error;

        fn send<B>(self, arg: B) -> Result
        where
            B: Bytes,
        {
            self.session
                .send_reply(self.conn, self.buf.unique(), 0, arg)
                .map_err(Error::Fatal)?;
            Ok(Replied { _priv: () })
        }
    }
}

pub type ReplyUnit<'req> = reply::ReplyUnit<ReplySender<'req>>;
pub type ReplyEntry<'req> = reply::ReplyEntry<ReplySender<'req>>;
pub type ReplyAttr<'req> = reply::ReplyAttr<ReplySender<'req>>;
pub type ReplyOpen<'req> = reply::ReplyOpen<ReplySender<'req>>;
pub type ReplyCreate<'req> = reply::ReplyCreate<ReplySender<'req>>;
pub type ReplyData<'req> = reply::ReplyData<ReplySender<'req>>;
pub type ReplyDir<'req> = reply::ReplyDir<ReplySender<'req>>;
pub type ReplyWrite<'req> = reply::ReplyWrite<ReplySender<'req>>;
pub type ReplyStatfs<'req> = reply::ReplyStatfs<ReplySender<'req>>;
pub type ReplyXattr<'req> = reply::ReplyXattr<ReplySender<'req>>;
pub type ReplyBmap<'req> = reply::ReplyBmap<ReplySender<'req>>;
pub type ReplyLock<'req> = reply::ReplyLock<ReplySender<'req>>;
pub type ReplyPoll<'req> = reply::ReplyPoll<ReplySender<'req>>;
pub type ReplyLseek<'req> = reply::ReplyLseek<ReplySender<'req>>;

pub struct Daemon {
    global: Arc<Global>,
    config: KernelConfig,
    fusermount: Fusermount,
    join_set: JoinSet<io::Result<()>>,
}

impl Daemon {
    pub async fn mount(
        mountpoint: PathBuf,
        mountopts: MountOptions,
        mut config: KernelConfig,
    ) -> io::Result<Self> {
        let (conn, fusermount) = mount(mountpoint, mountopts)?;
        let conn = Connection::from(conn);

        let mut session = Session::new();
        session.init(&conn, &mut config)?;

        Ok(Self {
            global: Arc::new(Global {
                session,
                conn,
                notify_unique: AtomicU64::new(0),
            }),
            config,
            fusermount,
            join_set: JoinSet::new(),
        })
    }

    pub fn config(&self) -> &KernelConfig {
        &self.config
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

    fn new_request_buffer(&self) -> io::Result<RequestBuffer> {
        if self.config.flags.contains(KernelFlags::SPLICE_READ) {
            RequestBuffer::new_splice(self.config.request_buffer_size())
        } else {
            RequestBuffer::new_fallback(self.config.request_buffer_size())
        }
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
            let buf = self.new_request_buffer()?;
            self.join_set
                .spawn(async move { worker.run(buf, fs).await });
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
    async fn run<T>(mut self, mut buf: RequestBuffer, fs: Arc<T>) -> io::Result<()>
    where
        T: Filesystem,
    {
        while self.read_request(&mut buf).await? {
            self.handle_request(&buf, &fs).await?;
        }
        self.join_set.shutdown().await;
        Ok(())
    }

    async fn read_request(&self, buf: &mut RequestBuffer) -> io::Result<bool> {
        loop {
            let mut guard = self.conn.readable().await?;
            match guard.try_io(|conn| self.global.session.recv_request(conn.get_ref(), buf)) {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }

    async fn handle_request<T>(&mut self, buf: &RequestBuffer, fs: &Arc<T>) -> io::Result<()>
    where
        T: Filesystem,
    {
        let span = tracing::debug_span!("handle_request", unique = ?buf.unique());
        let _enter = span.enter();

        let (op, data) = buf
            .operation()
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        tracing::debug!(?op);

        let req = Request {
            global: &self.global,
            buf,
            join_set: &mut self.join_set,
        };

        let sender = ReplySender {
            session: &self.global.session,
            conn: self.conn.get_ref(),
            buf,
        };

        let result = match op {
            Operation::Lookup(op) => fs.lookup(req, op, ReplyEntry::new(sender)).await,
            Operation::Getattr(op) => fs.getattr(req, op, ReplyAttr::new(sender)).await,
            Operation::Setattr(op) => fs.setattr(req, op, ReplyAttr::new(sender)).await,
            Operation::Readlink(op) => fs.readlink(req, op, ReplyData::new(sender)).await,
            Operation::Symlink(op) => fs.symlink(req, op, ReplyEntry::new(sender)).await,
            Operation::Mknod(op) => fs.mknod(req, op, ReplyEntry::new(sender)).await,
            Operation::Mkdir(op) => fs.mkdir(req, op, ReplyEntry::new(sender)).await,
            Operation::Unlink(op) => fs.unlink(req, op, ReplyUnit::new(sender)).await,
            Operation::Rmdir(op) => fs.rmdir(req, op, ReplyUnit::new(sender)).await,
            Operation::Rename(op) => fs.rename(req, op, ReplyUnit::new(sender)).await,
            Operation::Link(op) => fs.link(req, op, ReplyEntry::new(sender)).await,
            Operation::Open(op) => fs.open(req, op, ReplyOpen::new(sender)).await,
            Operation::Read(op) => fs.read(req, op, ReplyData::new(sender)).await,
            Operation::Release(op) => fs.release(req, op, ReplyUnit::new(sender)).await,
            Operation::Statfs(op) => fs.statfs(req, op, ReplyStatfs::new(sender)).await,
            Operation::Fsync(op) => fs.fsync(req, op, ReplyUnit::new(sender)).await,
            Operation::Setxattr(op) => fs.setxattr(req, op, ReplyUnit::new(sender)).await,
            Operation::Getxattr(op) => fs.getxattr(req, op, ReplyXattr::new(sender)).await,
            Operation::Listxattr(op) => fs.listxattr(req, op, ReplyXattr::new(sender)).await,
            Operation::Removexattr(op) => fs.removexattr(req, op, ReplyUnit::new(sender)).await,
            Operation::Flush(op) => fs.flush(req, op, ReplyUnit::new(sender)).await,
            Operation::Opendir(op) => fs.opendir(req, op, ReplyOpen::new(sender)).await,
            Operation::Readdir(op) => {
                let capacity = op.size() as usize;
                fs.readdir(req, op, ReplyDir::new(sender, capacity)).await
            }
            Operation::Releasedir(op) => fs.releasedir(req, op, ReplyUnit::new(sender)).await,
            Operation::Fsyncdir(op) => fs.fsyncdir(req, op, ReplyUnit::new(sender)).await,
            Operation::Getlk(op) => fs.getlk(req, op, ReplyLock::new(sender)).await,
            Operation::Setlk(op) => fs.setlk(req, op, ReplyUnit::new(sender)).await,
            Operation::Flock(op) => fs.flock(req, op, ReplyUnit::new(sender)).await,
            Operation::Access(op) => fs.access(req, op, ReplyUnit::new(sender)).await,
            Operation::Create(op) => fs.create(req, op, ReplyCreate::new(sender)).await,
            Operation::Bmap(op) => fs.bmap(req, op, ReplyBmap::new(sender)).await,
            Operation::Fallocate(op) => fs.fallocate(req, op, ReplyUnit::new(sender)).await,
            Operation::CopyFileRange(op) => {
                fs.copy_file_range(req, op, ReplyWrite::new(sender)).await
            }
            Operation::Poll(op) => fs.poll(req, op, ReplyPoll::new(sender)).await,
            Operation::Lseek(op) => fs.lseek(req, op, ReplyLseek::new(sender)).await,
            Operation::Write(op) => {
                fs.write(req, op, Data { inner: data }, ReplyWrite::new(sender))
                    .await
            }
            Operation::NotifyReply(op) => {
                fs.notify_reply(op, Data { inner: data }).await?;
                return Ok(());
            }
            Operation::Forget(forgets) => {
                fs.forget(forgets.as_ref()).await;
                return Ok(());
            }
            Operation::Interrupt(op) => {
                tracing::warn!("interrupted(unique={})", op.unique());
                // TODO: handle interrupt requests.
                Err(ENOSYS.into())
            }
        };

        match result {
            Ok(..) => {}
            Err(Error::Fatal(err)) => match err.raw_os_error() {
                Some(ENOENT) => {
                    // missing in processing queue
                }
                _ => return Err(err),
            },
            Err(Error::Code(errno)) => {
                self.global
                    .session
                    .send_reply(self.conn.get_ref(), buf.unique(), errno, ())?
            }
        }

        Ok(())
    }
}
