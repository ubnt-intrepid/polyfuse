//! A FUSE (Filesystem in Userspace) library for Rust.

#![doc(html_root_url = "https://docs.rs/polyfuse/0.4.0")]
#![forbid(clippy::todo, clippy::unimplemented)]

#[macro_use]
pub mod nix;

mod decoder;
mod session;

pub mod bytes;
pub mod dev;
pub mod mount;
pub mod op;
pub mod reply;

pub use crate::{
    dev::{ClonedConnection, Device},
    mount::MountOptions,
    op::Operation,
    session::{Data, KernelConfig, Request, Session},
};
