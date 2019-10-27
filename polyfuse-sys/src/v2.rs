//! FFI bindings for libfuse2.

use libc::{c_char, c_int};

#[repr(C)]
pub struct fuse_args {
    pub argc: c_int,
    pub argv: *const *const c_char,
    pub allocated: c_int,
}

extern "C" {
    pub fn fuse_mount_compat25(mountpoint: *const c_char, args: *mut fuse_args) -> c_int;
    pub fn fuse_unmount_compat22(mountpoint: *const c_char);
    pub fn fuse_opt_free_args(args: *mut fuse_args);
}
