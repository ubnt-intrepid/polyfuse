use crate::{
    bytes::Bytes,
    conn::Connection,
    mount::MountOptions,
    notify::{self, Notify},
    op::{self, Forget, Operation},
    reply::{
        AttrOut, BmapOut, EntryOut, LkOut, LseekOut, OpenOut, PollOut, ReaddirOut, StatfsOut,
        WriteOut, XattrOut,
    },
    request::{RemainingData, RequestBuffer},
    session::{KernelConfig, KernelFlags, Session},
    types::{FileLock, FileType, NodeID, PollEvents, Statfs, GID, PID, UID},
};
use libc::{EIO, ENOENT, ENOSYS};
use std::{
    ffi::OsStr,
    future::Future,
    io,
    path::PathBuf,
    sync::{Arc, Weak},
};
use tokio::{
    io::unix::AsyncFd,
    task::{AbortHandle, JoinSet},
};

pub type Result<T = Replied, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("error during reply: {}", _0)]
    Reply(#[source] io::Error),

    #[error("Operation failed with {}", _0)]
    Code(i32),
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

macro_rules! define_ops {
    ($( $name:ident: { $Arg:ident, $Reply:ident } ),*$(,)*) => {$(
        #[allow(unused_variables)]
        fn $name(
            self: &Arc<Self>,
            req: &mut Request<'_>,
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
        req: &mut Request<'_>,
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
    fn init(
        self: &Arc<Self>,
        cx: &mut InitContext<'_>,
    ) -> impl Future<Output = io::Result<()>> + Send {
        async { Ok(()) }
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
    #[allow(dead_code)]
    mountpoint: PathBuf,
    #[allow(dead_code)]
    mountopts: MountOptions,
    config: KernelConfig,
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

pub struct InitContext<'a> {
    global: &'a Arc<Global>,
    join_set: &'a mut JoinSet<io::Result<()>>,
}

impl InitContext<'_> {
    pub fn notifier(&self) -> Notifier {
        self.global.notifier()
    }

    pub fn spawner(&mut self) -> Spawner<'_> {
        Spawner {
            join_set: self.join_set,
        }
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

/// The ZST to ensure that the filesystem responds to a FUSE request exactly once.
#[derive(Debug)]
pub struct Replied {
    _private: (),
}

struct ReplyBase<'req> {
    session: &'req Session,
    conn: &'req Connection,
    buf: &'req RequestBuffer,
}

impl ReplyBase<'_> {
    fn send<B>(self, arg: B) -> Result
    where
        B: Bytes,
    {
        self.session
            .send_reply(self.conn, self.buf.unique(), 0, arg)
            .map_err(Error::Reply)?;
        Ok(Replied { _private: () })
    }
}

pub struct ReplyUnit<'req> {
    base: ReplyBase<'req>,
}

impl ReplyUnit<'_> {
    pub fn send(self) -> Result {
        self.base.send(())
    }
}

pub struct ReplyEntry<'req> {
    base: ReplyBase<'req>,
    out: EntryOut,
}
impl ReplyEntry<'_> {
    pub fn out(&mut self) -> &mut EntryOut {
        &mut self.out
    }

    pub fn send(self) -> Result {
        self.base.send(&self.out)
    }
}

pub struct ReplyAttr<'req> {
    base: ReplyBase<'req>,
    out: AttrOut,
}
impl ReplyAttr<'_> {
    pub fn out(&mut self) -> &mut AttrOut {
        &mut self.out
    }

    pub fn send(self) -> Result {
        self.base.send(&self.out)
    }
}

pub struct ReplyData<'req> {
    base: ReplyBase<'req>,
}
impl ReplyData<'_> {
    pub fn send<B>(self, bytes: B) -> Result
    where
        B: Bytes,
    {
        self.base.send(bytes)
    }
}

pub struct ReplyWrite<'req> {
    base: ReplyBase<'req>,
}
impl ReplyWrite<'_> {
    pub fn send(self, size: u32) -> Result {
        let mut out = WriteOut::default();
        WriteOut::size(&mut out, size);
        self.base.send(out)
    }
}

pub struct ReplyDir<'req> {
    base: ReplyBase<'req>,
    out: ReaddirOut,
}

impl ReplyDir<'_> {
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
        self.base.send(&self.out)
    }
}

pub struct ReplyOpen<'req> {
    base: ReplyBase<'req>,
    out: OpenOut,
}

