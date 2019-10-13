//! Evented I/O that communicates with the FUSE kernel driver.

#![deny(missing_debug_implementations, clippy::unimplemented)]

use libc::{c_char, c_int, c_void, iovec};
use mio::{unix::EventedFd, Evented, PollOpt, Ready, Token};
use std::{
    ffi::{CString, OsStr}, //
    io::{self, IoSlice, IoSliceMut, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        io::{AsRawFd, RawFd},
    },
    path::Path,
};

#[allow(nonstandard_style)]
#[repr(C)]
struct fuse_args {
    argc: c_int,
    argv: *const *const c_char,
    allocated: c_int,
}

extern "C" {
    fn fuse_mount_compat25(mountpoint: *const c_char, args: *mut fuse_args) -> c_int;
    fn fuse_unmount_compat22(mountpoint: *const c_char);
    fn fuse_opt_free_args(args: *mut fuse_args);
}

/// An evented I/O object that communicates with the FUSE kernel driver.
#[derive(Debug)]
pub struct Channel {
    raw_fd: RawFd,
    mountpoint: CString,
}

impl Drop for Channel {
    fn drop(&mut self) {
        unsafe {
            fuse_unmount_compat22(self.mountpoint.as_ptr());
        }
    }
}

impl Channel {
    /// Open a new channel.
    pub fn open(
        mountpoint: impl AsRef<Path>,
        mountopts: impl IntoIterator<Item = impl AsRef<OsStr>>,
    ) -> io::Result<Self> {
        let mountpoint = mountpoint.as_ref();
        let c_mountpoint = CString::new(mountpoint.as_os_str().as_bytes())?;

        let args: Vec<CString> = mountopts
            .into_iter()
            .map(|opt| CString::new(opt.as_ref().as_bytes()))
            .collect::<Result<_, _>>()?;
        let c_args: Vec<*const c_char> = args.iter().map(|arg| arg.as_ptr()).collect();

        let mut f_args = fuse_args {
            argc: c_args.len() as c_int,
            argv: c_args.as_ptr(),
            allocated: 0,
        };

        let raw_fd = unsafe { fuse_mount_compat25(c_mountpoint.as_ptr(), &mut f_args) };
        unsafe {
            fuse_opt_free_args(&mut f_args);
        }
        if raw_fd == -1 {
            return Err(io::Error::last_os_error());
        }

        set_nonblocking(raw_fd)?;

        Ok(Channel {
            raw_fd,
            mountpoint: c_mountpoint,
        })
    }
}

impl AsRawFd for Channel {
    fn as_raw_fd(&self) -> RawFd {
        self.raw_fd
    }
}

impl Read for Channel {
    fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        let res = unsafe {
            libc::read(
                self.raw_fd, //
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
                self.raw_fd, //
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

impl Write for Channel {
    fn write(&mut self, src: &[u8]) -> io::Result<usize> {
        let res = unsafe {
            libc::write(
                self.raw_fd, //
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
                self.raw_fd, //
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
        let res = unsafe { libc::fsync(self.raw_fd) };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Evented for Channel {
    fn register(
        &self,
        poll: &mio::Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.raw_fd).register(poll, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &mio::Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.raw_fd).reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &mio::Poll) -> io::Result<()> {
        EventedFd(&self.raw_fd).deregister(poll)
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
