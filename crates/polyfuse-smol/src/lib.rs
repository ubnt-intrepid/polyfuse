#![doc(html_root_url = "https://docs.rs/polyfuse-smol/0.1.0-dev")]

//! Tokio integration for `polyfuse`.

#![deny(missing_docs, missing_debug_implementations)]

mod buf;
mod channel;
mod pool;
mod server;
mod session;
mod util;

pub use crate::server::{Builder, Server};

use polyfuse::Filesystem;
use std::{ffi::OsStr, io, path::Path};

/// Run a FUSE filesystem mounted onto the specified path.
pub async fn mount<F>(fs: F, mountpoint: impl AsRef<Path>, mountopts: &[&OsStr]) -> io::Result<()>
where
    F: Filesystem + Send + 'static,
{
    let mut server = Server::mount(mountpoint, mountopts).await?;
    server.run(fs).await?;
    Ok(())
}

#[test]
fn test_html_root_url() {
    version_sync::assert_html_root_url_updated!("src/lib.rs");
}
