//! FUSE application binary interface for `polyfuse`.
//!
//! The binding is compatible with ABI 7.44 (Linux 6.16.11).

#![allow(nonstandard_style, clippy::identity_op)]

use libc::{c_char, c_ulong};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, TryFromBytes};

/// The major version number of FUSE protocol.
pub const FUSE_KERNEL_VERSION: u32 = 7;

/// The minor version number of FUSE protocol.
pub const FUSE_KERNEL_MINOR_VERSION: u32 = 44;

/// The minimum length of read buffer.
pub const FUSE_MIN_READ_BUFFER: u32 = 8192;

/// Maximum of in_iovecs + out_iovecs
pub const FUSE_IOCTL_MAX_IOV: u32 = 256;

// Bitmasks for fuse_setattr_in.valid
pub const FATTR_MODE: u32 = 1 << 0;
pub const FATTR_UID: u32 = 1 << 1;
pub const FATTR_GID: u32 = 1 << 2;
pub const FATTR_SIZE: u32 = 1 << 3;
pub const FATTR_ATIME: u32 = 1 << 4;
pub const FATTR_MTIME: u32 = 1 << 5;
pub const FATTR_FH: u32 = 1 << 6;
pub const FATTR_ATIME_NOW: u32 = 1 << 7;
pub const FATTR_MTIME_NOW: u32 = 1 << 8;
pub const FATTR_LOCKOWNER: u32 = 1 << 9;
pub const FATTR_CTIME: u32 = 1 << 10;
pub const FATTR_KILL_SUIDGID: u32 = 1 << 11;

// Bitmasks for fuse_attr.flags
pub const FUSE_ATTR_SUBMOUNT: u32 = 1 << 0;

// Flags returned by the OPEN request.
pub const FOPEN_DIRECT_IO: u32 = 1 << 0;
pub const FOPEN_KEEP_CACHE: u32 = 1 << 1;
pub const FOPEN_NONSEEKABLE: u32 = 1 << 2;
pub const FOPEN_CACHE_DIR: u32 = 1 << 3;
pub const FOPEN_STREAM: u32 = 1 << 4;
pub const FOPEN_NOFLUSH: u32 = 1 << 5;
pub const FOPEN_PARALLEL_DIRECT_WRITES: u32 = 1 << 6;
pub const FOPEN_PASSTHROUGH: u32 = 1 << 7;

// INIT request/reply flags.
pub const FUSE_ASYNC_READ: u32 = 1;
pub const FUSE_POSIX_LOCKS: u32 = 1 << 1;
pub const FUSE_FILE_OPS: u32 = 1 << 2;
pub const FUSE_ATOMIC_O_TRUNC: u32 = 1 << 3;
pub const FUSE_EXPORT_SUPPORT: u32 = 1 << 4;
pub const FUSE_BIG_WRITES: u32 = 1 << 5;
pub const FUSE_DONT_MASK: u32 = 1 << 6;
pub const FUSE_SPLICE_WRITE: u32 = 1 << 7;
pub const FUSE_SPLICE_MOVE: u32 = 1 << 8;
pub const FUSE_SPLICE_READ: u32 = 1 << 9;
pub const FUSE_FLOCK_LOCKS: u32 = 1 << 10;
pub const FUSE_HAS_IOCTL_DIR: u32 = 1 << 11;
pub const FUSE_AUTO_INVAL_DATA: u32 = 1 << 12;
pub const FUSE_DO_READDIRPLUS: u32 = 1 << 13;
pub const FUSE_READDIRPLUS_AUTO: u32 = 1 << 14;
pub const FUSE_ASYNC_DIO: u32 = 1 << 15;
pub const FUSE_WRITEBACK_CACHE: u32 = 1 << 16;
pub const FUSE_NO_OPEN_SUPPORT: u32 = 1 << 17;
pub const FUSE_PARALLEL_DIROPS: u32 = 1 << 18;
pub const FUSE_HANDLE_KILLPRIV: u32 = 1 << 19;
pub const FUSE_POSIX_ACL: u32 = 1 << 20;
pub const FUSE_ABORT_ERROR: u32 = 1 << 21;
pub const FUSE_MAX_PAGES: u32 = 1 << 22;
pub const FUSE_CACHE_SYMLINKS: u32 = 1 << 23;
pub const FUSE_NO_OPENDIR_SUPPORT: u32 = 1 << 24;
pub const FUSE_EXPLICIT_INVAL_DATA: u32 = 1 << 25;
pub const FUSE_MAP_ALIGNMENT: u32 = 1 << 26;
pub const FUSE_SUBMOUNTS: u32 = 1 << 27;
pub const FUSE_HANDLE_KILLPRIV_V2: u32 = 1 << 28;
pub const FUSE_SETXATTR_EXT: u32 = 1 << 29;
pub const FUSE_INIT_EXT: u32 = 1 << 30;
pub const FUSE_INIT_RESERVED: u32 = 1 << 31;
// flags2.
pub const FUSE_SECURITY_CTX: u64 = 1 << 32;
pub const FUSE_HAS_INODE_DAX: u64 = 1 << 33;
pub const FUSE_CREATE_SUPP_GROUP: u64 = 1 << 34;
pub const FUSE_HAS_EXPIRE_ONLY: u64 = 1 << 35;
pub const FUSE_DIRECT_IO_ALLOW_MMAP: u64 = 1 << 36;
pub const FUSE_PASSTHROUGH: u64 = 1 << 37;
pub const FUSE_NO_EXPORT_SUPPORT: u64 = 1 << 38;
pub const FUSE_HAS_RESEND: u64 = 1 << 39;
pub const FUSE_ALLOW_IDMAP: u64 = 1 << 40;
pub const FUSE_OVER_IO_URING: u64 = 1 << 41;
pub const FUSE_REQUEST_TIMEOUT: u64 = 1 << 42;

