use crate::{
    bytes::Bytes,
    mount::MountOptions,
    op::{self, Forget},
    Connection, KernelConfig, Operation, Session,
};
use libc::{EIO, ENOSYS};
use std::{ffi::OsStr, io, path::PathBuf, thread};

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
        write: Write,
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
    ) -> io::Result<()> {
        Ok(())
    }
}

#[non_exhaustive]
pub struct Context<'scope, 'env: 'scope> {
    pub notifier: Notifier<'env>,
    pub scope: &'scope thread::Scope<'scope, 'env>,
}

#[derive(Clone)]
pub struct Notifier<'env> {
    session: &'env Session,
    conn: &'env Connection,
}

impl Notifier<'_> {
    pub fn inval_inode(&self, ino: u64, off: i64, len: i64) -> io::Result<()> {
        self.session.notify_inval_inode(self.conn, ino, off, len)
    }

    pub fn inval_entry(&self, parent: u64, name: &OsStr) -> io::Result<()> {
        self.session.notify_inval_entry(self.conn, parent, name)
    }

    pub fn delete(&self, parent: u64, child: u64, name: &OsStr) -> io::Result<()> {
        self.session.notify_delete(self.conn, parent, child, name)
    }

    pub fn store<B>(&self, ino: u64, offset: u64, data: B) -> io::Result<()>
    where
        B: Bytes,
    {
        self.session.notify_store(self.conn, ino, offset, data)
    }

    pub fn retrieve(&self, ino: u64, offset: u64, size: u32) -> io::Result<u64> {
        self.session.notify_retrieve(self.conn, ino, offset, size)
    }

    pub fn poll_wakeup(&self, kh: u64) -> io::Result<()> {
        self.session.notify_poll_wakeup(self.conn, kh)
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
    req: &'req crate::session::Request,
    arg: T,
}

impl<T> Request<'_, T> {
    pub fn uid(&self) -> u32 {
        self.req.uid()
    }

    pub fn gid(&self) -> u32 {
        self.req.gid()
    }

    pub fn pid(&self) -> u32 {
        self.req.pid()
    }

    pub fn arg(&self) -> &T {
        &self.arg
    }

    pub fn reply<B>(self, arg: B) -> Result
    where
        B: Bytes,
    {
        self.session
            .reply(self.conn, self.req, arg)
            .map_err(Error::Reply)?;
        Ok(Replied { _private: () })
    }
}

impl io::Read for Request<'_, op::Write<'_>> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.req.read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        self.req.read_vectored(bufs)
    }
}

impl io::Read for Request<'_, op::NotifyReply<'_>> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.req.read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        self.req.read_vectored(bufs)
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

    // MEMO: 'env
    let fs = &fs;
    let session = &session;
    let conn = &conn;

    thread::scope(|scope| -> io::Result<()> {
        fs.init(Context {
            notifier: Notifier { session, conn },
            scope,
        })?;

        for _i in 0..num_cpus::get() {
            let worker_conn = conn.try_ioc_clone()?;
            let mut req = session.new_request_buffer()?;
            scope.spawn(move || -> io::Result<()> {
                while session.read_request(&worker_conn, &mut req)? {
                    let span = tracing::debug_span!("handle_request", unique = req.unique());
                    let _enter = span.enter();

                    let op = req
                        .operation()
                        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
                    tracing::debug!(?op);

                    let cx = Context {
                        notifier: Notifier { session, conn },
                        scope,
                    };

                    macro_rules! req {
                        ($arg:expr) => {
                            Request {
                                session,
                                conn: &worker_conn,
                                req: &req,
                                arg: $arg,
                            }
                        };
                    }

                    let res = match op {
                        Operation::Lookup(op) => fs.lookup(cx, req!(op)),
                        Operation::Getattr(op) => fs.getattr(cx, req!(op)),
                        Operation::Setattr(op) => fs.setattr(cx, req!(op)),
                        Operation::Readlink(op) => fs.readlink(cx, req!(op)),
                        Operation::Symlink(op) => fs.symlink(cx, req!(op)),
                        Operation::Mknod(op) => fs.mknod(cx, req!(op)),
                        Operation::Mkdir(op) => fs.mkdir(cx, req!(op)),
                        Operation::Unlink(op) => fs.unlink(cx, req!(op)),
                        Operation::Rmdir(op) => fs.rmdir(cx, req!(op)),
                        Operation::Rename(op) => fs.rename(cx, req!(op)),
                        Operation::Link(op) => fs.link(cx, req!(op)),
                        Operation::Open(op) => fs.open(cx, req!(op)),
                        Operation::Read(op) => fs.read(cx, req!(op)),
                        Operation::Write(op) => fs.write(cx, req!(op)),
                        Operation::Release(op) => fs.release(cx, req!(op)),
                        Operation::Statfs(op) => fs.statfs(cx, req!(op)),
                        Operation::Fsync(op) => fs.fsync(cx, req!(op)),
                        Operation::Setxattr(op) => fs.setxattr(cx, req!(op)),
                        Operation::Getxattr(op) => fs.getxattr(cx, req!(op)),
                        Operation::Listxattr(op) => fs.listxattr(cx, req!(op)),
                        Operation::Removexattr(op) => fs.removexattr(cx, req!(op)),
                        Operation::Flush(op) => fs.flush(cx, req!(op)),
                        Operation::Opendir(op) => fs.opendir(cx, req!(op)),
                        Operation::Readdir(op) => fs.readdir(cx, req!(op)),
                        Operation::Releasedir(op) => fs.releasedir(cx, req!(op)),
                        Operation::Fsyncdir(op) => fs.fsyncdir(cx, req!(op)),
                        Operation::Getlk(op) => fs.getlk(cx, req!(op)),
                        Operation::Setlk(op) => fs.setlk(cx, req!(op)),
                        Operation::Flock(op) => fs.flock(cx, req!(op)),
                        Operation::Access(op) => fs.access(cx, req!(op)),
                        Operation::Create(op) => fs.create(cx, req!(op)),
                        Operation::Bmap(op) => fs.bmap(cx, req!(op)),
                        Operation::Fallocate(op) => fs.fallocate(cx, req!(op)),
                        Operation::CopyFileRange(op) => fs.copy_file_range(cx, req!(op)),
                        Operation::Poll(op) => fs.poll(cx, req!(op)),
                        Operation::NotifyReply(op) => {
                            fs.notify_reply(cx, req!(op))?;
                            continue;
                        }
                        Operation::Forget(forgets) => {
                            fs.forget(cx, forgets.as_ref());
                            continue;
                        }
                        Operation::Interrupt(op) => {
                            tracing::warn!("interrupt: {:?}", op);
                            continue;
                        }
                    };

                    match res {
                        Err(Error::Reply(err)) => return Err(err),
                        Err(Error::Code(code)) => session.reply_error(&worker_conn, &req, code)?,
                        Ok(..) => (),
                    }
                }
                Ok(())
            });
        }

        // TODO: on_destroy

        Ok(())
    })?;

    fusermount.unmount()?;

    Ok(())
}
