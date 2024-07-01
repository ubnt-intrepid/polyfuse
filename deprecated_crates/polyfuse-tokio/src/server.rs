//! Serve FUSE filesystem.

use crate::{
    buf::RequestBuffer,
    channel::Channel,
    pool::{Pool, PoolEntry},
};
use futures::{
    future::{Future, FutureExt},
    io::AsyncReadExt,
    select,
};
use libc::c_int;
use polyfuse::{io::unite, Filesystem, Session, SessionInitializer};
use std::{ffi::OsStr, io, path::Path, sync::Arc};
use tokio::signal::unix::{signal, SignalKind};

const DEFAULT_POOL_SIZE: usize = 1024;

#[derive(Debug, Clone)]
struct Config {
    pool_size: usize,
    initial_pooled_contexts: Option<usize>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pool_size: DEFAULT_POOL_SIZE,
            initial_pooled_contexts: None,
        }
    }
}

/// A builder for `Server`.
#[derive(Debug, Default)]
pub struct Builder {
    initializer: SessionInitializer,
    config: Config,
}

impl Builder {
    /// Return a reference to the session initializer.
    #[inline]
    pub fn session(&mut self) -> &mut SessionInitializer {
        &mut self.initializer
    }

    /// Set the pool size of request contexts.
    ///
    /// The default value is `1024`.
    ///
    /// # Panic
    /// The provided value is less than `initial_pooled_contexts`.
    pub fn pool_size(&mut self, amt: usize) -> &mut Self {
        assert!(
            self.config.initial_pooled_contexts.unwrap_or(0) <= amt,
            "the provided pool size is less than initial_pooled_contexts"
        );
        self.config.pool_size = amt;
        self
    }

    /// Set the number of request contexts stored in the pool before the start of the main loop.
    ///
    /// By default, the pool of request contexts does not contain any entries.
    ///
    /// # Panic
    /// The provided value is greater than `pool_size`.
    pub fn initial_pooled_contexts(&mut self, amt: usize) -> &mut Self {
        assert!(
            amt <= self.config.pool_size,
            "the provided initial_pooled_contexts is greater than pool_size"
        );
        self.config.initial_pooled_contexts = Some(amt);
        self
    }

    /// Build a `Server` mounted on the specified path.
    pub async fn mount<'a>(
        &'a self,
        mountpoint: impl AsRef<Path>,
        mountopts: &'a [&'a OsStr],
    ) -> io::Result<Server> {
        // FIXME: make async.
        let mut channel = Channel::open(
            mountpoint.as_ref(), //
            mountopts,
        )?;

        let mut buf = vec![0u8; self.initializer.init_buf_size()];
        let session = loop {
            channel.read(&mut buf[..]).await?;
            match self
                .initializer
                .try_init(&mut unite(&mut &buf[..], &mut channel))
                .await?
            {
                Some(session) => break session,
                None => continue,
            }
        };

        Ok(Server {
            config: Arc::new(self.config.clone()),
            session: Arc::new(session),
            channel,
        })
    }
}

/// A FUSE filesystem server running on Tokio runtime.
#[derive(Debug)]
pub struct Server {
    config: Arc<Config>,
    session: Arc<Session>,
    channel: Channel,
}

