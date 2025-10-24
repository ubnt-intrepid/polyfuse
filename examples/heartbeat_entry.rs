//! test script:
//!
//! ```shell-session
//! $ for i in {1..1000}; do clear; ls -al /path/to/heartbeat_entry; usleep 500000; done
//! ```

#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented, clippy::todo)]
#![forbid(unsafe_code)]

use polyfuse::{
    mount::MountOptions,
    op::Operation,
    reply::{AttrOut, EntryOut, ReaddirOut},
    request::SpliceBuf,
    session::{KernelConfig, Session},
    types::{FileAttr, FileMode, FilePermissions, FileType, NodeID},
    Connection,
};

use anyhow::{ensure, Context as _, Result};
use chrono::Local;
use rustix::io::Errno;
use std::{
    borrow::Cow,
    io, mem,
    os::unix::prelude::*,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

const FILE_INO: NodeID = match NodeID::from_raw(2) {
    Some(ino) => ino,
    None => panic!("unreachable"),
};

const DEFAULT_TTL: Duration = Duration::from_secs(0);
const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);

fn main() -> Result<()> {
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

    let (session, conn, mount) =
        polyfuse::session::connect(mountpoint.into(), MountOptions::new(), KernelConfig::new())?;

    let session = &session;
    let conn = &conn;
    thread::scope(|scope| -> Result<()> {
        // spawn heartbeat thread.
        scope.spawn({
            let fs = fs.clone();
            move || fs.heartbeat(session, conn)
        });

        let mut buf = SpliceBuf::new(session.request_buffer_size())?;
        while session.recv_request(conn, &mut buf)? {
            let (req, op) = session.decode(&mut buf)?;
            match op {
                Some(Operation::Lookup(op)) => {
                    if op.parent != NodeID::ROOT {
                        req.reply_error(conn, Errno::NOTDIR)?;
                        continue;
                    }

                    let mut current = fs.current.lock().unwrap();

                    if op.name.as_bytes() != current.filename.as_bytes() {
                        req.reply_error(conn, Errno::NOENT)?;
                        continue;
                    }

                    req.reply(
                        conn,
                        EntryOut {
                            ino: Some(fs.file_attr.ino),
                            attr: Cow::Borrowed(&fs.file_attr),
                            entry_valid: Some(fs.ttl),
                            attr_valid: Some(fs.ttl),
                            generation: 0,
                        },
                    )?;

                    current.nlookup += 1;
                }

                Some(Operation::Forget(forgets)) => {
                    let mut current = fs.current.lock().unwrap();
                    for forget in forgets.as_ref() {
                        if forget.ino() == FILE_INO {
                            current.nlookup -= forget.nlookup();
                        }
                    }
                }

                Some(Operation::Getattr(op)) => {
                    let attr = match op.ino {
                        NodeID::ROOT => &fs.root_attr,
                        FILE_INO => &fs.file_attr,
                        _ => {
                            req.reply_error(conn, Errno::NOENT)?;
                            continue;
                        }
                    };

                    req.reply(
                        conn,
                        AttrOut {
                            attr: Cow::Borrowed(attr),
                            valid: Some(fs.ttl),
                        },
                    )?;
                }

                Some(Operation::Read(op)) => match op.ino {
                    NodeID::ROOT => req.reply_error(conn, Errno::ISDIR)?,
                    FILE_INO => req.reply(conn, ())?,
                    _ => req.reply_error(conn, Errno::NOENT)?,
                },

                Some(Operation::Readdir(op)) => {
                    if op.ino != NodeID::ROOT {
                        req.reply_error(conn, Errno::NOTDIR)?;
                        continue;
                    }
                    if op.offset > 0 {
                        req.reply(conn, ())?;
                        continue;
                    }

                    let mut buf = ReaddirOut::new(op.size as usize);
                    let current = fs.current.lock().unwrap();
                    buf.push_entry(current.filename.as_ref(), FILE_INO, None, 1);

                    req.reply(conn, buf)?;
                }

                _ => req.reply_error(conn, Errno::NOSYS)?,
            }
        }

        Ok(())
    })?;

    mount.unmount()?;

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

    fn heartbeat(&self, session: &Session, conn: &Connection) -> io::Result<()> {
        let span = tracing::debug_span!("heartbeat", notify = !self.no_notify);
        let _enter = span.enter();

        loop {
            tracing::info!("heartbeat");

            let new_filename = generate_filename();
            let mut current = self.current.lock().unwrap();
            tracing::debug!(filename = ?current.filename, nlookup = ?current.nlookup);
            tracing::debug!(?new_filename);
            let old_filename = mem::replace(&mut current.filename, new_filename);

            if !self.no_notify && current.nlookup > 0 {
                tracing::info!("send notify_inval_entry");
                session.notify_inval_entry(conn, NodeID::ROOT, old_filename)?;
            }

            drop(current);

            std::thread::sleep(self.update_interval);
        }
    }
}
