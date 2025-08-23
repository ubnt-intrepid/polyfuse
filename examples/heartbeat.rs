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
    KernelConfig,
};

use anyhow::{anyhow, ensure, Context as _, Result};
use chrono::Local;
use libc::ENOENT;
use std::{
    collections::HashMap,
    io::{self, prelude::*},
    mem,
    path::PathBuf,
    sync::{mpsc, Mutex},
    time::Duration,
};

const ROOT_INO: u64 = 1;

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
    retrieves: Mutex<HashMap<u64, mpsc::Sender<Vec<u8>>>>,
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
        attr.st_ino = ROOT_INO;
        attr.st_mode = libc::S_IFREG | 0o444;
        attr.st_size = content.len() as libc::off_t;

        Self {
            inner: Mutex::new(Inner { content, attr }),
            retrieves: Mutex::default(),
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

    fn notify(&self, cx: &fs::InitContext<'_, '_>) -> io::Result<()> {
        match self.kind {
            Some(NotifyKind::Store) => {
                let inner = &*self.inner.lock().unwrap();
                let content = &inner.content;

                tracing::info!("send notify_store(data={:?})", content);
                cx.notify_store(ROOT_INO, 0, content)?;

                // To check if the cache is updated correctly, pull the
                // content from the kernel using notify_retrieve.
                tracing::info!("send notify_retrieve");
                let data = {
                    // FIXME: choose appropriate atomic ordering.
                    let unique = cx.notify_retrieve(ROOT_INO, 0, 1024)?;
                    let (tx, rx) = mpsc::channel();
                    self.retrieves.lock().unwrap().insert(unique, tx);
                    rx.recv().unwrap()
                };
                tracing::info!("--> content={:?}", data);

                if data[..content.len()] != *content.as_bytes() {
                    tracing::error!("mismatched data");
                }
            }

            Some(NotifyKind::Invalidate) => {
                tracing::info!("send notify_invalidate_inode");
                cx.notify_inval_inode(ROOT_INO, 0, 0)?;
            }

            None => { /* do nothing */ }
        }
        Ok(())
    }
}

impl Filesystem for Heartbeat {
    fn init<'scope, 'env>(&'env self, cx: fs::InitContext<'scope, 'env>) -> io::Result<()> {
        // Spawn a task that beats the heart.
        cx.spawn({
            let cx = cx.clone();
            move || -> Result<()> {
                loop {
                    tracing::info!("heartbeat");
                    self.update_content();
                    self.notify(&cx)?;
                    std::thread::sleep(self.update_interval);
                }
            }
        });
        Ok(())
    }

    fn getattr(&self, cx: fs::Context<'_>, op: op::Getattr<'_>) -> fs::Result {
        match op.ino() {
            ROOT_INO => {
                let inner = self.inner.lock().unwrap();
                let mut out = AttrOut::default();
                fill_attr(out.attr(), &inner.attr);
                cx.reply(out)
            }
            _ => Err(ENOENT)?,
        }
    }

    fn open(&self, cx: fs::Context<'_>, op: op::Open<'_>) -> fs::Result {
        match op.ino() {
            ROOT_INO => {
                let mut out = OpenOut::default();
                out.keep_cache(true);
                cx.reply(out)
            }
            _ => Err(ENOENT)?,
        }
    }

    fn read(&self, cx: fs::Context<'_>, op: polyfuse::op::Read<'_>) -> fs::Result {
        match op.ino() {
            ROOT_INO => {
                let inner = self.inner.lock().unwrap();

                let offset = op.offset() as usize;
                if offset >= inner.content.len() {
                    cx.reply(())
                } else {
                    let size = op.size() as usize;
                    let data = &inner.content.as_bytes()[offset..];
                    let data = &data[..std::cmp::min(data.len(), size)];
                    cx.reply(data)
                }
            }
            _ => Err(ENOENT)?,
        }
    }

    fn notify_reply(&self, mut cx: fs::Context<'_>, op: op::NotifyReply<'_>) -> io::Result<()> {
        let mut retrieves = self.retrieves.lock().unwrap();
        if let Some(tx) = retrieves.remove(&op.unique()) {
            let mut buf = vec![0u8; op.size() as usize];
            cx.read_exact(&mut buf)?;
            tx.send(buf).unwrap();
        }
        Ok(())
    }
}

fn fill_attr(attr: &mut FileAttr, st: &libc::stat) {
    attr.ino(st.st_ino);
    attr.size(st.st_size as u64);
    attr.mode(st.st_mode);
    attr.nlink(st.st_nlink as u32);
    attr.uid(st.st_uid);
    attr.gid(st.st_gid);
    attr.rdev(st.st_rdev as u32);
    attr.blksize(st.st_blksize as u32);
    attr.blocks(st.st_blocks as u64);
    attr.atime(Duration::new(st.st_atime as u64, st.st_atime_nsec as u32));
    attr.mtime(Duration::new(st.st_mtime as u64, st.st_mtime_nsec as u32));
    attr.ctime(Duration::new(st.st_ctime as u64, st.st_ctime_nsec as u32));
}
