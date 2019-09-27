//! libfuse3 bindings.

#![allow(nonstandard_style)]

use libc::{c_char, c_int};
use std::{
    ffi::{CString, OsStr}, //
    fmt,
    io,
    os::unix::{
        ffi::OsStrExt,
        io::{AsRawFd, RawFd},
    },
    path::Path,
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
    raw_fd: RawFd,
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
            .field("raw_fd", &self.raw_fd)
            .finish()
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

        let mut args = vec![CString::new(fsname.as_bytes())?];
        for opt in mountopts {
            args.push(CString::new(opt.as_ref().as_bytes())?);
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

        let raw_fd = unsafe { fuse_session_fd(se.as_ptr()) };

        Ok(Connection { se, raw_fd })
    }
}

impl AsRawFd for Connection {
    fn as_raw_fd(&self) -> RawFd {
        self.raw_fd
    }
}
