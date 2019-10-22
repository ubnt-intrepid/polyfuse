//! libfuse bindings.

#![allow(nonstandard_style)]

pub mod v2 {
    use libc::{c_char, c_int};

    #[repr(C)]
    pub struct fuse_args {
        argc: c_int,
        argv: *const *const c_char,
        allocated: c_int,
    }

    impl fuse_args {
        pub const fn new(argc: c_int, argv: *const *const c_char) -> Self {
            Self {
                argc,
                argv,
                allocated: 0,
            }
        }
    }

    extern "C" {
        pub fn fuse_mount_compat25(mountpoint: *const c_char, args: *mut fuse_args) -> c_int;
        pub fn fuse_unmount_compat22(mountpoint: *const c_char);
        pub fn fuse_opt_free_args(args: *mut fuse_args);
    }
}
