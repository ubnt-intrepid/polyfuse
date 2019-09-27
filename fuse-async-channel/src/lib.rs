mod libfuse3;
use libfuse3::Connection;

use fuse_async_io::{set_nonblocking, FdSource};
use futures_io::{AsyncRead, AsyncWrite, Initializer, IoSlice, IoSliceMut};
use futures_util::ready;
use mio::{unix::UnixReady, Ready};
use std::{
    ffi::OsStr,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    task::{self, Poll},
};
use tokio_net::util::PollEvented;

/// Asynchronous I/O to communicate with the kernel.
#[derive(Debug)]
pub struct Channel {
    conn: Connection,
    fd: PollEvented<FdSource>,
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
            fd: PollEvented::new(FdSource(raw_fd)),
            mountpoint: mountpoint.into(),
        })
    }

    pub fn mountpoint(&self) -> &Path {
        &self.mountpoint
    }

    fn poll_read_fn<F, R>(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        f: F,
    ) -> Poll<io::Result<R>>
    where
        F: FnOnce(&mut FdSource) -> io::Result<R>,
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

    fn poll_write_fn<F, R>(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        f: F,
    ) -> Poll<io::Result<R>>
    where
        F: FnOnce(&mut FdSource) -> io::Result<R>,
    {
        ready!(self.fd.poll_write_ready(cx))?;

        match f(self.fd.get_mut()) {
            Ok(ret) => Poll::Ready(Ok(ret)),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.fd.clear_write_ready(cx)?;
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl AsyncRead for Channel {
    unsafe fn initializer(&self) -> Initializer {
        Initializer::nop()
    }

    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_fn(cx, |fd| fd.read(dst))
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [IoSliceMut],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_fn(cx, |fd| fd.read_vectored(dst))
    }
}

impl AsyncWrite for Channel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_fn(cx, |fd| fd.write(src))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[IoSlice],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_fn(cx, |fd| fd.write_vectored(src))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        self.poll_write_fn(cx, |fd| fd.flush())
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
