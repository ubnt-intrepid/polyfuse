#![allow(bad_style, deprecated, clippy::all)]

use libc::*;
use polyfuse_kernel::*;
use std::mem::size_of;

include!(concat!(env!("OUT_DIR"), "/kernel.rs"));

// TODO: use static_assert
const _: [(); size_of::<fuse_statfs_out_compat_3>()] = [(); FUSE_COMPAT_STATFS_SIZE];
const _: [(); size_of::<fuse_init_out_compat_3>()] = [(); FUSE_COMPAT_INIT_OUT_SIZE];
const _: [(); size_of::<fuse_attr_out_compat_8>()] = [(); FUSE_COMPAT_ATTR_OUT_SIZE];
const _: [(); size_of::<fuse_entry_out_compat_8>()] = [(); FUSE_COMPAT_ENTRY_OUT_SIZE];
const _: [(); size_of::<fuse_write_in_compat_8>()] = [(); FUSE_COMPAT_WRITE_IN_SIZE];
const _: [(); size_of::<fuse_mknod_in_compat_11>()] = [(); FUSE_COMPAT_MKNOD_IN_SIZE];
const _: [(); size_of::<fuse_init_out_compat_22>()] = [(); FUSE_COMPAT_22_INIT_OUT_SIZE];
const _: [(); size_of::<fuse_setxattr_in_compat_32>()] = [(); FUSE_COMPAT_SETXATTR_IN_SIZE];
