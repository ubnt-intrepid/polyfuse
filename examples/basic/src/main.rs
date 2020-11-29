use polyfuse::{
    op,
    reply::{self, AttrOut},
    Config, Operation, Session,
};
use polyfuse_async_std::Connection;

use anyhow::Context as _;
use std::{path::PathBuf, time::Duration};

#[async_std::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args
        .free_from_str()?
        .context("missing mountpoint specified")?;
    anyhow::ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    // Establish connection to FUSE kernel driver mounted on the specified path.
    let conn = Connection::open(&mountpoint, &[]).await?;

    // Start FUSE session.
    let session = Session::start(&conn, Config::default()).await?;

    // Receive an incoming FUSE request from the kernel.
    while let Some(req) = session.next_request(&conn).await? {
        // Process the request.
        let op = req.operation(&conn)?;
        let _replied = match op {
            // Dispatch your callbacks to the supported operations...
            Operation::Getattr { op, reply, .. } => getattr(op, reply).await?,

            // Or annotate that the operation is not supported.
            op => op.default()?,
        };
    }

    Ok(())
}

async fn getattr<Op, R>(op: Op, reply: R) -> Result<R::Ok, R::Error>
where
    Op: op::Getattr,
    R: reply::ReplyAttr,
{
    if op.ino() != 1 {
        return reply.error(libc::ENOENT);
    }

    reply.send(|out: &mut dyn AttrOut| {
        let attr = out.attr();
        attr.ino(1);
        attr.mode(libc::S_IFREG as u32 | 0o444);
        attr.nlink(1);
        attr.uid(unsafe { libc::getuid() });
        attr.gid(unsafe { libc::getgid() });

        out.ttl(Duration::from_secs(1));
    })
}
