use polyfuse::{
    mount::MountOptions,
    notify::Notifier as _,
    op::{AccessMode, OpenFlags, Operation},
    reply::{OpenOutFlags, ReplySender as _},
    types::{
        FileAttr, FileID, FileMode, FilePermissions, FileType, NodeID, PollEvents, PollWakeupID,
    },
    KernelConfig,
};

use anyhow::{ensure, Context as _, Result};
use rustix::{
    io::Errno,
    process::{getgid, getuid},
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock, RwLock,
    },
    thread,
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

    let fs = Arc::new(PollFS::new(wakeup_interval));

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    let (session, conn, mount) =
        polyfuse::connect(mountpoint, MountOptions::new(), KernelConfig::new())?;

    let conn = &conn;
    let session = &session;
    thread::scope(|scope| -> Result<()> {
        let mut buf = session.new_splice_buffer()?;
        while session.recv_request(conn, &mut buf)? {
            let (req, op, _remains) = session.decode(conn, &mut buf)?;
            match op {
                Operation::Getattr(_op) => req.reply_attr(
                    &FileAttr {
                        ino: NodeID::ROOT,
                        nlink: 1,
                        mode: FileMode::new(FileType::Regular, FilePermissions::READ),
                        uid: getuid(),
                        gid: getgid(),
                        ..FileAttr::new()
                    },
                    Some(Duration::from_secs(u64::MAX / 2)),
                )?,

                Operation::Open(op) => {
                    if op.options.access_mode() != Some(AccessMode::ReadOnly) {
                        req.reply_error(Errno::ACCESS)?;
                        continue;
                    }

                    let is_nonblock = op.options.flags().contains(OpenFlags::NONBLOCK);

                    let fh = fs.next_fh.fetch_add(1, Ordering::SeqCst);
                    let deadline = Instant::now() + fs.wakeup_interval;
                    let handle = Arc::new(FileHandle {
                        is_nonblock,
                        kh: OnceLock::new(),
                        deadline,
                    });

                    tracing::info!("spawn reading task");
                    {
                        let handle = Arc::downgrade(&handle);
                        let wakeup_interval = fs.wakeup_interval;
                        scope.spawn(move || -> Result<()> {
                            let span = tracing::debug_span!("reading_task", fh=?fh);
                            let _enter = span.enter();

                            tracing::info!("start reading");
                            std::thread::sleep(wakeup_interval);

                            tracing::info!("reading completed");

                            // Do nothing when the handle is already closed.
                            if let Some(handle) = handle.upgrade() {
                                if let Some(kh) = handle.kh.get().copied() {
                                    tracing::info!("send wakeup notification, kh={}", kh);
                                    session.notifier(conn).poll_wakeup(kh)?;
                                }
                            }

                            Ok(())
                        });
                    }

                    let fh = FileID::from_raw(fh);
                    fs.handles.write().unwrap().insert(fh, handle);

                    req.reply_open(fh, OpenOutFlags::DIRECT_IO | OpenOutFlags::NONSEEKABLE, 0)?;
                }

                Operation::Read(op) => {
                    let handle = {
                        let handles = fs.handles.read().unwrap();
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

                    req.reply_bytes(&content[..std::cmp::min(content.len(), bufsize)])?;
                }

                Operation::Poll(op) => {
                    let handle = {
                        let handles = fs.handles.read().unwrap();
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

                    req.reply_poll(revents)?;
                }

                Operation::Release(op) => {
                    drop(fs.handles.write().unwrap().remove(&op.fh));
                    req.reply_bytes(())?;
                }

                _ => req.reply_error(Errno::NOSYS)?,
            }
        }

        Ok(())
    })?;

    mount.unmount()?;

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

struct FileHandle {
    is_nonblock: bool,
    kh: OnceLock<PollWakeupID>,
    deadline: Instant,
}
