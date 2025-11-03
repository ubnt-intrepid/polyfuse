#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]
#![forbid(unsafe_code)]

use polyfuse::{
    mount::MountOptions,
    op::Operation,
    reply::{DirEntryBuf, ReplySender as _},
    types::{FileAttr, FileMode, FilePermissions, FileType, NodeID},
    KernelConfig,
};

use anyhow::{ensure, Context as _, Result};
use libc::{SIGHUP, SIGINT, SIGTERM};
use rustix::{
    fs::{Gid, Uid},
    io::Errno,
    process::{getgid, getuid},
};
use signal_hook::iterator::Signals;
use std::{os::unix::prelude::*, path::PathBuf, thread, time::Duration};

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
        polyfuse::connect(mountpoint, MountOptions::new(), KernelConfig::new())?;

    let fs = Hello::new();

    thread::scope(|scope| -> Result<()> {
        let mut signals = Signals::new([SIGHUP, SIGTERM, SIGINT])?;
        let signals_handle = signals.handle();
        scope.spawn(move || {
            if let Some(sig) = signals.wait().next() {
                tracing::debug!(
                    "caught the termination signal: {}",
                    match sig {
                        SIGHUP => "SIGHUP",
                        SIGTERM => "SIGTERM",
                        SIGINT => "SIGINT",
                        _ => "<unknown>",
                    }
                );
                let _ = mount.unmount();
            }
        });

        let mut buf = session.new_splice_buffer()?;
        while session.recv_request(&conn, &mut buf)? {
            let (req, op, _remains) = session.decode(&conn, &mut buf)?;
            match op {
                Operation::Lookup(op) => match op.parent {
                    NodeID::ROOT if op.name.as_bytes() == HELLO_FILENAME.as_bytes() => {
                        req.reply_entry(Some(HELLO_INO), &fs.hello_attr(), 0, Some(TTL), Some(TTL))?
                    }
                    _ => req.reply_error(Errno::NOENT)?,
                },

                Operation::Getattr(op) => {
                    let attr = match op.ino {
                        NodeID::ROOT => fs.root_attr(),
                        HELLO_INO => fs.hello_attr(),
                        _ => Err(Errno::NOENT)?,
                    };
                    req.reply_attr(&attr, Some(TTL))?;
                }

                Operation::Read(op) => {
                    match op.ino {
                        HELLO_INO => (),
                        NodeID::ROOT => {
                            req.reply_error(Errno::ISDIR)?;
                            continue;
                        }
                        _ => {
                            req.reply_error(Errno::NOENT)?;
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

                    req.reply_bytes(data)?;
                }

                Operation::Readdir(op) => {
                    if op.ino != NodeID::ROOT {
                        req.reply_error(Errno::NOTDIR)?;
                        continue;
                    }

                    let mut buf = DirEntryBuf::new(op.size as usize);
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

                    req.reply_dir(&buf)?;
                }

                _ => req.reply_error(Errno::NOSYS)?,
            }
        }

        signals_handle.close();

        Ok(())
    })?;

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
