use polyfuse::{
    fs::{self, Filesystem},
    op,
    reply::{AttrOut, OpenOut, PollOut},
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
    handles: DashMap<u64, Arc<FileHandle>>,
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
    fn getattr<'env, 'req>(
        &'env self,
        _: fs::Context<'_, 'env>,
        req: fs::Request<'req, op::Getattr<'req>>,
    ) -> fs::Result {
        let mut out = AttrOut::default();
        out.attr().ino(1);
        out.attr().nlink(1);
        out.attr().mode(libc::S_IFREG | 0o444);
        out.attr().uid(unsafe { libc::getuid() });
        out.attr().gid(unsafe { libc::getgid() });
        out.ttl(Duration::from_secs(u64::max_value() / 2));
        req.reply(out)
    }

    fn open<'env, 'req>(
        &'env self,
        cx: fs::Context<'_, 'env>,
        req: fs::Request<'req, op::Open<'req>>,
    ) -> fs::Result {
        if req.arg().flags() as i32 & libc::O_ACCMODE != libc::O_RDONLY {
            Err(EACCES)?;
        }

        let is_nonblock = req.arg().flags() as i32 & libc::O_NONBLOCK != 0;

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

        self.handles.insert(fh, handle);

        req.reply({
            let mut out = OpenOut::default();
            out.fh(fh);
            out.direct_io(true);
            out.nonseekable(true);
            out
        })
    }

    fn read<'env, 'req>(
        &'env self,
        _: fs::Context<'_, 'env>,
        req: fs::Request<'req, op::Read<'req>>,
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

        req.reply(&content[..std::cmp::min(content.len(), bufsize)])
    }

    fn poll<'env, 'req>(
        &'env self,
        _: fs::Context<'_, 'env>,
        req: fs::Request<'req, op::Poll<'req>>,
    ) -> fs::Result {
        let handle = &*self.handles.get(&req.arg().fh()).ok_or(EINVAL)?;
        let state = &mut *handle.state.lock().unwrap();

        let mut out = PollOut::default();

        if state.is_ready {
            tracing::info!("file is ready to read");
            out.revents(req.arg().events() & libc::POLLIN as u32);
        } else if let Some(kh) = req.arg().kh() {
            tracing::info!("register the poll handle for notification: kh={}", kh);
            state.kh = Some(kh);
        }

        req.reply(out)
    }

    fn release<'env, 'req>(
        &'env self,
        _: fs::Context<'_, 'env>,
        req: fs::Request<'req, op::Release<'req>>,
    ) -> fs::Result {
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
    kh: Option<u64>,
}
