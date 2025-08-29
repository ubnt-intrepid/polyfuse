use crate::{
    bytes::Bytes,
    mount::MountOptions,
    notify,
    op::{self, Forget},
    request::RemainingData,
    Connection, KernelConfig, Operation, RequestBuffer, Session,
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
    ($( $name:ident: $Arg:ident ),*$(,)*) => {$(
        #[allow(unused_variables)]
        fn $name<'env, 'req>(&'env self, cx: Context<'_, 'env>, req: Request<'req, op::$Arg<'req>>) -> Result {
            Err(Error::Code(ENOSYS))
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
    }

    #[allow(unused_variables)]
    fn write<'env, 'req>(
        &'env self,
        cx: Context<'_, 'env>,
        req: Request<'req, op::Write<'req>>,
        data: Data<'req>,
    ) -> Result {
        Err(Error::Code(ENOSYS))
    }

    #[allow(unused_variables)]
    fn forget<'env>(&'env self, cx: Context<'_, 'env>, forgets: &[Forget]) {}

    #[allow(unused_variables)]
    fn init<'env>(&'env self, cx: Context<'_, 'env>) -> io::Result<()> {
        Ok(())
    }

    #[allow(unused_variables)]
    fn notify_reply<'env, 'req>(
        &'env self,
        cx: Context<'_, 'env>,
        req: Request<'req, op::NotifyReply<'req>>,
        data: Data<'req>,
    ) -> io::Result<()> {
        Ok(())
    }
}

pub type Spawner<'scope, 'env> = &'scope thread::Scope<'scope, 'env>;

#[non_exhaustive]
pub struct Context<'scope, 'env: 'scope> {
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

    pub fn inval_inode(&self, ino: u64, off: i64, len: i64) -> io::Result<()> {
        self.send(notify::InvalNode::new(ino, off, len))
    }

    pub fn inval_entry(&self, parent: u64, name: &OsStr) -> io::Result<()> {
        self.send(notify::InvalEntry::new(parent, name))
    }

    pub fn delete(&self, parent: u64, child: u64, name: &OsStr) -> io::Result<()> {
        self.send(notify::Delete::new(parent, child, name))
    }

    pub fn store<B>(&self, ino: u64, offset: u64, data: B) -> io::Result<()>
    where
        B: Bytes,
    {
        self.send(notify::Store::new(ino, offset, data))
    }

    pub fn retrieve(&self, ino: u64, offset: u64, size: u32) -> io::Result<u64> {
        let unique = self.notify_unique.fetch_add(1, Ordering::SeqCst);
        self.send(notify::Retrieve::new(unique, ino, offset, size))?;
        Ok(unique)
    }

    pub fn poll_wakeup(&self, kh: u64) -> io::Result<()> {
        self.send(notify::PollWakeup::new(kh))
    }
}

/// The ZST to ensure that the filesystem responds to a FUSE request exactly once.
#[derive(Debug)]
pub struct Replied {
    _private: (),
}

/// The context for a single FUSE request used by the filesystem.
pub struct Request<'req, T: 'req> {
    session: &'req Session,
    conn: &'req Connection,
    buf: &'req RequestBuffer,
    arg: T,
}

