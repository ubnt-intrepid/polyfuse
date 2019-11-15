//! Components for using `polyfuse` filesystem with Tokio runtime.

#![warn(clippy::checked_conversions)]
#![deny(
    missing_debug_implementations,
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::invalid_upcast_comparisons,
    clippy::unimplemented
)]

mod channel;
mod lock;
mod mount;
mod server;

pub use crate::{
    channel::Channel,
    mount::MountOptions,
    server::{Notifier, RetrieveHandle, Server},
};

use bytes::Bytes;
use polyfuse::Filesystem;
use std::{io, path::Path};

/// Run a FUSE filesystem daemon mounted onto the specified path.
pub async fn run<F>(fs: F, mountpoint: impl AsRef<Path>) -> io::Result<()>
where
    F: Filesystem<Bytes> + Send + 'static,
{
    let server = Server::mount(mountpoint, Default::default()).await?;
    server.run(fs).await?;
    Ok(())
}