impl ReplyOpen<'_> {
    pub fn out(&mut self) -> &mut OpenOut {
        &mut self.out
    }

    pub fn send(self) -> Result {
        self.base.send(&self.out)
    }
}

pub struct ReplyCreate<'req> {
    base: ReplyBase<'req>,
    entry_out: EntryOut,
    open_out: OpenOut,
}
impl ReplyCreate<'_> {
    pub fn entry_out(&mut self) -> &mut EntryOut {
        &mut self.entry_out
    }
    pub fn open_out(&mut self) -> &mut OpenOut {
        &mut self.open_out
    }
    pub fn send(self) -> Result {
        self.base.send((self.entry_out, self.open_out))
    }
}

pub struct ReplyStatfs<'req> {
    base: ReplyBase<'req>,
}

impl ReplyStatfs<'_> {
    pub fn send(self, st: Statfs) -> Result {
        let mut out = StatfsOut::default();
        out.statfs(st);
        self.base.send(out)
    }
}

pub struct ReplyXattr<'req> {
    base: ReplyBase<'req>,
}

impl ReplyXattr<'_> {
    pub fn send_size(self, size: u32) -> Result {
        let mut out = XattrOut::default();
        XattrOut::size(&mut out, size);
        self.base.send(out)
    }

    pub fn send_value<B>(self, data: B) -> Result
    where
        B: Bytes,
    {
        self.base.send(data)
    }
}

pub struct ReplyLock<'req> {
    base: ReplyBase<'req>,
}
impl ReplyLock<'_> {
    pub fn send(self, lk: FileLock) -> Result {
        let mut out = LkOut::default();
        out.file_lock(&lk);
        self.base.send(out)
    }
}

pub struct ReplyBmap<'req> {
    base: ReplyBase<'req>,
}
impl ReplyBmap<'_> {
    pub fn send(self, block: u64) -> Result {
        let mut out = BmapOut::default();
        out.block(block);
        self.base.send(out)
    }
}

pub struct ReplyPoll<'req> {
    base: ReplyBase<'req>,
}
impl ReplyPoll<'_> {
    pub fn send(self, revents: PollEvents) -> Result {
        let mut out = PollOut::default();
        out.revents(revents);
        self.base.send(out)
    }
}

pub struct ReplyLseek<'req> {
    base: ReplyBase<'req>,
}
impl ReplyLseek<'_> {
    pub fn send(self, offset: u64) -> Result {
        let mut out = LseekOut::default();
        out.offset(offset);
        self.base.send(out)
    }
}

