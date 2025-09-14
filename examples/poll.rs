use polyfuse::{
    fs::{
        self,
        reply::{self, ReplyAttr, ReplyData, ReplyOpen, ReplyPoll, ReplyUnit},
        Daemon, Filesystem,
    },
    notify,
    op::{self, AccessMode, OpenFlags},
    types::{
        FileAttr, FileID, FileMode, FilePermissions, FileType, NodeID, PollEvents, PollWakeupID,
        GID, UID,
    },
};

use anyhow::{ensure, Context as _, Result};
use libc::{EACCES, EAGAIN, EINVAL};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};
use tokio::sync::RwLock;

const CONTENT: &str = "Hello, world!\n";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let wakeup_interval = Duration::from_secs(
        args //
            .opt_value_from_str("--interval")?
            .unwrap_or(5),
    );

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    let daemon = Daemon::mount(mountpoint, Default::default(), Default::default()).await?;
    daemon
        .run(Arc::new(PollFS::new(wakeup_interval)), None)
        .await?;

    Ok(())
}

struct PollFS {
    handles: RwLock<HashMap<FileID, Arc<FileHandle>>>,
    next_fh: AtomicU64,
    wakeup_interval: Duration,
}

impl PollFS {
    fn new(wakeup_interval: Duration) -> Self {
        Self {
            handles: RwLock::new(HashMap::new()),
            next_fh: AtomicU64::new(0),
            wakeup_interval,
        }
    }
}

impl Filesystem for PollFS {
    async fn getattr(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        _: op::Getattr<'_>,
        mut reply: ReplyAttr<'_>,
    ) -> reply::Result {
        reply.out().attr({
            let mut attr = FileAttr::new();
            attr.ino = NodeID::ROOT;
            attr.nlink = 1;
            attr.mode = FileMode::new(FileType::Regular, FilePermissions::READ);
            attr.uid = UID::current();
            attr.gid = GID::current();
            attr
        });
        reply.out().ttl(Duration::from_secs(u64::max_value() / 2));
        reply.send()
    }

    async fn open(
        self: &Arc<Self>,
        mut req: fs::Request<'_>,
        op: op::Open<'_>,
        mut reply: ReplyOpen<'_>,
    ) -> reply::Result {
        if op.options().access_mode() != Some(AccessMode::ReadOnly) {
            Err(EACCES)?;
        }

        let is_nonblock = op.options().flags().contains(OpenFlags::NONBLOCK);

        let fh = self.next_fh.fetch_add(1, Ordering::SeqCst);
        let deadline = Instant::now() + self.wakeup_interval;
        let handle = Arc::new(FileHandle {
            is_nonblock,
            kh: OnceLock::new(),
            deadline,
        });

        tracing::info!("spawn reading task");
        {
            let notifier = req.notifier();
            let handle = Arc::downgrade(&handle);
            let wakeup_interval = self.wakeup_interval;
            let _ = req.spawner().spawn(async move {
                let span = tracing::debug_span!("reading_task", fh=?fh);
                let _enter = span.enter();

                tracing::info!("start reading");
                tokio::time::sleep(wakeup_interval).await;

                tracing::info!("reading completed");

                // Do nothing when the handle is already closed.
                if let Some(handle) = handle.upgrade() {
                    if let Some(kh) = handle.kh.get() {
                        tracing::info!("send wakeup notification, kh={}", kh);
                        notifier.send(notify::PollWakeup::new(*kh))?;
                    }
                }

                Ok(())
            });
        }

        let fh = FileID::from_raw(fh);
        self.handles.write().await.insert(fh, handle);

        reply.out().fh(fh);
        reply.out().direct_io(true);
        reply.out().nonseekable(true);
        reply.send()
    }

    async fn read(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Read<'_>,
        reply: ReplyData<'_>,
    ) -> reply::Result {
        let handle = {
            let handles = self.handles.read().await;
            handles.get(&op.fh()).cloned().ok_or(EINVAL)?
        };
        if handle.is_nonblock {
            if handle.deadline > Instant::now() {
                tracing::info!("send EAGAIN immediately");
                Err(EAGAIN)?;
            }
        } else {
            tracing::info!("wait for the completion of background task");
            let now = Instant::now();
            if handle.deadline > now {
                tokio::time::sleep(handle.deadline.duration_since(now)).await;
            }
        }

        tracing::info!("ready to read contents");

        let offset = op.offset() as usize;
        let bufsize = op.size() as usize;
        let content = CONTENT.as_bytes().get(offset..).unwrap_or(&[]);

        reply.send(&content[..std::cmp::min(content.len(), bufsize)])
    }

    async fn poll(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Poll<'_>,
        reply: ReplyPoll<'_>,
    ) -> reply::Result {
        let handle = {
            let handles = self.handles.read().await;
            handles.get(&op.fh()).cloned().ok_or(EINVAL)?
        };
        let now = Instant::now();

        let mut revents = PollEvents::empty();
        if handle.deadline <= now {
            tracing::info!("file is ready to read");
            revents = op.events() & PollEvents::IN;
        } else if let Some(kh) = op.kh() {
            tracing::info!("register the poll handle for notification: kh={}", kh);
            let _ = handle.kh.set(kh);
        }

        reply.send(revents)
    }

    async fn release(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Release<'_>,
        reply: ReplyUnit<'_>,
    ) -> reply::Result {
        drop(self.handles.write().await.remove(&op.fh()));
        reply.send()
    }
}

struct FileHandle {
    is_nonblock: bool,
    kh: OnceLock<PollWakeupID>,
    deadline: Instant,
}
