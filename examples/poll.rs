use polyfuse::{
    fs::{self, Daemon, Filesystem},
    op::{self, AccessMode, OpenFlags},
    reply::{self, AttrOut, OpenOut, OpenOutFlags, PollOut},
    types::{
        FileAttr, FileID, FileMode, FilePermissions, FileType, NodeID, PollEvents, PollWakeupID,
    },
};

use anyhow::{ensure, Context as _, Result};
use rustix::{
    io::Errno,
    process::{getgid, getuid},
};
use std::{
    borrow::Cow,
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock, RwLock,
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

    let daemon = Daemon::mount(mountpoint, Default::default(), Default::default())?;
    daemon.run(Arc::new(PollFS::new(wakeup_interval)), None)?;

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
    fn getattr(self: &Arc<Self>, req: fs::Request<'_>, _: op::Getattr<'_>) -> fs::Result {
        req.reply(AttrOut {
            attr: Cow::Owned(FileAttr {
                ino: NodeID::ROOT,
                nlink: 1,
                mode: FileMode::new(FileType::Regular, FilePermissions::READ),
                uid: getuid(),
                gid: getgid(),
                ..FileAttr::new()
            }),
            valid: Some(Duration::from_secs(u64::MAX / 2)),
        })
    }

    fn open(self: &Arc<Self>, mut req: fs::Request<'_>, op: op::Open<'_>) -> fs::Result {
        if op.options.access_mode() != Some(AccessMode::ReadOnly) {
            Err(Errno::ACCESS)?;
        }

        let is_nonblock = op.options.flags().contains(OpenFlags::NONBLOCK);

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
            req.spawner().spawn(move || {
                let span = tracing::debug_span!("reading_task", fh=?fh);
                let _enter = span.enter();

                tracing::info!("start reading");
                std::thread::sleep(wakeup_interval);

                tracing::info!("reading completed");

                // Do nothing when the handle is already closed.
                if let Some(handle) = handle.upgrade() {
                    if let Some(kh) = handle.kh.get() {
                        tracing::info!("send wakeup notification, kh={}", kh);
                        notifier.poll_wakeup(*kh)?;
                    }
                }

                Ok(())
            });
        }

        let fh = FileID::from_raw(fh);
        self.handles.write().unwrap().insert(fh, handle);

        req.reply(OpenOut {
            fh,
            open_flags: OpenOutFlags::DIRECT_IO | OpenOutFlags::NONSEEKABLE,
        })
    }

    fn read(self: &Arc<Self>, req: fs::Request<'_>, op: op::Read<'_>) -> fs::Result {
        let handle = {
            let handles = self.handles.read().unwrap();
            handles.get(&op.fh).cloned().ok_or(Errno::INVAL)?
        };
        if handle.is_nonblock {
            if handle.deadline > Instant::now() {
                tracing::info!("send EAGAIN immediately");
                Err(Errno::AGAIN)?;
            }
        } else {
            tracing::info!("wait for the completion of background task");
            let now = Instant::now();
            if handle.deadline > now {
                std::thread::sleep(handle.deadline.duration_since(now));
            }
        }

        tracing::info!("ready to read contents");

        let offset = op.offset as usize;
        let bufsize = op.size as usize;
        let content = CONTENT.as_bytes().get(offset..).unwrap_or(&[]);

        req.reply(reply::Raw(
            &content[..std::cmp::min(content.len(), bufsize)],
        ))
    }

    fn poll(self: &Arc<Self>, req: fs::Request<'_>, op: op::Poll<'_>) -> fs::Result {
        let handle = {
            let handles = self.handles.read().unwrap();
            handles.get(&op.fh).cloned().ok_or(Errno::INVAL)?
        };
        let now = Instant::now();

        let mut revents = PollEvents::empty();
        if handle.deadline <= now {
            tracing::info!("file is ready to read");
            revents = op.events & PollEvents::IN;
        } else if let Some(kh) = op.kh {
            tracing::info!("register the poll handle for notification: kh={}", kh);
            let _ = handle.kh.set(kh);
        }

        req.reply(PollOut::new(revents))
    }

    fn release(self: &Arc<Self>, req: fs::Request<'_>, op: op::Release<'_>) -> fs::Result {
        drop(self.handles.write().unwrap().remove(&op.fh));
        req.reply(())
    }
}

struct FileHandle {
    is_nonblock: bool,
    kh: OnceLock<PollWakeupID>,
    deadline: Instant,
}
