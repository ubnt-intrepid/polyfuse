use futures::{
    io::{AsyncRead, AsyncWrite},
    task::{self, Poll},
};
use std::{
    io::{self, IoSlice, IoSliceMut},
    pin::Pin,
};

/// Unite a pair of asynchrounous reader and writer into a single I/O object.
pub fn unite<R, W>(reader: R, writer: W) -> Unite<R, W>
where
    R: AsyncRead,
    W: AsyncWrite,
{
    Unite { reader, writer }
}

#[derive(Debug)]
pub struct Unite<R, W> {
    reader: R,
    writer: W,
}

impl<R, W> Unite<R, W> {
    fn reader(self: Pin<&mut Self>) -> Pin<&mut R> {
        unsafe { self.map_unchecked_mut(|this| &mut this.reader) }
    }

    fn writer(self: Pin<&mut Self>) -> Pin<&mut W> {
        unsafe { self.map_unchecked_mut(|this| &mut this.writer) }
    }

    pub fn split(self) -> (R, W) {
        (self.reader, self.writer)
    }
}

impl<R, W> AsyncRead for Unite<R, W>
where
    R: AsyncRead,
{
    #[inline]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.reader().poll_read(cx, dst)
    }

    #[inline]
    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        self.reader().poll_read_vectored(cx, dst)
    }
}

impl<R, W> AsyncWrite for Unite<R, W>
where
    W: AsyncWrite,
{
    #[inline]
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.writer().poll_write(cx, src)
    }

    #[inline]
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.writer().poll_write_vectored(cx, src)
    }

    #[inline]
    fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        self.writer().poll_flush(cx)
    }

    #[inline]
    fn poll_close(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        self.writer().poll_close(cx)
    }
}
