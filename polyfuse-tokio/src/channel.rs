//! Establish connection with FUSE kernel driver.

use crate::mount::{Connection, MountOptions};
use futures::{
    io::{AsyncRead, AsyncWrite},
    ready,
    task::{self, Poll},
};
use mio::{unix::UnixReady, Ready};
use std::{
    io::{self, IoSlice, IoSliceMut, Read, Write},
    path::Path,
    pin::Pin,
};
use tokio::io::PollEvented;

/// Asynchronous I/O object that communicates with the FUSE kernel driver.
#[derive(Debug)]
pub struct Channel(PollEvented<Connection>);

impl Channel {
    /// Establish a connection with the FUSE kernel driver.
    pub fn open(mountpoint: &Path, mountopts: &MountOptions) -> io::Result<Self> {
        let conn = Connection::open(mountpoint, mountopts)?;
        let evented = PollEvented::new(conn)?;
        Ok(Self(evented))
    }

    fn poll_read_with<F, R>(&mut self, cx: &mut task::Context<'_>, f: F) -> Poll<io::Result<R>>
    where
        F: FnOnce(&mut Connection) -> io::Result<R>,
    {
        let mut ready = Ready::readable();
        ready.insert(UnixReady::error());
        ready!(self.0.poll_read_ready(cx, ready))?;

        match f(self.0.get_mut()) {
            Ok(ret) => Poll::Ready(Ok(ret)),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.0.clear_read_ready(cx, ready)?;
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_write_with<F, R>(&mut self, cx: &mut task::Context<'_>, f: F) -> Poll<io::Result<R>>
    where
        F: FnOnce(&mut Connection) -> io::Result<R>,
    {
        ready!(self.0.poll_write_ready(cx))?;

        match f(self.0.get_mut()) {
            Ok(ret) => Poll::Ready(Ok(ret)),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.0.clear_write_ready(cx)?;
                Poll::Pending
            }
            Err(e) => {
                tracing::debug!("write error: {}", e);
                Poll::Ready(Err(e))
            }
        }
    }

    /// Attempt to create a clone of this channel.
    pub fn try_clone(&self) -> io::Result<Self> {
        let conn = self.0.get_ref().try_clone()?;
        Ok(Self(PollEvented::new(conn)?))
    }
}

impl AsyncRead for Channel {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_with(cx, |fd| fd.read(dst))
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [IoSliceMut],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_with(cx, |fd| fd.read_vectored(dst))
    }
}

impl AsyncWrite for Channel {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_with(cx, |fd| fd.write(src))
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[IoSlice],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_with(cx, |fd| fd.write_vectored(src))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        self.poll_write_with(cx, |fd| fd.flush())
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
