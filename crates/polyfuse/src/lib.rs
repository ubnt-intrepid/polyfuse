#![warn(clippy::todo, clippy::unimplemented)]

#[macro_use]
mod syscall;

mod conn;
mod decoder;
mod session;
mod write;

pub mod bytes;
pub mod op;
pub mod reply;

pub use crate::{
    conn::Connection,
    op::Operation,
    session::{CapabilityFlags, Config, ConnectionInfo, Data, Request, Session},
};
