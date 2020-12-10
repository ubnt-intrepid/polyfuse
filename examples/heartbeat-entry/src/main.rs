//! test script:
//!
//! ```shell-session
//! $ for i in {1..1000}; do clear; ls -al /path/to/heartbeat_entry; usleep 500000; done
//! ```

#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented, clippy::todo)]

use polyfuse::{
    reply::{AttrOut, EntryOut, FileAttr, ReaddirOut, Reply},
    Config, MountOptions, Operation, Request, Session,
};
use polyfuse_example_async_std_support::{AsyncConnection, Writer};

use anyhow::{ensure, Context as _, Result};
use async_std::{sync::Mutex, task};
use chrono::Local;
use std::{io, mem, os::unix::prelude::*, path::PathBuf, sync::Arc, time::Duration};

const ROOT_INO: u64 = 1;
const FILE_INO: u64 = 2;

#[async_std::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let no_notify = args.contains("--no-notify");
    let timeout = args.value_from_str("--timeout").map(Duration::from_secs)?;
    let update_interval = args
        .value_from_str("--update-interval")
        .map(Duration::from_secs)?;

    let mountpoint: PathBuf = args.free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_dir(), "mountpoint must be a directory");

    let conn = AsyncConnection::open(mountpoint, MountOptions::default()).await?;
    let session = Session::start(&conn, &conn, Config::default()).await?;

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
            timeout,
            update_interval,
            current: Mutex::new(CurrentFile {
                filename: generate_filename(),
                nlookup: 0,
            }),
        })
    };

    // Spawn a task that beats the heart.
    task::spawn({
        let heartbeat = fs.clone();
        let mut notifier = None;
        if !no_notify {
            notifier = Some(Notifier {
                session: Arc::clone(&session),
                writer: conn.writer(),
            });
        }
        heartbeat.heartbeat(notifier)
    });

    while let Some(req) = session.next_request(&conn).await? {
        let fs = fs.clone();
        let writer = conn.writer();
        let _ = task::spawn(fs.handle_request(req, writer));
    }

    Ok(())
}

fn generate_filename() -> String {
    Local::now().format("Time_is_%Hh_%Mm_%Ss").to_string()
}

struct Notifier {
    session: Arc<Session>,
    writer: Writer,
}

struct Heartbeat {
    root_attr: libc::stat,
    file_attr: libc::stat,
    timeout: Duration,
    update_interval: Duration,
    current: Mutex<CurrentFile>,
}

#[derive(Debug)]
struct CurrentFile {
    filename: String,
    nlookup: u64,
}

impl Heartbeat {
    async fn heartbeat(self: Arc<Self>, notifier: Option<Notifier>) -> io::Result<()> {
        let span = tracing::debug_span!("heartbeat", notify = notifier.is_some());
        loop {
            let new_filename = generate_filename();
            let mut current = self.current.lock().await;
            span.in_scope(|| {
                tracing::debug!(filename = ?current.filename, nlookup = ?current.nlookup);
                tracing::debug!(?new_filename);
            });
            let old_filename = mem::replace(&mut current.filename, new_filename);

            match notifier {
                Some(Notifier {
                    ref session,
                    ref writer,
                }) if current.nlookup > 0 => {
                    span.in_scope(|| tracing::debug!("send notify_inval_entry"));
                    session.notify_inval_entry(writer, ROOT_INO, old_filename)?;
                }
                _ => (),
            }

            drop(current);

            task::sleep(self.update_interval).await;
        }
    }

    async fn handle_request(self: Arc<Self>, req: Request, writer: Writer) -> anyhow::Result<()> {
        let span = tracing::debug_span!("handle_request", unique = req.unique());

        let op = req.operation()?;
        span.in_scope(|| tracing::debug!(?op));

        let reply = ReplyWriter { req: &req, writer };

        match op {
            Operation::Lookup(op) => match op.parent() {
                ROOT_INO => {
                    let mut current = self.current.lock().await;

                    if op.name().as_bytes() == current.filename.as_bytes() {
                        let mut out = EntryOut::default();
                        out.ino(self.file_attr.st_ino);
                        fill_attr(out.attr(), &self.file_attr);
                        out.ttl_entry(self.timeout);
                        out.ttl_attr(self.timeout);

                        reply.ok(out)?;

                        current.nlookup += 1;
                    } else {
                        reply.error(libc::ENOENT)?;
                    }
                }
                _ => reply.error(libc::ENOTDIR)?,
            },

            Operation::Forget(forgets) => {
                let mut current = self.current.lock().await;
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
                    _ => return reply.error(libc::ENOENT).map_err(Into::into),
                };

                let mut out = AttrOut::default();
                fill_attr(out.attr(), attr);
                out.ttl(self.timeout);

                reply.ok(out)?;
            }

            Operation::Read(op) => match op.ino() {
                ROOT_INO => reply.error(libc::EISDIR)?,
                FILE_INO => reply.ok(&[])?,
                _ => reply.error(libc::ENOENT)?,
            },

            Operation::Readdir(op) => match op.ino() {
                ROOT_INO => {
                    if op.offset() == 0 {
                        let current = self.current.lock().await;

                        let mut out = ReaddirOut::new(op.size() as usize);
                        out.entry(current.filename.as_ref(), FILE_INO, 0, 1);
                        reply.ok(out)?;
                    } else {
                        reply.ok(&[])?;
                    }
                }
                _ => reply.error(libc::ENOTDIR)?,
            },

            _ => reply.error(libc::ENOSYS)?,
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

struct ReplyWriter<'req> {
    req: &'req Request,
    writer: Writer,
}

impl ReplyWriter<'_> {
    fn ok<T>(self, arg: T) -> io::Result<()>
    where
        T: polyfuse::bytes::Bytes,
    {
        polyfuse::bytes::write_bytes(&self.writer, Reply::new(self.req.unique(), 0, arg))
    }

    fn error(self, code: i32) -> io::Result<()> {
        polyfuse::bytes::write_bytes(&self.writer, Reply::new(self.req.unique(), code, ()))
    }
}
