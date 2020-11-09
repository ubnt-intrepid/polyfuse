#![doc(html_root_url = "https://docs.rs/polyfuse/0.4.0-dev")]

//! A FUSE (Filesystem in userspace) library for Rust.

#![deny(
    missing_docs,
    missing_debug_implementations,
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::checked_conversions,
    clippy::invalid_upcast_comparisons
)]
#![forbid(clippy::unimplemented, clippy::todo)]

pub mod op;
pub mod reply;

mod common;
mod dirent;
mod fs;
mod session;
mod util;

use polyfuse_kernel as kernel;

#[doc(inline)]
pub use crate::{
    common::{FileAttr, FileLock, Forget, LockOwner, StatFs},
    dirent::DirEntry,
    fs::Filesystem,
    session::{CapabilityFlags, ConnectionInfo, Session, SessionInitializer},
};

/// A re-export of [`async_trait`] for implementing `Filesystem`.
///
/// [`async_trait`]: https://docs.rs/async-trait
pub use async_trait::async_trait;

#[test]
fn test_html_root_url() {
    version_sync::assert_html_root_url_updated!("src/lib.rs");
}
