//! FUSE (Filesystem in userspace) framework for Rust.

#![warn(missing_debug_implementations, clippy::unimplemented)]

mod common;
mod error;
mod op;
mod session;

pub mod abi;
pub mod io;
pub mod reply;
pub mod request;

pub use crate::common::{CapFlags, FileAttr, FileLock, Statfs};
pub use crate::error::{Error, Result};
pub use crate::op::Operations;
pub use crate::session::Session;

pub const MAX_WRITE_SIZE: usize = 16 * 1024 * 1024;
