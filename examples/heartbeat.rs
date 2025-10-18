//! test script:
//!
//! ```shell-session
//! $ for i in {1..1000}; do clear; cat /path/to/heartbeat; usleep 500000; done
//! ```

#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]
#![forbid(unsafe_code)]

use polyfuse::{
    bytes::POD,
    mount::MountOptions,
    op::Operation,
    reply::{self, AttrOut, OpenOut, OpenOutFlags},
    request::{SpliceBuf, ToRequestParts},
    session::{KernelConfig, Session},
    types::{FileAttr, FileID, FileMode, FilePermissions, FileType, NodeID, NotifyID},
    Connection,
};

use anyhow::{anyhow, ensure, Context as _, Result};
use chrono::Local;
use dashmap::DashMap;
use polyfuse_kernel::{
    fuse_notify_code, fuse_notify_inval_inode_out, fuse_notify_retrieve_out, fuse_notify_store_out,
};
use rustix::{io::Errno, param::page_size};
use std::{
    borrow::Cow,
    io::{self, prelude::*},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
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

    let fs = Arc::new(Heartbeat::new(kind, update_interval));

    let (conn, mount) = polyfuse::mount::mount(&mountpoint.into(), &MountOptions::new())?;
    let session = Session::init(&conn, KernelConfig::new())?;

    let conn = &conn;
    let session = &session;
    thread::scope(|scope| -> Result<()> {
        // Spawn a task that beats the heart.
        scope.spawn({
            let fs = fs.clone();
            move || -> Result<()> {
                loop {
                    tracing::info!("heartbeat");
                    fs.update_content();
                    fs.notify(session, conn)?;
                    thread::sleep(fs.update_interval);
                }
            }
        });

        let mut buf = SpliceBuf::new(session.request_buffer_size())?;
        while session.recv_request(conn, &mut buf)? {
            let (header, arg, remains) = buf.to_request_parts();
            match Operation::decode(session.config(), header, arg) {
                Ok(Operation::Getattr(op)) => {
                    if op.ino == NodeID::ROOT {
                        let inner = fs.inner.lock().unwrap();
                        session.send_reply(
                            conn,
                            header.unique(),
                            None,
                            AttrOut {
                                attr: Cow::Borrowed(&inner.attr),
                                valid: None,
                            },
                        )?;
                    } else {
                        session.send_reply(conn, header.unique(), Some(Errno::NOENT), ())?;
                    }
                }

                Ok(Operation::Open(op)) => {
                    if op.ino == NodeID::ROOT {
                        session.send_reply(
                            conn,
                            header.unique(),
                            None,
                            OpenOut {
                                fh: FileID::from_raw(0),
                                open_flags: OpenOutFlags::KEEP_CACHE,
                                backing_id: 0,
                            },
                        )?;
                    } else {
                        session.send_reply(conn, header.unique(), Some(Errno::NOENT), ())?;
                    }
                }

                Ok(Operation::Read(op)) => {
                    if op.ino == NodeID::ROOT {
                        let inner = fs.inner.lock().unwrap();

                        let offset = op.offset as usize;
                        if offset >= inner.content.len() {
                            session.send_reply(conn, header.unique(), None, ())?;
                            continue;
                        }

                        let size = op.size as usize;
                        let data = &inner.content.as_bytes()[offset..];
                        let data = &data[..std::cmp::min(data.len(), size)];
                        session.send_reply(conn, header.unique(), None, reply::Raw(data))?;
                    } else {
                        session.send_reply(conn, header.unique(), Some(Errno::NOENT), ())?;
                    }
                }

                Ok(Operation::NotifyReply(op)) => {
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

                _ => session.send_reply(conn, header.unique(), Some(Errno::NOSYS), ())?,
            }
        }

        Ok(())
    })?;

    mount.unmount()?;

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
    next_notify_unique: AtomicU64,
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
            next_notify_unique: AtomicU64::new(0),
        }
    }

    fn update_content(&self) {
        let mut inner = self.inner.lock().unwrap();
        let content = Local::now().to_rfc3339();
        inner.attr.size = content.len() as u64;
        inner.content = content;
    }

    fn notify(&self, session: &Session, conn: &Connection) -> io::Result<()> {
        match self.kind {
            Some(NotifyKind::Store) => {
                let inner = &*self.inner.lock().unwrap();
                let content = inner.content.clone();

                tracing::info!("send notify_store(data={:?})", content);
                session.send_notify(
                    conn,
                    fuse_notify_code::FUSE_NOTIFY_STORE,
                    (
                        POD(fuse_notify_store_out {
                            nodeid: NodeID::ROOT.into_raw(),
                            offset: 0,
                            size: content.len() as u32,
                            padding: 0,
                        }),
                        &content,
                    ),
                )?;

                // To check if the cache is updated correctly, pull the
                // content from the kernel using notify_retrieve.
                tracing::info!("send notify_retrieve");
                let notify_unique = self.next_notify_unique.fetch_add(1, Ordering::AcqRel);
                session.send_notify(
                    conn,
                    fuse_notify_code::FUSE_NOTIFY_RETRIEVE,
                    POD(fuse_notify_retrieve_out {
                        notify_unique,
                        nodeid: NodeID::ROOT.into_raw(),
                        offset: 0,
                        size: page_size() as u32,
                        padding: 0,
                    }),
                )?;
                self.retrieves
                    .insert(NotifyID::from_raw(notify_unique), content);
            }

            Some(NotifyKind::Invalidate) => {
                tracing::info!("send notify_invalidate_inode");
                session.send_notify(
                    conn,
                    fuse_notify_code::FUSE_NOTIFY_INVAL_INODE,
                    POD(fuse_notify_inval_inode_out {
                        ino: NodeID::ROOT.into_raw(),
                        off: 0,
                        len: 0,
                    }),
                )?;
            }

            None => { /* do nothing */ }
        }
        Ok(())
    }
}
