use polyfuse::{op, reply, Config, Connection, Operation, Session};

use anyhow::Context as _;
use async_io::Async;
use std::{path::PathBuf, time::Duration};

#[async_std::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args
        .free_from_str()?
        .context("missing mountpoint specified")?;
    anyhow::ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    // Start the FUSE daemon mounted on the specified path.
    let conn = Connection::open(&mountpoint, &[])?;
    let writer = conn.writer();
    let conn = Async::new(conn)?;

    let session = Session::start(&conn, Config::default()).await?;

    // Receive an incoming FUSE request from the kernel.
    while let Some(req) = session.next_request(&conn).await? {
        // Process the request.
        req.process(&writer, |op| async {
            match op {
                // Dispatch your callbacks to the supported operations...
                Operation::Getattr { op, reply, .. } => getattr(op, reply).await,

                // Or annotate that the operation is not supported.
                _ => Err(polyfuse::reply::error_code(libc::ENOSYS)),
            }
        })
        .await?;
    }

    Ok(())
}

async fn getattr<Op, R>(op: Op, reply: R) -> Result<R::Ok, R::Error>
where
    Op: op::Getattr,
    R: reply::ReplyAttr,
{
    if op.ino() != 1 {
        return Err(polyfuse::reply::error_code(libc::ENOENT));
    }

    let mut attr = unsafe { std::mem::zeroed::<libc::stat>() };
    attr.st_ino = 1;
    attr.st_mode = libc::S_IFREG as u32 | 0o444;
    attr.st_nlink = 1;
    attr.st_uid = unsafe { libc::getuid() };
    attr.st_gid = unsafe { libc::getgid() };

    reply.attr(attr, Some(Duration::from_secs(1)))
}
