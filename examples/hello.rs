#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]
#![forbid(unsafe_code)]

use polyfuse::{
    mount::MountOptions,
    op::Operation,
    reply::{self, AttrOut, EntryOut, ReaddirOut},
    request::SpliceBuf,
    session::KernelConfig,
    types::{FileAttr, FileMode, FilePermissions, FileType, NodeID},
};

use anyhow::{ensure, Context as _, Result};
use rustix::{
    fs::{Gid, Uid},
    io::Errno,
    process::{getgid, getuid},
};
use std::{borrow::Cow, os::unix::prelude::*, path::PathBuf, time::Duration};

const TTL: Duration = Duration::from_secs(60 * 60 * 24 * 365);
const HELLO_INO: NodeID = match NodeID::from_raw(2) {
    Some(ino) => ino,
    None => panic!("unreachable"),
};
const HELLO_FILENAME: &str = "hello.txt";
const HELLO_CONTENT: &[u8] = b"Hello, world!\n";

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_dir(), "mountpoint must be a directory");

    let (session, conn, mount) =
        polyfuse::session::connect(mountpoint.into(), MountOptions::new(), KernelConfig::new())?;

    let fs = Hello::new();

    let mut buf = SpliceBuf::new(session.request_buffer_size())?;
    while session.recv_request(&conn, &mut buf)? {
        let (req, op) = session.decode(&mut buf)?;
        match op {
            Some(Operation::Lookup(op)) => match op.parent {
                NodeID::ROOT if op.name.as_bytes() == HELLO_FILENAME.as_bytes() => req.reply(
                    &conn,
                    EntryOut {
                        ino: Some(HELLO_INO),
                        generation: 0,
                        attr: Cow::Owned(fs.hello_attr()),
                        attr_valid: Some(TTL),
                        entry_valid: Some(TTL),
                    },
                )?,
                _ => req.reply_error(&conn, Errno::NOENT)?,
            },

            Some(Operation::Getattr(op)) => {
                let attr = match op.ino {
                    NodeID::ROOT => fs.root_attr(),
                    HELLO_INO => fs.hello_attr(),
                    _ => Err(Errno::NOENT)?,
                };
                req.reply(
                    &conn,
                    AttrOut {
                        attr: Cow::Owned(attr),
                        valid: Some(TTL),
                    },
                )?;
            }

            Some(Operation::Read(op)) => {
                match op.ino {
                    HELLO_INO => (),
                    NodeID::ROOT => {
                        req.reply_error(&conn, Errno::ISDIR)?;
                        continue;
                    }
                    _ => {
                        req.reply_error(&conn, Errno::NOENT)?;
                        continue;
                    }
                }

                let mut data: &[u8] = &[];

                let offset = op.offset as usize;
                if offset < HELLO_CONTENT.len() {
                    let size = op.size as usize;
                    data = &HELLO_CONTENT[offset..];
                    data = &data[..std::cmp::min(data.len(), size)];
                }

                req.reply(&conn, reply::Raw(data))?;
            }

            Some(Operation::Readdir(op)) => {
                if op.ino != NodeID::ROOT {
                    req.reply_error(&conn, Errno::NOTDIR)?;
                    continue;
                }

                let mut buf = ReaddirOut::new(op.size as usize);
                for (i, entry) in fs.dir_entries().skip(op.offset as usize) {
                    let full = buf.push_entry(
                        entry.name.as_ref(), //
                        entry.ino,
                        entry.typ,
                        i + 1,
                    );
                    if full {
                        break;
                    }
                }

                req.reply(&conn, buf)?;
            }

            _ => req.reply_error(&conn, Errno::NOSYS)?,
        }
    }

    mount.unmount()?;

    Ok(())
}

struct Hello {
    entries: Vec<DirEntry>,
    uid: Uid,
    gid: Gid,
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
            uid: getuid(),
            gid: getgid(),
        }
    }

    fn root_attr(&self) -> FileAttr {
        FileAttr {
            ino: NodeID::ROOT,
            mode: FileMode::new(
                FileType::Directory,
                FilePermissions::READ | FilePermissions::EXEC,
            ),
            nlink: 2, // ".", ".."
            uid: self.uid,
            gid: self.gid,
            ..FileAttr::new()
        }
    }

    fn hello_attr(&self) -> FileAttr {
        FileAttr {
            ino: HELLO_INO,
            size: HELLO_CONTENT.len() as u64,
            mode: FileMode::new(FileType::Regular, FilePermissions::READ),
            nlink: 1,
            uid: self.uid,
            gid: self.gid,
            ..FileAttr::new()
        }
    }

    fn dir_entries(&self) -> impl Iterator<Item = (u64, &DirEntry)> + '_ {
        self.entries.iter().enumerate().map(|(i, ent)| {
            let offset = (i + 1) as u64;
            (offset, ent)
        })
    }
}
