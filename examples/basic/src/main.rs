use polyfuse::{op, reply, types::FileAttr, Daemon, Operation};

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

    // Start the FUSE daemon mounted on the specified path.
    let mut daemon = Daemon::mount(mountpoint, &[]).await?;

    // Receive an incoming FUSE request from the kernel.
    while let Some(req) = daemon.next_request().await? {
        // Process the request.
        req.process(|op| async {
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

    reply.attr(
        &FileAttr {
            ino: 1,
            mode: libc::S_IFREG as u32 | 0o444,
            nlink: 1,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            ..Default::default()
        },
        Some(Duration::from_secs(1)),
    )
}
