use futures::{
    io::{AsyncRead, AsyncWrite},
    ready,
};
use std::{
    cell::UnsafeCell,
    io::{self, IoSlice, IoSliceMut},
    pin::Pin,
    sync::Arc,
    task::{self, Poll},
};
use tokio::sync::semaphore::{Permit, Semaphore};

#[derive(Debug)]
pub struct Lock<T> {
    inner: Arc<LockInner<T>>,
    permit: Permit,
}

#[derive(Debug)]
struct LockInner<T> {
    val: UnsafeCell<T>,
    semaphore: Semaphore,
}

impl<T> Clone for Lock<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            permit: Permit::new(),
        }
    }
}

impl<T> Drop for Lock<T> {
    fn drop(&mut self) {
        self.release_lock();
    }
}

unsafe impl<T: Send> Send for Lock<T> {}

impl<T> Lock<T> {
    pub fn new(val: T) -> Self {
        Self {
            inner: Arc::new(LockInner {
                val: UnsafeCell::new(val),
                semaphore: Semaphore::new(1),
            }),
            permit: Permit::new(),
        }
    }

    fn poll_lock_with<F, R>(self: Pin<&mut Self>, cx: &mut task::Context, f: F) -> Poll<R>
    where
        F: FnOnce(Pin<&mut T>, &mut task::Context) -> Poll<R>,
    {
        let this = Pin::into_inner(self);

        ready!(this.poll_acquire_lock(cx));

        let t = unsafe { Pin::new_unchecked(&mut (*this.inner.val.get())) };
        let ret = ready!(f(t, cx));

        this.release_lock();

        Poll::Ready(ret)
    }

    fn poll_acquire_lock(&mut self, cx: &mut task::Context) -> Poll<()> {
        ready!(self.permit.poll_acquire(cx, &self.inner.semaphore))
            .unwrap_or_else(|e| unreachable!("{}", e));
        Poll::Ready(())
    }

    fn release_lock(&mut self) {
        self.permit.release(&self.inner.semaphore);
    }
}

impl<T> AsyncRead for Lock<T>
where
    T: AsyncRead,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context,
        dst: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_lock_with(cx, |reader, cx| reader.poll_read(cx, dst))
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context,
        dst: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        self.poll_lock_with(cx, |reader, cx| reader.poll_read_vectored(cx, dst))
    }
}

impl<T> AsyncWrite for Lock<T>
where
    T: AsyncWrite,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context,
        src: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_lock_with(cx, |writer, cx| writer.poll_write(cx, src))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context,
        src: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.poll_lock_with(cx, |writer, cx| writer.poll_write_vectored(cx, src))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context) -> Poll<io::Result<()>> {
        self.poll_lock_with(cx, |writer, cx| writer.poll_flush(cx))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut task::Context) -> Poll<io::Result<()>> {
        self.poll_lock_with(cx, |writer, cx| writer.poll_close(cx))
    }
}
