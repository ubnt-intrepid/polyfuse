//! A FUSE (Filesystem in Userspace) library for Rust.

#![doc(html_root_url = "https://docs.rs/polyfuse/0.4.0")]
#![forbid(clippy::todo, clippy::unimplemented)]

mod conn;
mod init;
mod msg;

pub mod buf;
pub mod bytes;
pub mod io;
pub mod mount;
pub mod notify;
pub mod op;
pub mod reply;
pub mod session;
pub mod types;

pub use crate::{
    conn::Connection,
    init::{KernelConfig, KernelFlags},
};
