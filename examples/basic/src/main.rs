use polyfuse::{op, reply::AttrOut, Config, Connection, MountOptions, Operation, Request, Session};

use anyhow::Context as _;
use async_io::Async;
use std::{io, path::PathBuf, time::Duration};

#[async_std::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args
        .free_from_str()?
        .context("missing mountpoint specified")?;
    anyhow::ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    // Establish connection to FUSE kernel driver mounted on the specified path.
    let conn = async_std::task::spawn_blocking(move || {
        Connection::open(&mountpoint, &MountOptions::default())
    })
    .await?;
    let conn = Async::new(conn)?;

    // Start FUSE session.
    let session = Session::start(&conn, conn.get_ref(), Config::default()).await?;

    // Receive an incoming FUSE request from the kernel.
    while let Some(req) = session.next_request(&conn).await? {
        // Process the request.
        let op = req.operation()?;
        match op {
            // Dispatch your callbacks to the supported operations...
            Operation::Getattr(op) => getattr(&req, op, conn.get_ref()).await?,

            // Or annotate that the operation is not supported.
            _ => req.reply_error(conn.get_ref(), libc::ENOSYS)?,
        };
    }

    Ok(())
}

async fn getattr<W>(req: &Request, op: op::Getattr<'_>, writer: W) -> io::Result<()>
where
    W: io::Write,
{
    if op.ino() != 1 {
        return req.reply_error(writer, libc::ENOENT);
    }

    let mut out = AttrOut::default();
    out.attr().ino(1);
    out.attr().mode(libc::S_IFREG as u32 | 0o444);
    out.attr().nlink(1);
    out.attr().uid(unsafe { libc::getuid() });
    out.attr().gid(unsafe { libc::getgid() });
    out.ttl(Duration::from_secs(1));

    req.reply(writer, out)
}
