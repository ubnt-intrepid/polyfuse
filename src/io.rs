//! Additional I/O primitives for FUSE implementation.

use rustix::pipe::{PipeFlags, SpliceFlags};
use std::{io, os::unix::prelude::*};

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
        let (reader, writer) = rustix::pipe::pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)?;
        Ok(Self {
            reader,
            writer,
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
        offset: Option<&mut u64>,
        len: usize,
        flags: SpliceFlags,
    ) -> io::Result<usize> {
        let amount = rustix::pipe::splice(self.reader.as_fd(), None, fd, offset, len, flags)?;
        self.len = self.len.saturating_sub(amount);
        Ok(amount)
    }

    /// Splice the specified amount of bytes from `fd` to the pipe buffer.
    pub fn splice_from(
        &mut self,
        fd: BorrowedFd<'_>,
        offset: Option<&mut u64>,
        len: usize,
        flags: SpliceFlags,
    ) -> io::Result<usize> {
        let amount = rustix::pipe::splice(fd, offset, self.writer.as_fd(), None, len, flags)?;
        self.len += amount;
        Ok(amount)
    }
}

impl io::Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_vectored(&mut [io::IoSliceMut::new(buf)])
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        let amount = rustix::io::readv(self.reader.as_fd(), bufs)?;
        self.len = self.len.saturating_sub(amount);
        Ok(amount)
    }
}

impl io::Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_vectored(&[io::IoSlice::new(buf)])
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        let amount = rustix::io::writev(self.writer.as_fd(), bufs)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::prelude::*;

    #[test]
    fn test_read_write() -> io::Result<()> {
        let mut pipe = Pipe::new()?;
        assert_eq!(pipe.len(), 0);
        assert!(pipe.is_empty());

        let n = pipe.write(b"foo bar baz")?;
        assert_eq!(n, 11);
        assert_eq!(pipe.len(), 11);
        assert!(!pipe.is_empty());

        let mut buf = [0u8; 32];
        let n = pipe.read(&mut buf[..])?;
        assert_eq!(n, 11);
        assert_eq!(pipe.len(), 0);
        assert_eq!(buf[..n], *b"foo bar baz");

        Ok(())
    }

    #[test]
    fn splice_from() -> io::Result<()> {
        const CONTENT: &[u8] = b"hello, splice world";

        let mut src = tempfile::tempfile()?;
        src.write_all(CONTENT)?;
        src.seek(io::SeekFrom::Start(0))?;
        src.flush()?;

        let mut pipe = Pipe::new()?;
        let n = pipe.splice_from(src.as_fd(), None, 1024, SpliceFlags::NONBLOCK)?;
        assert_eq!(n, CONTENT.len());
        assert_eq!(pipe.len(), CONTENT.len());

        let mut dst = [0u8; 32];
        let n = pipe.read(&mut dst[..])?;
        assert_eq!(n, CONTENT.len());
        assert_eq!(dst[..n], *CONTENT);

        Ok(())
    }

    #[test]
    fn splice_to() -> io::Result<()> {
        const CONTENT: &[u8] = b"hello, splice world";

        let mut pipe = Pipe::new()?;
        pipe.write_all(CONTENT)?;

        let mut dst = tempfile::tempfile()?;
        let n = pipe.splice_to(dst.as_fd(), None, 1024, SpliceFlags::NONBLOCK)?;
        assert_eq!(n, CONTENT.len());

        dst.seek(io::SeekFrom::Start(0))?;

        let mut buf = Vec::new();
        dst.read_to_end(&mut buf)?;
        assert_eq!(buf[..], *CONTENT);

        Ok(())
    }
}
