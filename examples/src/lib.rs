#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

pub mod fs;

pub mod prelude {
    pub use anyhow::{anyhow, ensure};
    pub use futures::future::{Future, FutureExt};
    pub use polyfuse::{
        async_trait,
        io::{Reader, ScatteredBytes, Writer},
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

// FIXME: use either crate.
pub enum Either<L, R> {
    Left(L),
    Right(R),
}

impl<L, R> polyfuse::io::ScatteredBytes for Either<L, R>
where
    L: polyfuse::io::ScatteredBytes,
    R: polyfuse::io::ScatteredBytes,
{
    #[inline]
    fn collect<'a, C: ?Sized>(&'a self, collector: &mut C)
    where
        C: polyfuse::io::Collector<'a>,
    {
        match self {
            Either::Left(l) => l.collect(collector),
            Either::Right(r) => r.collect(collector),
        }
    }
}
