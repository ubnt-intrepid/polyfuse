#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

use polyfuse_examples::prelude::*;

use examples::memfs::MemFS;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mountpoint = examples::get_mountpoint()?;
    anyhow::ensure!(mountpoint.is_dir(), "the mountpoint must be a directory");

    let memfs = MemFS::new(&std::fs::metadata(&mountpoint)?);

    polyfuse_tokio::mount(memfs, mountpoint, &[]).await?;

    Ok(())
}
