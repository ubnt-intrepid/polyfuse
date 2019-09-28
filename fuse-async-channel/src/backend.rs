//! libfuse bindings.

#![allow(nonstandard_style)]

use libc::{c_char, c_int};
use std::{
    ffi::{CString, OsStr}, //
    io,
    os::unix::{
        ffi::OsStrExt,
        io::{AsRawFd, RawFd},
    },
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
    pub fn new(
        fsname: impl AsRef<OsStr>,
        mountpoint: impl AsRef<Path>,
        mountopts: impl IntoIterator<Item = impl AsRef<OsStr>>,
    ) -> io::Result<Self> {
        let fsname = fsname.as_ref();
        let mountpoint = mountpoint.as_ref();

        let c_mountpoint = CString::new(mountpoint.as_os_str().as_bytes())?;

        let mut args = vec![CString::new(fsname.as_bytes())?];
        for opt in mountopts {
            args.push(CString::new(opt.as_ref().as_bytes())?);
        }
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

        Ok(Connection {
            raw_fd,
            mountpoint: c_mountpoint,
        })
    }
}

impl AsRawFd for Connection {
    fn as_raw_fd(&self) -> RawFd {
        self.raw_fd
    }
}
