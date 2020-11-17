#![warn(clippy::todo, clippy::unimplemented)]

mod conn;
mod daemon;
mod parse;
mod request;
mod session;
mod util;
mod write;

pub mod op;

pub use crate::{
    daemon::{Builder, Daemon},
    op::Operation,
    request::Request,
    session::{CapabilityFlags, ConnectionInfo},
};
