//! Additional I/O primitives for FUSE implementation.

use crate::nix;
use bitflags::bitflags;
use libc::c_int;
use std::{
    cmp,
    io::{self, prelude::*},
    mem,
    os::unix::prelude::*,
    ptr,
};

/// A pair of anonymous pipe.
#[derive(Debug)]
#[non_exhaustive]
pub struct Pipe {
    pub reader: PipeReader,
    pub writer: PipeWriter,
}

impl Pipe {
    /// Create a pair of anonymous pipe.
    pub fn new() -> io::Result<Self> {
        let mut fds = [0; 2];
        syscall! { pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
        Ok(Self {
            reader: PipeReader::new(fds[0]),
            writer: PipeWriter::new(fds[1]),
        })
    }

    pub fn clear(&mut self) -> io::Result<()> {
        if self.reader.remaining_bytes()? > 0 {
            let new_pipe = Self::new()?;
            drop(mem::replace(self, new_pipe));
        }
        Ok(())
    }
}

/// The read-half of pipe buffer.
#[derive(Debug)]
pub struct PipeReader(OwnedFd);

impl AsFd for PipeReader {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl AsRawFd for PipeReader {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl PipeReader {
    fn new(fd: RawFd) -> Self {
        Self(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Acquire the number of remaining bytes in the pipe buffer.
    pub fn remaining_bytes(&mut self) -> io::Result<usize> {
        let mut nbytes: c_int = 0;
        syscall! { ioctl(self.0.as_raw_fd(), libc::FIONREAD, ptr::addr_of_mut!(nbytes)) };
        Ok(nbytes as usize)
    }

    /// Splice the specified amount of bytes from the pipe buffer to `fd`.
    pub fn splice_to(
        &mut self,
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
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_vectored(&mut [io::IoSliceMut::new(buf)])
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        nix::readv(self.0.as_fd(), bufs)
    }
}

/// The write-half of pipe buffer.
#[derive(Debug)]
pub struct PipeWriter(OwnedFd);

impl AsFd for PipeWriter {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl AsRawFd for PipeWriter {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl PipeWriter {
    fn new(fd: RawFd) -> Self {
        Self(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Splice the specified amount of bytes from `fd` to the pipe buffer.
    pub fn splice_from(
        &mut self,
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
        self.write_vectored(&[io::IoSlice::new(buf)])
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        nix::writev(self.0.as_fd(), bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct SpliceFlags: u32 {
        const MOVE = libc::SPLICE_F_MOVE;
        const NONBLOCK = libc::SPLICE_F_NONBLOCK;
        const MORE = libc::SPLICE_F_MORE;
        const GIFT = libc::SPLICE_F_GIFT;
    }
}

pub trait SpliceRead: io::Read {
    /// Splice the chunk of bytes to the specified pipe buffer.
    ///
    ///
    fn splice_read(&mut self, dst: &mut Pipe, bufsize: usize) -> io::Result<usize>;
}

impl<R: ?Sized> SpliceRead for &mut R
where
    R: SpliceRead,
{
    #[inline]
    fn splice_read(&mut self, dst: &mut Pipe, bufsize: usize) -> io::Result<usize> {
        (**self).splice_read(dst, bufsize)
    }
}

impl SpliceRead for &[u8] {
    fn splice_read(&mut self, dst: &mut Pipe, bufsize: usize) -> io::Result<usize> {
        let count = cmp::min(self.len(), bufsize);
        dst.writer.write(&self[..count])
    }
}
