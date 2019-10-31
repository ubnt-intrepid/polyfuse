//! Serve FUSE filesystem.

use crate::{
    conn::Connection,
    session::{Buffer, Filesystem, Session},
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
    cell::UnsafeCell,
    io::{self, IoSlice, IoSliceMut, Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    task::{self, Poll},
};
use tokio::{
    net::util::PollEvented,
    signal::unix::{signal, SignalKind},
    sync::semaphore::{Permit, Semaphore},
};

pub use crate::conn::MountOptions;

/// FUSE filesystem server.
#[derive(Debug)]
pub struct Server {
    io: Channel,
}

impl Server {
    /// Create a FUSE server mounted on the specified path.
    pub fn mount(mointpoint: impl AsRef<Path>, mountopts: MountOptions) -> io::Result<Self> {
        let io = Channel::open(mointpoint, mountopts)?;
        Ok(Server { io })
    }

    /// Run a FUSE filesystem.
    pub async fn run<F>(self, fs: F) -> io::Result<()>
    where
        F: for<'a> Filesystem<&'a [u8]> + 'static,
    {
        let sig = default_shutdown_signal()?;
        let _sig = self.run_until(fs, sig).await?;
        Ok(())
    }

    /// Run a FUSE filesystem until the specified signal is received.
    pub async fn run_until<F, S>(self, fs: F, sig: S) -> io::Result<Option<S::Output>>
    where
        F: for<'a> Filesystem<&'a [u8]> + 'static,
        S: Future + Unpin,
    {
        let mut io = self.io;
        let mut sig = sig.fuse();
        let fs = Arc::new(fs);

        let session = Session::start(&mut io, Default::default())
            .await
            .map(Arc::new)?;

        let mut main_loop = Box::pin(main_loop(&session, &mut io, &fs)).fuse();

        // FIXME: graceful shutdown the background tasks.
        select! {
            _ = main_loop => Ok(None),
            sig = sig => Ok(Some(sig)),
        }
    }
}

async fn main_loop<I, F>(session: &Arc<Session>, channel: &mut I, fs: &Arc<F>) -> io::Result<()>
where
    F: for<'a> Filesystem<&'a [u8]> + 'static,
    I: AsyncRead + AsyncWrite + Send + Unpin + Clone + 'static,
{
    loop {
        let mut buf = Buffer::default();
        let terminated = buf.receive(&mut *channel).await?;
        if terminated {
            log::debug!("connection was closed by the kernel");
            return Ok(());
        }

        let session = Arc::clone(session);
        let fs = Arc::clone(fs);
        let mut channel = channel.clone();

        tokio::spawn(async move {
            let (req, data) = match buf.decode() {
                Ok(t) => t,
                Err(err) => {
                    log::error!("failed to decode a request: {}", err);
                    return;
                }
            };
            log::debug!(
                "Got a request: unique={}, opcode={:?}, data={:?}",
                req.unique(),
                req.opcode(),
                data.as_ref().map(|_| "<data>")
            );

            if let Err(err) = session.process(&*fs, req, data, &mut channel).await {
                log::error!("error during processing a request: {}", err);
            }
        });
    }
}

/// Asynchronous I/O object that communicates with the FUSE kernel driver.
#[derive(Debug, Clone)]
pub struct Channel {
    reader: Lock<PollEvented<Connection>>,
    writer: Lock<PollEvented<Connection>>,
    mountpoint: Arc<PathBuf>,
}

unsafe impl Send for Channel {}

impl Channel {
    /// Open a new communication channel mounted to the specified path.
    pub fn open(mountpoint: impl AsRef<Path>, mountopts: MountOptions) -> io::Result<Self> {
        let mountpoint = mountpoint.as_ref();

        let reader = Connection::open(mountpoint, mountopts)?;
        let writer = reader.duplicate(false)?;

        Ok(Self {
            reader: Lock::new(PollEvented::new(reader)?),
            writer: Lock::new(PollEvented::new(writer)?),
            mountpoint: Arc::new(mountpoint.into()),
        })
    }

    fn poll_read_with<F, R>(&mut self, cx: &mut task::Context<'_>, f: F) -> Poll<io::Result<R>>
    where
        F: FnOnce(&mut Connection) -> io::Result<R>,
    {
        self.reader.poll_lock_with(cx, |cx, conn| {
            let mut ready = Ready::readable();
            ready.insert(UnixReady::error());
            ready!(conn.poll_read_ready(cx, ready))?;

            match f(conn.get_mut()) {
                Ok(ret) => Poll::Ready(Ok(ret)),
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    conn.clear_read_ready(cx, ready)?;
                    Poll::Pending
                }
                Err(e) => Poll::Ready(Err(e)),
            }
        })
    }

    fn poll_write_with<F, R>(&mut self, cx: &mut task::Context<'_>, f: F) -> Poll<io::Result<R>>
    where
        F: FnOnce(&mut Connection) -> io::Result<R>,
    {
        self.writer.poll_lock_with(cx, |cx, conn| {
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

impl AsyncRead for Channel {
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

impl AsyncWrite for Channel {
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

#[derive(Debug)]
struct Lock<T> {
    inner: Arc<LockInner<T>>,
    permit: Permit,
}

#[derive(Debug)]
struct LockInner<T> {
    val: UnsafeCell<T>,
    semaphore: Semaphore,
}

impl<T> Clone for Lock<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            permit: Permit::new(),
        }
    }
}

impl<T> Drop for Lock<T> {
    fn drop(&mut self) {
        self.release_lock();
    }
}

impl<T> Lock<T> {
    fn new(val: T) -> Self {
        Self {
            inner: Arc::new(LockInner {
                val: UnsafeCell::new(val),
                semaphore: Semaphore::new(1),
            }),
            permit: Permit::new(),
        }
    }

    fn poll_lock_with<F, R>(&mut self, cx: &mut task::Context, f: F) -> Poll<R>
    where
        F: FnOnce(&mut task::Context, &mut T) -> Poll<R>,
    {
        ready!(self.poll_acquire_lock(cx));

        let val = unsafe { &mut (*self.inner.val.get()) };
        let ret = ready!(f(cx, val));

        self.release_lock();

        Poll::Ready(ret)
    }

    fn poll_acquire_lock(&mut self, cx: &mut task::Context) -> Poll<()> {
        ready!(self.permit.poll_acquire(cx, &self.inner.semaphore))
            .unwrap_or_else(|e| unreachable!("{}", e));
        Poll::Ready(())
    }

    fn release_lock(&mut self) {
        self.permit.release(&self.inner.semaphore);
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
