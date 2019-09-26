//! libfuse bindings.

#![allow(nonstandard_style)]

use libc::{c_char, c_int, c_void, iovec};
use mio::{unix::EventedFd, Evented, PollOpt, Ready, Token};
use std::{
    ffi::{CString, OsStr}, //
    fmt,
    io::{self, IoSlice, IoSliceMut, Read, Write},
    os::unix::{ffi::OsStrExt, io::RawFd},
    path::{Path, PathBuf},
    ptr::NonNull,
};

#[repr(C)]
struct fuse_session {
    _unused: [u8; 0],
}

extern "C" {
    fn fuse_session_destroy(se: *mut fuse_session);
    fn fuse_session_fd(se: *mut fuse_session) -> c_int;
    fn fuse_session_mount(se: *mut fuse_session, mountpoint: *const c_char) -> c_int;
    fn fuse_session_unmount(se: *mut fuse_session);
}

extern "C" {
    fn fuse_session_new_empty(argc: c_int, argv: *const *const c_char) -> *mut fuse_session;
}

pub(crate) struct Connection {
    se: NonNull<fuse_session>,
    fd: OwnedEventedFd,
    mountpoint: PathBuf,
}

impl Drop for Connection {
    fn drop(&mut self) {
        unsafe {
            fuse_session_unmount(self.se.as_ptr());
            fuse_session_destroy(self.se.as_ptr());
        }
    }
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Channel")
            .field("fd", &self.fd)
            .field("mountpoint", &self.mountpoint)
            .finish()
    }
}

impl Connection {
    pub fn new(fsname: &OsStr, mountpoint: &Path, _mountopts: &[&OsStr]) -> io::Result<Self> {
        let args = vec![CString::new(fsname.as_bytes())?];
        let c_args: Vec<*const c_char> = args.iter().map(|arg| arg.as_ptr()).collect();

        let se =
            NonNull::new(unsafe { fuse_session_new_empty(c_args.len() as c_int, c_args.as_ptr()) }) //
                .ok_or_else(|| io::Error::last_os_error())?;

        unsafe {
            let c_mountpoint = CString::new(mountpoint.as_os_str().as_bytes())?;

            let res = fuse_session_mount(se.as_ptr(), c_mountpoint.as_ptr());
            if res == -1 {
                return Err(io::Error::last_os_error());
            }
        }

        let raw_fd = unsafe {
            let fd = fuse_session_fd(se.as_ptr());

            let flags = libc::fcntl(fd, libc::F_GETFL, 0);
            if flags < 0 {
                return Err(io::Error::last_os_error());
            }

            let res = libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            if res < 0 {
                return Err(io::Error::last_os_error());
            }

            fd
        };

        Ok(Connection {
            se,
            fd: OwnedEventedFd(raw_fd),
            mountpoint: mountpoint.to_path_buf(),
        })
    }

    pub fn mountpoint(&self) -> &Path {
        &self.mountpoint
    }

    pub fn raw_fd(&self) -> &OwnedEventedFd {
        &self.fd
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OwnedEventedFd(RawFd);

impl Read for OwnedEventedFd {
    fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        let res = unsafe {
            libc::read(
                self.0, //
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
                self.0, //
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

impl Write for OwnedEventedFd {
    fn write(&mut self, src: &[u8]) -> io::Result<usize> {
        let res = unsafe {
            libc::write(
                self.0, //
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
                self.0, //
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
        let res = unsafe { libc::fsync(self.0) };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Evented for OwnedEventedFd {
    fn register(
        &self,
        poll: &mio::Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.0).register(poll, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &mio::Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.0).reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &mio::Poll) -> io::Result<()> {
        EventedFd(&self.0).deregister(poll)
    }
}
