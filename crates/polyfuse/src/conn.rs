use crate::{bytes::Bytes, nix};
use polyfuse_kernel::{fuse_in_header, fuse_notify_code, FUSE_DEV_IOC_CLONE};
use std::{ffi::CStr, io, mem, os::unix::prelude::*};
use zerocopy::IntoBytes;

const FUSE_DEV_NAME: &CStr = c"/dev/fuse";

pub trait Reader {
    /// Read an incoming request from the kernel to the specified buffer.
    ///
    /// The returned value is the number of received bytes excluding the header part.
    fn read_request(&mut self, header: &mut fuse_in_header, arg: &mut [u8]) -> io::Result<usize>;
}

impl<R: ?Sized> Reader for &mut R
where
    R: Reader,
{
    #[inline]
    fn read_request(&mut self, header: &mut fuse_in_header, arg: &mut [u8]) -> io::Result<usize> {
        (**self).read_request(header, arg)
    }
}

impl Reader for &[u8] {
    fn read_request(&mut self, header: &mut fuse_in_header, arg: &mut [u8]) -> io::Result<usize> {
        let this = self;

        if this.len() < mem::size_of::<fuse_in_header>() {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        header
            .as_mut_bytes()
            .copy_from_slice(&this[..mem::size_of::<fuse_in_header>()]);
        *this = &this[mem::size_of::<fuse_in_header>()..];

        let arg_len = this.len();
        arg[..arg_len].copy_from_slice(this);

        Ok(arg_len)
    }
}

pub trait Writer {
    /// Send a reply associated with a processing request to the kernel.
    fn write_reply<B>(&mut self, unique: u64, error: i32, arg: B) -> io::Result<()>
    where
        B: Bytes;

    /// Send a notification message to the kernel.
    fn write_notify<B>(&mut self, code: fuse_notify_code, arg: B) -> io::Result<()>
    where
        B: Bytes;
}

impl<W: ?Sized> Writer for &mut W
where
    W: Writer,
{
    #[inline]
    fn write_reply<B>(&mut self, unique: u64, error: i32, arg: B) -> io::Result<()>
    where
        B: Bytes,
    {
        (**self).write_reply(unique, error, arg)
    }

    #[inline]
    fn write_notify<B>(&mut self, code: fuse_notify_code, arg: B) -> io::Result<()>
    where
        B: Bytes,
    {
        (**self).write_notify(code, arg)
    }
}

impl Writer for Vec<u8> {
    fn write_reply<B>(&mut self, unique: u64, error: i32, arg: B) -> io::Result<()>
    where
        B: Bytes,
    {
        crate::util::write_bytes(self, crate::util::Reply::new(unique, error, arg))
    }

    fn write_notify<B>(&mut self, code: fuse_notify_code, arg: B) -> io::Result<()>
    where
        B: Bytes,
    {
        crate::util::write_bytes(self, crate::util::Notify::new(code, arg))
    }
}

// ---- Connection ----

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

impl Reader for Connection {
    #[inline]
    fn read_request(&mut self, header: &mut fuse_in_header, arg: &mut [u8]) -> io::Result<usize> {
        (&*self).read_request(header, arg)
    }
}

impl Reader for &Connection {
    fn read_request(&mut self, header: &mut fuse_in_header, arg: &mut [u8]) -> io::Result<usize> {
        let len = nix::readv(
            self.fd.as_fd(),
            &mut [
                io::IoSliceMut::new(header.as_mut_bytes()),
                io::IoSliceMut::new(arg),
            ],
        )?;

        if len < mem::size_of::<fuse_in_header>() {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }

        Ok(len - mem::size_of::<fuse_in_header>())
    }
}

impl io::Write for &Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_vectored(&[io::IoSlice::new(buf)])
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        nix::writev(self.fd.as_fd(), bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Writer for Connection {
    #[inline]
    fn write_reply<B>(&mut self, unique: u64, error: i32, arg: B) -> io::Result<()>
    where
        B: crate::bytes::Bytes,
    {
        (&*self).write_reply(unique, error, arg)
    }

    #[inline]
    fn write_notify<B>(&mut self, code: polyfuse_kernel::fuse_notify_code, arg: B) -> io::Result<()>
    where
        B: crate::bytes::Bytes,
    {
        (&*self).write_notify(code, arg)
    }
}

impl Writer for &Connection {
    fn write_reply<B>(&mut self, unique: u64, error: i32, arg: B) -> io::Result<()>
    where
        B: Bytes,
    {
        crate::util::write_bytes(self, crate::util::Reply::new(unique, error, arg))
    }

    fn write_notify<B>(&mut self, code: polyfuse_kernel::fuse_notify_code, arg: B) -> io::Result<()>
    where
        B: Bytes,
    {
        crate::util::write_bytes(self, crate::util::Notify::new(code, arg))
    }
}
