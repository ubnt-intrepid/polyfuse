use crate::io::{Pipe, SpliceRead, SpliceWrite};
use polyfuse_kernel::FUSE_DEV_IOC_CLONE;
use rustix::{
    fs::{Mode, OFlags},
    ioctl::{self, ioctl},
    pipe::SpliceFlags,
};
use std::{ffi::CStr, io, os::unix::prelude::*};

pub(crate) const FUSE_DEV_NAME: &CStr = c"/dev/fuse";

/// A connection with the FUSE kernel driver.
#[derive(Debug)]
pub struct Device {
    fd: OwnedFd,
}

impl Device {
    pub(crate) const fn from_fd(fd: OwnedFd) -> Self {
        Self { fd }
    }

    pub fn try_ioc_clone(&self) -> io::Result<Self> {
        let newfd = rustix::fs::open(FUSE_DEV_NAME, OFlags::RDWR | OFlags::CLOEXEC, Mode::empty())?;
        unsafe {
            ioctl(
                &newfd,
                ioctl::Setter::<FUSE_DEV_IOC_CLONE, _>::new(self.fd.as_raw_fd()),
            )?;
        }
        Ok(Self { fd: newfd })
    }
}

impl AsFd for Device {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl AsRawFd for Device {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl io::Read for Device {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        (&*self).read_vectored(bufs)
    }
}

impl io::Write for Device {
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

impl io::Read for &Device {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        rustix::io::read(self.fd.as_fd(), buf).map_err(Into::into)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        rustix::io::readv(self.fd.as_fd(), bufs).map_err(Into::into)
    }
}

impl io::Write for &Device {
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

impl SpliceRead for Device {
    #[inline]
    fn splice_read(&mut self, dst: &mut Pipe, len: usize, flags: SpliceFlags) -> io::Result<usize> {
        (&*self).splice_read(dst, len, flags)
    }
}

impl SpliceRead for &Device {
    fn splice_read(&mut self, dst: &mut Pipe, len: usize, flags: SpliceFlags) -> io::Result<usize> {
        dst.splice_from(self.fd.as_fd(), None, len, flags)
    }
}

impl SpliceWrite for Device {
    #[inline]
    fn splice_write(
        &mut self,
        dst: &mut Pipe,
        len: usize,
        flags: SpliceFlags,
    ) -> io::Result<usize> {
        (&*self).splice_write(dst, len, flags)
    }
}

impl SpliceWrite for &Device {
    fn splice_write(
        &mut self,
        src: &mut Pipe,
        len: usize,
        flags: SpliceFlags,
    ) -> io::Result<usize> {
        src.splice_to(self.fd.as_fd(), None, len, flags)
    }
}
