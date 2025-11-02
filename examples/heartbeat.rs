//! test script:
//!
//! ```shell-session
//! $ for i in {1..1000}; do clear; cat /path/to/heartbeat; usleep 500000; done
//! ```

#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]
#![forbid(unsafe_code)]

use polyfuse::{
    mount::MountOptions,
    notify::Notifier as _,
    op::Operation,
    reply::{OpenOutFlags, ReplySender as _},
    session::Session,
    types::{FileAttr, FileID, FileMode, FilePermissions, FileType, NodeID, NotifyID},
    Device, KernelConfig,
};

use anyhow::{anyhow, ensure, Context as _, Result};
use chrono::Local;
use dashmap::DashMap;
use libc::{SIGHUP, SIGINT, SIGTERM};
use rustix::{io::Errno, param::page_size};
use signal_hook::iterator::Signals;
use std::{
    io::{self, prelude::*},
    path::PathBuf,
    sync::Mutex,
    thread,
    time::Duration,
};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let kind = args.opt_value_from_str("--notify-kind")?;
    let update_interval = args
        .value_from_str("--update-interval")
        .map(Duration::from_secs)?;

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    let fs = Heartbeat::new(kind, update_interval);

    let (session, conn, mount) =
        polyfuse::connect(mountpoint, MountOptions::new(), KernelConfig::new())?;

    let conn = &conn;
    let session = &session;
    let fs = &fs;
    thread::scope(|scope| -> Result<()> {
        let mut signals = Signals::new([SIGTERM, SIGHUP, SIGINT])?;
        scope.spawn(move || {
            if let Some(_sig) = signals.forever().next() {
                let _ = mount.unmount();
            }
        });

        // Spawn a task that beats the heart.
        scope.spawn(move || -> Result<()> {
            loop {
                tracing::info!("heartbeat");
                fs.update_content();
                fs.notify(session, conn)?;
                thread::sleep(fs.update_interval);
            }
        });

        let mut buf = session.new_splice_buffer()?;
        while session.recv_request(conn, &mut buf)? {
            let (req, op, remains) = session.decode(conn, &mut buf)?;
            match op {
                Operation::Getattr(op) => {
                    if op.ino == NodeID::ROOT {
                        let inner = fs.inner.lock().unwrap();
                        req.reply_attr(&inner.attr, None)?;
                    } else {
                        req.reply_error(Errno::NOENT)?;
                    }
                }

                Operation::Open(op) => {
                    if op.ino == NodeID::ROOT {
                        req.reply_open(FileID::from_raw(0), OpenOutFlags::KEEP_CACHE, 0)?;
                    } else {
                        req.reply_error(Errno::NOENT)?;
                    }
                }

                Operation::Read(op) => {
                    if op.ino == NodeID::ROOT {
                        let inner = fs.inner.lock().unwrap();

                        let offset = op.offset as usize;
                        if offset >= inner.content.len() {
                            req.reply_bytes(())?;
                            continue;
                        }

                        let size = op.size as usize;
                        let data = &inner.content.as_bytes()[offset..];
                        let data = &data[..std::cmp::min(data.len(), size)];
                        req.reply_bytes(data)?;
                    } else {
                        req.reply_error(Errno::NOENT)?;
                    }
                }

                Operation::NotifyReply(op) => {
                    if let Some((_, original)) = fs.retrieves.remove(&op.unique) {
                        let data = {
                            let mut buf = vec![0u8; op.size as usize];
                            remains.read_exact(&mut buf)?;
                            buf
                        };

                        if data[..original.len()] == *original.as_bytes() {
                            tracing::info!("matched data");
                        } else {
                            tracing::error!("mismatched data");
                        }
                    }
                }

                _ => req.reply_error(Errno::NOSYS)?,
            }
        }

        Ok(())
    })?;

    Ok(())
}

#[derive(Debug, Copy, Clone, PartialEq)]
enum NotifyKind {
    Store,
    Invalidate,
}

impl std::str::FromStr for NotifyKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "store" => Ok(NotifyKind::Store),
            "invalidate" => Ok(NotifyKind::Invalidate),
            s => Err(anyhow!("invalid notify kind: {:?}", s)),
        }
    }
}

struct Heartbeat {
    inner: Mutex<Inner>,
    retrieves: DashMap<NotifyID, String>,
    kind: Option<NotifyKind>,
    update_interval: Duration,
}

struct Inner {
    content: String,
    attr: FileAttr,
}

impl Heartbeat {
    fn new(kind: Option<NotifyKind>, update_interval: Duration) -> Self {
        let content = Local::now().to_rfc3339();

        let attr = FileAttr {
            ino: NodeID::ROOT,
            mode: FileMode::new(FileType::Regular, FilePermissions::READ),
            size: content.len() as u64,
            ..FileAttr::new()
        };

        Self {
            inner: Mutex::new(Inner { content, attr }),
            retrieves: DashMap::new(),
            kind,
            update_interval,
        }
    }

    fn update_content(&self) {
        let mut inner = self.inner.lock().unwrap();
        let content = Local::now().to_rfc3339();
        inner.attr.size = content.len() as u64;
        inner.content = content;
    }

    fn notify(&self, session: &Session, conn: &Device) -> io::Result<()> {
        match self.kind {
            Some(NotifyKind::Store) => {
                let inner = &*self.inner.lock().unwrap();
                let content = inner.content.clone();

                tracing::info!("send notify_store(data={:?})", content);
                session.notifier(conn).store(NodeID::ROOT, 0, &content)?;

                // To check if the cache is updated correctly, pull the
                // content from the kernel using notify_retrieve.
                tracing::info!("send notify_retrieve");
                let notify_unique =
                    session
                        .notifier(conn)
                        .retrieve(NodeID::ROOT, 0, page_size() as u32)?;
                self.retrieves.insert(notify_unique, content);
            }

            Some(NotifyKind::Invalidate) => {
                tracing::info!("send notify_invalidate_inode");
                session.notifier(conn).inval_inode(NodeID::ROOT, 0, 0)?;
            }

            None => { /* do nothing */ }
        }
        Ok(())
    }
}
