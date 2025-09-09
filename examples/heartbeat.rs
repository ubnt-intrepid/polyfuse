//! test script:
//!
//! ```shell-session
//! $ for i in {1..1000}; do clear; cat /path/to/heartbeat; usleep 500000; done
//! ```

#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]
#![forbid(unsafe_code)]

use polyfuse::{
    fs::{self, Filesystem},
    mount::MountOptions,
    op,
    types::{FileAttr, FileMode, FilePermissions, FileType, NodeID, NotifyID},
    KernelConfig,
};

use anyhow::{anyhow, ensure, Context as _, Result};
use chrono::Local;
use dashmap::DashMap;
use libc::ENOENT;
use std::{
    io::{self, prelude::*},
    path::PathBuf,
    sync::Mutex,
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

    polyfuse::fs::run(
        Heartbeat::new(kind, update_interval),
        mountpoint,
        MountOptions::default(),
        KernelConfig::default(),
    )?;

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

        let mut attr = FileAttr::new();
        attr.ino = NodeID::ROOT;
        attr.mode = FileMode::new(FileType::Regular, FilePermissions::READ);
        attr.size = content.len() as u64;

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

    fn notify(&self, notifier: &fs::Notifier<'_>) -> io::Result<()> {
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
}

impl Filesystem for Heartbeat {
    fn init<'env>(&'env self, env: fs::Env<'_, 'env>) -> io::Result<()> {
        // Spawn a task that beats the heart.
        env.spawner.spawn(move || -> Result<()> {
            loop {
                tracing::info!("heartbeat");
                self.update_content();
                self.notify(&env.notifier)?;
                std::thread::sleep(self.update_interval);
            }
        });
        Ok(())
    }

    fn getattr(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        arg: op::Getattr<'_>,
        mut reply: fs::ReplyAttr<'_>,
    ) -> fs::Result {
        if arg.ino() != NodeID::ROOT {
            Err(ENOENT)?;
        }
        let inner = self.inner.lock().unwrap();
        reply.out().attr(inner.attr.clone());
        reply.send()
    }

    fn open(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        arg: op::Open<'_>,
        mut reply: fs::ReplyOpen<'_>,
    ) -> fs::Result {
        if arg.ino() != NodeID::ROOT {
            Err(ENOENT)?;
        }
        reply.out().keep_cache(true);
        reply.send()
    }

    fn read(
        &self,
        _: fs::Env<'_, '_>,
        _: fs::Request<'_>,
        arg: op::Read<'_>,
        reply: fs::ReplyData<'_>,
    ) -> fs::Result {
        if arg.ino() != NodeID::ROOT {
            Err(ENOENT)?
        }

        let inner = self.inner.lock().unwrap();

        let offset = arg.offset() as usize;
        if offset >= inner.content.len() {
            return reply.send(());
        }

        let size = arg.size() as usize;
        let data = &inner.content.as_bytes()[offset..];
        let data = &data[..std::cmp::min(data.len(), size)];
        reply.send(data)
    }

    fn notify_reply(
        &self,
        _: fs::Env<'_, '_>,
        arg: op::NotifyReply<'_>,
        mut data: fs::Data<'_>,
    ) -> io::Result<()> {
        if let Some((_, original)) = self.retrieves.remove(&arg.unique()) {
            let data = {
                let mut buf = vec![0u8; arg.size() as usize];
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
