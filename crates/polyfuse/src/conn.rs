use crate::{
    bytes::{write_bytes, Bytes, POD},
    nix,
};
use polyfuse_kernel::{fuse_in_header, fuse_out_header, FUSE_DEV_IOC_CLONE};
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
        write_bytes((POD(header), arg), |bufs| nix::writev(self.as_fd(), bufs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use polyfuse_kernel::{fuse_access_in, fuse_init_in, fuse_opcode};
    use std::io::Read;
    use zerocopy::FromBytes;

    // FIXME: replace to std::io::pipe (since 1.87)
    fn pipe() -> io::Result<(AnonPipe, AnonPipe)> {
        let mut fds = [0; 2];
        syscall! { pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
        Ok((AnonPipe::new(fds[0]), AnonPipe::new(fds[1])))
    }
    struct AnonPipe(OwnedFd);
    impl AnonPipe {
        fn new(fd: RawFd) -> Self {
            Self(unsafe { OwnedFd::from_raw_fd(fd) })
        }
    }
    impl io::Read for AnonPipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.read_vectored(&mut [io::IoSliceMut::new(buf)])
        }
        fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
            nix::readv(self.0.as_fd(), bufs)
        }
    }

    #[test]
    fn test_read_request() {
        let (AnonPipe(r), AnonPipe(w)) = pipe().expect("pipe2");
        let mut conn = Connection::from(r);

        write_bytes(
            (
                POD(fuse_in_header {
                    len: (mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_access_in>())
                        as u32,
                    opcode: fuse_opcode::FUSE_ACCESS as u32,
                    unique: 2,
                    nodeid: 1,
                    uid: 4,
                    gid: 5,
                    pid: 9,
                    padding: 0,
                }),
                POD(fuse_access_in {
                    mask: 42u32,
                    padding: 0,
                }),
            ),
            |bufs| nix::writev(w.as_fd(), bufs),
        )
        .expect("write");

        let mut header = fuse_in_header::default();
        let mut arg = vec![0; mem::size_of::<fuse_access_in>()];
        let len = conn
            .read_request(&mut header, &mut arg[..])
            .expect("read_request");
        assert_eq!(len, mem::size_of::<fuse_access_in>());
        let arg = fuse_access_in::ref_from_bytes(&arg[..]).expect("arg");

        assert_eq!(
            header.len,
            (mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_access_in>()) as u32
        );
        assert_eq!(header.opcode, fuse_opcode::FUSE_ACCESS as u32);
        assert_eq!(header.unique, 2);
        assert_eq!(header.nodeid, 1);
        assert_eq!(header.uid, 4);
        assert_eq!(header.gid, 5);
        assert_eq!(header.pid, 9);

        assert_eq!(arg.mask, 42);
    }

    #[test]
    fn test_read_request_short_payload() {
        let (AnonPipe(r), AnonPipe(w)) = pipe().expect("pipe2");
        let mut conn = Connection::from(r);

        write_bytes(b"a" as &[u8], |bufs| nix::writev(w.as_fd(), bufs)).expect("write");

        let mut header = fuse_in_header::default();
        let mut arg = vec![0; mem::size_of::<fuse_init_in>()];
        assert!(conn.read_request(&mut header, &mut arg[..]).is_err());
    }

    #[test]
    fn test_write_reply() {
        let (mut r, AnonPipe(w)) = pipe().expect("pipe2");
        let mut conn = Connection::from(w);

        conn.write_reply(
            fuse_out_header {
                error: -libc::ENOSYS,
                unique: 4,
                ..Default::default()
            },
            (),
        )
        .expect("write_reply");

        let mut out_header = fuse_out_header::default();
        let mut arg = vec![0u8; 64];
        let len = r
            .read_vectored(&mut [
                io::IoSliceMut::new(out_header.as_mut_bytes()),
                io::IoSliceMut::new(&mut arg[..]),
            ])
            .expect("pipe_read");
        assert_eq!(len, mem::size_of::<fuse_out_header>());
        assert_eq!(out_header.len, mem::size_of::<fuse_out_header>() as u32);
        assert_eq!(out_header.error, -libc::ENOSYS);
        assert_eq!(out_header.unique, 4);
    }
}
