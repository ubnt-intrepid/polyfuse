//! FUSE (Filesystem in userspace) framework for Rust.

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

mod lock;
mod server;

pub mod io;
pub mod session;

// re-exports from sub modules.
#[doc(inline)]
pub use crate::{
    server::{Notifier, Server},
    session::{Context, Filesystem, Operation},
};
