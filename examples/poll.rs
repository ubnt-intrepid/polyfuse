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
        Arc, Condvar, Mutex,
    },
    time::Duration,
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
        _: fs::Request<'_, op::Getattr<'_>>,
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
        req: fs::Request<'_, op::Open<'_>>,
        mut reply: fs::ReplyOpen<'_>,
    ) -> fs::Result {
        if req.arg().options().access_mode() != Some(AccessMode::ReadOnly) {
            Err(EACCES)?;
        }

        let is_nonblock = req.arg().options().flags().contains(OpenFlags::NONBLOCK);

        let fh = self.next_fh.fetch_add(1, Ordering::SeqCst);
        let handle = Arc::new(FileHandle {
            is_nonblock,
            state: Mutex::default(),
            condvar: Condvar::new(),
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
                    let state = &mut *handle.state.lock().unwrap();

                    if let Some(kh) = state.kh {
                        tracing::info!("send wakeup notification, kh={}", kh);
                        cx.notifier.poll_wakeup(kh)?;
                    }

                    state.is_ready = true;
                    handle.condvar.notify_one();
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
        req: fs::Request<'_, op::Read<'_>>,
        reply: fs::ReplyData<'_>,
    ) -> fs::Result {
        let handle = &**self.handles.get(&req.arg().fh()).ok_or(EINVAL)?;
        let mut state = handle.state.lock().unwrap();
        if handle.is_nonblock {
            if !state.is_ready {
                tracing::info!("send EAGAIN immediately");
                Err(EAGAIN)?;
            }
        } else {
            tracing::info!("wait for the completion of background task");
            state = handle
                .condvar
                .wait_while(state, |state| !state.is_ready)
                .unwrap();
        }

        debug_assert!(state.is_ready);

        tracing::info!("complete reading");

        let offset = req.arg().offset() as usize;
        let bufsize = req.arg().size() as usize;
        let content = CONTENT.as_bytes().get(offset..).unwrap_or(&[]);

        reply.send(&content[..std::cmp::min(content.len(), bufsize)])
    }

    fn poll(
        &self,
        _: fs::Env<'_, '_>,
        req: fs::Request<'_, op::Poll<'_>>,
        reply: fs::ReplyPoll<'_>,
    ) -> fs::Result {
        let handle = &*self.handles.get(&req.arg().fh()).ok_or(EINVAL)?;
        let state = &mut *handle.state.lock().unwrap();

        let mut revents = PollEvents::empty();
        if state.is_ready {
            tracing::info!("file is ready to read");
            revents = req.arg().events() & PollEvents::IN;
        } else if let Some(kh) = req.arg().kh() {
            tracing::info!("register the poll handle for notification: kh={}", kh);
            state.kh = Some(kh);
        }

        reply.send(revents)
    }

    fn release(
        &self,
        _: fs::Env<'_, '_>,
        req: fs::Request<'_, op::Release<'_>>,
        reply: fs::ReplyUnit<'_>,
    ) -> fs::Result {
        drop(self.handles.remove(&req.arg().fh()));
        reply.send()
    }
}

struct FileHandle {
    is_nonblock: bool,
    state: Mutex<FileHandleState>,
    condvar: Condvar,
}

#[derive(Default)]
struct FileHandleState {
    is_ready: bool,
    kh: Option<PollWakeupID>,
}
