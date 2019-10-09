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

mod buffer;
mod op;
mod session;
mod tokio;

pub mod abi;
pub mod reply;
pub mod request;

pub use crate::buffer::{Buffer, Data};
pub use crate::op::Operations;
pub use crate::session::Session;

pub const MAX_WRITE_SIZE: usize = 16 * 1024 * 1024;
