use crate::{
    bytes::Bytes,
    conn::Connection,
    mount::MountOptions,
    notify,
    op::{self, Forget, Operation},
    reply::{
        AttrOut, BmapOut, EntryOut, LkOut, LseekOut, OpenOut, PollOut, ReaddirOut, StatfsOut,
        WriteOut, XattrOut,
    },
    request::{RemainingData, RequestBuffer},
    session::{KernelConfig, KernelFlags, Session},
    types::{
        FileLock, FileType, NodeID, NotifyID, PollEvents, PollWakeupID, Statfs, GID, PID, UID,
    },
};
use libc::{EIO, ENOENT, ENOSYS};
use std::{
    ffi::OsStr,
    io,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
};

pub type Result<T = Replied, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("The request has already been cancelled by the kernel")]
    Cancelled,

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
        fn $name<'env, 'req>(
            &'env self, env: Env<'_, 'env>,
            req: Request<'req, op::$Arg<'req>>,
            reply: $Reply<'req>,
        ) -> Result {
            Err(Error::Code(ENOSYS))
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
    fn write<'env, 'req>(
        &'env self,
        env: Env<'_, 'env>,
        req: Request<'req, op::Write<'req>>,
        data: Data<'req>,
        reply: ReplyWrite<'req>,
    ) -> Result {
        Err(Error::Code(ENOSYS))
    }

    #[allow(unused_variables)]
    fn forget<'env>(&'env self, env: Env<'_, 'env>, forgets: &[Forget]) {}

    #[allow(unused_variables)]
    fn init<'env>(&'env self, env: Env<'_, 'env>) -> io::Result<()> {
        Ok(())
    }

    #[allow(unused_variables)]
    fn notify_reply<'env, 'req>(
        &'env self,
        env: Env<'_, 'env>,
        req: Request<'req, op::NotifyReply<'req>>,
        data: Data<'req>,
    ) -> io::Result<()> {
        Ok(())
    }
}

pub type Spawner<'scope, 'env> = &'scope thread::Scope<'scope, 'env>;

#[non_exhaustive]
pub struct Env<'scope, 'env: 'scope> {
    pub notifier: Notifier<'env>,
    pub spawner: Spawner<'scope, 'env>,
}

#[derive(Clone)]
pub struct Notifier<'env> {
    session: &'env Session,
    conn: &'env Connection,
    notify_unique: &'env AtomicU64,
}

impl Notifier<'_> {
    fn send<T>(&self, notify: T) -> io::Result<()>
    where
        T: notify::Notify,
    {
        self.session.send_notify(self.conn, notify)
    }

    pub fn inval_inode(&self, ino: NodeID, off: i64, len: i64) -> io::Result<()> {
        self.send(notify::InvalNode::new(ino, off, len))
    }

    pub fn inval_entry(&self, parent: NodeID, name: &OsStr) -> io::Result<()> {
        self.send(notify::InvalEntry::new(parent, name))
    }

    pub fn delete(&self, parent: NodeID, child: NodeID, name: &OsStr) -> io::Result<()> {
        self.send(notify::Delete::new(parent, child, name))
    }

    pub fn store<B>(&self, ino: NodeID, offset: u64, data: B) -> io::Result<()>
    where
        B: Bytes,
    {
        self.send(notify::Store::new(ino, offset, data))
    }

    pub fn retrieve(&self, ino: NodeID, offset: u64, size: u32) -> io::Result<NotifyID> {
        let unique = self.notify_unique.fetch_add(1, Ordering::SeqCst);
        let unique = NotifyID::from_raw(unique);
        self.send(notify::Retrieve::new(unique, ino, offset, size))?;
        Ok(unique)
    }

    pub fn poll_wakeup(&self, kh: PollWakeupID) -> io::Result<()> {
        self.send(notify::PollWakeup::new(kh))
    }
}

/// The context for a single FUSE request used by the filesystem.
pub struct Request<'req, T: 'req> {
    buf: &'req RequestBuffer,
    arg: T,
}

