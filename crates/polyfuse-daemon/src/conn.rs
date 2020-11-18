use libc::{c_int, c_void, iovec};
use std::{
    cmp,
    ffi::OsStr,
    io,
    mem::{self, MaybeUninit},
    os::unix::{net::UnixDatagram, prelude::*},
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    ptr,
    sync::{Arc, Weak},
};

const FUSERMOUNT_PROG: &str = "fusermount";
const FUSE_COMMFD_ENV: &str = "_FUSE_COMMFD";

macro_rules! syscall {
    ($fn:ident ( $($arg:expr),* $(,)* ) ) => {{
        let res = unsafe { libc::$fn($($arg),*) };
        if res == -1 {
            return Err(io::Error::last_os_error());
        }
        res
    }};
}

#[derive(Debug)]
struct Inner {
    fd: RawFd,
    mountpoint: PathBuf,
}

impl Inner {
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

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = Command::new(FUSERMOUNT_PROG)
            .args(&["-u", "-q", "-z", "--"])
            .arg(&self.mountpoint)
            .status();

        unsafe {
            libc::close(self.fd);
        }
    }
}

/// A connection with the FUSE kernel driver.
#[derive(Debug)]
pub struct Connection {
    inner: Arc<Inner>,
}

impl Connection {
    /// Establish a connection with the FUSE kernel driver.
    pub fn open(mountpoint: &Path, mountopts: &[&OsStr]) -> io::Result<Self> {
        let fd = open_connection(mountpoint, mountopts)?;

        Ok(Self {
            inner: Arc::new(Inner {
                fd,
                mountpoint: mountpoint.into(),
            }),
        })
    }

    pub fn set_nonblocking(&self) -> io::Result<()> {
        let flags = syscall! { fcntl(self.as_raw_fd(), libc::F_GETFL, 0) };
        syscall! { fcntl(self.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
        Ok(())
    }

    pub fn writer(&self) -> Writer {
        Writer {
            inner: Arc::downgrade(&self.inner),
        }
    }
}

impl AsRawFd for Connection {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.fd
    }
}

impl io::Read for Connection {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        (&*self).read_vectored(bufs)
    }
}

impl io::Read for &Connection {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        self.inner.read_vectored(bufs)
    }
}

impl io::Write for Connection {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.inner.write_vectored(bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Write for &Connection {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.inner.write_vectored(bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct Writer {
    inner: Weak<Inner>,
}

impl io::Write for Writer {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(inner) = self.inner.upgrade() {
            inner.write(buf)
        } else {
            Ok(0)
        }
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        if let Some(inner) = self.inner.upgrade() {
            inner.write_vectored(bufs)
        } else {
            Ok(0)
        }
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Write for &Writer {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(inner) = self.inner.upgrade() {
            inner.write(buf)
        } else {
            Ok(0)
        }
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        if let Some(inner) = self.inner.upgrade() {
            inner.write_vectored(bufs)
        } else {
            Ok(0)
        }
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ==== mount ====

fn open_connection(mountpoint: &Path, mountopts: &[&OsStr]) -> io::Result<RawFd> {
    let (pid, mut reader) = exec_fusermount(mountpoint, mountopts)?;

    // Check if the `fusermount` command is started successfully.
    let mut status = 0;
    syscall! { waitpid(pid, &mut status, 0) };
    let status = ExitStatus::from_raw(status);
    if !status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("fusermount failed with: {}", status),
        ));
    }

    let fd = receive_fd(&mut reader)?;

    // Unmounting is executed when `reader` is dropped and the connection
    // with `fusermount` is closed.
    let _ = reader.into_raw_fd();

    Ok(fd)
}

fn exec_fusermount(mountpoint: &Path, mountopts: &[&OsStr]) -> io::Result<(c_int, UnixDatagram)> {
    let (reader, writer) = UnixDatagram::pair()?;

    let pid = syscall! { fork() };
    if pid == 0 {
        drop(reader);
        let writer = writer.into_raw_fd();
        unsafe { libc::fcntl(writer, libc::F_SETFD, 0) };

        let mut fusermount = Command::new(FUSERMOUNT_PROG);
        fusermount.env(FUSE_COMMFD_ENV, writer.to_string());
        fusermount.args(mountopts);
        fusermount.arg("--").arg(mountpoint);

        return Err(fusermount.exec());
    }

    Ok((pid, reader))
}

fn receive_fd(reader: &mut UnixDatagram) -> io::Result<RawFd> {
    let mut buf = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr() as *mut c_void,
        iov_len: 1,
    };

    #[repr(C)]
    struct Cmsg {
        header: libc::cmsghdr,
        fd: c_int,
    }
    let mut cmsg = MaybeUninit::<Cmsg>::uninit();

    let mut msg = libc::msghdr {
        msg_name: ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: cmsg.as_mut_ptr() as *mut c_void,
        msg_controllen: mem::size_of_val(&cmsg),
        msg_flags: 0,
    };

    syscall! { recvmsg(reader.as_raw_fd(), &mut msg, 0) };

    if msg.msg_controllen < mem::size_of_val(&cmsg) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "too short control message length",
        ));
    }
    let cmsg = unsafe { cmsg.assume_init() };

    if cmsg.header.cmsg_type != libc::SCM_RIGHTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "got control message with unknown type",
        ));
    }

    Ok(cmsg.fd)
}
