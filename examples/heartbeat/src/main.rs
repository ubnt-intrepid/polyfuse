//! test script:
//!
//! ```shell-session
//! $ for i in {1..1000}; do clear; cat /path/to/heartbeat; usleep 500000; done
//! ```

#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

use polyfuse::{
    reply::{AttrOut, FileAttr, OpenOut},
    Connection, KernelConfig, MountOptions, Operation, Session,
};

use anyhow::{anyhow, ensure, Context as _, Result};
use chrono::Local;
use std::{
    collections::HashMap,
    io::{self, prelude::*},
    mem,
    path::PathBuf,
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

const ROOT_INO: u64 = 1;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let no_notify = args.contains("--no-notify");
    let notify_kind = args
        .opt_value_from_os_str("--notify-kind", |s| match s.to_str() {
            Some("store") => Ok(NotifyKind::Store),
            Some("invalidate") => Ok(NotifyKind::Invalidate),
            s => Err(anyhow!("invalid notify kind: {:?}", s)),
        })?
        .unwrap_or(NotifyKind::Store);
    let update_interval: u64 = args.value_from_str("--update-interval")?;

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    let conn = MountOptions::default().mount(mountpoint).map(Arc::new)?;
    let session = Session::init(&conn, KernelConfig::default()).map(Arc::new)?;

    let heartbeat = Arc::new(Heartbeat::now());

    // Spawn a task that beats the heart.
    std::thread::spawn({
        let heartbeat = heartbeat.clone();
        let session = session.clone();
        let conn = conn.clone();
        let notifier = if !no_notify {
            Some(session.clone())
        } else {
            None
        };
        move || -> Result<()> {
            loop {
                tracing::info!("heartbeat");

                heartbeat.update_content();

                if let Some(ref notifier) = notifier {
                    match notify_kind {
                        NotifyKind::Store => heartbeat.notify_store(&conn, notifier)?,
                        NotifyKind::Invalidate => heartbeat.notify_inval_inode(&conn, notifier)?,
                    }
                }

                std::thread::sleep(Duration::from_secs(update_interval));
            }
        }
    });

    // Run the filesystem daemon on the foreground.
    while let Some(req) = session.next_request(&conn)? {
        let heartbeat = heartbeat.clone();
        let session = session.clone();
        let conn = conn.clone();

        std::thread::spawn(move || -> Result<()> {
            let span = tracing::debug_span!("handle request", unique = req.unique());
            let _enter = span.enter();

            let op = req.operation(&session)?;
            tracing::debug!(?op);

            match op {
                Operation::Getattr(op) => match op.ino() {
                    ROOT_INO => {
                        let inner = heartbeat.inner.lock().unwrap();
                        let mut out = AttrOut::default();
                        fill_attr(out.attr(), &inner.attr);
                        session.reply(&conn, &req, out)?;
                    }
                    _ => session.reply_error(&conn, &req, libc::ENOENT)?,
                },
                Operation::Open(op) => match op.ino() {
                    ROOT_INO => {
                        let mut out = OpenOut::default();
                        out.keep_cache(true);
                        session.reply(&conn, &req, out)?;
                    }
                    _ => session.reply_error(&conn, &req, libc::ENOENT)?,
                },
                Operation::Read(op) => match op.ino() {
                    ROOT_INO => {
                        let inner = heartbeat.inner.lock().unwrap();

                        let offset = op.offset() as usize;
                        if offset >= inner.content.len() {
                            session.reply(&conn, &req, &[])?;
                        } else {
                            let size = op.size() as usize;
                            let data = &inner.content.as_bytes()[offset..];
                            let data = &data[..std::cmp::min(data.len(), size)];
                            session.reply(&conn, &req, data)?;
                        }
                    }
                    _ => session.reply_error(&conn, &req, libc::ENOENT)?,
                },
                Operation::NotifyReply(op, mut data) => {
                    let mut retrieves = heartbeat.retrieves.lock().unwrap();
                    if let Some(tx) = retrieves.remove(&op.unique()) {
                        let mut buf = vec![0u8; op.size() as usize];
                        data.read_exact(&mut buf)?;
                        tx.send(buf).unwrap();
                    }
                }

                _ => session.reply_error(&conn, &req, libc::ENOSYS)?,
            }

            Ok(())
        });
    }

    Ok(())
}

#[derive(Debug, Copy, Clone, PartialEq)]
enum NotifyKind {
    Store,
    Invalidate,
}

struct Heartbeat {
    inner: Mutex<Inner>,
    retrieves: Mutex<HashMap<u64, mpsc::Sender<Vec<u8>>>>,
}

struct Inner {
    content: String,
    attr: libc::stat,
}

impl Heartbeat {
    fn now() -> Self {
        let content = Local::now().to_rfc3339();

        let mut attr = unsafe { mem::zeroed::<libc::stat>() };
        attr.st_ino = ROOT_INO;
        attr.st_mode = libc::S_IFREG | 0o444;
        attr.st_size = content.len() as libc::off_t;

        Self {
            inner: Mutex::new(Inner { content, attr }),
            retrieves: Mutex::default(),
        }
    }

    fn update_content(&self) {
        let mut inner = self.inner.lock().unwrap();
        let content = Local::now().to_rfc3339();
        inner.attr.st_size = content.len() as libc::off_t;
        inner.content = content;
    }

    fn notify_store(&self, conn: &Connection, notifier: &Session) -> io::Result<()> {
        let inner = self.inner.lock().unwrap();
        let content = &inner.content;

        tracing::info!("send notify_store(data={:?})", content);
        notifier.store(conn, ROOT_INO, 0, content)?;

        // To check if the cache is updated correctly, pull the
        // content from the kernel using notify_retrieve.
        tracing::info!("send notify_retrieve");
        let data = {
            // FIXME: choose appropriate atomic ordering.
            let unique = notifier.retrieve(conn, ROOT_INO, 0, 1024)?;
            let (tx, rx) = mpsc::channel();
            self.retrieves.lock().unwrap().insert(unique, tx);
            rx.recv().unwrap()
        };
        tracing::info!("--> content={:?}", data);

        if data[..content.len()] != *content.as_bytes() {
            tracing::error!("mismatched data");
        }

        Ok(())
    }

    fn notify_inval_inode(&self, conn: &Connection, notifier: &Session) -> io::Result<()> {
        tracing::info!("send notify_invalidate_inode");
        notifier.inval_inode(conn, ROOT_INO, 0, 0)?;
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
