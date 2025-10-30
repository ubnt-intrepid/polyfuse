use crate::{
    io::{Pipe, SpliceRead, SpliceWrite},
    types::BackingID,
};
use polyfuse_kernel::{
    fuse_backing_map, FUSE_DEV_IOC_BACKING_CLOSE, FUSE_DEV_IOC_BACKING_OPEN, FUSE_DEV_IOC_CLONE,
};
use rustix::{
    fs::{Mode, OFlags},
    ioctl::{self, ioctl, Ioctl},
    pipe::SpliceFlags,
};
use std::{ffi::CStr, io, os::unix::prelude::*};

pub(crate) const FUSE_DEV_NAME: &CStr = c"/dev/fuse";

struct BackingOpen(fuse_backing_map);
unsafe impl Ioctl for BackingOpen {
    const IS_MUTATING: bool = false;

    type Output = i32;

    fn opcode(&self) -> ioctl::Opcode {
        FUSE_DEV_IOC_BACKING_OPEN
    }

    fn as_ptr(&mut self) -> *mut rustix::ffi::c_void {
        std::ptr::addr_of_mut!(self.0).cast()
    }

    unsafe fn output_from_ptr(
        out: ioctl::IoctlOutput,
        _: *mut rustix::ffi::c_void,
    ) -> rustix::io::Result<Self::Output> {
        Ok(out)
    }
}

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

    pub unsafe fn backing_open(&self, fd: BorrowedFd<'_>) -> io::Result<BackingID> {
        let backing_id = unsafe {
            ioctl(
                &self.fd,
                BackingOpen(fuse_backing_map {
                    fd: fd.as_raw_fd(),
                    flags: 0,
                    padding: 0,
                }),
            )?
        };
        BackingID::from_raw(backing_id).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "The backing ID must be nonzero")
        })
    }

    pub unsafe fn backing_close(&self, backing_id: BackingID) -> io::Result<()> {
        unsafe {
            ioctl(
                &self.fd,
                ioctl::Setter::<FUSE_DEV_IOC_BACKING_CLOSE, _>::new(backing_id.into_raw()),
            )?;
        }
        Ok(())
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