// CUSE INIT request/reply flags.
pub const CUSE_UNRESTRICTED_IOCTL: u32 = 1 << 0;

// Release flags.
pub const FUSE_RELEASE_FLUSH: u32 = 1 << 0;
pub const FUSE_RELEASE_FLOCK_UNLOCK: u32 = 1 << 1;

// Getattr flags.
pub const FUSE_GETATTR_FH: u32 = 1;

// Lock flags.
pub const FUSE_LK_FLOCK: u32 = 1 << 0;

// WRITE flags.
pub const FUSE_WRITE_CACHE: u32 = 1 << 0;
pub const FUSE_WRITE_LOCKOWNER: u32 = 1 << 1;
pub const FUSE_WRITE_KILL_SUIDGID: u32 = 1 << 2;

#[deprecated(since = "0.3.0", note = "use `FUSE_WRITE_KILL_SUIDGID` instead")]
pub const FUSE_WRITE_KILL_PRIV: u32 = FUSE_WRITE_KILL_SUIDGID;

// Read flags.
pub const FUSE_READ_LOCKOWNER: u32 = 1 << 1;

// Ioctl flags.
pub const FUSE_IOCTL_COMPAT: u32 = 1 << 0;
pub const FUSE_IOCTL_UNRESTRICTED: u32 = 1 << 1;
pub const FUSE_IOCTL_RETRY: u32 = 1 << 2;
pub const FUSE_IOCTL_32BIT: u32 = 1 << 3;
pub const FUSE_IOCTL_DIR: u32 = 1 << 4;
pub const FUSE_IOCTL_COMPAT_X32: u32 = 1 << 5;

// Poll flags.
pub const FUSE_POLL_SCHEDULE_NOTIFY: u32 = 1 << 0;

// Fsync flags.
pub const FUSE_FSYNC_FDATASYNC: u32 = 1 << 0;

// Open flags.
pub const FUSE_OPEN_KILL_SUIDGID: u32 = 1 << 0;

// notify_inval_entry flags.
pub const FUSE_EXPIRE_ONLY: u32 = 1 << 0;

// misc
pub const CUSE_INIT_INFO_MAX: u32 = 4096;

// FUSE_SETUPMAPPING flags.
pub const FUSE_SETUPMAPPING_FLAG_WRITE: u64 = 1 << 0;
pub const FUSE_SETUPMAPPING_FLAG_READ: u64 = 1 << 1;

// Setxattr flags.
pub const FUSE_SETXATTR_ACL_KILL_SGID: u32 = 1 << 0;

// Device ioctls
pub const FUSE_DEV_IOC_MAGIC: u32 = 229;
pub const FUSE_DEV_IOC_CLONE: c_ulong = libc::_IOR::<u32>(FUSE_DEV_IOC_MAGIC, 0);
pub const FUSE_DEV_IOC_BACKING_OPEN: c_ulong =
    libc::_IOW::<fuse_backing_map>(FUSE_DEV_IOC_MAGIC, 1);
