//! test script:
//!
//! ```shell-session
//! $ for i in {1..1000}; do clear; ls -al /path/to/heartbeat_entry; usleep 500000; done
//! ```

#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented, clippy::todo)]

use polyfuse::{
    reply::{AttrOut, EntryOut, FileAttr, ReaddirOut},
    Connection, KernelConfig, MountOptions, Operation, Request, Session,
};

use anyhow::{ensure, Context as _, Result};
use chrono::Local;
use std::{
    mem,
    os::unix::prelude::*,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

const ROOT_INO: u64 = 1;
const FILE_INO: u64 = 2;

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

    let conn = MountOptions::default().mount(mountpoint).map(Arc::new)?;
    let session = Session::init(&conn, KernelConfig::default()).map(Arc::new)?;

    let fs = {
        let mut root_attr = unsafe { mem::zeroed::<libc::stat>() };
        root_attr.st_ino = ROOT_INO;
        root_attr.st_mode = libc::S_IFDIR | 0o555;
        root_attr.st_nlink = 1;

        let mut file_attr = unsafe { mem::zeroed::<libc::stat>() };
        file_attr.st_ino = FILE_INO;
        file_attr.st_mode = libc::S_IFREG | 0o444;
        file_attr.st_nlink = 1;

        Arc::new(Heartbeat {
            root_attr,
            file_attr,
            ttl,
            update_interval,
            current: Mutex::new(CurrentFile {
                filename: generate_filename(),
                nlookup: 0,
            }),
        })
    };

    // Spawn a task that beats the heart.
    std::thread::spawn({
        let fs = fs.clone();
        let conn = conn.clone();
        let notifier = if !no_notify {
            Some(session.clone())
        } else {
            None
        };
        move || -> Result<()> {
            fs.heartbeat(&conn, notifier)?;
            Ok(())
        }
    });

    while let Some(req) = session.next_request(&conn)? {
        let fs = fs.clone();
        let session = session.clone();
        let conn = conn.clone();
        std::thread::spawn(move || -> Result<()> {
            fs.handle_request(&session, &conn, &req)?;
            Ok(())
        });
    }

    Ok(())
}

fn generate_filename() -> String {
    Local::now().format("Time_is_%Hh_%Mm_%Ss").to_string()
}

struct Heartbeat {
    root_attr: libc::stat,
    file_attr: libc::stat,
    ttl: Duration,
    update_interval: Duration,
    current: Mutex<CurrentFile>,
}

#[derive(Debug)]
struct CurrentFile {
    filename: String,
    nlookup: u64,
}

impl Heartbeat {
    fn heartbeat(&self, conn: &Connection, notifier: Option<Arc<Session>>) -> Result<()> {
        let span = tracing::debug_span!("heartbeat", notify = notifier.is_some());
        let _enter = span.enter();

        loop {
            tracing::info!("heartbeat");

            let new_filename = generate_filename();
            let mut current = self.current.lock().unwrap();
            tracing::debug!(filename = ?current.filename, nlookup = ?current.nlookup);
            tracing::debug!(?new_filename);
            let old_filename = mem::replace(&mut current.filename, new_filename);

            match notifier {
                Some(ref notifier) if current.nlookup > 0 => {
                    tracing::info!("send notify_inval_entry");
                    notifier.inval_entry(conn, ROOT_INO, old_filename)?;
                }
                _ => (),
            }

            drop(current);

            std::thread::sleep(self.update_interval);
        }
    }

    fn handle_request(&self, session: &Session, conn: &Connection, req: &Request) -> Result<()> {
        let span = tracing::debug_span!("handle_request", unique = req.unique());
        let _enter = span.enter();

        let op = req.operation(session)?;
        tracing::debug!(?op);

        match op {
            Operation::Lookup(op) => match op.parent() {
                ROOT_INO => {
                    let mut current = self.current.lock().unwrap();

                    if op.name().as_bytes() == current.filename.as_bytes() {
                        let mut out = EntryOut::default();
                        out.ino(self.file_attr.st_ino);
                        fill_attr(out.attr(), &self.file_attr);
                        out.ttl_entry(self.ttl);
                        out.ttl_attr(self.ttl);

                        session.reply(conn, req, out)?;

                        current.nlookup += 1;
                    } else {
                        session.reply_error(conn, req, libc::ENOENT)?;
                    }
                }
                _ => session.reply_error(conn, req, libc::ENOTDIR)?,
            },

            Operation::Forget(forgets) => {
                let mut current = self.current.lock().unwrap();
                for forget in forgets.as_ref() {
                    if forget.ino() == FILE_INO {
                        current.nlookup -= forget.nlookup();
                    }
                }
            }

            Operation::Getattr(op) => {
                let attr = match op.ino() {
                    ROOT_INO => &self.root_attr,
                    FILE_INO => &self.file_attr,
                    _ => {
                        return session
                            .reply_error(conn, req, libc::ENOENT)
                            .map_err(Into::into)
                    }
                };

                let mut out = AttrOut::default();
                fill_attr(out.attr(), attr);
                out.ttl(self.ttl);

                session.reply(conn, req, out)?;
            }

            Operation::Read(op) => match op.ino() {
                ROOT_INO => session.reply_error(conn, req, libc::EISDIR)?,
                FILE_INO => session.reply(conn, req, &[])?,
                _ => session.reply_error(conn, req, libc::ENOENT)?,
            },

            Operation::Readdir(op) => match op.ino() {
                ROOT_INO => {
                    if op.offset() == 0 {
                        let current = self.current.lock().unwrap();

                        let mut out = ReaddirOut::new(op.size() as usize);
                        out.entry(current.filename.as_ref(), FILE_INO, 0, 1);
                        session.reply(conn, req, out)?;
                    } else {
                        session.reply(conn, req, &[])?;
                    }
                }
                _ => session.reply_error(conn, req, libc::ENOTDIR)?,
            },

            _ => session.reply_error(conn, req, libc::ENOSYS)?,
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
