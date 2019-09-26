//! libfuse3 bindings.

#![allow(nonstandard_style)]

use super::OwnedEventedFd;
use libc::{c_char, c_int};
use std::{
    ffi::{CString, OsStr}, //
    fmt,
    io,
    os::unix::ffi::OsStrExt,
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
    pub fn new(fsname: &OsStr, mountpoint: &Path, mountopts: &[&OsStr]) -> io::Result<Self> {
        let mut args = Vec::with_capacity(mountopts.len() + 1);
        args.push(CString::new(fsname.as_bytes())?);
        for opt in mountopts {
            args.push(CString::new(opt.as_bytes())?);
        }
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
