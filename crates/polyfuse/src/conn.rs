use libc::{c_int, c_void, iovec};
use std::{
    cmp,
    ffi::OsStr,
    io,
    os::unix::prelude::*,
    path::{Path, PathBuf},
};

/// A connection with the FUSE kernel driver.
#[derive(Debug)]
pub struct Connection {
    fd: RawFd,
    mountpoint: PathBuf,
}

impl Drop for Connection {
    fn drop(&mut self) {
        crate::mount::unmount(self.fd, &self.mountpoint);
    }
}

impl Connection {
    /// Establish a connection with the FUSE kernel driver.
    pub fn open(mountpoint: &Path, mountopts: &[&OsStr]) -> io::Result<Self> {
        let fd = crate::mount::mount(mountpoint, mountopts)?;
        Ok(Self {
            fd,
            mountpoint: mountpoint.into(),
        })
    }

    pub fn set_nonblocking(&self) -> io::Result<()> {
        let flags = syscall! { fcntl(self.as_raw_fd(), libc::F_GETFL, 0) };
        syscall! { fcntl(self.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
        Ok(())
    }

    fn read(&self, dst: &mut [u8]) -> io::Result<usize> {
        let len = syscall! {
            read(
                self.fd, //
                dst.as_mut_ptr() as *mut c_void,
                dst.len(),
            )
        };
        Ok(len as usize)
    }

    fn read_vectored(&self, dst: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        let len = syscall! {
            readv(
                self.fd, //
                dst.as_mut_ptr() as *mut iovec,
                cmp::min(dst.len(), c_int::max_value() as usize) as c_int,
            )
        };
        Ok(len as usize)
    }

    fn write(&self, src: &[u8]) -> io::Result<usize> {
        let res = syscall! {
            write(
                self.fd, //
                src.as_ptr() as *const c_void,
                src.len(),
            )
        };
        Ok(res as usize)
    }

    fn write_vectored(&self, src: &[io::IoSlice<'_>]) -> io::Result<usize> {
        let res = syscall! {
            writev(
                self.fd, //
                src.as_ptr() as *const iovec,
                cmp::min(src.len(), c_int::max_value() as usize) as c_int,
            )
        };
        Ok(res as usize)
    }
}

impl AsRawFd for Connection {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl io::Read for Connection {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (*self).read(buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        (*self).read_vectored(bufs)
    }
}

impl io::Read for &Connection {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read(buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        (**self).read_vectored(bufs)
    }
}

impl io::Write for Connection {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (*self).write(buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        (*self).write_vectored(bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Write for &Connection {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (**self).write(buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        (**self).write_vectored(bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
