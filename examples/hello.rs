#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]
#![forbid(unsafe_code)]

use polyfuse::{
    mount::MountOptions,
    op::Operation,
    reply::{self, AttrOut, EntryOut, ReaddirOut},
    request::{FallbackBuf, SpliceBuf, ToRequestParts},
    session::{KernelConfig, Session},
    types::{FileAttr, FileMode, FilePermissions, FileType, NodeID},
};

use anyhow::{ensure, Context as _, Result};
use polyfuse_kernel::FUSE_MIN_READ_BUFFER;
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

    let (conn, mount) = polyfuse::mount::mount(&mountpoint.into(), &MountOptions::new())?;
    let session = Session::init(
        &conn,
        FallbackBuf::new(FUSE_MIN_READ_BUFFER as usize),
        KernelConfig::new(),
    )?;

    let fs = Hello::new();

    let mut buf = SpliceBuf::new(session.request_buffer_size())?;
    while session.recv_request(&conn, &mut buf)? {
        let (header, arg, _remains) = buf.to_request_parts();
        match Operation::decode(session.config(), header, arg) {
            Ok(Operation::Lookup(op)) => match op.parent {
                NodeID::ROOT if op.name.as_bytes() == HELLO_FILENAME.as_bytes() => session
                    .send_reply(
                        &conn,
                        header.unique(),
                        None,
                        EntryOut {
                            ino: Some(HELLO_INO),
                            generation: 0,
                            attr: Cow::Owned(fs.hello_attr()),
                            attr_valid: Some(TTL),
                            entry_valid: Some(TTL),
                        },
                    )?,
                _ => session.send_reply(&conn, header.unique(), Some(Errno::NOENT), ())?,
            },

            Ok(Operation::Getattr(op)) => {
                let attr = match op.ino {
                    NodeID::ROOT => fs.root_attr(),
                    HELLO_INO => fs.hello_attr(),
                    _ => Err(Errno::NOENT)?,
                };
                session.send_reply(
                    &conn,
                    header.unique(),
                    None,
                    AttrOut {
                        attr: Cow::Owned(attr),
                        valid: Some(TTL),
                    },
                )?;
            }

            Ok(Operation::Read(op)) => {
                match op.ino {
                    HELLO_INO => (),
                    NodeID::ROOT => {
                        session.send_reply(&conn, header.unique(), Some(Errno::ISDIR), ())?;
                        continue;
                    }
                    _ => {
                        session.send_reply(&conn, header.unique(), Some(Errno::NOENT), ())?;
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

                session.send_reply(&conn, header.unique(), None, reply::Raw(data))?;
            }

            Ok(Operation::Readdir(op)) => {
                if op.ino != NodeID::ROOT {
                    session.send_reply(&conn, header.unique(), Some(Errno::NOTDIR), ())?;
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

                session.send_reply(&conn, header.unique(), None, buf)?;
            }

            _ => session.send_reply(&conn, header.unique(), Some(Errno::NOSYS), ())?,
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
