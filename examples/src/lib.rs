#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

pub mod fs;
pub mod memfs;

pub mod prelude {
    pub use anyhow::{anyhow, ensure};
    pub use futures::future::{Future, FutureExt};
    pub use polyfuse::{
        async_trait,
        io::{Reader, Writer},
        op::Operation,
        Context, Filesystem,
    };
    pub use std::{
        ffi::{OsStr, OsString},
        os::unix::ffi::OsStrExt,
        path::{Path, PathBuf},
        time::Duration,
    };

    pub use crate as examples;
}

pub fn get_mountpoint() -> anyhow::Result<std::path::PathBuf> {
    std::env::args()
        .nth(1)
        .map(Into::into)
        .ok_or_else(|| anyhow::anyhow!("missing mountpoint"))
}
