use polyfuse::{op, reply::ReplyAttr as _, types::FileAttr};
use polyfuse_daemon::Operation;

use anyhow::Context as _;
use async_std::task;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args
        .free_from_str()?
        .context("missing mountpoint specified")?;
    anyhow::ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    task::block_on(async {
        // Start the FUSE daemon mounted on the specified path.
        let mut daemon = polyfuse_daemon::mount(mountpoint, &[]).await?;

        loop {
            // Receive an incoming FUSE request from the kernel.
            let mut req = match daemon.next_request().await {
                Some(req) => req,
                None => break,
            };

            // Process the request.
            req.process(|op| async {
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
        None,
    )
}
