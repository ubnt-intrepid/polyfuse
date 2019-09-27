use std::io::{self, IoSlice};
use std::{
    future::Future,
    pin::Pin,
    task::{self, Poll},
};
use tokio_io::AsyncWrite;

pub trait AsyncWriteVectored: AsyncWrite {
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[IoSlice],
    ) -> Poll<io::Result<usize>>;
}

pub trait AsyncWriteVectoredExt: AsyncWriteVectored {
    fn write_vectored<'a>(&'a mut self, src: &'a [IoSlice<'a>]) -> WriteVectored<'a, Self> {
        WriteVectored {
            writer: self,
            buf: src,
        }
    }
}

impl<T: AsyncWriteVectored + ?Sized> AsyncWriteVectoredExt for T {}

#[doc(hidden)]
#[allow(missing_debug_implementations)]
pub struct WriteVectored<'a, W: ?Sized> {
    writer: &'a mut W,
    buf: &'a [IoSlice<'a>],
}

impl<W: ?Sized> Future for WriteVectored<'_, W>
where
    W: AsyncWriteVectored + Unpin,
{
    type Output = io::Result<usize>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let me = &mut *self;
        Pin::new(&mut *me.writer).poll_write_vectored(cx, me.buf)
    }
}
