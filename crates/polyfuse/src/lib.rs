#![warn(clippy::todo, clippy::unimplemented)]

mod conn;
mod daemon;
mod session;
mod util;
mod write;

pub mod op;
pub mod reply;
pub mod request;
pub mod types;

pub use crate::{
    daemon::{Builder, Daemon},
    request::{Operation, Request},
    session::{CapabilityFlags, ConnectionInfo},
};
