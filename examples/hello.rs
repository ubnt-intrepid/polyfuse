#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]
#![forbid(unsafe_code)]

use polyfuse::{
    fs::{
        self,
        reply::{self, ReplyAttr, ReplyData, ReplyDir, ReplyEntry},
        Daemon, Filesystem,
    },
    op,
    types::{FileAttr, FileMode, FilePermissions, FileType, NodeID, GID, UID},
};

use anyhow::{ensure, Context as _, Result};
use libc::{EISDIR, ENOENT, ENOTDIR};
use std::{os::unix::prelude::*, path::PathBuf, sync::Arc, time::Duration};

const TTL: Duration = Duration::from_secs(60 * 60 * 24 * 365);
const HELLO_INO: NodeID = NodeID::from_raw(2);
const HELLO_FILENAME: &str = "hello.txt";
const HELLO_CONTENT: &[u8] = b"Hello, world!\n";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_dir(), "mountpoint must be a directory");

    let daemon = Daemon::mount(mountpoint, Default::default(), Default::default()).await?;
    daemon.run(Arc::new(Hello::new()), None).await?;

    Ok(())
}

struct Hello {
    entries: Vec<DirEntry>,
    uid: UID,
    gid: GID,
}

struct DirEntry {
    name: &'static str,
    ino: NodeID,
    typ: Option<FileType>,
}

impl Hello {
    fn new() -> Self {
        Self {
            entries: vec![
                DirEntry {
                    name: ".",
                    ino: NodeID::ROOT,
                    typ: Some(FileType::Directory),
                },
                DirEntry {
                    name: "..",
                    ino: NodeID::ROOT,
                    typ: Some(FileType::Directory),
                },
                DirEntry {
                    name: HELLO_FILENAME,
                    ino: HELLO_INO,
                    typ: Some(FileType::Regular),
                },
            ],
            uid: UID::current(),
            gid: GID::current(),
        }
    }

    fn root_attr(&self) -> FileAttr {
        let mut attr = FileAttr::new();
        attr.ino = NodeID::ROOT;
        attr.mode = FileMode::new(
            FileType::Directory,
            FilePermissions::READ | FilePermissions::EXEC,
        );
        attr.nlink = 2; // ".", ".."
        attr.uid = self.uid;
        attr.gid = self.gid;
        attr
    }

    fn hello_attr(&self) -> FileAttr {
        let mut attr = FileAttr::new();
        attr.ino = HELLO_INO;
        attr.size = HELLO_CONTENT.len() as u64;
        attr.mode = FileMode::new(FileType::Regular, FilePermissions::READ);
        attr.nlink = 1;
        attr.uid = self.uid;
        attr.gid = self.gid;
        attr
    }

    fn dir_entries(&self) -> impl Iterator<Item = (u64, &DirEntry)> + '_ {
        self.entries.iter().enumerate().map(|(i, ent)| {
            let offset = (i + 1) as u64;
            (offset, ent)
        })
    }
}

impl Filesystem for Hello {
    async fn lookup(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Lookup<'_>,
        mut reply: ReplyEntry<'_>,
    ) -> reply::Result {
        match op.parent() {
            NodeID::ROOT if op.name().as_bytes() == HELLO_FILENAME.as_bytes() => {
                reply.out().attr(self.hello_attr());
                reply.out().ino(HELLO_INO);
                reply.out().ttl_attr(TTL);
                reply.out().ttl_entry(TTL);
                reply.send()
            }
            _ => Err(ENOENT)?,
        }
    }

    async fn getattr(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Getattr<'_>,
        mut reply: ReplyAttr<'_>,
    ) -> reply::Result {
        let attr = match op.ino() {
            NodeID::ROOT => self.root_attr(),
            HELLO_INO => self.hello_attr(),
            _ => Err(ENOENT)?,
        };

        reply.out().attr(attr);
        reply.out().ttl(TTL);
        reply.send()
    }

    async fn read(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Read<'_>,
        reply: ReplyData<'_>,
    ) -> reply::Result {
        match op.ino() {
            HELLO_INO => (),
            NodeID::ROOT => Err(EISDIR)?,
            _ => Err(ENOENT)?,
        }

        let mut data: &[u8] = &[];

        let offset = op.offset() as usize;
        if offset < HELLO_CONTENT.len() {
            let size = op.size() as usize;
            data = &HELLO_CONTENT[offset..];
            data = &data[..std::cmp::min(data.len(), size)];
        }

        reply.send(data)
    }

    async fn readdir(
        self: &Arc<Self>,
        _: fs::Request<'_>,
        op: op::Readdir<'_>,
        mut reply: ReplyDir<'_>,
    ) -> reply::Result {
        if op.ino() != NodeID::ROOT {
            Err(ENOTDIR)?
        }

        for (i, entry) in self.dir_entries().skip(op.offset() as usize) {
            let full = reply.push_entry(
                entry.name.as_ref(), //
                entry.ino,
                entry.typ,
                i + 1,
            );
            if full {
                break;
            }
        }

        reply.send()
    }
}
