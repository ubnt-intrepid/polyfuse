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

pub mod abi;
pub mod reply;
pub mod request;
pub mod session;
pub mod tokio;

pub use crate::op::Operations;

// ==== impl ====

use crate::{
    reply::ReplyRaw,
    request::Buffer,
    session::{Background, Session},
};
use futures::{
    future::{poll_fn, FusedFuture, FutureExt},
    io::{AsyncRead, AsyncWrite},
    ready,
};
use std::{io, task::Poll};

/// Run a FUSE filesystem.
pub async fn run<I, S, T>(channel: I, sig: S, ops: T) -> io::Result<Option<S::Output>>
where
    I: AsyncRead + AsyncWrite + Unpin + Clone,
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
    let mut background = Background::new();

    poll_fn(move |cx| {
        loop {
            if let Poll::Ready(sig) = sig.poll_unpin(cx) {
                // FIXME: graceful shutdown the background tasks.
                return Poll::Ready(Ok(Some(sig)));
            }

            log::trace!("run background tasks (num = {})", background.num_remains());
            let poll_background = background.poll_tasks(cx)?;

            if session.got_destroy() {
                continue;
            }

            match ready!(buf.poll_receive(cx, &mut channel))? {
                /* terminated = */
                true => {
                    log::debug!("connection was closed by the kernel");
                    log::trace!(
                        "remaining background tasks (num = {})",
                        background.num_remains()
                    );

                    if poll_background.is_ready() {
                        // FIXME: graceful shutdown the background tasks.
                    }
                    return Poll::Ready(Ok(None));
                }
                /* terminated = */
                false => {
                    let (request, data) = buf.extract()?;
                    log::debug!(
                        "Got a request: unique={}, opcode={:?}, arg={:?}, data={:?}",
                        request.header.unique,
                        request.header.opcode(),
                        request.arg,
                        data
                    );

                    let reply = ReplyRaw::new(request.header.unique, channel.clone());
                    session.dispatch(request, data, reply, &mut ops, &mut background);
                }
            }
        }
    })
    .await
}

/// Run a FUSE filesystem mounted on the specified path.
#[cfg(feature = "tokio")]
pub async fn mount<T>(
    fsname: impl AsRef<std::ffi::OsStr>,
    mointpoint: impl AsRef<std::path::Path>,
    mountopts: &[&std::ffi::OsStr],
    ops: T,
) -> io::Result<()>
where
    T: for<'a> Operations<&'a [u8]>,
{
    let channel = crate::tokio::Channel::mount(fsname, mointpoint, mountopts)?;
    let sig = crate::tokio::default_shutdown_signal()?;

    crate::run(channel, sig, ops).await?;
    Ok(())
}
