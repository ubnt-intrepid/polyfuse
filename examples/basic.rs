#![forbid(unsafe_code)]

use polyfuse::{
    mount::MountOptions,
    op::{self, Operation},
    reply::{self, AttrOut},
    request::FallbackBuf,
    session::{KernelConfig, Request},
    types::{FileAttr, FileMode, FilePermissions, FileType, NodeID},
    Connection,
};

use anyhow::{ensure, Context as _, Result};
use rustix::{
    io::Errno,
    process::{getgid, getuid},
};
use std::{borrow::Cow, io, path::PathBuf, time::Duration};

const CONTENT: &[u8] = b"Hello from FUSE!\n";

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    // Establish connection to FUSE kernel driver mounted on the specified path.
    let (session, mut conn, mount) =
        polyfuse::session::connect(mountpoint.into(), MountOptions::new(), KernelConfig::new())?;

    // Receive an incoming FUSE request from the kernel.
    let mut buf = FallbackBuf::new(session.request_buffer_size());
    while session.recv_request(&mut conn, &mut buf)? {
        let Some((req, op, ..)) = session.decode(&conn, &mut buf)? else {
            continue;
        };
        match op {
            // Dispatch your callbacks to the supported operations...
            Operation::Getattr(op) => getattr(req, op)?,
            Operation::Read(op) => read(req, op)?,

            // Or annotate that the operation is not supported.
            _ => req.reply_error(Errno::NOSYS)?,
        };
    }

    mount.unmount()?;

    Ok(())
}

fn getattr(req: Request<'_, &Connection>, op: op::Getattr<'_>) -> io::Result<()> {
    if op.ino != NodeID::ROOT {
        return req.reply_error(Errno::NOENT);
    }

    req.reply(AttrOut {
        attr: Cow::Owned(FileAttr {
            ino: NodeID::ROOT,
            size: CONTENT.len() as u64,
            mode: FileMode::new(FileType::Regular, FilePermissions::READ),
            nlink: 1,
            uid: getuid(),
            gid: getgid(),
            ..FileAttr::new()
        }),
        valid: Some(Duration::from_secs(1)),
    })
}

fn read(req: Request<'_, &Connection>, op: op::Read<'_>) -> io::Result<()> {
    if op.ino != NodeID::ROOT {
        return req.reply_error(Errno::NOENT);
    }

    let mut data: &[u8] = &[];

    let offset = op.offset as usize;
    if offset < CONTENT.len() {
        let size = op.size as usize;
        data = &CONTENT[offset..];
        data = &data[..std::cmp::min(data.len(), size)];
    }

    req.reply(reply::Raw(data))
}
