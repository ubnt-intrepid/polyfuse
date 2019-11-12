//! FUSE (Filesystem in userspace) framework for Rust.

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

pub mod reply;
pub mod request;

mod dirent;
mod fs;
mod session;

pub use crate::{
    dirent::{DirEntry, DirEntryType},
    fs::{FileAttr, FileLock, Filesystem, Forget, FsStatistics, Operation},
    request::{Buffer, BytesBuffer},
    session::{
        CapabilityFlags, ConnectionInfo, Context, Interrupt, NotifyRetrieve, Session,
        SessionInitializer,
    },
};
