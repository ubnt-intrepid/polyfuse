use crate::Channel;
use std::{
    io::{self, IoSlice, IoSliceMut},
    pin::Pin,
    task::{self, Poll},
};
use tokio_fuse_io::{AsyncReadVectored, AsyncWriteVectored};
use tokio_io::{AsyncRead, AsyncWrite};

impl AsyncRead for Channel {
    #[inline]
    unsafe fn prepare_uninitialized_buffer(&self, _: &mut [u8]) -> bool {
        false
    }

    #[inline]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read(cx, dst)
    }
}

impl AsyncReadVectored for Channel {
    #[inline]
    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [IoSliceMut],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_vectored(cx, dst)
    }
}

impl AsyncWrite for Channel {
    #[inline]
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write(cx, src)
    }

    #[inline]
    fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }

    #[inline]
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        self.poll_shutdown(cx)
    }
}

impl AsyncWriteVectored for Channel {
    #[inline]
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[IoSlice],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_vectored(cx, src)
    }
}
