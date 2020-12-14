#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented, clippy::todo)]

use polyfuse::{
    bytes::{write_bytes, Bytes},
    op,
    reply::{AttrOut, EntryOut, FileAttr, ReaddirOut, Reply},
    Config, MountOptions, Operation, Request, Session,
};

use anyhow::Context as _;
use std::{io, os::unix::prelude::*, path::PathBuf, time::Duration};

const TTL: Duration = Duration::from_secs(60 * 60 * 24 * 365);
const ROOT_INO: u64 = 1;
const HELLO_INO: u64 = 2;
const HELLO_FILENAME: &str = "hello.txt";
const HELLO_CONTENT: &[u8] = b"Hello, world!\n";

#[async_std::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.free_from_str()?.context("missing mountpoint")?;
    anyhow::ensure!(mountpoint.is_dir(), "mountpoint must be a directory");

    let session =
        AsyncSession::mount(mountpoint, MountOptions::default(), Config::default()).await?;

    let fs = Hello::new();

    while let Some(req) = session.next_request().await? {
        let reply = ReplyWriter { req: &req };

        match req.operation()? {
            Operation::Lookup(op) => fs.lookup(op, reply).await?,
            Operation::Getattr(op) => fs.getattr(op, reply).await?,
            Operation::Read(op) => fs.read(op, reply).await?,
            Operation::Readdir(op) => fs.readdir(op, reply).await?,
            _ => reply.error(libc::ENOSYS)?,
        }
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

    async fn lookup(&self, op: op::Lookup<'_>, reply: ReplyWriter<'_>) -> io::Result<()> {
        match op.parent() {
            ROOT_INO if op.name().as_bytes() == HELLO_FILENAME.as_bytes() => {
                let mut out = EntryOut::default();
                self.fill_hello_attr(out.attr());
                out.ino(HELLO_INO);
                out.ttl_attr(TTL);
                out.ttl_entry(TTL);
                reply.ok(out)
            }
            _ => reply.error(libc::ENOENT),
        }
    }

    async fn getattr(&self, op: op::Getattr<'_>, reply: ReplyWriter<'_>) -> io::Result<()> {
        let fill_attr = match op.ino() {
            ROOT_INO => Self::fill_root_attr,
            HELLO_INO => Self::fill_hello_attr,
            _ => return reply.error(libc::ENOENT),
        };

        let mut out = AttrOut::default();
        fill_attr(self, out.attr());
        out.ttl(TTL);

        reply.ok(out)
    }

    async fn read(&self, op: op::Read<'_>, reply: ReplyWriter<'_>) -> io::Result<()> {
        match op.ino() {
            HELLO_INO => (),
            ROOT_INO => return reply.error(libc::EISDIR),
            _ => return reply.error(libc::ENOENT),
        }

        let mut data: &[u8] = &[];

        let offset = op.offset() as usize;
        if offset < HELLO_CONTENT.len() {
            let size = op.size() as usize;
            data = &HELLO_CONTENT[offset..];
            data = &data[..std::cmp::min(data.len(), size)];
        }

        reply.ok(data)
    }

    fn dir_entries(&self) -> impl Iterator<Item = (u64, &DirEntry)> + '_ {
        self.entries.iter().enumerate().map(|(i, ent)| {
            let offset = (i + 1) as u64;
            (offset, ent)
        })
    }

    async fn readdir(&self, op: op::Readdir<'_>, reply: ReplyWriter<'_>) -> io::Result<()> {
        if op.ino() != ROOT_INO {
            return reply.error(libc::ENOTDIR);
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

        reply.ok(out)
    }
}

struct ReplyWriter<'req> {
    req: &'req Request,
}

impl ReplyWriter<'_> {
    fn ok<T>(self, arg: T) -> io::Result<()>
    where
        T: Bytes,
    {
        write_bytes(self.req, Reply::new(self.req.unique(), 0, arg))
    }

    fn error(self, code: i32) -> io::Result<()> {
        write_bytes(self.req, Reply::new(self.req.unique(), code, ()))
    }
}

// ==== AsyncSession ====

struct AsyncSession {
    inner: async_io::Async<Session>,
}

impl AsyncSession {
    async fn mount(
        mountpoint: PathBuf,
        mountopts: MountOptions,
        config: Config,
    ) -> io::Result<Self> {
        async_std::task::spawn_blocking(move || {
            let session = Session::mount(mountpoint, mountopts, config)?;
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
