#![forbid(unsafe_code)]

use polyfuse::{
    mount::{mount, MountOptions},
    op::{self, Operation},
    reply::AttrOut,
    request::{FallbackBuf, RequestBuf as _, RequestHeader},
    session::{KernelConfig, Session},
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
    let mountopts = MountOptions::new();
    let (mut conn, mount) = mount(&mountpoint.into(), &mountopts)?;

    // Initialize the FUSE session.
    let session = Session::init(&mut conn, KernelConfig::default())?;

    // Receive an incoming FUSE request from the kernel.
    let mut buf = FallbackBuf::new(session.request_buffer_size());
    while session.recv_request(&mut conn, &mut buf)? {
        let (header, arg, _remains) = buf.parts();
        match Operation::decode(header, arg)? {
            // Dispatch your callbacks to the supported operations...
            Operation::Getattr(op) => getattr(&session, &mut conn, header, op)?,
            Operation::Read(op) => read(&session, &mut conn, header, op)?,

            // Or annotate that the operation is not supported.
            _ => session.send_reply(&mut conn, header.unique(), Some(Errno::NOSYS), ())?,
        };
    }

    mount.unmount()?;

    Ok(())
}

fn getattr(
    session: &Session,
    conn: &mut Connection,
    header: &RequestHeader,
    op: op::Getattr<'_>,
) -> io::Result<()> {
    if op.ino != NodeID::ROOT {
        return session.send_reply(conn, header.unique(), Some(Errno::NOENT), ());
    }

    let mut attr = FileAttr::new();
    attr.ino = NodeID::ROOT;
    attr.size = CONTENT.len() as u64;
    attr.mode = FileMode::new(FileType::Regular, FilePermissions::READ);
    attr.nlink = 1;
    attr.uid = getuid();
    attr.gid = getgid();

    session.send_reply(
        conn,
        header.unique(),
        None,
        AttrOut {
            attr: Cow::Owned(attr),
            valid: Some(Duration::from_secs(1)),
        },
    )
}

fn read(
    session: &Session,
    conn: &mut Connection,
    header: &RequestHeader,
    op: op::Read<'_>,
) -> io::Result<()> {
    if op.ino != NodeID::ROOT {
        return session.send_reply(conn, header.unique(), Some(Errno::NOENT), ());
    }

    let mut data: &[u8] = &[];

    let offset = op.offset as usize;
    if offset < CONTENT.len() {
        let size = op.size as usize;
        data = &CONTENT[offset..];
        data = &data[..std::cmp::min(data.len(), size)];
    }

    session.send_reply(conn, header.unique(), None, data)
}
