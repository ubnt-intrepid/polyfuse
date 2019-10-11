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

mod op;
mod tokio;

pub mod abi;
pub mod reply;
pub mod request;
pub mod session;

pub use crate::op::Operations;

// ==== impl ====

use crate::{reply::ReplyRaw, request::Buffer, session::Session};
use futures::{
    future::{FusedFuture, FutureExt},
    io::{AsyncRead, AsyncWrite},
    select,
};
use std::{io, pin::Pin};

/// Run a FUSE filesystem.
pub async fn run<I, S, T>(channel: I, sig: S, ops: T) -> io::Result<Option<S::Output>>
where
    I: AsyncRead + AsyncWrite + Unpin,
    S: FusedFuture + Unpin,
    T: for<'a> Operations<&'a [u8]>,
{
    let mut channel = channel;
    let mut sig = sig;
    let mut ops = ops;

    let mut buf = Buffer::default();

    let mut session = Session::initializer() //
        .start(&mut channel, &mut buf)
        .await?;

    loop {
        let mut turn = async {
            let terminated = buf.receive(&mut channel).await?;
            if terminated {
                log::debug!("closing the session");
                return Ok(true);
            }

            let (request, data) = buf.extract()?;
            log::debug!(
                "Got a request: unique={}, opcode={:?}, arg={:?}, data={:?}",
                request.header.unique,
                request.header.opcode(),
                request.arg,
                data
            );

            let reply = ReplyRaw::new(request.header.unique, &mut channel);
            session.process(request, data, reply, &mut ops).await
        };
        let mut turn = unsafe { Pin::new_unchecked(&mut turn) }.fuse();

        select! {
            res = turn => {
                let destroyed = res?;
                if destroyed {
                    log::debug!("The session is closed successfully");
                    return Ok(None);
                }
            },
            res = sig => {
                log::debug!("Got shutdown signal");
                return Ok(Some(res))
            },
        };
    }
}

/// Run a FUSE filesystem mounted on the specified path.
#[cfg(all(feature = "tokio", feature = "channel"))]
pub async fn mount<T>(
    fsname: impl AsRef<std::ffi::OsStr>,
    mointpoint: impl AsRef<std::path::Path>,
    mountopts: &[&std::ffi::OsStr],
    ops: T,
) -> io::Result<()>
where
    T: for<'a> Operations<&'a [u8]>,
{
    let channel = fuse_async_channel::tokio::Channel::mount(fsname, mointpoint, mountopts)?;
    let sig = crate::tokio::default_shutdown_signal()?;

    crate::run(channel, sig, ops).await?;
    Ok(())
}
