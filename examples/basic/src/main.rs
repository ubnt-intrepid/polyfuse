use polyfuse::{op, reply::ReplyAttr as _, types::FileAttr};
use polyfuse_daemon::{Daemon, Operation};

use anyhow::Context as _;
use async_std::task;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args
        .free_from_str()?
        .context("missing mountpoint specified")?;
    anyhow::ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    task::block_on(async {
        // Start the FUSE daemon mounted on the specified path.
        let mut daemon = Daemon::mount(mountpoint, &[]).await?;

        // Receive an incoming FUSE request from the kernel.
        while let Some(req) = daemon.next_request().await? {
            // Process the request.
            req.process(|op| async {
                tracing::trace!("in process()");

                match op {
                    // Dispatch your callbacks to the supported operations...
                    Operation::Getattr(op) => getattr(op).await,

                    // Or annotate that the operation is not supported.
                    op => op.unimplemented().await,
                }
            })
            .await?;
        }

        Ok(())
    })
}

async fn getattr<Op>(op: Op) -> Result<Op::Ok, Op::Error>
where
    Op: op::Getattr,
{
    if op.ino() != 1 {
        return Err(polyfuse::error::code(libc::ENOENT));
    }

    op.reply().attr(
        &FileAttr {
            ino: 1,
            mode: libc::S_IFREG as u32 | 0o444,
            nlink: 1,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            ..Default::default()
        },
        Some(std::time::Duration::from_secs(1)),
    )
}
