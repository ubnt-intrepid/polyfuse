use polyfuse::{
    bytes::POD,
    mount::MountOptions,
    op::{AccessMode, OpenFlags, Operation},
    reply::{self, AttrOut, OpenOut, OpenOutFlags, PollOut},
    request::SpliceBuf,
    session::KernelConfig,
    types::{
        FileAttr, FileID, FileMode, FilePermissions, FileType, NodeID, PollEvents, PollWakeupID,
    },
};

use anyhow::{ensure, Context as _, Result};
use polyfuse_kernel::{fuse_notify_code, fuse_notify_poll_wakeup_out};
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
        polyfuse::session::connect(mountpoint.into(), MountOptions::new(), KernelConfig::new())?;

    let conn = &conn;
    let session = &session;
    thread::scope(|scope| -> Result<()> {
        let mut buf = SpliceBuf::new(session.request_buffer_size())?;
        while session.recv_request(conn, &mut buf)? {
            let (header, op, _remains) = session.decode(&mut buf)?;
            match op {
                Some(Operation::Getattr(_op)) => session.send_reply(
                    conn,
                    header.unique(),
                    None,
                    AttrOut {
                        attr: Cow::Owned(FileAttr {
                            ino: NodeID::ROOT,
                            nlink: 1,
                            mode: FileMode::new(FileType::Regular, FilePermissions::READ),
                            uid: getuid(),
                            gid: getgid(),
                            ..FileAttr::new()
                        }),
                        valid: Some(Duration::from_secs(u64::MAX / 2)),
                    },
                )?,

                Some(Operation::Open(op)) => {
                    if op.options.access_mode() != Some(AccessMode::ReadOnly) {
                        session.send_reply(conn, header.unique(), Some(Errno::ACCESS), ())?;
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
                                if let Some(kh) = handle.kh.get() {
                                    tracing::info!("send wakeup notification, kh={}", kh);
                                    session.send_notify(
                                        conn,
                                        fuse_notify_code::FUSE_NOTIFY_POLL,
                                        POD(fuse_notify_poll_wakeup_out { kh: kh.into_raw() }),
                                    )?;
                                }
                            }

                            Ok(())
                        });
                    }

                    let fh = FileID::from_raw(fh);
                    fs.handles.write().unwrap().insert(fh, handle);

                    session.send_reply(
                        conn,
                        header.unique(),
                        None,
                        OpenOut {
                            fh,
                            open_flags: OpenOutFlags::DIRECT_IO | OpenOutFlags::NONSEEKABLE,
                            backing_id: 0,
                        },
                    )?;
                }

                Some(Operation::Read(op)) => {
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

                    session.send_reply(
                        conn,
                        header.unique(),
                        None,
                        reply::Raw(&content[..std::cmp::min(content.len(), bufsize)]),
                    )?;
                }

                Some(Operation::Poll(op)) => {
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

                    session.send_reply(conn, header.unique(), None, PollOut::new(revents))?;
                }

                Some(Operation::Release(op)) => {
                    drop(fs.handles.write().unwrap().remove(&op.fh));
                    session.send_reply(conn, header.unique(), None, ())?;
                }

                _ => session.send_reply(conn, header.unique(), Some(Errno::NOSYS), ())?,
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
