use crate::{
    bytes::Bytes,
    conn::Connection,
    mount::{Fusermount, MountOptions},
    notify::{self, Notify},
    op::{self, Forget, Operation},
    request::{RemainingData, RequestBuffer},
    session::{KernelConfig, KernelFlags, Session},
    types::{GID, PID, UID},
};
use libc::{ENOENT, ENOSYS};
use std::{
    future::Future,
    io,
    num::NonZeroUsize,
    panic,
    path::PathBuf,
    sync::{Arc, Weak},
};
use tokio::{
    io::unix::AsyncFd,
    task::{AbortHandle, JoinSet},
};

use self::reply::{
    ReplyAttr, ReplyBmap, ReplyCreate, ReplyData, ReplyDir, ReplyEntry, ReplyLock, ReplyLseek,
    ReplyOpen, ReplyPoll, ReplyStatfs, ReplyUnit, ReplyWrite, ReplyXattr,
};

const NUM_WORKERS_PER_THREAD: usize = 4;

macro_rules! define_ops {
    ($( $name:ident: { $Arg:ident, $Reply:ident } ),*$(,)*) => {$(
        #[allow(unused_variables)]
        fn $name(
            self: &Arc<Self>,
            req: Request<'_>,
            op: op::$Arg<'_>,
            reply: reply::$Reply<'_>,
        ) -> impl Future<Output = reply::Result> + Send {
            async {
                Err(reply::Error::unimplemented())
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
        reply: reply::ReplyWrite<'_>,
    ) -> impl Future<Output = reply::Result> + Send {
        async { Err(reply::Error::unimplemented()) }
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
}

impl Global {
    fn notifier(self: &Arc<Self>) -> Notifier {
        Notifier {
            global: Arc::downgrade(self),
        }
    }

    fn send_notify<T>(&self, notify: T) -> io::Result<()>
    where
        T: notify::Notify,
    {
        self.session.send_notify(&self.conn, notify)
    }
}

pub struct Notifier {
    global: Weak<Global>,
}

impl Notifier {
    pub fn send(&self, notify: impl Notify) -> io::Result<()> {
        if let Some(global) = self.global.upgrade() {
            global.send_notify(notify)?;
        }
        Ok(())
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

struct ReplySender<'req> {
    session: &'req Session,
    conn: &'req Connection,
    buf: &'req RequestBuffer,
}

impl ReplySender<'_> {
    fn send<B>(self, arg: B) -> reply::Result
    where
        B: Bytes,
    {
        self.session
            .send_reply(self.conn, self.buf.unique(), 0, arg)
            .map_err(reply::Error::Fatal)?;
        Ok(reply::Replied::new())
    }
}

pub mod reply {
    use super::ReplySender;
    use crate::{
        bytes::Bytes,
        out::{
            AttrOut, BmapOut, EntryOut, LkOut, LseekOut, OpenOut, PollOut, ReaddirOut, StatfsOut,
            WriteOut, XattrOut,
        },
        types::{FileLock, FileType, NodeID, PollEvents, Statfs},
    };
    use libc::{EIO, ENOSYS};
    use std::{ffi::OsStr, io};

    pub type Result<T = Replied, E = Error> = std::result::Result<T, E>;

    /// The ZST to ensure that the filesystem responds to a FUSE request exactly once.
    #[derive(Debug)]
    pub struct Replied {
        _private: (),
    }
    impl Replied {
        pub(super) const fn new() -> Self {
            Self { _private: () }
        }
    }

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

    impl Error {
        pub const fn unimplemented() -> Self {
            Self::Code(ENOSYS)
        }
    }

    pub struct ReplyUnit<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyUnit<'req> {
        pub(super) const fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send(self) -> Result {
            self.sender.send(())
        }
    }

    pub struct ReplyEntry<'req> {
        sender: ReplySender<'req>,
        out: EntryOut,
    }

    impl<'req> ReplyEntry<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self {
                sender,
                out: EntryOut::default(),
            }
        }

        pub fn out(&mut self) -> &mut EntryOut {
            &mut self.out
        }

        pub fn send(self) -> Result {
            self.sender.send(&self.out)
        }
    }

    pub struct ReplyAttr<'req> {
        sender: ReplySender<'req>,
        out: AttrOut,
    }

    impl<'req> ReplyAttr<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self {
                sender,
                out: AttrOut::default(),
            }
        }

        pub fn out(&mut self) -> &mut AttrOut {
            &mut self.out
        }

        pub fn send(self) -> Result {
            self.sender.send(&self.out)
        }
    }

    pub struct ReplyData<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyData<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send<B>(self, bytes: B) -> Result
        where
            B: Bytes,
        {
            self.sender.send(bytes)
        }
    }

    pub struct ReplyWrite<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyWrite<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send(self, size: u32) -> Result {
            let mut out = WriteOut::default();
            WriteOut::size(&mut out, size);
            self.sender.send(out)
        }
    }

    pub struct ReplyDir<'req> {
        sender: ReplySender<'req>,
        out: ReaddirOut,
    }

    impl<'req> ReplyDir<'req> {
        pub(super) fn new(sender: ReplySender<'req>, capacity: usize) -> Self {
            Self {
                sender,
                out: ReaddirOut::new(capacity),
            }
        }

        pub fn push_entry(
            &mut self,
            name: &OsStr,
            ino: NodeID,
            typ: Option<FileType>,
            offset: u64,
        ) -> bool {
            self.out.entry(name, ino, typ, offset)
        }

        pub fn send(self) -> Result {
            self.sender.send(&self.out)
        }
    }

    pub struct ReplyOpen<'req> {
        sender: ReplySender<'req>,
        out: OpenOut,
    }

    impl<'req> ReplyOpen<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self {
                sender,
                out: OpenOut::default(),
            }
        }

        pub fn out(&mut self) -> &mut OpenOut {
            &mut self.out
        }

        pub fn send(self) -> Result {
            self.sender.send(&self.out)
        }
    }

    pub struct ReplyCreate<'req> {
        sender: ReplySender<'req>,
        entry_out: EntryOut,
        open_out: OpenOut,
    }

    impl<'req> ReplyCreate<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self {
                sender,
                entry_out: EntryOut::default(),
                open_out: OpenOut::default(),
            }
        }

        pub fn entry_out(&mut self) -> &mut EntryOut {
            &mut self.entry_out
        }

        pub fn open_out(&mut self) -> &mut OpenOut {
            &mut self.open_out
        }

        pub fn send(self) -> Result {
            self.sender.send((self.entry_out, self.open_out))
        }
    }

    pub struct ReplyStatfs<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyStatfs<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send(self, st: Statfs) -> Result {
            let mut out = StatfsOut::default();
            out.statfs(st);
            self.sender.send(out)
        }
    }

    pub struct ReplyXattr<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyXattr<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send_size(self, size: u32) -> Result {
            let mut out = XattrOut::default();
            XattrOut::size(&mut out, size);
            self.sender.send(out)
        }

        pub fn send_value<B>(self, data: B) -> Result
        where
            B: Bytes,
        {
            self.sender.send(data)
        }
    }

    pub struct ReplyLock<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyLock<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send(self, lk: FileLock) -> Result {
            let mut out = LkOut::default();
            out.file_lock(&lk);
            self.sender.send(out)
        }
    }

    pub struct ReplyBmap<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyBmap<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send(self, block: u64) -> Result {
            let mut out = BmapOut::default();
            out.block(block);
            self.sender.send(out)
        }
    }

    pub struct ReplyPoll<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyPoll<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send(self, revents: PollEvents) -> Result {
            let mut out = PollOut::default();
            out.revents(revents);
            self.sender.send(out)
        }
    }

    pub struct ReplyLseek<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyLseek<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send(self, offset: u64) -> Result {
            let mut out = LseekOut::default();
            out.offset(offset);
            self.sender.send(out)
        }
    }
}

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
        let (conn, fusermount) = crate::mount::mount(mountpoint, mountopts)?;
        let conn = Connection::from(conn);

        let mut session = Session::new();
        session.init(&conn, &mut config)?;

        Ok(Self {
            global: Arc::new(Global { session, conn }),
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
            Err(reply::Error::Fatal(err)) => match err.raw_os_error() {
                Some(ENOENT) => {
                    // missing in processing queue
                }
                _ => return Err(err),
            },
            Err(reply::Error::Code(errno)) => {
                self.global
                    .session
                    .send_reply(self.conn.get_ref(), buf.unique(), errno, ())?
            }
        }

        Ok(())
    }
}
