//! test script:
//!
//! ```shell-session
//! $ for i in {1..1000}; do clear; cat /path/to/heartbeat; usleep 500000; done
//! ```

#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

use polyfuse::{
    fs::{self, Filesystem},
    mount::MountOptions,
    op,
    reply::{AttrOut, FileAttr, OpenOut},
    types::{DeviceID, NodeID, NotifyID, GID, UID},
    KernelConfig,
};

use anyhow::{anyhow, ensure, Context as _, Result};
use chrono::Local;
use dashmap::DashMap;
use libc::{ENOENT, S_IFREG};
use std::{
    io::{self, prelude::*},
    mem,
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
    attr: libc::stat,
}

impl Heartbeat {
    fn new(kind: Option<NotifyKind>, update_interval: Duration) -> Self {
        let content = Local::now().to_rfc3339();

        let mut attr = unsafe { mem::zeroed::<libc::stat>() };
        attr.st_ino = NodeID::ROOT.into_raw();
        attr.st_mode = S_IFREG | 0o444;
        attr.st_size = content.len() as libc::off_t;

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
        inner.attr.st_size = content.len() as libc::off_t;
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
    fn init<'env>(&'env self, cx: fs::Context<'_, 'env>) -> io::Result<()> {
        // Spawn a task that beats the heart.
        cx.spawner.spawn(move || -> Result<()> {
            loop {
                tracing::info!("heartbeat");
                self.update_content();
                self.notify(&cx.notifier)?;
                std::thread::sleep(self.update_interval);
            }
        });
        Ok(())
    }

    fn getattr(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Getattr<'_>>) -> fs::Result {
        if req.arg().ino() != NodeID::ROOT {
            Err(ENOENT)?;
        }
        let inner = self.inner.lock().unwrap();
        let mut out = AttrOut::default();
        fill_attr(out.attr(), &inner.attr);
        req.reply(out)
    }

    fn open(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Open<'_>>) -> fs::Result {
        if req.arg().ino() != NodeID::ROOT {
            Err(ENOENT)?;
        }
        let mut out = OpenOut::default();
        out.keep_cache(true);
        req.reply(out)
    }

    fn read(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Read<'_>>) -> fs::Result {
        if req.arg().ino() != NodeID::ROOT {
            Err(ENOENT)?
        }

        let inner = self.inner.lock().unwrap();

        let offset = req.arg().offset() as usize;
        if offset >= inner.content.len() {
            return req.reply(());
        }

        let size = req.arg().size() as usize;
        let data = &inner.content.as_bytes()[offset..];
        let data = &data[..std::cmp::min(data.len(), size)];
        req.reply(data)
    }

    fn notify_reply(
        &self,
        _: fs::Context<'_, '_>,
        req: fs::Request<'_, op::NotifyReply<'_>>,
        mut data: fs::Data<'_>,
    ) -> io::Result<()> {
        if let Some((_, original)) = self.retrieves.remove(&req.arg().unique()) {
            let data = {
                let mut buf = vec![0u8; req.arg().size() as usize];
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

fn fill_attr(attr: &mut FileAttr, st: &libc::stat) {
    attr.ino(NodeID::from_raw(st.st_ino));
    attr.size(st.st_size as u64);
    attr.mode(st.st_mode);
    attr.nlink(st.st_nlink as u32);
    attr.uid(UID::from_raw(st.st_uid));
    attr.gid(GID::from_raw(st.st_gid));
    attr.rdev(DeviceID::from_userspace_dev(st.st_rdev));
    attr.blksize(st.st_blksize as u32);
    attr.blocks(st.st_blocks as u64);
    attr.atime(Duration::new(st.st_atime as u64, st.st_atime_nsec as u32));
    attr.mtime(Duration::new(st.st_mtime as u64, st.st_mtime_nsec as u32));
    attr.ctime(Duration::new(st.st_ctime as u64, st.st_ctime_nsec as u32));
}
