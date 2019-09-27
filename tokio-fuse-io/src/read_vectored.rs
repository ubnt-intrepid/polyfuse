use std::io::{self, IoSliceMut};
use std::{
    future::Future,
    pin::Pin,
    task::{self, Poll},
};
use tokio_io::AsyncRead;

pub trait AsyncReadVectored: AsyncRead {
    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [IoSliceMut],
    ) -> Poll<io::Result<usize>> {
        let buf = dst
            .iter_mut()
            .find(|b| !b.is_empty())
            .map_or(&mut [][..], |b| &mut **b);
        self.poll_read(cx, buf)
    }
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
#[allow(missing_debug_implementations)]
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
