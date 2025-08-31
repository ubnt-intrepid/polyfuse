#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

use polyfuse::{
    fs::{self, Filesystem},
    mount::MountOptions,
    op,
    reply::{AttrOut, EntryOut, ReaddirOut},
    types::NodeID,
    KernelConfig,
};

use anyhow::{ensure, Context as _, Result};
use libc::{getgid, getuid, DT_DIR, DT_REG, EISDIR, ENOENT, S_IFDIR, S_IFREG};
use std::{mem, os::unix::prelude::*, path::PathBuf, time::Duration};

const TTL: Duration = Duration::from_secs(60 * 60 * 24 * 365);
const HELLO_INO: NodeID = unsafe { NodeID::from_raw_unchecked(2) };
const HELLO_FILENAME: &str = "hello.txt";
const HELLO_CONTENT: &[u8] = b"Hello, world!\n";

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_dir(), "mountpoint must be a directory");

    polyfuse::fs::run(
        Hello::new(),
        mountpoint,
        MountOptions::default(),
        KernelConfig::default(),
    )?;

    Ok(())
}

struct Hello {
    entries: Vec<DirEntry>,
    uid: u32,
    gid: u32,
}

struct DirEntry {
    name: &'static str,
    ino: NodeID,
    typ: u32,
}

impl Hello {
    fn new() -> Self {
        let mut entries = Vec::with_capacity(3);
        entries.push(DirEntry {
            name: ".",
            ino: NodeID::ROOT,
            typ: DT_DIR as u32,
        });
        entries.push(DirEntry {
            name: "..",
            ino: NodeID::ROOT,
            typ: DT_DIR as u32,
        });
        entries.push(DirEntry {
            name: HELLO_FILENAME,
            ino: HELLO_INO,
            typ: DT_REG as u32,
        });

        Self {
            entries,
            uid: unsafe { getuid() },
            gid: unsafe { getgid() },
        }
    }

    fn root_attr(&self) -> libc::stat {
        let mut st = unsafe { mem::zeroed::<libc::stat>() };
        st.st_ino = NodeID::ROOT.into_raw();
        st.st_mode = S_IFDIR | 0o555;
        st.st_nlink = 2; // ".", ".."
        st.st_uid = self.uid;
        st.st_gid = self.gid;
        st
    }

    fn hello_attr(&self) -> libc::stat {
        let mut st = unsafe { mem::zeroed::<libc::stat>() };
        st.st_ino = NodeID::ROOT.into_raw();
        st.st_size = HELLO_CONTENT.len() as _;
        st.st_mode = S_IFREG | 0o444;
        st.st_nlink = 1;
        st.st_uid = self.uid;
        st.st_gid = self.gid;
        st
    }

    fn dir_entries(&self) -> impl Iterator<Item = (u64, &DirEntry)> + '_ {
        self.entries.iter().enumerate().map(|(i, ent)| {
            let offset = (i + 1) as u64;
            (offset, ent)
        })
    }
}

impl Filesystem for Hello {
    fn lookup(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Lookup<'_>>) -> fs::Result {
        match req.arg().parent() {
            NodeID::ROOT if req.arg().name().as_bytes() == HELLO_FILENAME.as_bytes() => {
                let mut out = EntryOut::default();
                out.attr(self.hello_attr());
                out.ino(HELLO_INO);
                out.ttl_attr(TTL);
                out.ttl_entry(TTL);
                req.reply(out)
            }
            _ => Err(ENOENT.into()),
        }
    }

    fn getattr(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Getattr<'_>>) -> fs::Result {
        let attr = match req.arg().ino() {
            NodeID::ROOT => self.root_attr(),
            HELLO_INO => self.hello_attr(),
            _ => Err(ENOENT)?,
        };

        req.reply(AttrOut::new(attr).ttl(TTL))
    }

    fn read(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Read<'_>>) -> fs::Result {
        match req.arg().ino() {
            HELLO_INO => (),
            NodeID::ROOT => Err(EISDIR)?,
            _ => Err(ENOENT)?,
        }

        let mut data: &[u8] = &[];

        let offset = req.arg().offset() as usize;
        if offset < HELLO_CONTENT.len() {
            let size = req.arg().size() as usize;
            data = &HELLO_CONTENT[offset..];
            data = &data[..std::cmp::min(data.len(), size)];
        }

        req.reply(data)
    }

    fn readdir(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Readdir<'_>>) -> fs::Result {
        if req.arg().ino() != NodeID::ROOT {
            return Err(libc::ENOTDIR.into());
        }

        let mut out = ReaddirOut::new(req.arg().size() as usize);

        for (i, entry) in self.dir_entries().skip(req.arg().offset() as usize) {
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
