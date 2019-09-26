use crate::{
    conn::{Connection, OwnedEventedFd},
    io::{AsyncReadVectored, AsyncWriteVectored},
};
use futures::ready;
use mio::{unix::UnixReady, Ready};
use std::{
    ffi::OsStr,
    io::{self, IoSlice, IoSliceMut, Read, Write},
    path::Path,
    pin::Pin,
    task::{self, Poll},
};
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_net::util::PollEvented;

/// Asynchronous I/O to communicate with the kernel.
#[derive(Debug)]
pub struct Channel {
    session: Connection,
    fd: PollEvented<OwnedEventedFd>,
}

impl Channel {
    pub fn new(
        fsname: impl AsRef<OsStr>,
        mountpoint: impl AsRef<Path>,
        mountopts: &[&OsStr],
    ) -> io::Result<Self> {
        let session = Connection::new(fsname.as_ref(), mountpoint.as_ref(), mountopts)?;
        let fd = PollEvented::new(session.raw_fd().clone());
        Ok(Self { session, fd })
    }

    pub fn mountpoint(&self) -> &Path {
        self.session.mountpoint()
    }

    fn fd(self: Pin<&mut Self>) -> Pin<&mut PollEvented<OwnedEventedFd>> {
        unsafe { Pin::map_unchecked_mut(self, |this| &mut this.fd) }
    }
}

impl AsyncRead for Channel {
    unsafe fn prepare_uninitialized_buffer(&self, _: &mut [u8]) -> bool {
        false
    }

    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut ready = Ready::readable();
        ready.insert(UnixReady::error());
        ready!(self.fd.poll_read_ready(cx, ready))?;

        match self.fd.get_mut().read(dst) {
            Ok(count) => Poll::Ready(Ok(count)),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.fd.clear_read_ready(cx, ready)?;
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
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
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [IoSliceMut],
    ) -> Poll<io::Result<usize>> {
        let mut ready = Ready::readable();
        ready.insert(UnixReady::error());
        ready!(self.fd.poll_read_ready(cx, ready))?;

        match self.fd.get_mut().read_vectored(dst) {
            Ok(count) => Poll::Ready(Ok(count)),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.fd.clear_read_ready(cx, ready)?;
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
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