impl<T> Request<'_, T> {
    pub fn uid(&self) -> u32 {
        self.buf.uid()
    }

    pub fn gid(&self) -> u32 {
        self.buf.gid()
    }

    pub fn pid(&self) -> u32 {
        self.buf.pid()
    }

    pub fn arg(&self) -> &T {
        &self.arg
    }

    pub fn reply<B>(self, arg: B) -> Result
    where
        B: Bytes,
    {
        self.session
            .reply(self.conn, self.buf, arg)
            .map_err(|err| match err.raw_os_error() {
                Some(ENOENT) => Error::Cancelled, // missing in processing queue
                _ => Error::Reply(err),
            })?;
        Ok(Replied { _private: () })
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

pub fn run<T>(
    fs: T,
    mountpoint: PathBuf,
    mountopts: MountOptions,
    config: KernelConfig,
) -> io::Result<()>
where
    T: Filesystem + Sync,
{
    let span = tracing::debug_span!("polyfuse::fs::run");
    let _enter = span.enter();

    let (conn, fusermount) = crate::mount::mount(mountpoint, mountopts)?;
    let conn = Connection::from(conn);
    let session = Session::init(&conn, config)?;

    let num_workers = num_cpus::get();

    let notify_unique = AtomicU64::new(0);

    thread::scope(|spawner| -> io::Result<()> {
        fs.init(Context {
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
                buf: session.new_request_buffer()?,
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
        while self.session.read_request(&self.conn, &mut self.buf)? {
            self.handle_request(spawner)?;
        }
        Ok(())
    }

    fn req<Arg>(&self, arg: Arg) -> Request<'_, Arg> {
        Request {
            session: self.session,
            conn: &self.conn,
            buf: &self.buf,
            arg,
        }
    }

    fn handle_request<'scope>(&mut self, spawner: Spawner<'scope, 'env>) -> io::Result<()>
    where
        'env: 'scope,
    {
        let span = tracing::debug_span!("handle_request", unique = self.buf.unique());
        let _enter = span.enter();

        let (op, data) = self
            .buf
            .operation()
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        tracing::debug!(?op);

        let cx = Context {
            notifier: self.notifier.clone(),
            spawner,
        };

        let result = match op {
            Operation::Lookup(op) => self.fs.lookup(cx, self.req(op)),
            Operation::Getattr(op) => self.fs.getattr(cx, self.req(op)),
            Operation::Setattr(op) => self.fs.setattr(cx, self.req(op)),
            Operation::Readlink(op) => self.fs.readlink(cx, self.req(op)),
            Operation::Symlink(op) => self.fs.symlink(cx, self.req(op)),
            Operation::Mknod(op) => self.fs.mknod(cx, self.req(op)),
            Operation::Mkdir(op) => self.fs.mkdir(cx, self.req(op)),
            Operation::Unlink(op) => self.fs.unlink(cx, self.req(op)),
            Operation::Rmdir(op) => self.fs.rmdir(cx, self.req(op)),
            Operation::Rename(op) => self.fs.rename(cx, self.req(op)),
            Operation::Link(op) => self.fs.link(cx, self.req(op)),
            Operation::Open(op) => self.fs.open(cx, self.req(op)),
            Operation::Read(op) => self.fs.read(cx, self.req(op)),
            Operation::Release(op) => self.fs.release(cx, self.req(op)),
            Operation::Statfs(op) => self.fs.statfs(cx, self.req(op)),
            Operation::Fsync(op) => self.fs.fsync(cx, self.req(op)),
            Operation::Setxattr(op) => self.fs.setxattr(cx, self.req(op)),
            Operation::Getxattr(op) => self.fs.getxattr(cx, self.req(op)),
            Operation::Listxattr(op) => self.fs.listxattr(cx, self.req(op)),
            Operation::Removexattr(op) => self.fs.removexattr(cx, self.req(op)),
            Operation::Flush(op) => self.fs.flush(cx, self.req(op)),
            Operation::Opendir(op) => self.fs.opendir(cx, self.req(op)),
            Operation::Readdir(op) => self.fs.readdir(cx, self.req(op)),
            Operation::Releasedir(op) => self.fs.releasedir(cx, self.req(op)),
            Operation::Fsyncdir(op) => self.fs.fsyncdir(cx, self.req(op)),
            Operation::Getlk(op) => self.fs.getlk(cx, self.req(op)),
            Operation::Setlk(op) => self.fs.setlk(cx, self.req(op)),
            Operation::Flock(op) => self.fs.flock(cx, self.req(op)),
            Operation::Access(op) => self.fs.access(cx, self.req(op)),
            Operation::Create(op) => self.fs.create(cx, self.req(op)),
            Operation::Bmap(op) => self.fs.bmap(cx, self.req(op)),
            Operation::Fallocate(op) => self.fs.fallocate(cx, self.req(op)),
            Operation::CopyFileRange(op) => self.fs.copy_file_range(cx, self.req(op)),
            Operation::Poll(op) => self.fs.poll(cx, self.req(op)),
            Operation::Write(op) => self.fs.write(cx, self.req(op), Data { inner: data }),
            Operation::NotifyReply(op) => {
                self.fs
                    .notify_reply(cx, self.req(op), Data { inner: data })?;
                return Ok(());
            }
            Operation::Forget(forgets) => {
                self.fs.forget(cx, forgets.as_ref());
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
            Err(Error::Code(code)) => self.session.reply_error(&self.conn, &self.buf, code)?,
        }

        Ok(())
    }
}
