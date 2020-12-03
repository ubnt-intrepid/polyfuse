pub use polyfuse::conn::Writer;

use async_io::Async;
use futures::{
    io::AsyncRead,
    task::{self, Poll},
};
use std::{
    ffi::OsStr,
    io::{self, IoSlice, IoSliceMut},
    path::Path,
    pin::Pin,
};

pub struct Connection {
    inner: Async<polyfuse::conn::Connection>,
}

impl Connection {
    pub async fn open(mountpoint: &Path, mountopts: &[&OsStr]) -> io::Result<Self> {
        // TODO: asyncify.
        let inner = polyfuse::conn::Connection::open(mountpoint, mountopts)?;
        Ok(Self {
            inner: Async::new(inner)?,
        })
    }

    #[inline]
    pub fn writer(&self) -> Writer {
        self.inner.get_ref().writer()
    }
}

impl AsyncRead for Connection {
    #[inline]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.get_mut()).poll_read(cx, buf)
    }

    #[inline]
    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.get_mut()).poll_read_vectored(cx, bufs)
    }
}

impl AsyncRead for &Connection {
    #[inline]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &self.get_mut().inner).poll_read(cx, buf)
    }

    #[inline]
    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &self.get_mut().inner).poll_read_vectored(cx, bufs)
    }
}

impl io::Write for Connection {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self).write(buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        (&*self).write_vectored(bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        (&*self).flush()
    }
}

impl io::Write for &Connection {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.get_ref().write(buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.inner.get_ref().write_vectored(bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