impl<T> Request<'_, T> {
    pub fn uid(&self) -> UID {
        self.buf.uid()
    }

    pub fn gid(&self) -> GID {
        self.buf.gid()
    }

    pub fn pid(&self) -> PID {
        self.buf.pid()
    }

    pub fn arg(&self) -> &T {
        &self.arg
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
            .map_err(|err| match err.raw_os_error() {
                Some(ENOENT) => Error::Cancelled, // missing in processing queue
                _ => Error::Reply(err),
            })?;
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

pub fn run<T>(
    fs: T,
    mountpoint: PathBuf,
    mountopts: MountOptions,
    mut config: KernelConfig,
) -> io::Result<()>
where
    T: Filesystem + Sync,
{
    let span = tracing::debug_span!("polyfuse::fs::run");
    let _enter = span.enter();

    let (conn, fusermount) = crate::mount::mount(mountpoint, mountopts)?;
    let conn = Connection::from(conn);
    let mut session = Session::new();
    session.init(&conn, &mut config)?;

    let num_workers = num_cpus::get();

    let notify_unique = AtomicU64::new(0);

    thread::scope(|spawner| -> io::Result<()> {
        fs.init(Env {
            notifier: Notifier {
                session: &session,
                conn: &conn,
                notify_unique: &notify_unique,
            },
            spawner,
        })?;

        for _i in 0..num_workers {
            let worker = Worker {
                fs: &fs,
                session: &session,
                notifier: Notifier {
                    session: &session,
                    conn: &conn,
                    notify_unique: &notify_unique,
                },
                conn: conn.try_ioc_clone()?,
                buf: if config.flags.contains(KernelFlags::SPLICE_READ) {
                    RequestBuffer::new_splice(config.request_buffer_size())?
                } else {
                    RequestBuffer::new_fallback(config.request_buffer_size())?
                },
            };
            spawner.spawn(move || worker.run(spawner));
        }

        // TODO: on_destroy

        Ok(())
    })?;

    fusermount.unmount()?;

    Ok(())
}

struct Worker<'env, T> {
    fs: &'env T,
    session: &'env Session,
    notifier: Notifier<'env>,
    conn: Connection,
    buf: RequestBuffer,
}