pub async fn run<T>(
    fs: T,
    mountpoint: PathBuf,
    mountopts: MountOptions,
    mut config: KernelConfig,
) -> io::Result<()>
where
    T: Filesystem + Send + Sync + 'static,
{
    let span = tracing::debug_span!("polyfuse::fs::run");
    let _enter = span.enter();

    let (conn, fusermount) = crate::mount::mount(mountpoint.clone(), mountopts.clone())?;
    let conn = Connection::from(conn);

    let mut session = Session::new();
    session.init(&conn, &mut config)?;

    let global = Arc::new(Global {
        session,
        conn,
        mountpoint,
        mountopts,
        config,
    });
    let mut join_set = JoinSet::new();
    let fs = Arc::new(fs);

    fs.init(&mut InitContext {
        global: &global,
        join_set: &mut join_set,
    })
    .await?;

    let num_workers = num_cpus::get() * 4;
    for _i in 0..num_workers {
        let conn = global.conn.try_ioc_clone()?;
        let worker = Worker {
            global: global.clone(),
            conn: AsyncFd::new(conn)?,
            join_set: JoinSet::new(),
        };
        let fs = fs.clone();
        let buf = if global.config.flags.contains(KernelFlags::SPLICE_READ) {
            RequestBuffer::new_splice(global.config.request_buffer_size())?
        } else {
            RequestBuffer::new_fallback(global.config.request_buffer_size())?
        };
        join_set.spawn(async move { worker.run(buf, fs).await });
    }

    // TODO: on_destroy
    let _results = join_set.join_all().await;

    fusermount.unmount()?;

    Ok(())
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

        let req = &mut Request {
            global: &self.global,
            buf,
            join_set: &mut self.join_set,
        };

        let base = ReplyBase {
            session: &self.global.session,
            conn: self.conn.get_ref(),
            buf,
        };

        let result = match op {
            Operation::Lookup(op) => {
                fs.lookup(
                    req,
                    op,
                    ReplyEntry {
                        base,
                        out: EntryOut::default(),
                    },
                )
                .await
            }
            Operation::Getattr(op) => {
                fs.getattr(
                    req,
                    op,
                    ReplyAttr {
                        base,
                        out: AttrOut::default(),
                    },
                )
                .await
            }
            Operation::Setattr(op) => {
                fs.setattr(
                    req,
                    op,
                    ReplyAttr {
                        base,
                        out: AttrOut::default(),
                    },
                )
                .await
            }
            Operation::Readlink(op) => fs.readlink(req, op, ReplyData { base }).await,
            Operation::Symlink(op) => {
                fs.symlink(
                    req,
                    op,
                    ReplyEntry {
                        base,
                        out: EntryOut::default(),
                    },
                )
                .await
            }
            Operation::Mknod(op) => {
                fs.mknod(
                    req,
                    op,
                    ReplyEntry {
                        base,
                        out: EntryOut::default(),
                    },
                )
                .await
            }
            Operation::Mkdir(op) => {
                fs.mkdir(
                    req,
                    op,
                    ReplyEntry {
                        base,
                        out: EntryOut::default(),
                    },
                )
                .await
            }
            Operation::Unlink(op) => fs.unlink(req, op, ReplyUnit { base }).await,
            Operation::Rmdir(op) => fs.rmdir(req, op, ReplyUnit { base }).await,
            Operation::Rename(op) => fs.rename(req, op, ReplyUnit { base }).await,
            Operation::Link(op) => {
                fs.link(
                    req,
                    op,
                    ReplyEntry {
                        base,
                        out: EntryOut::default(),
                    },
                )
                .await
            }
            Operation::Open(op) => {
                fs.open(
                    req,
                    op,
                    ReplyOpen {
                        base,
                        out: OpenOut::default(),
                    },
                )
                .await
            }
            Operation::Read(op) => fs.read(req, op, ReplyData { base }).await,
            Operation::Release(op) => fs.release(req, op, ReplyUnit { base }).await,
            Operation::Statfs(op) => fs.statfs(req, op, ReplyStatfs { base }).await,
            Operation::Fsync(op) => fs.fsync(req, op, ReplyUnit { base }).await,
            Operation::Setxattr(op) => fs.setxattr(req, op, ReplyUnit { base }).await,
            Operation::Getxattr(op) => fs.getxattr(req, op, ReplyXattr { base }).await,
            Operation::Listxattr(op) => fs.listxattr(req, op, ReplyXattr { base }).await,
            Operation::Removexattr(op) => fs.removexattr(req, op, ReplyUnit { base }).await,
            Operation::Flush(op) => fs.flush(req, op, ReplyUnit { base }).await,
            Operation::Opendir(op) => {
                fs.opendir(
                    req,
                    op,
                    ReplyOpen {
                        base,
                        out: OpenOut::default(),
                    },
                )
                .await
            }
            Operation::Readdir(op) => {
                let capacity = op.size() as usize;
                fs.readdir(
                    req,
                    op,
                    ReplyDir {
                        base,
                        out: ReaddirOut::new(capacity),
                    },
                )
                .await
            }
            Operation::Releasedir(op) => fs.releasedir(req, op, ReplyUnit { base }).await,
            Operation::Fsyncdir(op) => fs.fsyncdir(req, op, ReplyUnit { base }).await,
            Operation::Getlk(op) => fs.getlk(req, op, ReplyLock { base }).await,
            Operation::Setlk(op) => fs.setlk(req, op, ReplyUnit { base }).await,
            Operation::Flock(op) => fs.flock(req, op, ReplyUnit { base }).await,
            Operation::Access(op) => fs.access(req, op, ReplyUnit { base }).await,
            Operation::Create(op) => {
                fs.create(
                    req,
                    op,
                    ReplyCreate {
                        base,
                        entry_out: EntryOut::default(),
                        open_out: OpenOut::default(),
                    },
                )
                .await
            }
            Operation::Bmap(op) => fs.bmap(req, op, ReplyBmap { base }).await,
            Operation::Fallocate(op) => fs.fallocate(req, op, ReplyUnit { base }).await,
            Operation::CopyFileRange(op) => fs.copy_file_range(req, op, ReplyWrite { base }).await,
            Operation::Poll(op) => fs.poll(req, op, ReplyPoll { base }).await,
            Operation::Lseek(op) => fs.lseek(req, op, ReplyLseek { base }).await,
            Operation::Write(op) => {
                fs.write(req, op, Data { inner: data }, ReplyWrite { base })
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
            Err(Error::Reply(err)) => match err.raw_os_error() {
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
