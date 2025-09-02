use polyfuse::{
    mount::{mount, MountOptions},
    op,
    reply::AttrOut,
    types::{NodeID, GID, UID},
    Connection, KernelConfig, Operation, RequestBuffer, Session,
};

use anyhow::{ensure, Context as _, Result};
use libc::{ENOENT, ENOSYS, S_IFREG};
use std::{io, path::PathBuf, time::Duration};

const CONTENT: &[u8] = b"Hello from FUSE!\n";

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    // Establish connection to FUSE kernel driver mounted on the specified path.
    let (devfd, fusermount) = mount(mountpoint, MountOptions::default())?;
    let mut conn = Connection::from(devfd);

    // Initialize the FUSE session.
    let mut session = Session::new();
    let mut config = KernelConfig::default();
    session.init(&mut conn, &mut config)?;

    // Receive an incoming FUSE request from the kernel.
    let mut buf = RequestBuffer::new_fallback(config.request_buffer_size())?;
    while session.recv_request(&mut conn, &mut buf)? {
        match buf.operation()? {
            // Dispatch your callbacks to the supported operations...
            (Operation::Getattr(op), ..) => getattr(&session, &mut conn, &buf, op)?,
            (Operation::Read(op), ..) => read(&session, &mut conn, &buf, op)?,

            // Or annotate that the operation is not supported.
            _ => session.send_reply(&mut conn, buf.unique(), ENOSYS, ())?,
        };
    }

    fusermount.unmount()?;

    Ok(())
}

fn getattr(
    session: &Session,
    conn: &mut Connection,
    req: &RequestBuffer,
    op: op::Getattr<'_>,
) -> io::Result<()> {
    if op.ino() != NodeID::ROOT {
        return session.send_reply(conn, req.unique(), ENOENT, ());
    }

    let mut out = AttrOut::default();
    out.attr().ino(NodeID::ROOT);
    out.attr().mode(S_IFREG | 0o444);
    out.attr().size(CONTENT.len() as u64);
    out.attr().nlink(1);
    out.attr().uid(UID::current());
    out.attr().gid(GID::current());
    out.ttl(Duration::from_secs(1));

    session.send_reply(conn, req.unique(), 0, out)
}

fn read(
    session: &Session,
    conn: &mut Connection,
    req: &RequestBuffer,
    op: op::Read<'_>,
) -> io::Result<()> {
    if op.ino() != NodeID::ROOT {
        return session.send_reply(conn, req.unique(), ENOENT, ());
    }

    let mut data: &[u8] = &[];

    let offset = op.offset() as usize;
    if offset < CONTENT.len() {
        let size = op.size() as usize;
        data = &CONTENT[offset..];
        data = &data[..std::cmp::min(data.len(), size)];
    }

    session.send_reply(conn, req.unique(), 0, data)
}
