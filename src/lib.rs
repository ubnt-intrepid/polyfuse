//! FUSE (Filesystem in userspace) framework for Rust.

#![warn(missing_debug_implementations, clippy::unimplemented)]

pub use fuse_async_abi as abi;

mod error;
mod op;
mod session;

pub mod reply;
pub mod request;

pub use crate::error::{Error, Result};
pub use crate::op::Operations;
pub use crate::session::Session;

pub const MAX_WRITE_SIZE: usize = 16 * 1024 * 1024;
