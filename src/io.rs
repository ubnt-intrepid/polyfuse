//! Additional I/O primitives for FUSE implementation.

use crate::nix;
use std::{
    cmp,
    io::{self, prelude::*},
    os::unix::prelude::*,
};

pub use crate::nix::SpliceFlags;

/// A pair of anonymous pipe.
#[derive(Debug)]
pub struct Pipe {
    reader: OwnedFd,
    writer: OwnedFd,
    len: usize,
}

impl Pipe {
    /// Create a pair of anonymous pipe.
    pub fn new() -> io::Result<Self> {
        let mut fds = [0; 2];
        syscall! { pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
        Ok(Self {
            reader: unsafe { OwnedFd::from_raw_fd(fds[0]) },
            writer: unsafe { OwnedFd::from_raw_fd(fds[1]) },
            len: 0,
        })
    }

    /// Return the amount of remaining bytes in the pipe buffer.
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Splice the specified amount of bytes from the pipe buffer to `fd`.
    pub fn splice_to(
        &mut self,
        fd: BorrowedFd<'_>,
        offset: Option<i64>,
        len: usize,
        flags: SpliceFlags,
    ) -> io::Result<usize> {
        let amount = nix::splice(self.reader.as_fd(), None, fd, offset, len, flags)?;
        self.len = self.len.saturating_sub(amount);
        Ok(amount)
    }

    /// Splice the specified amount of bytes from `fd` to the pipe buffer.
    pub fn splice_from(
        &mut self,
        fd: BorrowedFd<'_>,
        offset: Option<i64>,
        len: usize,
        flags: SpliceFlags,
    ) -> io::Result<usize> {
        let amount = nix::splice(fd, offset, self.writer.as_fd(), None, len, flags)?;
        self.len += amount;
        Ok(amount)
    }
}

impl io::Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_vectored(&mut [io::IoSliceMut::new(buf)])
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        let amount = nix::readv(self.reader.as_fd(), bufs)?;
        self.len = self.len.saturating_sub(amount);
        Ok(amount)
    }
}

impl io::Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_vectored(&[io::IoSlice::new(buf)])
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        let amount = nix::writev(self.writer.as_fd(), bufs)?;
        self.len += amount;
        Ok(amount)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
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
        let amount = dst.write(&self[..count])?;
        *self = &self[amount..];
        Ok(amount)
    }
}
