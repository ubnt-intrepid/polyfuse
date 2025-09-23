use crate::io::{Pipe, SpliceRead};
use polyfuse_kernel::FUSE_DEV_IOC_CLONE;
use rustix::pipe::SpliceFlags;
use std::{ffi::CStr, io, os::unix::prelude::*};

const FUSE_DEV_NAME: &CStr = c"/dev/fuse";

/// A connection with the FUSE kernel driver.
#[derive(Debug)]
pub struct Connection {
    fd: OwnedFd,
}

impl From<OwnedFd> for Connection {
    fn from(fd: OwnedFd) -> Self {
        Self { fd }
    }
}

impl Connection {
    pub fn try_ioc_clone(&self) -> io::Result<Self> {
        let newfd = syscall! { open(FUSE_DEV_NAME.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        syscall! { ioctl(newfd, FUSE_DEV_IOC_CLONE, &self.fd.as_raw_fd()) };
        Ok(Self {
            fd: unsafe { OwnedFd::from_raw_fd(newfd) },
        })
    }
}

impl AsFd for Connection {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl AsRawFd for Connection {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl io::Read for Connection {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        (&*self).read_vectored(bufs)
    }
}

impl io::Write for Connection {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self).write(buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        (&*self).write_vectored(bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        (&*self).flush()
    }
}

impl io::Read for &Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        rustix::io::read(self.fd.as_fd(), buf).map_err(Into::into)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        rustix::io::readv(self.fd.as_fd(), bufs).map_err(Into::into)
    }
}

impl io::Write for &Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        rustix::io::write(self.fd.as_fd(), buf).map_err(Into::into)
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        rustix::io::writev(self.fd.as_fd(), bufs).map_err(Into::into)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl SpliceRead for Connection {
    #[inline]
    fn splice_read(&mut self, pipe: &mut Pipe, bufsize: usize) -> io::Result<usize> {
        (&*self).splice_read(pipe, bufsize)
    }
}

impl SpliceRead for &Connection {
    fn splice_read(&mut self, pipe: &mut Pipe, bufsize: usize) -> io::Result<usize> {
        pipe.splice_from(self.fd.as_fd(), None, bufsize, SpliceFlags::NONBLOCK)
    }
}
