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
mod server;

pub use crate::server::{Notifier, Server};

use bytes::Bytes;
use polyfuse::Filesystem;
use std::{ffi::OsStr, io, path::Path};

/// Run a FUSE filesystem daemon mounted onto the specified path.
pub async fn run<F>(fs: F, mountpoint: impl AsRef<Path>, mountopts: &[&OsStr]) -> io::Result<()>
where
    F: Filesystem<Bytes> + Send + 'static,
{
    let server = Server::mount(mountpoint, mountopts).await?;
    server.run(fs).await?;
    Ok(())
}
