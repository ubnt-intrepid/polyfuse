use polyfuse::{
    fs::{self, Filesystem},
    op,
    reply::{AttrOut, OpenOut, PollOut},
    types::{FileID, NodeID, PollWakeupID, GID, UID},
};

use anyhow::{ensure, Context as _, Result};
use dashmap::DashMap;
use libc::{EACCES, EAGAIN, EINVAL, O_ACCMODE, O_NONBLOCK, O_RDONLY, POLLIN, S_IFREG};
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
    fn getattr(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Getattr<'_>>) -> fs::Result {
        let mut out = AttrOut::default();
        out.attr().ino(NodeID::ROOT);
        out.attr().nlink(1);
        out.attr().mode(S_IFREG | 0o444);
        out.attr().uid(UID::current());
        out.attr().gid(GID::current());
        out.ttl(Duration::from_secs(u64::max_value() / 2));
        req.reply(out)
    }

    fn open(&self, cx: fs::Context<'_, '_>, req: fs::Request<'_, op::Open<'_>>) -> fs::Result {
        if req.arg().flags() as i32 & O_ACCMODE != O_RDONLY {
            Err(EACCES)?;
        }

        let is_nonblock = req.arg().flags() as i32 & O_NONBLOCK != 0;

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

        req.reply({
            let mut out = OpenOut::default();
            out.fh(fh);
            out.direct_io(true);
            out.nonseekable(true);
            out
        })
    }

    fn read(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Read<'_>>) -> fs::Result {
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

        req.reply(&content[..std::cmp::min(content.len(), bufsize)])
    }

    fn poll(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Poll<'_>>) -> fs::Result {
        let handle = &*self.handles.get(&req.arg().fh()).ok_or(EINVAL)?;
        let state = &mut *handle.state.lock().unwrap();

        let mut out = PollOut::default();

        if state.is_ready {
            tracing::info!("file is ready to read");
            out.revents(req.arg().events() & POLLIN as u32);
        } else if let Some(kh) = req.arg().kh() {
            tracing::info!("register the poll handle for notification: kh={}", kh);
            state.kh = Some(kh);
        }

        req.reply(out)
    }

    fn release(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Release<'_>>) -> fs::Result {
        drop(self.handles.remove(&req.arg().fh()));
        req.reply(())
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
