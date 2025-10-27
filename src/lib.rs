//! A FUSE (Filesystem in Userspace) library for Rust.

#![doc(html_root_url = "https://docs.rs/polyfuse/0.4.0")]
#![forbid(clippy::todo, clippy::unimplemented)]

mod connect;
mod device;
mod init;
mod msg;
mod util;

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
    connect::connect,
    device::Device,
    init::{KernelConfig, KernelFlags},
};
