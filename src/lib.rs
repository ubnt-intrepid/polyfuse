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

mod main_loop;
mod op;

pub mod abi;
pub mod reply;
pub mod request;
pub mod session;
pub mod tokio;

pub use crate::main_loop::main_loop;
pub use crate::op::Operations;
