mod libfuse3;
use libfuse3::Connection;

use crate::io::{AsyncReadVectored, AsyncWriteVectored};
use futures::ready;
use libc::{c_int, c_void, iovec};
use mio::{
    unix::{EventedFd, UnixReady},
    Evented, PollOpt, Ready, Token,
};
use std::{
    ffi::OsStr,
    io::{self, IoSlice, IoSliceMut, Read, Write},
    os::unix::io::RawFd,
    path::{Path, PathBuf},
    pin::Pin,
    task::{self, Poll},
};
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

#[derive(Debug)]
struct OwnedEventedFd(RawFd);

impl Read for OwnedEventedFd {
    fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        let res = unsafe {
            libc::read(
                self.0, //
                dst.as_mut_ptr() as *mut c_void,
                dst.len(),
            )
        };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(res as usize)
    }

    fn read_vectored(&mut self, dst: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        let res = unsafe {
            libc::readv(
                self.0, //
                dst.as_mut_ptr() as *mut iovec,
                dst.len() as c_int,
            )
        };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(res as usize)
    }
}

impl Write for OwnedEventedFd {
    fn write(&mut self, src: &[u8]) -> io::Result<usize> {
        let res = unsafe {
            libc::write(
                self.0, //
                src.as_ptr() as *const c_void,
                src.len(),
            )
        };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(res as usize)
    }

    fn write_vectored(&mut self, src: &[IoSlice]) -> io::Result<usize> {
        let res = unsafe {
            libc::writev(
                self.0, //
                src.as_ptr() as *const iovec,
                src.len() as c_int,
            )
        };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(res as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        let res = unsafe { libc::fsync(self.0) };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Evented for OwnedEventedFd {
    fn register(
        &self,
        poll: &mio::Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.0).register(poll, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &mio::Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.0).reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &mio::Poll) -> io::Result<()> {
        EventedFd(&self.0).deregister(poll)
    }
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }

    let res = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if res < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}