pub const FUSE_DEV_IOC_BACKING_CLOSE: c_ulong = libc::_IOW::<u32>(FUSE_DEV_IOC_MAGIC, 2);

// FUSE over io_uring.
pub const FUSE_URING_IN_OUT_HEADER_SZ: usize = 128;
pub const FUSE_URING_OP_IN_OUT_SZ: usize = 128;

// ~ ABI 7.8
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_attr_compat_8 {
    pub ino: u64,
    pub size: u64,
    pub blocks: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub atimensec: u32,
    pub mtimensec: u32,
    pub ctimensec: u32,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u32,
}

// ABI 7.9 ~
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_attr {
    pub ino: u64,
    pub size: u64,
    pub blocks: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub atimensec: u32,
    pub mtimensec: u32,
    pub ctimensec: u32,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u32,
    pub blksize: u32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_dirent {
    pub ino: u64,
    pub off: u64,
    pub namelen: u32,
    pub typ: u32,
    pub name: [u8; 0],
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_direntplus {
    pub entry_out: fuse_entry_out,
    pub dirent: fuse_dirent,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_sx_time {
    pub tv_sec: i64,
    pub tv_nsec: u32,
    pub __reserved: i32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_statx {
    pub mask: u32,
    pub blksize: u32,
    pub attributes: u64,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub mode: u16,
    pub __spare0: [u16; 1],
    pub ino: u64,
    pub size: u64,
    pub blocks: u64,
    pub attributes_mask: u64,
    pub atime: fuse_sx_time,
    pub btime: fuse_sx_time,
    pub ctime: fuse_sx_time,
    pub mtime: fuse_sx_time,
    pub rdev_major: u32,
    pub rdev_minor: u32,
    pub dev_major: u32,
    pub dev_minor: u32,
    pub __spare2: [u64; 14],
}

// ~ ABI 7.3
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_kstatfs_compat_3 {
    pub blocks: u64,
    pub bfree: u64,
    pub bavail: u64,
    pub files: u64,
    pub ffree: u64,
    pub bsize: u32,
    pub namelen: u32,
}

// ABI 7.4 ~
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_kstatfs {
    pub blocks: u64,
    pub bfree: u64,
    pub bavail: u64,
    pub files: u64,
    pub ffree: u64,
    pub bsize: u32,
    pub namelen: u32,
    pub frsize: u32,
    pub padding: u32,
    pub spare: [u32; 6usize],
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_file_lock {
    pub start: u64,
    pub end: u64,
    pub typ: u32,
    pub pid: u32,
}

macro_rules! define_opcode {
    ($(
        $(#[$m:meta])*
        $VARIANT:ident = $val:expr,
    )*) => {
        $(
            #[doc(hidden)]
            pub const $VARIANT: u32 = $val;
        )*

        #[derive(Clone, Copy, Debug, Hash, PartialEq, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
        #[repr(u32)]
        pub enum fuse_opcode {
            $(
                $(#[$m])*
                $VARIANT = $val,
            )*
        }
    };
}

define_opcode! {
    FUSE_LOOKUP = 1,
    FUSE_FORGET = 2,
    FUSE_GETATTR = 3,
    FUSE_SETATTR = 4,
    FUSE_READLINK = 5,
    FUSE_SYMLINK = 6,
    // _ = 7,
    FUSE_MKNOD = 8,
    FUSE_MKDIR = 9,
    FUSE_UNLINK = 10,
    FUSE_RMDIR = 11,
    FUSE_RENAME = 12,
    FUSE_LINK = 13,
    FUSE_OPEN = 14,
    FUSE_READ = 15,
    FUSE_WRITE = 16,
    FUSE_STATFS = 17,
    FUSE_RELEASE = 18,
    // _ = 19,
    FUSE_FSYNC = 20,
    FUSE_SETXATTR = 21,
    FUSE_GETXATTR = 22,
    FUSE_LISTXATTR = 23,
    FUSE_REMOVEXATTR = 24,
    FUSE_FLUSH = 25,
    FUSE_INIT = 26,
    FUSE_OPENDIR = 27,
    FUSE_READDIR = 28,
    FUSE_RELEASEDIR = 29,
    FUSE_FSYNCDIR = 30,
    FUSE_GETLK = 31,
    FUSE_SETLK = 32,
    FUSE_SETLKW = 33,
    FUSE_ACCESS = 34,
    FUSE_CREATE = 35,
    FUSE_INTERRUPT = 36,
    FUSE_BMAP = 37,
    FUSE_DESTROY = 38,
    FUSE_IOCTL = 39,
    FUSE_POLL = 40,
    FUSE_NOTIFY_REPLY = 41,
    FUSE_BATCH_FORGET = 42,
    FUSE_FALLOCATE = 43,
    FUSE_READDIRPLUS = 44,
    FUSE_RENAME2 = 45,
    FUSE_LSEEK = 46,
    FUSE_COPY_FILE_RANGE = 47,
    FUSE_SETUPMAPPING = 48,
    FUSE_REMOVEMAPPING = 49,
    FUSE_SYNCFS = 50,
    FUSE_TMPFILE = 51,
    FUSE_STATX = 52,

    CUSE_INIT = 4096,
}

pub const CUSE_INIT_BSWAP_RESERVED: u32 = 1048576; // CUSE_INIT << 8;
pub const FUSE_INIT_BSWAP_RESERVED: u32 = 436207616; // FUSE_INIT << 24;

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_in_header {
    pub len: u32,
    pub opcode: u32,
    pub unique: u64,
    pub nodeid: u64,
    pub uid: u32,
    pub gid: u32,
    pub pid: u32,
    pub total_extlen: u16,
    pub padding: u16,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_init_in_compat_35 {
    pub major: u32,
    pub minor: u32,
    pub max_readahead: u32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_init_in {
    pub major: u32,
    pub minor: u32,
    pub max_readahead: u32,
    pub flags: u32,
    pub flags2: u32,
    pub unused: [u32; 11],
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_forget_in {
    pub nlookup: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_getattr_in {
    pub getattr_flags: u32,
    pub dummy: u32,
    pub fh: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_setattr_in {
    pub valid: u32,
    pub padding: u32,
    pub fh: u64,
    pub size: u64,
    pub lock_owner: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub atimensec: u32,
    pub mtimensec: u32,
    pub ctimensec: u32,
    pub mode: u32,
    pub unused4: u32,
    pub uid: u32,
    pub gid: u32,
    pub unused5: u32,
}

// ~ ABI 7.11
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_mknod_in_compat_11 {
    pub mode: u32,
    pub rdev: u32,
}
pub const FUSE_COMPAT_MKNOD_IN_SIZE: usize = 8;

// ABI 7.12 ~
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_mknod_in {
    pub mode: u32,
    pub rdev: u32,
    pub umask: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_mkdir_in {
    pub mode: u32,
    pub umask: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_rename_in {
    pub newdir: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_link_in {
    pub oldnodeid: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_open_in {
    pub flags: u32,
    pub open_flags: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_read_in {
    pub fh: u64,
    pub offset: u64,
    pub size: u32,
    pub read_flags: u32,
    pub lock_owner: u64,
    pub flags: u32,
    pub padding: u32,
}

// ~ ABI 7.8
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_write_in_compat_8 {
    pub fh: u64,
    pub offset: u64,
    pub size: u32,
    pub write_flags: u32,
}
pub const FUSE_COMPAT_WRITE_IN_SIZE: usize = 24;

// ABI 7.9~
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_write_in {
    pub fh: u64,
    pub offset: u64,
    pub size: u32,
    pub write_flags: u32,
    pub lock_owner: u64,
    pub flags: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_flush_in {
    pub fh: u64,
    pub unused: u32,
    pub padding: u32,
    pub lock_owner: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_release_in {
    pub fh: u64,
    pub flags: u32,
    pub release_flags: u32,
    pub lock_owner: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_fsync_in {
    pub fh: u64,
    pub fsync_flags: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_getxattr_in {
    pub size: u32,
    pub padding: u32,
}

// ~ ABI 7.32
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_setxattr_in_compat_32 {
    pub size: u32,
    pub flags: u32,
}

pub const FUSE_COMPAT_SETXATTR_IN_SIZE: usize = 8;

// ABI 7.33 ~
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_setxattr_in {
    pub size: u32,
    pub flags: u32,
    pub setxattr_flags: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_lk_in {
    pub fh: u64,
    pub owner: u64,
    pub lk: fuse_file_lock,
    pub lk_flags: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_access_in {
    pub mask: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_create_in {
    pub flags: u32,
    pub mode: u32,
    pub umask: u32,
    pub open_flags: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_bmap_in {
    pub block: u64,
    pub blocksize: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_out_header {
    pub len: u32,
    pub error: i32,
    pub unique: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_attr_out_compat_8 {
    pub attr_valid: u64,
    pub attr_valid_nsec: u32,
    pub dummy: u32,
    pub attr: fuse_attr_compat_8,
}
pub const FUSE_COMPAT_ATTR_OUT_SIZE: usize = 96;

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_attr_out {
    pub attr_valid: u64,
    pub attr_valid_nsec: u32,
    pub dummy: u32,
    pub attr: fuse_attr,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_entry_out_compat_8 {
    pub nodeid: u64,
    pub generation: u64,
    pub entry_valid: u64,
    pub attr_valid: u64,
    pub entry_valid_nsec: u32,
    pub attr_valid_nsec: u32,
    pub attr: fuse_attr_compat_8,
}
pub const FUSE_COMPAT_ENTRY_OUT_SIZE: usize = 120;

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_entry_out {
    pub nodeid: u64,
    pub generation: u64,
    pub entry_valid: u64,
    pub attr_valid: u64,
    pub entry_valid_nsec: u32,
    pub attr_valid_nsec: u32,
    pub attr: fuse_attr,
}

// ~ ABI 7.3
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_init_out_compat_3 {
    pub major: u32,
    pub minor: u32,
}
pub const FUSE_COMPAT_INIT_OUT_SIZE: usize = 8;

// ~ ABI 7.5
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_init_out_compat_22 {
    pub major: u32,
    pub minor: u32,
    pub max_readahead: u32,
    pub flags: u32,
    pub max_background: u16,
    pub congestion_threshold: u16,
    pub max_write: u32,
}
pub const FUSE_COMPAT_22_INIT_OUT_SIZE: usize = 24;

// ABI 7.23 ~
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_init_out {
    pub major: u32,
    pub minor: u32,
    pub max_readahead: u32,
    pub flags: u32,
    pub max_background: u16,
    pub congestion_threshold: u16,
    pub max_write: u32,
    pub time_gran: u32,
    pub max_pages: u16,
    pub map_alignment: u16,
    pub flags2: u32,
    pub max_stack_depth: u32,
    pub request_timeout: u16,
    pub unused: [u16; 11],
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_getxattr_out {
    pub size: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_open_out {
    pub fh: u64,
    pub open_flags: u32,
    pub backing_id: i32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_write_out {
    pub size: u32,
    pub padding: u32,
}

// ~ ABI 7.3
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_statfs_out_compat_3 {
    pub st: fuse_kstatfs_compat_3,
}
pub const FUSE_COMPAT_STATFS_SIZE: usize = 48;

// ABI 7.4 ~
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_statfs_out {
    pub st: fuse_kstatfs,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_lk_out {
    pub lk: fuse_file_lock,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_bmap_out {
    pub block: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_ioctl_in {
    pub fh: u64,
    pub flags: u32,
    pub cmd: u32,
    pub arg: u64,
    pub in_size: u32,
    pub out_size: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_ioctl_out {
    pub result: i32,
    pub flags: u32,
    pub in_iovs: u32,
    pub out_iovs: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_ioctl_iovec {
    pub base: u64,
    pub len: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_poll_in {
    pub fh: u64,
    pub kh: u64,
    pub flags: u32,
    pub events: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_poll_out {
    pub revents: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_interrupt_in {
    pub unique: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_fallocate_in {
    pub fh: u64,
    pub offset: u64,
    pub length: u64,
    pub mode: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_batch_forget_in {
    pub count: u32,
    pub dummy: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_forget_one {
    pub nodeid: u64,
    pub nlookup: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_rename2_in {
    pub newdir: u64,
    pub flags: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_lseek_in {
    pub fh: u64,
    pub offset: u64,
    pub whence: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_lseek_out {
    pub offset: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_copy_file_range_in {
    pub fh_in: u64,
    pub off_in: u64,
    pub nodeid_out: u64,
    pub fh_out: u64,
    pub off_out: u64,
    pub len: u64,
    pub flags: u64,
}

macro_rules! define_notify_code {
    ($(
        $(#[$m:meta])*
        $VARIANT:ident = $val:expr,
    )*) => {
        $(
            #[doc(hidden)]
            pub const $VARIANT: u32 = $val;
        )*

        #[derive(Clone, Copy, Debug, PartialEq, Hash, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
        #[repr(u32)]
        pub enum fuse_notify_code {
            $(
                $(#[$m])*
                $VARIANT = $val,
            )*
        }
    };
}

define_notify_code! {
    FUSE_NOTIFY_POLL = 1,
    FUSE_NOTIFY_INVAL_INODE = 2,
    FUSE_NOTIFY_INVAL_ENTRY = 3,
    FUSE_NOTIFY_STORE = 4,
    FUSE_NOTIFY_RETRIEVE = 5,
    FUSE_NOTIFY_DELETE = 6,
    FUSE_NOTIFY_RESEND = 7,
    FUSE_NOTIFY_INC_EPOCH = 8,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_notify_poll_wakeup_out {
    pub kh: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_notify_inval_inode_out {
    pub ino: u64,
    pub off: i64,
    pub len: i64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_notify_inval_entry_out {
    pub parent: u64,
    pub namelen: u32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_notify_delete_out {
    pub parent: u64,
    pub child: u64,
    pub namelen: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_notify_store_out {
    pub nodeid: u64,
    pub offset: u64,
    pub size: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_notify_retrieve_out {
    pub notify_unique: u64,
    pub nodeid: u64,
    pub offset: u64,
    pub size: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_notify_retrieve_in {
    pub dummy1: u64,
    pub offset: u64,
    pub size: u32,
    pub dummy2: u32,
    pub dummy3: u64,
    pub dummy4: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_backing_map {
    pub fd: i32,
    pub flags: u32,
    pub padding: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct cuse_init_in {
    pub major: u32,
    pub minor: u32,
    pub unused: u32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct cuse_init_out {
    pub major: u32,
    pub minor: u32,
    pub unused: u32,
    pub flags: u32,
    pub max_read: u32,
    pub max_write: u32,
    pub dev_major: u32,
    pub dev_minor: u32,
    pub spare: [u32; 10],
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_setupmapping_in {
    pub fh: u64,
    pub foffset: u64,
    pub len: u64,
    pub flags: u64,
    pub moffset: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_removemapping_in {
    pub count: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_removemapping_one {
    pub moffset: u64,
    pub len: u64,
}

// #define FUSE_REMOVEMAPPING_MAX_ENTRY   \
// 		(PAGE_SIZE / sizeof(struct fuse_removemapping_one))

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_syncfs_in {
    pub padding: u64,
}

macro_rules! define_ext_type {
    ($(
        $(#[$m:meta])*
        $VARIANT:ident = $val:expr,
    )*) => {
        $(
            #[doc(hidden)]
            pub const $VARIANT: u32 = $val;
        )*

        #[derive(Clone, Copy, Debug, PartialEq, Hash, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
        #[repr(u32)]
        pub enum fuse_ext_type {
            $(
                $(#[$m])*
                $VARIANT = $val,
            )*
        }
    };
}

define_ext_type! {
    // 0 ~ 31: reserved for fuse_secctx_header
    FUSE_MAX_NR_SECCTX = 31,
    FUSE_EXT_GROUPS = 32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_secctx {
    pub size: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_secctx_header {
    pub size: u32,
    pub nr_secctx: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_ext_header {
    pub size: u32,
    pub typ: u32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_supp_groups {
    pub nr_groups: u32,
    pub groups: [u32; 0],
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_uring_ent_in_out {
    pub flags: u64,
    pub commit_id: u64,
    pub payload_sz: u32,
    pub padding: u32,
    pub reserved: u64,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_uring_req_header {
    pub in_out: [c_char; FUSE_URING_IN_OUT_HEADER_SZ],
    pub op_in: [c_char; FUSE_URING_OP_IN_OUT_SZ],
    pub ring_ent_in_out: fuse_uring_ent_in_out,
}

macro_rules! define_uring_cmd {
    ($(
        $(#[$m:meta])*
        $VARIANT:ident = $val:expr,
    )*) => {
        $(
            #[doc(hidden)]
            pub const $VARIANT: u32 = $val;
        )*

        #[derive(Clone, Copy, Debug, PartialEq, Hash, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
        #[repr(u32)]
        pub enum fuse_uring_cmd {
            $(
                $(#[$m])*
                $VARIANT = $val,
            )*
        }
    };
}

define_uring_cmd! {
    FUSE_IO_URING_CMD_INVALID = 0,
    FUSE_IO_URING_CMD_REGISTER = 1,
    FUSE_IO_URING_CMD_COMMIT_AND_FETCH = 2,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct fuse_uring_cmd_req {
    pub flags: u64,
    pub commit_id: u64,
    pub qid: u16,
    pub padding: [u8; 6],
}
