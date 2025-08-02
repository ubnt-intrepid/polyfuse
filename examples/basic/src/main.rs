use polyfuse::{
    op, reply::AttrOut, tokio::Connection, KernelConfig, MountOptions, Operation, Request, Session,
};

use anyhow::{ensure, Context as _, Result};
use std::{io, path::PathBuf, time::Duration};

const CONTENT: &[u8] = b"Hello from FUSE!\n";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    // Establish connection to FUSE kernel driver mounted on the specified path.
    let conn = MountOptions::default().mount(mountpoint)?;
    let mut conn = polyfuse::tokio::Connection::new(conn)?;

    // Initialize the FUSE session.
    let session = Session::init(&mut conn, KernelConfig::default()).await?;

    // Receive an incoming FUSE request from the kernel.
    while let Some(req) = session.next_request(&mut conn).await? {
        match req.operation(&session)? {
            // Dispatch your callbacks to the supported operations...
            Operation::Getattr(op) => getattr(&session, &mut conn, &req, op).await?,
            Operation::Read(op) => read(&session, &mut conn, &req, op).await?,

            // Or annotate that the operation is not supported.
            _ => session.reply_error(&mut conn, &req, libc::ENOSYS).await?,
        };
    }

    Ok(())
}

async fn getattr(
    session: &Session,
    conn: &mut Connection,
    req: &Request,
    op: op::Getattr<'_>,
) -> io::Result<()> {
    if op.ino() != 1 {
        return session.reply_error(conn, req, libc::ENOENT).await;
    }

    let mut out = AttrOut::default();
    out.attr().ino(1);
    out.attr().mode(libc::S_IFREG as u32 | 0o444);
    out.attr().size(CONTENT.len() as u64);
    out.attr().nlink(1);
    out.attr().uid(unsafe { libc::getuid() });
    out.attr().gid(unsafe { libc::getgid() });
    out.ttl(Duration::from_secs(1));

    session.reply(conn, &req, out).await
}

async fn read(
    session: &Session,
    conn: &mut Connection,
    req: &Request,
    op: op::Read<'_>,
) -> io::Result<()> {
    if op.ino() != 1 {
        return session.reply_error(conn, req, libc::ENOENT).await;
    }

    let mut data: &[u8] = &[];

    let offset = op.offset() as usize;
    if offset < CONTENT.len() {
        let size = op.size() as usize;
        data = &CONTENT[offset..];
        data = &data[..std::cmp::min(data.len(), size)];
    }

    session.reply(conn, req, data).await
}
