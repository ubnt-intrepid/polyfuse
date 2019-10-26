//! FUSE (Filesystem in userspace) framework for Rust.

#![cfg_attr(feature = "docs", feature(doc_cfg))]
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

mod buf;
mod channel;
mod conn;
mod dir;
mod session;

pub mod fs;
pub mod reply;
pub mod server;

// re-exports from sub modules.
#[doc(inline)]
pub use crate::fs::{Context, Filesystem, Operation};
#[doc(inline)]
pub use crate::server::Server;
