#![allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use libc::{c_int, c_void, iovec, ssize_t};
use mio::{unix::EventedFd, Evented, PollOpt, Ready, Token};
use polyfuse_sys::kernel::FUSE_DEV_IOC_CLONE;
use std::{
    cmp,
    ffi::{CStr, OsStr, OsString},
    io::{self, IoSlice, IoSliceMut, Read, Write},
    mem::{self, MaybeUninit},
    os::unix::{
        io::{AsRawFd, IntoRawFd, RawFd},
        net::UnixDatagram,
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Command,
    ptr,
};

const FUSERMOUNT_PROG: &str = "fusermount";
const FUSE_COMMFD_ENV: &str = "_FUSE_COMMFD";

#[derive(Debug, Default)]
pub struct MountOptions {
    opts: Option<OsString>,
}

impl MountOptions {
    pub fn options(&self) -> Option<&OsStr> {
        self.opts.as_ref().map(|opts| &**opts)
    }

    pub fn set_options(&mut self, opts: impl AsRef<OsStr>) {
        self.opts.replace(opts.as_ref().into());
    }
}

/// A connection with the FUSE kernel driver.
#[derive(Debug)]
pub struct Connection {
    fd: RawFd,
    mountpoint: Option<PathBuf>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _e = self.unmount();
    }
}

impl Connection {
    /// Establish a new connection with the FUSE kernel driver.
    pub fn open(mountpoint: &Path, mountopts: &MountOptions) -> io::Result<Self> {
        let (_pid, mut reader) = exec_fusermount(mountpoint, mountopts)?;

        let fd = receive_fd(&mut reader)?;
        set_nonblocking(fd)?;

        // Unmounting is executed when `reader` is dropped and the connection
        // with `fusermount` is closed.
        let _ = reader.into_raw_fd();

        Ok(Self {
            fd,
            mountpoint: Some(mountpoint.into()),
        })
    }

    /// Attempt to get a clone of this connection.
    pub fn try_clone(&self, ioc_clone: bool) -> io::Result<Self> {
        let clonefd;
        unsafe {
            if ioc_clone {
                let devname = CStr::from_bytes_with_nul_unchecked(b"/dev/fuse\0");

                clonefd = cvt(libc::open(devname.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC))?;

                if let Err(err) = cvt(libc::ioctl(clonefd, FUSE_DEV_IOC_CLONE.into(), &self.fd)) {
                    libc::close(clonefd);
                    return Err(err);
                }
            } else {
                clonefd = cvt(libc::dup(self.fd))?;
            }
        }

        Ok(Self {
            fd: clonefd,
            mountpoint: None,
        })
    }

    pub fn unmount(&mut self) -> io::Result<()> {
        if let Some(mountpoint) = self.mountpoint.take() {
            Command::new(FUSERMOUNT_PROG)
                .args(&["-u", "-q", "-z", "--"])
                .arg(&mountpoint)
                .status()?;
        }
        Ok(())
    }
}

fn exec_fusermount(
    mountpoint: &Path,
    mountopts: &MountOptions,
) -> io::Result<(c_int, UnixDatagram)> {
    let (reader, writer) = UnixDatagram::pair()?;

    let pid = cvt(unsafe { libc::fork() })?;
    if pid == 0 {
        drop(reader);
        let writer = writer.into_raw_fd();
        unsafe { libc::fcntl(writer, libc::F_SETFD, 0) };

        let mut fusermount = Command::new(FUSERMOUNT_PROG);
        fusermount.env(FUSE_COMMFD_ENV, writer.to_string());
        if let Some(opts) = mountopts.options() {
            fusermount.arg("-o").arg(opts);
        }
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

    cvt(unsafe { libc::recvmsg(reader.as_raw_fd(), &mut msg, 0) })?;

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

impl AsRawFd for Connection {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl Read for Connection {
    fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        let res = cvt(unsafe {
            libc::read(
                self.fd, //
                dst.as_mut_ptr() as *mut c_void,
                dst.len(),
            )
        })?;
        Ok(res as usize)
    }

    fn read_vectored(&mut self, dst: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        let res = cvt(unsafe {
            libc::readv(
                self.fd, //
                dst.as_mut_ptr() as *mut iovec,
                cmp::min(dst.len(), c_int::max_value() as usize) as c_int,
            )
        })?;
        Ok(res as usize)
    }
}

impl Write for Connection {
    fn write(&mut self, src: &[u8]) -> io::Result<usize> {
        let res = cvt(unsafe {
            libc::write(
                self.fd, //
                src.as_ptr() as *const c_void,
                src.len(),
            )
        })?;
        Ok(res as usize)
    }

    fn write_vectored(&mut self, src: &[IoSlice]) -> io::Result<usize> {
        let res = cvt(unsafe {
            libc::writev(
                self.fd, //
                src.as_ptr() as *const iovec,
                cmp::min(src.len(), c_int::max_value() as usize) as c_int,
            )
        })?;
        Ok(res as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        cvt(unsafe { libc::fsync(self.fd) })?;
        Ok(())
    }
}

impl Evented for Connection {
    fn register(
        &self,
        poll: &mio::Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.fd).register(poll, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &mio::Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.fd).reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &mio::Poll) -> io::Result<()> {
        EventedFd(&self.fd).deregister(poll)
    }
}

// copied from libstd/sys/unix/mod.rs
fn cvt<T: IsMinusOne>(t: T) -> io::Result<T> {
    if t.is_minus_one() {
        Err(io::Error::last_os_error())
    } else {
        Ok(t)
    }
}

trait IsMinusOne {
    fn is_minus_one(&self) -> bool;
}

impl IsMinusOne for c_int {
    fn is_minus_one(&self) -> bool {
        *self == -1
    }
}

impl IsMinusOne for ssize_t {
    fn is_minus_one(&self) -> bool {
        *self == -1
    }
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = cvt(unsafe { libc::fcntl(fd, libc::F_GETFL, 0) })?;
    cvt(unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) })?;
    Ok(())
}
