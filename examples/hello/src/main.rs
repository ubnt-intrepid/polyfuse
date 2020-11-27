#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

use polyfuse::{
    op,
    reply::{self, AttrOut, EntryOut, FileAttr},
    Config, Operation, Session,
};
use polyfuse_async_std::Connection;

use anyhow::Context as _;
use std::{os::unix::prelude::*, path::PathBuf, time::Duration};

const TTL: Duration = Duration::from_secs(60 * 60 * 24 * 365);
const ROOT_INO: u64 = 1;
const HELLO_INO: u64 = 2;
const HELLO_FILENAME: &str = "hello.txt";
const HELLO_CONTENT: &[u8] = b"Hello, world!\n";

#[async_std::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args
        .free_from_str()?
        .context("missing mountpoint specified")?;
    anyhow::ensure!(mountpoint.is_dir(), "the mountpoint must be a directory");

    let conn = Connection::open(&mountpoint, &[]).await?;

    let session = Session::start(&conn, Config::default()).await?;

    let fs = Hello::new();

    while let Some(req) = session.next_request(&conn).await? {
        let op = req.operation(&conn)?;
        let _replied = match op {
            Operation::Lookup { op, reply, .. } => fs.lookup(op, reply).await,
            Operation::Getattr { op, reply, .. } => fs.getattr(op, reply).await,
            Operation::Read { op, reply, .. } => fs.read(op, reply).await,
            Operation::Readdir { op, reply, .. } => fs.readdir(op, reply).await,
            _ => op.unimplemented(),
        };
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

    fn fill_root_attr(&self, attr: &mut dyn FileAttr) {
        attr.ino(ROOT_INO);
        attr.mode(libc::S_IFDIR as u32 | 0o555);
        attr.nlink(2);
        attr.uid(self.uid);
        attr.gid(self.gid);
    }

    fn fill_hello_attr(&self, attr: &mut dyn FileAttr) {
        attr.ino(HELLO_INO);
        attr.size(HELLO_CONTENT.len() as u64);
        attr.mode(libc::S_IFREG as u32 | 0o444);
        attr.nlink(1);
        attr.uid(self.uid);
        attr.gid(self.gid);
    }

    async fn lookup<Op, R>(&self, op: Op, reply: R) -> Result<R::Ok, R::Error>
    where
        Op: op::Lookup,
        R: reply::ReplyEntry,
    {
        match op.parent() {
            ROOT_INO if op.name().as_bytes() == HELLO_FILENAME.as_bytes() => {
                reply.send(|out: &mut dyn EntryOut| {
                    self.fill_hello_attr(out.attr());
                    out.ino(HELLO_INO);
                    out.ttl_attr(TTL);
                    out.ttl_entry(TTL);
                })
            }
            _ => reply.error(libc::ENOENT),
        }
    }

    async fn getattr<Op, R>(&self, op: Op, reply: R) -> Result<R::Ok, R::Error>
    where
        Op: op::Getattr,
        R: reply::ReplyAttr,
    {
        let fill_attr = match op.ino() {
            ROOT_INO => Self::fill_root_attr,
            HELLO_INO => Self::fill_hello_attr,
            _ => return reply.error(libc::ENOENT),
        };

        reply.send(|out: &mut dyn AttrOut| {
            fill_attr(self, out.attr());
            out.ttl(TTL);
        })
    }

    async fn read<Op, R>(&self, op: Op, mut reply: R) -> Result<R::Ok, R::Error>
    where
        Op: op::Read,
        R: reply::ReplyData,
    {
        match op.ino() {
            ROOT_INO => return reply.error(libc::EISDIR),
            HELLO_INO => (),
            _ => return reply.error(libc::ENOENT),
        }

        let offset = op.offset() as usize;
        if offset < HELLO_CONTENT.len() {
            let size = op.size() as usize;
            let data = &HELLO_CONTENT[offset..];
            let data = &data[..std::cmp::min(data.len(), size)];

            reply.chunk(data);
        }

        reply.send()
    }

    fn dir_entries(&self) -> impl Iterator<Item = (u64, &DirEntry)> + '_ {
        self.entries.iter().enumerate().map(|(i, ent)| {
            let offset = (i + 1) as u64;
            (offset, ent)
        })
    }

    async fn readdir<Op, R>(&self, op: Op, mut reply: R) -> Result<R::Ok, R::Error>
    where
        Op: op::Readdir,
        R: reply::ReplyDirs,
    {
        if op.ino() != ROOT_INO {
            return reply.error(libc::ENOTDIR);
        }

        for (i, entry) in self.dir_entries().skip(op.offset() as usize) {
            let full = reply.entry(entry.name.as_ref(), |out| {
                out.ino(entry.ino);
                out.typ(entry.typ);
                out.offset(i + 1);
            });
            if full {
                break;
            }
        }

        reply.send()
    }
}
