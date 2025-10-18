//! test script:
//!
//! ```shell-session
//! $ for i in {1..1000}; do clear; cat /path/to/heartbeat; usleep 500000; done
//! ```

#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]
#![forbid(unsafe_code)]

use polyfuse::{
    fs::{self, Daemon, Filesystem},
    op,
    reply::{self, AttrOut, OpenOut, OpenOutFlags},
    types::{FileAttr, FileID, FileMode, FilePermissions, FileType, NodeID, NotifyID},
};

use anyhow::{anyhow, ensure, Context as _, Result};
use chrono::Local;
use dashmap::DashMap;
use rustix::io::Errno;
use std::{borrow::Cow, io, path::PathBuf, sync::Arc, sync::Mutex, time::Duration};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let kind = args.opt_value_from_str("--notify-kind")?;
    let update_interval = args
        .value_from_str("--update-interval")
        .map(Duration::from_secs)?;

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    let fs = Arc::new(Heartbeat::new(kind, update_interval));

    let mut daemon = Daemon::mount(mountpoint, Default::default(), Default::default())?;
    fs.init(&mut daemon);
    daemon.run(fs, None)?;

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

    fn notify(&self, notifier: &fs::Notifier) -> io::Result<()> {
        match self.kind {
            Some(NotifyKind::Store) => {
                let inner = &*self.inner.lock().unwrap();
                let content = inner.content.clone();

                tracing::info!("send notify_store(data={:?})", content);
                notifier.store(NodeID::ROOT, 0, &content)?;

                // To check if the cache is updated correctly, pull the
                // content from the kernel using notify_retrieve.
                tracing::info!("send notify_retrieve");
                let unique = notifier.retrieve(NodeID::ROOT, 0, 1024)?;
                self.retrieves.insert(unique, content);
            }

            Some(NotifyKind::Invalidate) => {
                tracing::info!("send notify_invalidate_inode");
                notifier.inval_inode(NodeID::ROOT, 0, 0)?;
            }

            None => { /* do nothing */ }
        }
        Ok(())
    }

    fn init(self: &Arc<Self>, daemon: &mut fs::Daemon) {
        // Spawn a task that beats the heart.
        let this = self.clone();
        let notifier = daemon.notifier();
        daemon.spawner().spawn(move || loop {
            tracing::info!("heartbeat");
            this.update_content();
            this.notify(&notifier)?;
            std::thread::sleep(this.update_interval);
        });
    }
}

impl Filesystem for Heartbeat {
    fn getattr(self: &Arc<Self>, req: fs::Request<'_>, op: op::Getattr<'_>) -> fs::Result {
        if op.ino != NodeID::ROOT {
            Err(Errno::NOENT)?;
        }
        let inner = self.inner.lock().unwrap();
        req.reply(AttrOut {
            attr: Cow::Borrowed(&inner.attr),
            valid: None,
        })
    }

    fn open(self: &Arc<Self>, req: fs::Request<'_>, op: op::Open<'_>) -> fs::Result {
        if op.ino != NodeID::ROOT {
            Err(Errno::NOENT)?;
        }
        req.reply(OpenOut {
            fh: FileID::from_raw(0),
            open_flags: OpenOutFlags::KEEP_CACHE,
            backing_id: 0,
        })
    }

    fn read(self: &Arc<Self>, req: fs::Request<'_>, op: op::Read<'_>) -> fs::Result {
        if op.ino != NodeID::ROOT {
            Err(Errno::NOENT)?
        }

        let inner = self.inner.lock().unwrap();

        let offset = op.offset as usize;
        if offset >= inner.content.len() {
            return req.reply(());
        }

        let size = op.size as usize;
        let data = &inner.content.as_bytes()[offset..];
        let data = &data[..std::cmp::min(data.len(), size)];
        req.reply(reply::Raw(data))
    }

    fn notify_reply(
        self: &Arc<Self>,
        arg: op::NotifyReply<'_>,
        mut data: impl io::Read,
    ) -> io::Result<()> {
        if let Some((_, original)) = self.retrieves.remove(&arg.unique) {
            let data = {
                let mut buf = vec![0u8; arg.size as usize];
                data.read_exact(&mut buf)?;
                buf
            };

            if data[..original.len()] == *original.as_bytes() {
                tracing::info!("matched data");
            } else {
                tracing::error!("mismatched data");
            }
        }
        Ok(())
    }
}
