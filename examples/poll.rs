use polyfuse::{
    fs::{self, Filesystem},
    op::{self, AccessMode, OpenFlags},
    types::{
        FileAttr, FileID, FileMode, FilePermissions, FileType, NodeID, PollEvents, PollWakeupID,
        GID, UID,
    },
};

use anyhow::{ensure, Context as _, Result};
use dashmap::DashMap;
use libc::{EACCES, EAGAIN, EINVAL};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};

const CONTENT: &str = "Hello, world!\n";

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let wakeup_interval = Duration::from_secs(
        args //
            .opt_value_from_str("--interval")?
            .unwrap_or(5),
    );

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    polyfuse::fs::run(
        PollFS::new(wakeup_interval),
        mountpoint,
        Default::default(),
        Default::default(),
    )?;

    Ok(())
}

struct PollFS {
    handles: DashMap<FileID, Arc<FileHandle>>,
    next_fh: AtomicU64,
    wakeup_interval: Duration,
}

impl PollFS {
    fn new(wakeup_interval: Duration) -> Self {
        Self {
            handles: DashMap::new(),
            next_fh: AtomicU64::new(0),
            wakeup_interval,
        }
    }
}

impl Filesystem for PollFS {
    fn getattr(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        _: op::Getattr<'_>,
        mut reply: fs::ReplyAttr<'_>,
    ) -> fs::Result {
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

    fn open(
        &self,
        cx: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Open<'_>,
        mut reply: fs::ReplyOpen<'_>,
    ) -> fs::Result {
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
        cx.spawner.spawn({
            let handle = Arc::downgrade(&handle);
            let wakeup_interval = self.wakeup_interval;

            move || -> Result<()> {
                let span = tracing::debug_span!("reading_task", fh=?fh);
                let _enter = span.enter();

                tracing::info!("start reading");
                std::thread::sleep(wakeup_interval);

                tracing::info!("reading completed");

                // Do nothing when the handle is already closed.
                if let Some(handle) = handle.upgrade() {
                    if let Some(kh) = handle.kh.get() {
                        tracing::info!("send wakeup notification, kh={}", kh);
                        cx.notifier.poll_wakeup(*kh)?;
                    }
                }

                Ok(())
            }
        });

        let fh = FileID::from_raw(fh);
        self.handles.insert(fh, handle);

        reply.out().fh(fh);
        reply.out().direct_io(true);
        reply.out().nonseekable(true);
        reply.send()
    }

    fn read(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Read<'_>,
        reply: fs::ReplyData<'_>,
    ) -> fs::Result {
        let handle = &*self.handles.get(&op.fh()).ok_or(EINVAL)?;
        if handle.is_nonblock {
            if handle.deadline > Instant::now() {
                tracing::info!("send EAGAIN immediately");
                Err(EAGAIN)?;
            }
        } else {
            tracing::info!("wait for the completion of background task");
            let now = Instant::now();
            if handle.deadline > now {
                std::thread::sleep(handle.deadline.duration_since(now));
            }
        }

        tracing::info!("ready to read contents");

        let offset = op.offset() as usize;
        let bufsize = op.size() as usize;
        let content = CONTENT.as_bytes().get(offset..).unwrap_or(&[]);

        reply.send(&content[..std::cmp::min(content.len(), bufsize)])
    }

    fn poll(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Poll<'_>,
        reply: fs::ReplyPoll<'_>,
    ) -> fs::Result {
        let handle = &*self.handles.get(&op.fh()).ok_or(EINVAL)?;
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

    fn release(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        op: op::Release<'_>,
        reply: fs::ReplyUnit<'_>,
    ) -> fs::Result {
        drop(self.handles.remove(&op.fh()));
        reply.send()
    }
}

struct FileHandle {
    is_nonblock: bool,
    kh: OnceLock<PollWakeupID>,
    deadline: Instant,
}
