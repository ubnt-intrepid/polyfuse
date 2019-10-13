#![cfg(feature = "tokio")]
#![cfg_attr(feature = "docs", doc(cfg(tokio)))]

use fuse_async_channel::Channel as RawChannel;
use futures::{
    future::{FusedFuture, FutureExt},
    io::{AsyncRead, AsyncWrite},
    ready,
    stream::StreamExt,
};
use mio::{unix::UnixReady, Ready};
use std::{
    cell::UnsafeCell,
    ffi::{OsStr, OsString},
    io::{self, IoSlice, IoSliceMut, Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    task::{self, Poll},
};
use tokio_net::{signal::ctrl_c, util::PollEvented};
use tokio_sync::semaphore::{Permit, Semaphore};

pub fn default_shutdown_signal() -> io::Result<impl FusedFuture<Output = ()> + Unpin> {
    Ok(ctrl_c()?.into_future().map(|_| ()))
}

#[derive(Debug)]
pub struct Builder {
    fsname: OsString,
    mountopts: Vec<OsString>,
}

impl Builder {
    pub fn mountopts(mut self, opts: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Self {
        self.mountopts
            .extend(opts.into_iter().map(|opt| opt.as_ref().into()));
        self
    }

    pub fn mount(self, mountpoint: impl AsRef<Path>) -> io::Result<Channel> {
        let mountpoint = mountpoint.as_ref();

        let channel = RawChannel::new(self.fsname, mountpoint, self.mountopts)?;

        Ok(Channel {
            inner: Arc::new(Inner {
                channel: UnsafeCell::new(PollEvented::new(channel)),
                semaphore: Semaphore::new(1),
            }),
            permit: Permit::new(),
            mountpoint: mountpoint.into(),
        })
    }
}

/// Asynchronous I/O to communicate with the kernel.
#[derive(Debug)]
pub struct Channel {
    inner: Arc<Inner>,
    mountpoint: PathBuf,
    permit: Permit,
}

#[derive(Debug)]
struct Inner {
    channel: UnsafeCell<PollEvented<RawChannel>>,
    semaphore: Semaphore,
}

impl Clone for Channel {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            mountpoint: self.mountpoint.clone(),
            permit: Permit::new(),
        }
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.release_lock();
    }
}

impl Channel {
    pub fn builder(fsname: impl AsRef<OsStr>) -> Builder {
        Builder {
            fsname: fsname.as_ref().into(),
            mountopts: vec![],
        }
    }

    pub fn mount(
        fsname: impl AsRef<OsStr>,
        mountpoint: impl AsRef<Path>,
        mountopts: &[&OsStr],
    ) -> io::Result<Self> {
        Self::builder(fsname) //
            .mountopts(mountopts)
            .mount(mountpoint)
    }

    pub fn mountpoint(&self) -> &Path {
        &self.mountpoint
    }

    fn poll_lock<F, R>(mut self: Pin<&mut Self>, cx: &mut task::Context, f: F) -> Poll<R>
    where
        F: FnOnce(Pin<&mut PollEvented<RawChannel>>, &mut task::Context) -> Poll<R>,
    {
        let this = &mut *self;
        ready!(this.poll_acquire_lock(cx));

        let evented = unsafe { Pin::new_unchecked(&mut (*this.inner.channel.get())) };
        let ret = ready!(f(evented, cx));

        this.release_lock();
        Poll::Ready(ret)
    }

    fn poll_acquire_lock(&mut self, cx: &mut task::Context) -> Poll<()> {
        if self.permit.is_acquired() {
            return Poll::Ready(());
        }

        ready!(self.permit.poll_acquire(cx, &self.inner.semaphore))
            .unwrap_or_else(|e| unreachable!("{}", e));

        Poll::Ready(())
    }

    fn release_lock(&mut self) {
        if self.permit.is_acquired() {
            self.permit.release(&self.inner.semaphore);
        }
    }
}

fn poll_read_fn<F, R>(
    mut evented: Pin<&mut PollEvented<RawChannel>>,
    cx: &mut task::Context<'_>,
    f: F,
) -> Poll<io::Result<R>>
where
    F: FnOnce(&mut RawChannel) -> io::Result<R>,
{
    let evented = &mut *evented;

    let mut ready = Ready::readable();
    ready.insert(UnixReady::error());
    ready!(evented.poll_read_ready(cx, ready))?;

    match f(evented.get_mut()) {
        Ok(ret) => Poll::Ready(Ok(ret)),
        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
            evented.clear_read_ready(cx, ready)?;
            Poll::Pending
        }
        Err(e) => Poll::Ready(Err(e)),
    }
}

fn poll_write_fn<F, R>(
    mut evented: Pin<&mut PollEvented<RawChannel>>,
    cx: &mut task::Context<'_>,
    f: F,
) -> Poll<io::Result<R>>
where
    F: FnOnce(&mut RawChannel) -> io::Result<R>,
{
    let evented = &mut *evented;
    ready!(evented.poll_write_ready(cx))?;

    match f(evented.get_mut()) {
        Ok(ret) => Poll::Ready(Ok(ret)),
        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
            evented.clear_write_ready(cx)?;
            Poll::Pending
        }
        Err(e) => Poll::Ready(Err(e)),
    }
}

impl AsyncRead for Channel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_lock(cx, |evented, cx| {
            poll_read_fn(evented, cx, |fd| fd.read(dst))
        })
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [IoSliceMut],
    ) -> Poll<io::Result<usize>> {
        self.poll_lock(cx, |evented, cx| {
            poll_read_fn(evented, cx, |fd| fd.read_vectored(dst))
        })
    }
}

impl AsyncWrite for Channel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_lock(cx, |evented, cx| {
            poll_write_fn(evented, cx, |fd| fd.write(src))
        })
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[IoSlice],
    ) -> Poll<io::Result<usize>> {
        self.poll_lock(cx, |evented, cx| {
            poll_write_fn(evented, cx, |fd| fd.write_vectored(src))
        })
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        self.poll_lock(cx, |evented, cx| {
            poll_write_fn(evented, cx, |fd| fd.flush())
        })
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
