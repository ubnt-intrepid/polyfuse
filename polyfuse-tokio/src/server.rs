//! Serve FUSE filesystem.

use crate::{
    channel::{Channel, SharedWriter},
    mount::MountOptions,
};
use bytes::Bytes;
use futures::{
    future::{FusedFuture, Future, FutureExt},
    select,
    stream::StreamExt,
    task::{self, Poll},
};
use libc::c_int;
use polyfuse::{request::BytesBuffer, Filesystem, Session, SessionInitializer};
use std::{ffi::OsStr, io, path::Path, pin::Pin, sync::Arc};
use tokio::signal::unix::{signal, SignalKind};

/// FUSE filesystem server.
#[derive(Debug)]
pub struct Server {
    session: Arc<Session>,
    notifier: Arc<polyfuse::Notifier<Bytes>>,
    channel: Channel,
    notify_writer: Option<Arc<SharedWriter>>,
}

impl Server {
    /// Create a FUSE server mounted on the specified path.
    pub async fn mount(mountpoint: impl AsRef<Path>, mountopts: MountOptions) -> io::Result<Self> {
        let mut channel = Channel::open(mountpoint.as_ref(), &mountopts)?;
        let session = SessionInitializer::default() //
            .init(&mut channel)
            .await?;
        Ok(Server {
            session: Arc::new(session),
            notifier: Arc::new(polyfuse::Notifier::new()),
            channel,
            notify_writer: None,
        })
    }

    /// Create an instance of `Notifier` associated with this server.
    pub fn notifier(&mut self) -> io::Result<Notifier> {
        let writer = match self.notify_writer {
            Some(ref writer) => writer,
            None => {
                let writer = self.channel.shared_writer(false)?;
                self.notify_writer.get_or_insert(Arc::new(writer))
            }
        };

        Ok(Notifier {
            session: self.session.clone(),
            notifier: self.notifier.clone(),
            writer: writer.clone(),
        })
    }

    /// Run a FUSE filesystem daemon.
    pub async fn run<F>(self, fs: F) -> io::Result<()>
    where
        F: Filesystem<Bytes> + Send + 'static,
    {
        let sig = default_shutdown_signal()?;
        let _sig = self.run_until(fs, sig).await?;
        Ok(())
    }

    /// Run a FUSE filesystem until the specified signal is received.
    #[allow(clippy::unnecessary_mut_passed)]
    pub async fn run_until<F, S>(self, fs: F, sig: S) -> io::Result<Option<S::Output>>
    where
        F: Filesystem<Bytes> + Send + 'static,
        S: Future + Unpin,
    {
        let session = self.session;
        let notifier = self.notifier;
        let fs = Arc::new(fs);
        let mut channel = self.channel;
        let writer = Arc::new(channel.shared_writer(false)?);
        let mut sig = sig.fuse();

        let mut main_loop = Box::pin(async move {
            let mut req = BytesBuffer::new(session.buffer_size());
            loop {
                if let Err(err) = session.receive(&mut channel, &mut req, &notifier).await {
                    match err.raw_os_error() {
                        Some(libc::ENODEV) => {
                            log::debug!("connection was closed by the kernel");
                            return Ok(());
                        }
                        _ => return Err(err),
                    }
                }

                let session = session.clone();
                let fs = fs.clone();
                let writer = writer.clone();
                let mut req = std::mem::replace(&mut req, BytesBuffer::new(session.buffer_size()));
                tokio::spawn(async move {
                    if let Err(e) = session.process(&*fs, &mut req, &*writer).await {
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
    notifier: Arc<polyfuse::Notifier<Bytes>>,
    writer: Arc<SharedWriter>,
}

impl Notifier {
    pub async fn inval_inode(&self, ino: u64, off: i64, len: i64) -> io::Result<()> {
        self.notifier
            .inval_inode(&*self.writer, &*self.session, ino, off, len)
            .await
    }

    pub async fn inval_entry(&self, parent: u64, name: impl AsRef<OsStr>) -> io::Result<()> {
        self.notifier
            .inval_entry(&*self.writer, &*self.session, parent, name)
            .await
    }

    pub async fn delete(&self, parent: u64, child: u64, name: impl AsRef<OsStr>) -> io::Result<()> {
        self.notifier
            .delete(&*self.writer, &*self.session, parent, child, name)
            .await
    }

    pub async fn store(&self, ino: u64, offset: u64, data: &[&[u8]]) -> io::Result<()> {
        self.notifier
            .store(&*self.writer, &*self.session, ino, offset, data)
            .await
    }

    pub async fn retrieve(&self, ino: u64, offset: u64, size: u32) -> io::Result<RetrieveHandle> {
        self.notifier
            .retrieve(&*self.writer, &*self.session, ino, offset, size)
            .await
            .map(RetrieveHandle)
    }

    pub async fn poll_wakeup(&self, kh: u64) -> io::Result<()> {
        self.notifier
            .poll_wakeup(&*self.writer, &*self.session, kh)
            .await
    }
}

/// A handle for awaiting the result of a `retrieve` notification.
#[derive(Debug)]
pub struct RetrieveHandle(polyfuse::notify::RetrieveHandle<Bytes>);

impl Future for RetrieveHandle {
    type Output = (u64, Bytes);

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context) -> Poll<Self::Output> {
        Pin::new(&mut self.get_mut().0).poll(cx)
    }
}

impl FusedFuture for RetrieveHandle {
    fn is_terminated(&self) -> bool {
        self.0.is_terminated()
    }
}

#[allow(clippy::unnecessary_mut_passed)]
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
