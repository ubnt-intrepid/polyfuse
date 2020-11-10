use futures::{
    io::{AsyncRead, AsyncWrite},
    task::{self, Poll},
};
use pin_project_lite::pin_project;
use std::{
    io, mem,
    pin::Pin,
    slice,
    time::{Duration, SystemTime},
};

pin_project! {
    pub(crate) struct Unite<R, W> {
        #[pin]
        pub(crate) reader: R,
        #[pin]
        pub(crate) writer: W,
    }
}

impl<R, W> AsyncRead for Unite<R, W>
where
    R: AsyncRead,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.project().reader.poll_read(cx, dst)
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [io::IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        self.project().reader.poll_read_vectored(cx, dst)
    }
}

impl<R, W> AsyncWrite for Unite<R, W>
where
    W: AsyncWrite,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.project().writer.poll_write(cx, src)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.project().writer.poll_write_vectored(cx, src)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        self.project().writer.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        self.project().writer.poll_close(cx)
    }
}

#[inline(always)]
pub(crate) unsafe fn as_bytes<T: Sized>(t: &T) -> &[u8] {
    slice::from_raw_parts(t as *const T as *const u8, mem::size_of::<T>())
}

#[inline(always)]
pub(crate) unsafe fn as_bytes_mut<T: Sized>(t: &mut T) -> &mut [u8] {
    slice::from_raw_parts_mut(t as *mut T as *mut u8, mem::size_of::<T>())
}

#[inline]
pub(crate) fn make_system_time((sec, nsec): (u64, u32)) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::new(sec, nsec)
}

#[inline]
pub(crate) fn make_raw_time(time: SystemTime) -> (u64, u32) {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("invalid time");
    (duration.as_secs(), duration.subsec_nanos())
}

pub(crate) trait BuilderExt {
    fn if_some<T, F>(self, value: Option<T>, f: F) -> Self
    where
        Self: Sized,
        F: FnOnce(Self, T) -> Self,
    {
        match value {
            Some(value) => f(self, value),
            None => self,
        }
    }
}

impl<T> BuilderExt for T {}
