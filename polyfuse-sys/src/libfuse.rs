//! FFI bindings for libfuse.

use libc::{c_char, c_int};

pub enum fuse_session {}

extern "C" {
    pub fn fuse_session_fd(se: *mut fuse_session) -> c_int;
    pub fn fuse_session_mount(se: *mut fuse_session, mountpoint: *const c_char) -> c_int;
    pub fn fuse_session_unmount(se: *mut fuse_session);
    pub fn fuse_session_destroy(se: *mut fuse_session);
}

extern "C" {
    pub fn fuse_session_new_empty(argc: c_int, argv: *const *const c_char) -> *mut fuse_session;
}
