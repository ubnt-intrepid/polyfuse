use crate::{
    bytes::{Bytes, FillBytes, POD},
    nix,
};
use polyfuse_kernel::{fuse_in_header, fuse_out_header, FUSE_DEV_IOC_CLONE};
use std::{
    ffi::CStr,
    io::{self, IoSlice},
    mem::{self, MaybeUninit},
    os::unix::prelude::*,
};
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

pub trait Writer {
    /// Send a reply or a notification message to the kernel.
    fn write_reply<B>(&mut self, header: fuse_out_header, arg: B) -> io::Result<()>
    where
        B: Bytes;
}

impl<W: ?Sized> Writer for &mut W
where
    W: Writer,
{
    #[inline]
    fn write_reply<B>(&mut self, header: fuse_out_header, arg: B) -> io::Result<()>
    where
        B: Bytes,
    {
        (**self).write_reply(header, arg)
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

    #[inline]
    fn write_bytes<T>(&self, bytes: T) -> io::Result<()>
    where
        T: Bytes,
    {
        let size = bytes.size();
        let count = bytes.count();

        let written;

        macro_rules! small_write {
            ($n:expr) => {{
                let mut vec: [MaybeUninit<IoSlice<'_>>; $n] =
                    unsafe { MaybeUninit::uninit().assume_init() };
                bytes.fill_bytes(&mut FillWriteBytes {
                    vec: &mut vec[..],
                    offset: 0,
                });
                let vec = unsafe { slice_assume_init_ref(&vec[..]) };

                written = nix::writev(self.as_fd(), vec)?;
            }};
        }

        match count {
            // Skip writing.
            0 => return Ok(()),

            // Avoid heap allocation if count is small.
            1 => small_write!(1),
            2 => small_write!(2),
            3 => small_write!(3),
            4 => small_write!(4),

            count => {
                let mut vec: Vec<IoSlice<'_>> = Vec::with_capacity(count);
                unsafe {
                    let dst = std::slice::from_raw_parts_mut(
                        vec.as_mut_ptr().cast(), //
                        count,
                    );
                    bytes.fill_bytes(&mut FillWriteBytes {
                        vec: dst,
                        offset: 0,
                    });
                    vec.set_len(count);
                }

                written = nix::writev(self.as_fd(), &*vec)?;
            }
        }

        if written < size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "written data is too short",
            ));
        }

        Ok(())
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

impl Writer for Connection {
    #[inline]
    fn write_reply<B>(&mut self, header: fuse_out_header, arg: B) -> io::Result<()>
    where
        B: crate::bytes::Bytes,
    {
        (&*self).write_reply(header, arg)
    }
}

impl Writer for &Connection {
    fn write_reply<B>(&mut self, mut header: fuse_out_header, arg: B) -> io::Result<()>
    where
        B: Bytes,
    {
        header.len = (mem::size_of::<fuse_out_header>() + arg.size())
            .try_into()
            .map_err(|_e| io::Error::from_raw_os_error(libc::EINVAL))?;
        self.write_bytes((POD(header), arg))
    }
}

struct FillWriteBytes<'a, 'vec> {
    vec: &'vec mut [MaybeUninit<IoSlice<'a>>],
    offset: usize,
}

impl<'a, 'vec> FillBytes<'a> for FillWriteBytes<'a, 'vec> {
    fn put(&mut self, chunk: &'a [u8]) {
        self.vec[self.offset] = MaybeUninit::new(IoSlice::new(chunk));
        self.offset += 1;
    }
}

// FIXME: replace with stabilized MaybeUninit::slice_assume_init_ref.
#[inline(always)]
unsafe fn slice_assume_init_ref<T>(slice: &[MaybeUninit<T>]) -> &[T] {
    #[allow(unused_unsafe)]
    unsafe {
        &*(slice as *const [MaybeUninit<T>] as *const [T])
    }
}
