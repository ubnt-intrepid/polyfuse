use crate::{
    bytes::Bytes,
    mount::MountOptions,
    op::{self, Forget},
    Connection, KernelConfig, Operation, Request, Session,
};
use libc::ENOSYS;
use std::{io, path::PathBuf, thread};

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
        Self::Code(err.raw_os_error().unwrap_or(libc::EIO))
    }
}

macro_rules! define_ops {
    ($( $name:ident: $Arg:ident ),*$(,)*) => {$(
        #[allow(unused_variables)]
        fn $name(&self, cx: Context<'_>, op: op::$Arg<'_>) -> Result {
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
    fn forget(&self, forgets: &[Forget]) {}
}

#[derive(Debug)]
pub struct Replied {
    _private: (),
}

pub struct Context<'req> {
    session: &'req Session,
    conn: &'req Connection,
    req: &'req Request,
}

impl Context<'_> {
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

impl io::Read for Context<'_> {
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

    thread::scope(|scope| -> io::Result<()> {
        // TODO: on_init
        for _i in 0..num_cpus::get() {
            let conn = conn.try_ioc_clone()?;
            let mut req = session.new_request_buffer()?;
            scope.spawn(move || -> io::Result<()> {
                while session.read_request(&conn, &mut req)? {
                    let span = tracing::debug_span!("handle_request", unique = req.unique());
                    let _enter = span.enter();

                    let op = req
                        .operation()
                        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
                    tracing::debug!(?op);

                    let cx = Context {
                        session: &session,
                        conn: &conn,
                        req: &req,
                    };

                    let res = match op {
                        Operation::Lookup(op) => fs.lookup(cx, op),
                        Operation::Getattr(op) => fs.getattr(cx, op),
                        Operation::Setattr(op) => fs.setattr(cx, op),
                        Operation::Readlink(op) => fs.readlink(cx, op),
                        Operation::Symlink(op) => fs.symlink(cx, op),
                        Operation::Mknod(op) => fs.mknod(cx, op),
                        Operation::Mkdir(op) => fs.mkdir(cx, op),
                        Operation::Unlink(op) => fs.unlink(cx, op),
                        Operation::Rmdir(op) => fs.rmdir(cx, op),
                        Operation::Rename(op) => fs.rename(cx, op),
                        Operation::Link(op) => fs.link(cx, op),
                        Operation::Open(op) => fs.open(cx, op),
                        Operation::Read(op) => fs.read(cx, op),
                        Operation::Write(op) => fs.write(cx, op),
                        Operation::Release(op) => fs.release(cx, op),
                        Operation::Statfs(op) => fs.statfs(cx, op),
                        Operation::Fsync(op) => fs.fsync(cx, op),
                        Operation::Setxattr(op) => fs.setxattr(cx, op),
                        Operation::Getxattr(op) => fs.getxattr(cx, op),
                        Operation::Listxattr(op) => fs.listxattr(cx, op),
                        Operation::Removexattr(op) => fs.removexattr(cx, op),
                        Operation::Flush(op) => fs.flush(cx, op),
                        Operation::Opendir(op) => fs.opendir(cx, op),
                        Operation::Readdir(op) => fs.readdir(cx, op),
                        Operation::Releasedir(op) => fs.releasedir(cx, op),
                        Operation::Fsyncdir(op) => fs.fsyncdir(cx, op),
                        Operation::Getlk(op) => fs.getlk(cx, op),
                        Operation::Setlk(op) => fs.setlk(cx, op),
                        Operation::Flock(op) => fs.flock(cx, op),
                        Operation::Access(op) => fs.access(cx, op),
                        Operation::Create(op) => fs.create(cx, op),
                        Operation::Bmap(op) => fs.bmap(cx, op),
                        Operation::Fallocate(op) => fs.fallocate(cx, op),
                        Operation::CopyFileRange(op) => fs.copy_file_range(cx, op),
                        Operation::Poll(op) => fs.poll(cx, op),
                        Operation::Forget(forgets) => {
                            fs.forget(forgets.as_ref());
                            continue;
                        }
                        Operation::Interrupt(op) => {
                            tracing::warn!("interrupt: {:?}", op);
                            continue;
                        }
                        Operation::NotifyReply(op) => {
                            tracing::warn!("notify_reply: {:?}", op);
                            continue;
                        }
                    };

                    match res {
                        Err(Error::Reply(err)) => return Err(err),
                        Err(Error::Code(code)) => session.reply_error(&conn, &req, code)?,
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
