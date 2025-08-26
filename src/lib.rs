//! A FUSE (Filesystem in Userspace) library for Rust.

#![doc(html_root_url = "https://docs.rs/polyfuse/0.4.0")]
#![forbid(clippy::todo, clippy::unimplemented)]

#[macro_use]
pub mod nix;

mod request;
mod session;

pub mod bytes;
pub mod conn;
pub mod fs;
pub mod mount;
pub mod op;
pub mod reply;

pub use crate::{
    conn::Connection,
    op::Operation,
    request::RequestBuffer,
    session::{KernelConfig, KernelFlags, Session},
};
