pub mod daemon;
pub mod op;

pub use crate::daemon::Daemon;
pub use crate::op::Operation;

use std::{ffi::OsStr, path::Path};

/// Start a FUSE daemon mount on the specified path.
#[inline]
pub async fn mount(mountpoint: impl AsRef<Path>, mountopts: &[&OsStr]) -> anyhow::Result<Daemon> {
    Daemon::mount(mountpoint, mountopts).await
}
