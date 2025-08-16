//! A small facilities of Unix syscalls.

use libc::{c_int, c_void, iovec};
use std::{
    cmp, io,
    os::{fd::BorrowedFd, unix::prelude::*},
    ptr,
};

macro_rules! syscall {
    ($fn:ident ( $($arg:expr),* $(,)* ) ) => {{
        #[allow(unused_unsafe)]
        let res = unsafe { libc::$fn($($arg),*) };
        if res == -1 {
            return Err(std::io::Error::last_os_error());
        }
        res
    }};
}

pub fn read(fd: BorrowedFd<'_>, buf: &mut [u8]) -> io::Result<usize> {
    let len = syscall! {
        read(
            fd.as_raw_fd(), //
            buf.as_mut_ptr() as *mut c_void,
            buf.len(),
        )
    };
    Ok(len as usize)
}

pub fn readv(fd: BorrowedFd<'_>, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
    let len = syscall! {
        readv(
            fd.as_raw_fd(), //
            bufs.as_mut_ptr() as *mut iovec,
            cmp::min(bufs.len(), c_int::max_value() as usize) as c_int,
        )
    };
    Ok(len as usize)
}

pub fn write(fd: BorrowedFd<'_>, buf: &[u8]) -> io::Result<usize> {
    let res = syscall! {
        write(
            fd.as_raw_fd(), //
            buf.as_ptr() as *const c_void,
            buf.len(),
        )
    };
    Ok(res as usize)
}

pub fn writev(fd: BorrowedFd<'_>, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
    let res = syscall! {
        writev(
            fd.as_raw_fd(), //
            bufs.as_ptr() as *const iovec,
            cmp::min(bufs.len(), c_int::max_value() as usize) as c_int,
        )
    };
    Ok(res as usize)
}

// FIXME: replace with std::io::pipe
pub fn pipe() -> io::Result<(PipeReader, PipeWriter)> {
    let mut fds = [0; 2];
    syscall! { pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    Ok((PipeReader::new(fds[0]), PipeWriter::new(fds[1])))
}

#[derive(Debug)]
pub struct PipeReader(OwnedFd);

impl PipeReader {
    fn new(fd: RawFd) -> Self {
        Self(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    pub fn remaining_bytes(&self) -> io::Result<usize> {
        let mut nbytes: c_int = 0;
        syscall! { ioctl(self.0.as_raw_fd(), libc::FIONREAD, std::ptr::addr_of_mut!(nbytes)) };
        Ok(nbytes as usize)
    }

    /// Splice the specified amount of bytes from the pipe buffer to `fd`.
    #[cfg(target_os = "linux")]
    pub fn splice_to(
        &self,
        fd: BorrowedFd<'_>,
        len: usize,
        flags: SpliceFlags,
    ) -> io::Result<usize> {
        let n = syscall! {
            splice(
                self.0.as_raw_fd(),
                ptr::null_mut(),
                fd.as_raw_fd(),
                ptr::null_mut(),
                len,
                flags.bits(),
            )
        };
        Ok(n as usize)
    }
}

impl io::Read for PipeReader {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        (&*self).read_vectored(bufs)
    }
}

impl io::Read for &PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_vectored(&mut [io::IoSliceMut::new(buf)])
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        readv(self.0.as_fd(), bufs)
    }
}

#[derive(Debug)]
pub struct PipeWriter(OwnedFd);

impl PipeWriter {
    fn new(fd: RawFd) -> Self {
        Self(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Splice the specified amount of bytes from `fd` to the pipe buffer.
    #[cfg(target_os = "linux")]
    pub fn splice_from(
        &self,
        fd: BorrowedFd<'_>,
        len: usize,
        flags: SpliceFlags,
    ) -> io::Result<usize> {
        let n = syscall! {
            splice(
                fd.as_raw_fd(),
                ptr::null_mut(),
                self.0.as_raw_fd(),
                ptr::null_mut(),
                len,
                flags.bits(),
            )
        };
        Ok(n as usize)
    }

    /// Transfer the userspace pages to the kernel.
    ///
    /// # Safety
    /// When the flag `SpliceFlags::GIFT` is set, the ownership of `bufs` may be switched
    /// to the kernel.
    #[cfg(target_os = "linux")]
    pub unsafe fn vmsplice(
        &self,
        bufs: &[io::IoSlice<'_>],
        flags: SpliceFlags,
    ) -> io::Result<usize> {
        let n = syscall! {
            vmsplice(
                self.0.as_raw_fd(),
                bufs.as_ptr().cast(),
                bufs.len(),
                flags.bits(),
            )
        };
        Ok(n as usize)
    }
}

impl io::Write for PipeWriter {
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

impl io::Write for &PipeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_vectored(&[io::IoSlice::new(buf)])
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        writev(self.0.as_fd(), bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct SpliceFlags: u32 {
        const MOVE = libc::SPLICE_F_MOVE;
        const NONBLOCK = libc::SPLICE_F_NONBLOCK;
        const MORE = libc::SPLICE_F_MORE;
        const GIFT = libc::SPLICE_F_GIFT;
    }
}
