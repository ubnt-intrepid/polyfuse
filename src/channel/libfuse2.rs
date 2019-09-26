//! libfuse2 bindings

#![cfg(feature = "libfuse2")]

use libc::{c_char, c_int};
use std::{
    ffi::{CString, OsStr},
    io,
    os::unix::{ffi::OsStrExt, io::RawFd},
    path::Path,
};

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

#[derive(Debug)]
pub(crate) struct Connection {
    raw_fd: RawFd,
    mountpoint: CString,
}

impl Drop for Connection {
    fn drop(&mut self) {
        unsafe {
            fuse_unmount_compat22(self.mountpoint.as_ptr());
        }
    }
}

impl Connection {
    pub fn new(fsname: &OsStr, mountpoint: &Path, mountopts: &[&OsStr]) -> io::Result<Self> {
        let mountpoint = CString::new(mountpoint.as_os_str().as_bytes())?;

        let mut args = Vec::with_capacity(mountopts.len() + 1);
        args.push(CString::new(fsname.as_bytes())?);
        for opt in mountopts {
            args.push(CString::new(opt.as_bytes())?);
        }
        let c_args: Vec<*const c_char> = args.iter().map(|arg| arg.as_ptr()).collect();

        let mut fargs = fuse_args {
            argc: c_args.len() as c_int,
            argv: c_args.as_ptr(),
            allocated: 0,
        };
        let raw_fd = unsafe { fuse_mount_compat25(mountpoint.as_ptr(), &mut fargs) };
        unsafe {
            fuse_opt_free_args(&mut fargs);
        }
        if raw_fd < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Self { raw_fd, mountpoint })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.raw_fd
    }
}
