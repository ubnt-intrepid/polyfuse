#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented, clippy::todo)]

use polyfuse::{
    op,
    reply::{AttrOut, EntryOut, FileAttr, ReaddirOut},
    KernelConfig, Operation, Request, Session,
};

use anyhow::{ensure, Context as _, Result};
use async_std::task::{self, JoinHandle};
use std::{io, os::unix::prelude::*, path::PathBuf, sync::Arc, time::Duration};

const TTL: Duration = Duration::from_secs(60 * 60 * 24 * 365);
const ROOT_INO: u64 = 1;
const HELLO_INO: u64 = 2;
const HELLO_FILENAME: &str = "hello.txt";
const HELLO_CONTENT: &[u8] = b"Hello, world!\n";

#[async_std::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_dir(), "mountpoint must be a directory");

    let session = AsyncSession::mount(mountpoint, KernelConfig::default()).await?;

    let fs = Arc::new(Hello::new());

    while let Some(req) = session.next_request().await? {
        let fs = fs.clone();

        let _: JoinHandle<Result<()>> = task::spawn(async move {
            match req.operation()? {
                Operation::Lookup(op) => fs.lookup(&req, op).await?,
                Operation::Getattr(op) => fs.getattr(&req, op).await?,
                Operation::Read(op) => fs.read(&req, op).await?,
                Operation::Readdir(op) => fs.readdir(&req, op).await?,
                _ => req.reply_error(libc::ENOSYS)?,
            }

            Ok(())
        });
    }

    Ok(())
}

struct Hello {
    entries: Vec<DirEntry>,
    uid: u32,
    gid: u32,
}

struct DirEntry {
    name: &'static str,
    ino: u64,
    typ: u32,
}

impl Hello {
    fn new() -> Self {
        let mut entries = Vec::with_capacity(3);
        entries.push(DirEntry {
            name: ".",
            ino: ROOT_INO,
            typ: libc::DT_DIR as u32,
        });
        entries.push(DirEntry {
            name: "..",
            ino: ROOT_INO,
            typ: libc::DT_DIR as u32,
        });
        entries.push(DirEntry {
            name: HELLO_FILENAME,
            ino: HELLO_INO,
            typ: libc::DT_REG as u32,
        });

        Self {
            entries,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
        }
    }

    fn fill_root_attr(&self, attr: &mut FileAttr) {
        attr.ino(ROOT_INO);
        attr.mode(libc::S_IFDIR as u32 | 0o555);
        attr.nlink(2);
        attr.uid(self.uid);
        attr.gid(self.gid);
    }

    fn fill_hello_attr(&self, attr: &mut FileAttr) {
        attr.ino(HELLO_INO);
        attr.size(HELLO_CONTENT.len() as u64);
        attr.mode(libc::S_IFREG as u32 | 0o444);
        attr.nlink(1);
        attr.uid(self.uid);
        attr.gid(self.gid);
    }

    async fn lookup(&self, req: &Request, op: op::Lookup<'_>) -> io::Result<()> {
        match op.parent() {
            ROOT_INO if op.name().as_bytes() == HELLO_FILENAME.as_bytes() => {
                let mut out = EntryOut::default();
                self.fill_hello_attr(out.attr());
                out.ino(HELLO_INO);
                out.ttl_attr(TTL);
                out.ttl_entry(TTL);
                req.reply(out)
            }
            _ => req.reply_error(libc::ENOENT),
        }
    }

    async fn getattr(&self, req: &Request, op: op::Getattr<'_>) -> io::Result<()> {
        let mut out = AttrOut::default();
        match op.ino() {
            ROOT_INO => self.fill_root_attr(out.attr()),
            HELLO_INO => self.fill_hello_attr(out.attr()),
            _ => return req.reply_error(libc::ENOENT),
        };

        out.ttl(TTL);

        req.reply(out)
    }

    async fn read(&self, req: &Request, op: op::Read<'_>) -> io::Result<()> {
        match op.ino() {
            HELLO_INO => (),
            ROOT_INO => return req.reply_error(libc::EISDIR),
            _ => return req.reply_error(libc::ENOENT),
        }

        let mut data: &[u8] = &[];

        let offset = op.offset() as usize;
        if offset < HELLO_CONTENT.len() {
            let size = op.size() as usize;
            data = &HELLO_CONTENT[offset..];
            data = &data[..std::cmp::min(data.len(), size)];
        }

        req.reply(data)
    }

    fn dir_entries(&self) -> impl Iterator<Item = (u64, &DirEntry)> + '_ {
        self.entries.iter().enumerate().map(|(i, ent)| {
            let offset = (i + 1) as u64;
            (offset, ent)
        })
    }

    async fn readdir(&self, req: &Request, op: op::Readdir<'_>) -> io::Result<()> {
        if op.ino() != ROOT_INO {
            return req.reply_error(libc::ENOTDIR);
        }

        let mut out = ReaddirOut::new(op.size() as usize);

        for (i, entry) in self.dir_entries().skip(op.offset() as usize) {
            let full = out.entry(
                entry.name.as_ref(), //
                entry.ino,
                entry.typ,
                i + 1,
            );
            if full {
                break;
            }
        }

        req.reply(out)
    }
}

// ==== AsyncSession ====

struct AsyncSession {
    inner: async_io::Async<Session>,
}

impl AsyncSession {
    async fn mount(mountpoint: PathBuf, config: KernelConfig) -> io::Result<Self> {
        async_std::task::spawn_blocking(move || {
            let session = Session::mount(mountpoint, config)?;
            Ok(Self {
                inner: async_io::Async::new(session)?,
            })
        })
        .await
    }

    async fn next_request(&self) -> io::Result<Option<Request>> {
        use futures::{future::poll_fn, ready, task::Poll};

        poll_fn(|cx| loop {
            match self.inner.get_ref().next_request() {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    ready!(self.inner.poll_readable(cx))?;
                    continue;
                }
                res => return Poll::Ready(res),
            }
        })
        .await
    }
}
