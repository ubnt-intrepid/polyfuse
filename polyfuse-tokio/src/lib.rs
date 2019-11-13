mod channel;
mod conn;
mod lock;
mod server;

pub use crate::conn::MountOptions;
pub use crate::server::{Notifier, RetrieveHandle, Server};

use polyfuse::{request::BytesBuffer, Filesystem};
use std::{io, path::Path};

/// Run a FUSE filesystem daemon mounted onto the specified path.
pub async fn mount<F>(fs: F, mountpoint: impl AsRef<Path>) -> io::Result<()>
where
    F: Filesystem<BytesBuffer> + Send + 'static,
{
    let server = Server::mount(mountpoint, Default::default()).await?;
    server.run(fs).await?;
    Ok(())
}