impl<'env, T> Worker<'env, T>
where
    T: Filesystem,
{
    fn run<'scope>(mut self, spawner: Spawner<'scope, 'env>) -> io::Result<()>
    where
        'env: 'scope,
    {
        while self.session.recv_request(&self.conn, &mut self.buf)? {
            self.handle_request(spawner)?;
        }
        Ok(())
    }

    fn req<Arg>(&self, arg: Arg) -> Request<'_, Arg> {
        Request {
            buf: &self.buf,
            arg,
        }
    }

    fn handle_request<'scope>(&mut self, spawner: Spawner<'scope, 'env>) -> io::Result<()>
    where
        'env: 'scope,
    {
        let span = tracing::debug_span!("handle_request", unique = ?self.buf.unique());
        let _enter = span.enter();

        let (op, data) = self
            .buf
            .operation()
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        tracing::debug!(?op);

        let env = Env {
            notifier: self.notifier.clone(),
            spawner,
        };

        let base = ReplyBase {
            session: self.session,
            conn: &self.conn,
            buf: &self.buf,
        };

        let result = match op {
            Operation::Lookup(op) => self.fs.lookup(
                env,
                self.req(op),
                ReplyEntry {
                    base,
                    out: EntryOut::default(),
                },
            ),
            Operation::Getattr(op) => self.fs.getattr(
                env,
                self.req(op),
                ReplyAttr {
                    base,
                    out: AttrOut::default(),
                },
            ),
            Operation::Setattr(op) => self.fs.setattr(
                env,
                self.req(op),
                ReplyAttr {
                    base,
                    out: AttrOut::default(),
                },
            ),
            Operation::Readlink(op) => self.fs.readlink(env, self.req(op), ReplyData { base }),
            Operation::Symlink(op) => self.fs.symlink(
                env,
                self.req(op),
                ReplyEntry {
                    base,
                    out: EntryOut::default(),
                },
            ),
            Operation::Mknod(op) => self.fs.mknod(
                env,
                self.req(op),
                ReplyEntry {
                    base,
                    out: EntryOut::default(),
                },
            ),
            Operation::Mkdir(op) => self.fs.mkdir(
                env,
                self.req(op),
                ReplyEntry {
                    base,
                    out: EntryOut::default(),
                },
            ),
            Operation::Unlink(op) => self.fs.unlink(env, self.req(op), ReplyUnit { base }),
            Operation::Rmdir(op) => self.fs.rmdir(env, self.req(op), ReplyUnit { base }),
            Operation::Rename(op) => self.fs.rename(env, self.req(op), ReplyUnit { base }),
            Operation::Link(op) => self.fs.link(
                env,
                self.req(op),
                ReplyEntry {
                    base,
                    out: EntryOut::default(),
                },
            ),
            Operation::Open(op) => self.fs.open(
                env,
                self.req(op),
                ReplyOpen {
                    base,
                    out: OpenOut::default(),
                },
            ),
            Operation::Read(op) => self.fs.read(env, self.req(op), ReplyData { base }),
            Operation::Release(op) => self.fs.release(env, self.req(op), ReplyUnit { base }),
            Operation::Statfs(op) => self.fs.statfs(env, self.req(op), ReplyStatfs { base }),
            Operation::Fsync(op) => self.fs.fsync(env, self.req(op), ReplyUnit { base }),
            Operation::Setxattr(op) => self.fs.setxattr(env, self.req(op), ReplyUnit { base }),
            Operation::Getxattr(op) => self.fs.getxattr(env, self.req(op), ReplyXattr { base }),
            Operation::Listxattr(op) => self.fs.listxattr(env, self.req(op), ReplyXattr { base }),
            Operation::Removexattr(op) => {
                self.fs.removexattr(env, self.req(op), ReplyUnit { base })
            }
            Operation::Flush(op) => self.fs.flush(env, self.req(op), ReplyUnit { base }),
            Operation::Opendir(op) => self.fs.opendir(
                env,
                self.req(op),
                ReplyOpen {
                    base,
                    out: OpenOut::default(),
                },
            ),
            Operation::Readdir(op) => {
                let capacity = op.size() as usize;
                self.fs.readdir(
                    env,
                    self.req(op),
                    ReplyDir {
                        base,
                        out: ReaddirOut::new(capacity),
                    },
                )
            }
            Operation::Releasedir(op) => self.fs.releasedir(env, self.req(op), ReplyUnit { base }),
            Operation::Fsyncdir(op) => self.fs.fsyncdir(env, self.req(op), ReplyUnit { base }),
            Operation::Getlk(op) => self.fs.getlk(env, self.req(op), ReplyLock { base }),
            Operation::Setlk(op) => self.fs.setlk(env, self.req(op), ReplyUnit { base }),
            Operation::Flock(op) => self.fs.flock(env, self.req(op), ReplyUnit { base }),
            Operation::Access(op) => self.fs.access(env, self.req(op), ReplyUnit { base }),
            Operation::Create(op) => self.fs.create(
                env,
                self.req(op),
                ReplyCreate {
                    base,
                    entry_out: EntryOut::default(),
                    open_out: OpenOut::default(),
                },
            ),
            Operation::Bmap(op) => self.fs.bmap(env, self.req(op), ReplyBmap { base }),
            Operation::Fallocate(op) => self.fs.fallocate(env, self.req(op), ReplyUnit { base }),
            Operation::CopyFileRange(op) => {
                self.fs
                    .copy_file_range(env, self.req(op), ReplyWrite { base })
            }
            Operation::Poll(op) => self.fs.poll(env, self.req(op), ReplyPoll { base }),
            Operation::Lseek(op) => self.fs.lseek(env, self.req(op), ReplyLseek { base }),
            Operation::Write(op) => {
                self.fs
                    .write(env, self.req(op), Data { inner: data }, ReplyWrite { base })
            }
            Operation::NotifyReply(op) => {
                self.fs
                    .notify_reply(env, self.req(op), Data { inner: data })?;
                return Ok(());
            }
            Operation::Forget(forgets) => {
                self.fs.forget(env, forgets.as_ref());
                return Ok(());
            }
            Operation::Interrupt(op) => {
                tracing::warn!("interrupted(unique={})", op.unique());
                // TODO: handle interrupt requests.
                Err(ENOSYS.into())
            }
        };

        match result {
            Ok(..) | Err(Error::Cancelled) => {}
            Err(Error::Reply(err)) => return Err(err),
            Err(Error::Code(errno)) => {
                self.session
                    .send_reply(&self.conn, self.buf.unique(), errno, ())?
            }
        }

        Ok(())
    }
}
