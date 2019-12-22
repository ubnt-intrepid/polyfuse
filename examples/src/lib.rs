#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

pub mod memfs;

pub mod prelude {
    pub use anyhow::{anyhow, ensure};
    pub use futures::{
        future::{Future, FutureExt},
        io::AsyncRead,
    };
    pub use polyfuse::{async_trait, io::Writer, op::Operation, reply::ReplyWriter, Filesystem};
    pub use std::{
        ffi::{OsStr, OsString},
        os::unix::ffi::OsStrExt,
        path::{Path, PathBuf},
    };

    pub use crate as examples;
}

pub fn init_tracing() -> anyhow::Result<()> {
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(())
}

pub fn get_mountpoint() -> anyhow::Result<std::path::PathBuf> {
    std::env::args()
        .nth(1)
        .map(Into::into)
        .ok_or_else(|| anyhow::anyhow!("missing mountpoint"))
}
