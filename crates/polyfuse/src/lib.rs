#![warn(clippy::todo, clippy::unimplemented)]

mod conn;
mod daemon;
mod parse;
mod request;
mod session;
mod util;
mod write;

pub mod error;
pub mod op;
pub mod reply;
pub mod types;

pub use crate::{
    daemon::{Builder, Daemon},
    op::Operation,
    request::Request,
    session::{CapabilityFlags, ConnectionInfo},
};
