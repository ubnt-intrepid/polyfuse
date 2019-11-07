//! Serve FUSE filesystem.

use crate::{
    io::{unite, Connection, MountOptions},
    lock::Lock,
    session::{Filesystem, NotifyRetrieve, Session},
};
use futures::{
    future::{Future, FutureExt},
    io::{AsyncRead, AsyncWrite},
    ready, select,
    stream::StreamExt,
};
use libc::c_int;
use mio::{unix::UnixReady, Ready};
use std::{
    ffi::OsStr,
    io::{self, IoSlice, IoSliceMut, Read, Write},
    path::Path,
    pin::Pin,
    sync::Arc,
    task::{self, Poll},
};
use tokio::{
    net::util::PollEvented,
    signal::unix::{signal, SignalKind},
};

/// FUSE filesystem server.
#[derive(Debug)]
pub struct Server {
    session: Arc<Session>,
    reader: Reader,
    writer: Writer,
}

impl Server {
    /// Create a FUSE server mounted on the specified path.
    pub async fn mount(mointpoint: impl AsRef<Path>, mountopts: MountOptions) -> io::Result<Self> {
        let (mut reader, mut writer) = channel(mointpoint, mountopts, false)?;
        let session = Session::start(
            &mut unite(&mut reader, &mut writer), //
            Default::default(),
        )
        .await?;
        Ok(Server {
            session: Arc::new(session),
            reader,
            writer,
        })
    }

    pub fn notifier(&self) -> Notifier {
        Notifier {
            session: self.session.clone(),
            writer: self.writer.clone(),
        }
    }

    /// Run a FUSE filesystem.
    pub async fn run<F>(self, fs: F) -> io::Result<()>
    where
        F: Filesystem + 'static,
    {
        let sig = default_shutdown_signal()?;
        let _sig = self.run_until(fs, sig).await?;
        Ok(())
    }

    /// Run a FUSE filesystem until the specified signal is received.
    pub async fn run_until<F, S>(self, fs: F, sig: S) -> io::Result<Option<S::Output>>
    where
        F: Filesystem + 'static,
        S: Future + Unpin,
    {
        let session = self.session;
        let fs = Arc::new(fs);
        let mut reader = self.reader;
        let writer = self.writer;
        let mut sig = sig.fuse();

        let mut main_loop = Box::pin(async move {
            loop {
                let req = match session.receive(&mut reader).await? {
                    Some(req) => req,
                    None => {
                        log::debug!("connection was closed by the kernel");
                        return Ok::<_, io::Error>(());
                    }
                };

                let session = session.clone();
                let fs = fs.clone();
                let mut writer = writer.clone();
                tokio::spawn(async move {
                    if let Err(e) = session.process(&*fs, req, &mut writer).await {
                        log::error!("error during handling a request: {}", e);
                    }
                });
            }
        })
        .fuse();

        // FIXME: graceful shutdown the background tasks.
        select! {
            _ = main_loop => Ok(None),
            sig = sig => Ok(Some(sig)),
        }
    }
}

/// Notification sender to the kernel.
#[derive(Debug, Clone)]
pub struct Notifier {
    session: Arc<Session>,
    writer: Writer,
}

impl Notifier {
    pub async fn inval_inode(&mut self, ino: u64, off: i64, len: i64) -> io::Result<()> {
        self.session
            .notify_inval_inode(&mut self.writer, ino, off, len)
            .await
    }

    pub async fn inval_entry(&mut self, parent: u64, name: impl AsRef<OsStr>) -> io::Result<()> {
        self.session
            .notify_inval_entry(&mut self.writer, parent, name)
            .await
    }

    pub async fn delete(
        &mut self,
        parent: u64,
        child: u64,
        name: impl AsRef<OsStr>,
    ) -> io::Result<()> {
        self.session
            .notify_delete(&mut self.writer, parent, child, name)
            .await
    }

    pub async fn store(&mut self, ino: u64, offset: u64, data: &[&[u8]]) -> io::Result<()> {
        self.session
            .notify_store(&mut self.writer, ino, offset, data)
            .await
    }

    pub async fn retrieve(
        &mut self,
        ino: u64,
        offset: u64,
        size: u32,
    ) -> io::Result<NotifyRetrieve> {
        self.session
            .notify_retrieve(&mut self.writer, ino, offset, size)
            .await
    }
}

fn channel(
    mountpoint: impl AsRef<Path>,
    mountopts: MountOptions,
    ioc_clone: bool,
) -> io::Result<(Reader, Writer)> {
    let mountpoint = mountpoint.as_ref();

    let reader = Connection::open(mountpoint, mountopts)?;
    let writer = reader.try_clone(ioc_clone)?;

    Ok((
        Reader(PollEvented::new(reader)?),
        Writer(Lock::new(PollEvented::new(writer)?)),
    ))
}

#[derive(Debug)]
struct Reader(PollEvented<Connection>);

impl Reader {
    fn poll_read_with<F, R>(&mut self, cx: &mut task::Context<'_>, f: F) -> Poll<io::Result<R>>
    where
        F: FnOnce(&mut Connection) -> io::Result<R>,
    {
        let mut ready = Ready::readable();
        ready.insert(UnixReady::error());
        ready!(self.0.poll_read_ready(cx, ready))?;

        match f(self.0.get_mut()) {
            Ok(ret) => Poll::Ready(Ok(ret)),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.0.clear_read_ready(cx, ready)?;
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl AsyncRead for Reader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_with(cx, |fd| fd.read(dst))
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [IoSliceMut],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_with(cx, |fd| fd.read_vectored(dst))
    }
}

#[derive(Debug, Clone)]
struct Writer(Lock<PollEvented<Connection>>);

impl Writer {
    fn poll_write_with<F, R>(&mut self, cx: &mut task::Context<'_>, f: F) -> Poll<io::Result<R>>
    where
        F: FnOnce(&mut Connection) -> io::Result<R>,
    {
        self.0.poll_lock_with(cx, |cx, conn| {
            ready!(conn.poll_write_ready(cx))?;

            match f(conn.get_mut()) {
                Ok(ret) => Poll::Ready(Ok(ret)),
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    conn.clear_write_ready(cx)?;
                    Poll::Pending
                }
                Err(e) => {
                    log::debug!("write error: {}", e);
                    Poll::Ready(Err(e))
                }
            }
        })
    }
}

impl AsyncWrite for Writer {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_with(cx, |fd| fd.write(src))
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[IoSlice],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_with(cx, |fd| fd.write_vectored(src))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        self.poll_write_with(cx, |fd| fd.flush())
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn default_shutdown_signal() -> io::Result<impl Future<Output = c_int> + Unpin> {
    let mut sighup = signal(SignalKind::hangup())?.into_future();
    let mut sigint = signal(SignalKind::interrupt())?.into_future();
    let mut sigterm = signal(SignalKind::terminate())?.into_future();
    let mut sigpipe = signal(SignalKind::pipe())?.into_future();

    Ok(Box::pin(async move {
        loop {
            select! {
                _ = sighup => {
                    log::debug!("Got SIGHUP");
                    return libc::SIGHUP;
                },
                _ = sigint => {
                    log::debug!("Got SIGINT");
                    return libc::SIGINT;
                },
                _ = sigterm => {
                    log::debug!("Got SIGTERM");
                    return libc::SIGTERM;
                },
                _ = sigpipe => {
                    log::debug!("Got SIGPIPE (and ignored)");
                    continue
                }
            }
        }
    }))
}
