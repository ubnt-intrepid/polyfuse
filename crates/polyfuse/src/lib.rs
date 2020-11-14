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
pub mod types;

mod filesystem;

#[doc(inline)]
pub use crate::filesystem::{Filesystem, LocalFilesystem, Request};

#[test]
fn test_html_root_url() {
    version_sync::assert_html_root_url_updated!("src/lib.rs");
}
