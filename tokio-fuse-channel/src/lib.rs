mod libfuse3;
use libfuse3::Connection;

use futures::ready;
use mio::{unix::UnixReady, Ready};
use std::{
    ffi::OsStr,
    io::{self, IoSlice, IoSliceMut, Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    task::{self, Poll},
};
use tokio_fuse_io::{set_nonblocking, AsyncReadVectored, AsyncWriteVectored, OwnedEventedFd};
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_net::util::PollEvented;

/// Asynchronous I/O to communicate with the kernel.
#[derive(Debug)]
pub struct Channel {
    conn: Connection,
    fd: PollEvented<OwnedEventedFd>,
    mountpoint: PathBuf,
}

impl Channel {
    pub fn new(
        fsname: impl AsRef<OsStr>,
        mountpoint: impl AsRef<Path>,
        mountopts: &[&OsStr],
    ) -> io::Result<Self> {
        let fsname = fsname.as_ref();
        let mountpoint = mountpoint.as_ref();

        let conn = Connection::new(fsname, mountpoint, mountopts)?;

        let raw_fd = conn.raw_fd();
        set_nonblocking(raw_fd)?;

        Ok(Self {
            conn,
            fd: PollEvented::new(OwnedEventedFd(raw_fd)),
            mountpoint: mountpoint.into(),
        })
    }

    pub fn mountpoint(&self) -> &Path {
        &self.mountpoint
    }

    fn fd(self: Pin<&mut Self>) -> Pin<&mut PollEvented<OwnedEventedFd>> {
        unsafe { Pin::map_unchecked_mut(self, |this| &mut this.fd) }
    }

    fn poll_read_fn<F, R>(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        f: F,
    ) -> Poll<io::Result<R>>
    where
        F: FnOnce(&mut OwnedEventedFd) -> io::Result<R>,
    {
        let mut ready = Ready::readable();
        ready.insert(UnixReady::error());
        ready!(self.fd.poll_read_ready(cx, ready))?;

        match f(self.fd.get_mut()) {
            Ok(ret) => Poll::Ready(Ok(ret)),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.fd.clear_read_ready(cx, ready)?;
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl AsyncRead for Channel {
    unsafe fn prepare_uninitialized_buffer(&self, _: &mut [u8]) -> bool {
        false
    }

    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_fn(cx, |fd| fd.read(dst))
    }
}

impl AsyncWrite for Channel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(self.fd(), cx, src)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(self.fd(), cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(self.fd(), cx)
    }
}

impl AsyncReadVectored for Channel {
    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [IoSliceMut],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_fn(cx, |fd| fd.read_vectored(dst))
    }
}

impl AsyncWriteVectored for Channel {
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[IoSlice],
    ) -> Poll<io::Result<usize>> {
        ready!(self.fd.poll_write_ready(cx))?;
        match self.fd.get_mut().write_vectored(src) {
            Ok(count) => Poll::Ready(Ok(count)),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.fd.clear_write_ready(cx)?;
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}
