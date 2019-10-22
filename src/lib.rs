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

mod conn;
mod dir;
mod main_loop;
mod op;

pub mod abi;
pub mod buf;
pub mod reply;
pub mod session;
pub mod tokio;

#[doc(inline)]
pub use crate::abi::{FileAttr, FileLock, FileMode, Gid, Nodeid, Pid, Statfs, Uid};
pub use crate::dir::DirEntry;
pub use crate::main_loop::main_loop;
pub use crate::op::{AttrSet, Context, Operations};
