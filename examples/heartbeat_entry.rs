//! test script:
//!
//! ```shell-session
//! $ for i in {1..1000}; do clear; ls -al /path/to/heartbeat_entry; usleep 500000; done
//! ```

#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented, clippy::todo)]
#![forbid(unsafe_code)]

use polyfuse::{
    fs::{self, Daemon, Filesystem},
    op,
    reply::{AttrOut, EntryOut, ReaddirOut},
    types::{FileAttr, FileMode, FilePermissions, FileType, NodeID},
};

use anyhow::{ensure, Context as _, Result};
use chrono::Local;
use rustix::io::Errno;
use std::{borrow::Cow, io, mem, os::unix::prelude::*, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;

const FILE_INO: NodeID = match NodeID::from_raw(2) {
    Some(ino) => ino,
    None => panic!("unreachable"),
};

const DEFAULT_TTL: Duration = Duration::from_secs(0);
const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let no_notify = args.contains("--no-notify");

    let ttl = args
        .opt_value_from_str("--ttl")?
        .map_or(DEFAULT_TTL, Duration::from_secs);
    tracing::info!(?ttl);

    let update_interval = args
        .opt_value_from_str("--update-interval")?
        .map_or(DEFAULT_INTERVAL, Duration::from_secs);
    tracing::info!(?update_interval);

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_dir(), "mountpoint must be a directory");

    let fs = Arc::new(Heartbeat::new(ttl, update_interval, no_notify));

    let mut daemon = Daemon::mount(mountpoint, Default::default(), Default::default()).await?;

    fs.init(&mut daemon);

    daemon.run(fs, None).await?;

    Ok(())
}

fn generate_filename() -> String {
    Local::now().format("Time_is_%Hh_%Mm_%Ss").to_string()
}

struct Heartbeat {
    root_attr: FileAttr,
    file_attr: FileAttr,
    ttl: Duration,
    update_interval: Duration,
    current: Mutex<CurrentFile>,
    no_notify: bool,
}

#[derive(Debug)]
struct CurrentFile {
    filename: String,
    nlookup: u64,
}

impl Heartbeat {
    fn new(ttl: Duration, update_interval: Duration, no_notify: bool) -> Self {
        let root_attr = FileAttr {
            ino: NodeID::ROOT,
            mode: FileMode::new(
                FileType::Directory,
                FilePermissions::READ | FilePermissions::EXEC,
            ),
            nlink: 2, // "." and ".."
            ..FileAttr::new()
        };

        let file_attr = FileAttr {
            ino: FILE_INO,
            mode: FileMode::new(FileType::Regular, FilePermissions::READ),
            nlink: 1,
            ..FileAttr::new()
        };

        Self {
            root_attr,
            file_attr,
            ttl,
            update_interval,
            current: Mutex::new(CurrentFile {
                filename: generate_filename(),
                nlookup: 0,
            }),
            no_notify,
        }
    }

    async fn heartbeat(&self, notifier: &fs::Notifier) -> io::Result<()> {
        let span = tracing::debug_span!("heartbeat", notify = !self.no_notify);
        let _enter = span.enter();

        loop {
            tracing::info!("heartbeat");

            let new_filename = generate_filename();
            let mut current = self.current.lock().await;
            tracing::debug!(filename = ?current.filename, nlookup = ?current.nlookup);
            tracing::debug!(?new_filename);
            let old_filename = mem::replace(&mut current.filename, new_filename);

            if !self.no_notify && current.nlookup > 0 {
                tracing::info!("send notify_inval_entry");
                notifier.inval_entry(NodeID::ROOT, old_filename.as_ref())?;
            }

            drop(current);

            tokio::time::sleep(self.update_interval).await;
        }
    }

    fn init(self: &Arc<Self>, daemon: &mut fs::Daemon) {
        let this = self.clone();
        let notifier = daemon.notifier();
        let _ = daemon.spawner().spawn(async move {
            this.heartbeat(&notifier).await?;
            Ok(())
        });
    }
}

impl Filesystem for Heartbeat {
    async fn lookup(self: &Arc<Self>, req: fs::Request<'_>, arg: op::Lookup<'_>) -> fs::Result {
        if arg.parent != NodeID::ROOT {
            Err(Errno::NOTDIR)?;
        }

        let mut current = self.current.lock().await;

        if arg.name.as_bytes() == current.filename.as_bytes() {
            let res = req.reply(EntryOut {
                ino: Some(self.file_attr.ino),
                attr: Cow::Borrowed(&self.file_attr),
                entry_valid: Some(self.ttl),
                attr_valid: Some(self.ttl),
                generation: 0,
            })?;

            current.nlookup += 1;

            Ok(res)
        } else {
            Err(Errno::NOENT)?
        }
    }

    async fn forget(self: &Arc<Self>, forgets: &[op::Forget]) {
        let mut current = self.current.lock().await;
        for forget in forgets {
            if forget.ino() == FILE_INO {
                current.nlookup -= forget.nlookup();
            }
        }
    }

    async fn getattr(self: &Arc<Self>, req: fs::Request<'_>, arg: op::Getattr<'_>) -> fs::Result {
        let attr = match arg.ino {
            NodeID::ROOT => &self.root_attr,
            FILE_INO => &self.file_attr,
            _ => Err(Errno::NOENT)?,
        };

        req.reply(AttrOut {
            attr: Cow::Borrowed(attr),
            valid: Some(self.ttl),
        })
    }

    async fn read(self: &Arc<Self>, req: fs::Request<'_>, op: op::Read<'_>) -> fs::Result {
        match op.ino {
            NodeID::ROOT => Err(Errno::ISDIR)?,
            FILE_INO => req.reply(()),
            _ => Err(Errno::NOENT)?,
        }
    }

    async fn readdir(self: &Arc<Self>, req: fs::Request<'_>, op: op::Readdir<'_>) -> fs::Result {
        if op.ino != NodeID::ROOT {
            Err(Errno::NOTDIR)?;
        }
        if op.offset > 0 {
            return req.reply(());
        }

        let mut buf = ReaddirOut::new(op.size as usize);
        let current = self.current.lock().await;
        buf.push_entry(current.filename.as_ref(), FILE_INO, None, 1);

        req.reply(buf)
    }
}
