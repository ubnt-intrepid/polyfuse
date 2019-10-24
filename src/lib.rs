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

mod channel;
mod conn;
mod dir;
mod run;

pub mod buf;
pub mod fs;
pub mod reply;
pub mod session;

pub use crate::conn::MountOptions;
pub use crate::dir::DirEntry;
pub use crate::run::{mount, run};

// re-exports from polyfuse-abi
pub use polyfuse_abi::{FileAttr, FileLock, FileMode, Gid, Nodeid, Pid, Statfs, Uid};
