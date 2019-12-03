#![doc(html_root_url = "https://docs.rs/polyfuse-tokio/0.1.0")]

//! Tokio integration for `polyfuse`.

#![warn(clippy::checked_conversions)]
#![deny(
    missing_docs,
    missing_debug_implementations,
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::invalid_upcast_comparisons,
    clippy::unimplemented
)]
#![cfg_attr(test, deny(warnings))]

mod channel;
mod server;

pub use crate::server::{Notifier, Server};

use bytes::Bytes;
use polyfuse::Filesystem;
use std::{ffi::OsStr, io, path::Path};

/// Run a FUSE filesystem mounted onto the specified path.
pub async fn mount<F>(fs: F, mountpoint: impl AsRef<Path>, mountopts: &[&OsStr]) -> io::Result<()>
where
    F: Filesystem<Bytes> + Send + 'static,
{
    let server = Server::mount(mountpoint, mountopts).await?;
    server.run(fs).await?;
    Ok(())
}

#[test]
fn test_html_root_url() {
    version_sync::assert_html_root_url_updated!("src/lib.rs");
}
