//! Additional I/O primitives.

use libc::{c_int, c_void, iovec};
use mio::{unix::EventedFd, Evented, PollOpt, Ready, Token};
use std::io::{self, IoSlice, IoSliceMut};
use std::{
    future::Future,
    io::{Read, Write},
    os::unix::io::RawFd,
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

pub fn set_nonblocking(fd: RawFd) -> io::Result<()> {
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

#[derive(Debug)]
pub struct OwnedEventedFd(pub RawFd);

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
