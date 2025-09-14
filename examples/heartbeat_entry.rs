//! test script:
//!
//! ```shell-session
//! $ for i in {1..1000}; do clear; ls -al /path/to/heartbeat_entry; usleep 500000; done
//! ```

#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented, clippy::todo)]
#![forbid(unsafe_code)]

use polyfuse::{
    fs::{
        self,
        reply::{self, ReplyAttr, ReplyData, ReplyDir, ReplyEntry},
        Daemon, Filesystem,
    },
    notify, op,
    types::{FileAttr, FileMode, FilePermissions, FileType, NodeID},
};

use anyhow::{ensure, Context as _, Result};
use chrono::Local;
use libc::{EISDIR, ENOENT, ENOTDIR};
use std::{io, mem, os::unix::prelude::*, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;

const FILE_INO: NodeID = NodeID::from_raw(2);

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
        let mut root_attr = FileAttr::new();
        root_attr.ino = NodeID::ROOT;
        root_attr.mode = FileMode::new(
            FileType::Directory,
            FilePermissions::READ | FilePermissions::EXEC,
        );
        root_attr.nlink = 2; // "." and ".."

        let mut file_attr = FileAttr::new();
        file_attr.ino = FILE_INO;
        file_attr.mode = FileMode::new(FileType::Regular, FilePermissions::READ);
        file_attr.nlink = 1;

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
                notifier.send(notify::InvalEntry::new(NodeID::ROOT, old_filename.as_ref()))?;
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
    async fn lookup(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        arg: op::Lookup<'_>,
        mut reply: ReplyEntry<'_>,
    ) -> reply::Result {
        if arg.parent() != NodeID::ROOT {
            Err(ENOTDIR)?;
        }

        let mut current = self.current.lock().await;

        if arg.name().as_bytes() == current.filename.as_bytes() {
            reply.out().ino(self.file_attr.ino);
            reply.out().attr(self.file_attr.clone());
            reply.out().ttl_entry(self.ttl);
            reply.out().ttl_attr(self.ttl);
            let res = reply.send()?;

            current.nlookup += 1;

            Ok(res)
        } else {
            Err(ENOENT)?
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

    async fn getattr(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        arg: op::Getattr<'_>,
        mut reply: ReplyAttr<'_>,
    ) -> reply::Result {
        let attr = match arg.ino() {
            NodeID::ROOT => self.root_attr.clone(),
            FILE_INO => self.file_attr.clone(),
            _ => Err(ENOENT)?,
        };

        reply.out().attr(attr);
        reply.out().ttl(self.ttl);
        reply.send()
    }

    async fn read(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        arg: op::Read<'_>,
        reply: ReplyData<'_>,
    ) -> reply::Result {
        match arg.ino() {
            NodeID::ROOT => Err(EISDIR)?,
            FILE_INO => reply.send(()),
            _ => Err(ENOENT)?,
        }
    }

    async fn readdir(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        arg: op::Readdir<'_>,
        mut reply: ReplyDir<'_>,
    ) -> reply::Result {
        if arg.ino() != NodeID::ROOT {
            Err(ENOTDIR)?;
        }
        if arg.offset() > 0 {
            return reply.send();
        }

        let current = self.current.lock().await;
        reply.push_entry(current.filename.as_ref(), FILE_INO, None, 1);

        reply.send()
    }
}
