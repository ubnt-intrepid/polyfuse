use std::io::{self, IoSlice, IoSliceMut};
use std::{
    future::Future,
    pin::Pin,
    task::{self, Poll},
};
use tokio_io::{AsyncRead, AsyncWrite};

pub trait AsyncReadVectored: AsyncRead {
    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [IoSliceMut],
    ) -> Poll<io::Result<usize>>;
}

pub trait AsyncReadVectoredExt: AsyncReadVectored {
    fn read_vectored<'a>(&'a mut self, dst: &'a mut [IoSliceMut<'a>]) -> ReadVectored<'a, Self> {
        ReadVectored {
            reader: self,
            buf: dst,
        }
    }
}

impl<T: AsyncReadVectored + ?Sized> AsyncReadVectoredExt for T {}

#[doc(hidden)]
pub struct ReadVectored<'a, R: ?Sized> {
    reader: &'a mut R,
    buf: &'a mut [IoSliceMut<'a>],
}

impl<R: ?Sized> Future for ReadVectored<'_, R>
where
    R: AsyncReadVectored + Unpin,
{
    type Output = io::Result<usize>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let me = &mut *self;
        Pin::new(&mut *me.reader).poll_read_vectored(cx, me.buf)
    }
}

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