impl Server {
    /// Create a FUSE server mounted on the specified path.
    pub async fn mount<'a>(
        mountpoint: impl AsRef<Path>,
        mountopts: &'a [&'a OsStr],
    ) -> io::Result<Self> {
        Builder::default() //
            .mount(mountpoint, mountopts)
            .await
    }

    /// Attempt to make a clone of this instance.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            session: self.session.clone(),
            channel: self.channel.try_clone()?,
            config: self.config.clone(),
        })
    }

    /// Run a FUSE filesystem daemon.
    pub async fn run<F>(&mut self, fs: F) -> io::Result<()>
    where
        F: Filesystem + Send + 'static,
    {
        let sig = default_shutdown_signal()?;
        let _sig = self.run_until(fs, sig).await?;
        Ok(())
    }

    fn new_request_context<F>(&self, fs: &Arc<F>) -> io::Result<ProcessRequest<F>> {
        let writer = self.channel.try_clone()?;
        Ok(ProcessRequest {
            fs: fs.clone(),
            session: self.session.clone(),
            read_buf: RequestBuffer::new(self.session.buffer_size()),
            writer,
        })
    }

    async fn main_loop<F>(&mut self, fs: &Arc<F>) -> io::Result<()>
    where
        F: Filesystem + Send + 'static,
    {
        // Create a object pool for sharing the request contexts.
        let mut pool = Pool::new(self.config.pool_size);

        // Prepare the request contexts and store them into the pool.
        if let Some(amt) = self.config.initial_pooled_contexts {
            for _ in 0..amt {
                if let Some(entry) = pool.vacant_entry() {
                    entry.insert(self.new_request_context(&fs)?);
                } else {
                    break;
                }
            }
        }

        loop {
            // Acquire a value of request context from the pool.
            let mut cx = pool.acquire(|| self.new_request_context(&fs)).await?;

            // Transfer a FUSE request to the request context.
            match cx.receive_from(&mut self.channel).await {
                Ok(true) => return Ok(()),
                Ok(false) => (),
                Err(err) => return Err(err),
            }

            // spawn a task that handles a FUSE request.
            tokio::spawn(async move {
                if let Err(e) = cx.process().await {
                    tracing::error!("error during handling a request: {}", e);
                }
                PoolEntry::release(cx).await;
            });
        }
    }

    /// Run a FUSE filesystem until the specified signal is received.
    #[allow(clippy::unnecessary_mut_passed)]
    pub async fn run_until<F, S>(&mut self, fs: F, sig: S) -> io::Result<Option<S::Output>>
    where
        F: Filesystem + Send + 'static,
        S: Future + Unpin,
    {
        // TODO: graceful shutdown.
        let fs = Arc::new(fs);
        let mut main_loop = Box::pin(self.main_loop(&fs)).fuse();
        let mut sig = sig.fuse();
        select! {
            _ = main_loop => Ok(None),
            sig = sig => Ok(Some(sig)),
        }
    }

    /// Run a FUSE filesystem without spawning the new task for each request.
    pub async fn run_single<'a, F>(&'a mut self, fs: &'a F) -> io::Result<()>
    where
        F: Filesystem,
    {
        let mut buf = RequestBuffer::new(self.session.buffer_size());
        loop {
            match buf.receive_from(&mut self.channel).await {
                Ok(true) => return Ok(()),
                Ok(false) => (),
                Err(err) => return Err(err),
            }
            self.session
                .process(&*fs, &mut unite(&mut buf, &mut self.channel))
                .await?;
        }
    }

    /// Invalidate the specified range of cache data for an inode.
    ///
    /// When the kernel receives this notification, some requests are queued to read
    /// the updated data.
    pub async fn notify_inval_inode(&mut self, ino: u64, off: i64, len: i64) -> io::Result<()> {
        self.session
            .notify_inval_inode(&mut self.channel, ino, off, len)
            .await
    }

    /// Invalidate an entry with the specified name in the directory.
    pub async fn notify_inval_entry(
        &mut self,
        parent: u64,
        name: impl AsRef<OsStr>,
    ) -> io::Result<()> {
        self.session
            .notify_inval_entry(&mut self.channel, parent, name)
            .await
    }

    /// Notify that an entry with the specified name has been deleted from the directory.
    pub async fn notify_delete(
        &mut self,
        parent: u64,
        child: u64,
        name: impl AsRef<OsStr>,
    ) -> io::Result<()> {
        self.session
            .notify_delete(&mut self.channel, parent, child, name)
            .await
    }

    /// Replace the specified range of cache data with a new value.
    pub async fn notify_store(&mut self, ino: u64, offset: u64, data: &[&[u8]]) -> io::Result<()> {
        self.session
            .notify_store(&mut self.channel, ino, offset, data)
            .await
    }

    /// Retrieve the value of the cache data with the specified range.
    pub async fn notify_retrieve(&mut self, ino: u64, offset: u64, size: u32) -> io::Result<u64> {
        self.session
            .notify_retrieve(&mut self.channel, ino, offset, size)
            .await
    }

    /// Notify an I/O readiness.
    pub async fn notify_poll_wakeup(&mut self, kh: u64) -> io::Result<()> {
        self.session.notify_poll_wakeup(&mut self.channel, kh).await
    }
}

struct ProcessRequest<F> {
    fs: Arc<F>,
    session: Arc<Session>,
    read_buf: RequestBuffer,
    writer: Channel,
}

impl<F> ProcessRequest<F>
where
    F: Filesystem,
{
    async fn receive_from(&mut self, channel: &mut Channel) -> io::Result<bool> {
        self.read_buf.receive_from(channel).await
    }

    async fn process(&mut self) -> io::Result<()> {
        self.session
            .process(&*self.fs, &mut unite(&mut self.read_buf, &mut self.writer))
            .await
    }
}

#[allow(clippy::unnecessary_mut_passed)]
fn default_shutdown_signal() -> io::Result<impl Future<Output = c_int> + Unpin> {
    let mut sighup = signal(SignalKind::hangup())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigpipe = signal(SignalKind::pipe())?;

    Ok(Box::pin(async move {
        // TODO: use stabilized API.
        let mut sighup = Box::pin(sighup.recv()).fuse();
        let mut sigint = Box::pin(sigint.recv()).fuse();
        let mut sigterm = Box::pin(sigterm.recv()).fuse();
        let mut sigpipe = Box::pin(sigpipe.recv()).fuse();

        loop {
            select! {
                _ = sighup => {
                    tracing::debug!("Got SIGHUP");
                    return libc::SIGHUP;
                },
                _ = sigint => {
                    tracing::debug!("Got SIGINT");
                    return libc::SIGINT;
                },
                _ = sigterm => {
                    tracing::debug!("Got SIGTERM");
                    return libc::SIGTERM;
                },
                _ = sigpipe => {
                    tracing::debug!("Got SIGPIPE (and ignored)");
                    continue
                }
            }
        }
    }))
}
