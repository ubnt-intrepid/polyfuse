//! A FUSE (Filesystem in Userspace) library for Rust.

#![doc(html_root_url = "https://docs.rs/polyfuse/0.4.0")]
#![forbid(clippy::todo, clippy::unimplemented)]

mod conn;
mod decoder;
mod session;

pub mod bytes;
pub mod op;
pub mod reply;

pub use crate::{
    conn::MountOptions,
    op::Operation,
    session::{Data, KernelConfig, Notifier, Request, Session},
};
