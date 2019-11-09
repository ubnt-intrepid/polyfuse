// FIXME: re-enable lint rules
#![allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use libc::{c_int, c_void, iovec};
use mio::{unix::EventedFd, Evented, PollOpt, Ready, Token};
use polyfuse_sys::kernel::FUSE_DEV_IOC_CLONE;
use std::{
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
        let (reader, writer) = UnixDatagram::pair()?;

        let pid = unsafe { libc::fork() };
        if pid == -1 {
            return Err(io::Error::last_os_error());
        }

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

        drop(writer);

        let fd = receive_fd(reader.as_raw_fd())?;
        set_nonblocking(fd)?;

        // TODO: treat auto_unmount option.
        // if !auto_unmount {
        //     drop(rd);
        //     unsafe {
        //         libc::waitpid(pid, ptr::null_mut(), 0); // bury zombie.
        //     }
        // }
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

                clonefd = libc::open(devname.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC);
                if clonefd == -1 {
                    return Err(io::Error::last_os_error());
                }

                let res = libc::ioctl(clonefd, FUSE_DEV_IOC_CLONE.into(), &self.fd);
                if res == -1 {
                    let err = io::Error::last_os_error();
                    libc::close(clonefd);
                    return Err(err);
                }
            } else {
                clonefd = libc::dup(self.fd);
                if clonefd == -1 {
                    return Err(io::Error::last_os_error());
                }
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

fn receive_fd(fd_in: RawFd) -> io::Result<RawFd> {
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

    let ret = unsafe { libc::recvmsg(fd_in, &mut msg, 0) };
    if ret == -1 {
        return Err(io::Error::last_os_error());
    }

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
        let res = unsafe {
            libc::read(
                self.fd, //
                dst.as_mut_ptr() as *mut c_void,
                dst.len(),
            )
        };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(res as usize)
    }

    fn read_vectored(&mut self, dst: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        let res = unsafe {
            libc::readv(
                self.fd, //
                dst.as_mut_ptr() as *mut iovec,
                dst.len() as c_int,
            )
        };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(res as usize)
    }
}

impl Write for Connection {
    fn write(&mut self, src: &[u8]) -> io::Result<usize> {
        let res = unsafe {
            libc::write(
                self.fd, //
                src.as_ptr() as *const c_void,
                src.len(),
            )
        };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(res as usize)
    }

    fn write_vectored(&mut self, src: &[IoSlice]) -> io::Result<usize> {
        let res = unsafe {
            libc::writev(
                self.fd, //
                src.as_ptr() as *const iovec,
                src.len() as c_int,
            )
        };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(res as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        let res = unsafe { libc::fsync(self.fd) };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
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

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }

    let res = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if res < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}
