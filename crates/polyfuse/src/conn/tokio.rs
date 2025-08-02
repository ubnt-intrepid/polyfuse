#![cfg(feature = "tokio")]

use futures_util::{AsyncRead, AsyncWrite};
use std::{
    io::{self, prelude::*},
    pin::Pin,
    task::{self, Poll},
};
use tokio::io::unix::AsyncFd;

pub struct Connection {
    inner: AsyncFd<super::Connection>,
}

impl Connection {
    pub fn new(conn: super::Connection) -> io::Result<Self> {
        Ok(Self {
            inner: AsyncFd::new(conn)?,
        })
    }
}

impl AsyncRead for Connection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = task::ready!(self.inner.poll_read_ready(cx))?;
            match guard.try_io(|inner| inner.get_ref().read(buf)) {
                Ok(Ok(len)) => {
                    return Poll::Ready(Ok(len));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        bufs: &mut [io::IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = task::ready!(self.inner.poll_read_ready(cx))?;
            match guard.try_io(|inner| inner.get_ref().read_vectored(bufs)) {
                Ok(Ok(len)) => {
                    return Poll::Ready(Ok(len));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsyncWrite for Connection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = task::ready!(self.inner.poll_write_ready(cx))?;
            match guard.try_io(|inner| inner.get_ref().write(buf)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = task::ready!(self.inner.poll_write_ready(cx))?;
            match guard.try_io(|inner| inner.get_ref().write_vectored(bufs)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
