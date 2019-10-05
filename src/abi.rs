//! FUSE application binary interface.

#![warn(
missing_debug_implementations, //
    unsafe_code,
    clippy::unimplemented,
)]

use bitflags::bitflags;
use std::{convert::TryFrom, error, fmt};

pub mod consts {
    /// The major version number of FUSE protocol.
    pub const KERNEL_VERSION: u32 = 7;

    /// The minor version number of FUSE protocol.
    pub const KERNEL_MINOR_VERSION: u32 = 28;

    /// The minimum length of read buffer.
    pub const MIN_READ_BUFFER: u32 = 8192;

    /// Maximum of in_iovecs + out_iovecs
    pub const IOCTL_MAX_IOV: u32 = 256;

    // misc
    pub const COMPAT_ENTRY_OUT_SIZE: usize = 120;
    pub const COMPAT_ATTR_OUT_SIZE: usize = 96;
    pub const COMPAT_MKNOD_IN_SIZE: usize = 8;
    pub const COMPAT_WRITE_IN_SIZE: usize = 24;
    pub const COMPAT_STATFS_SIZE: usize = 48;
    pub const COMPAT_INIT_OUT_SIZE: usize = 8;
    pub const COMPAT_22_INIT_OUT_SIZE: usize = 24;

    pub const CUSE_INIT_INFO_MAX: u32 = 4096;
}

macro_rules! newtype {
    ($(
        $(#[$m:meta])*
        $vis:vis type $Name:ident = $T:ty ;
    )*) => {$(
        $(#[$m])*
        #[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash)]
        #[repr(transparent)]
        $vis struct $Name { raw: $T }

        impl $Name {
            #[inline]
            pub const fn from_raw(raw: $T) -> Self {
                Self { raw }
            }

            #[inline]
            pub const fn into_raw(self) -> $T {
                self.raw
            }
        }
    )*}
}

newtype! {
    /// The unique identifier of FUSE requests.
    pub type Unique = u64;

    /// The unique identifier of inode in the filesystem.
    pub type Nodeid = u64;
}

impl fmt::Display for Unique {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.raw, f)
    }
}

impl Nodeid {
    /// The node ID of the root inode.
    pub const ROOT: Self = Self::from_raw(1);
}

newtype! {
    pub type Uid = u32;
    pub type Gid = u32;
    pub type Pid = u32;
}

newtype! {
    pub type FileMode = u32;
}

/// ABI compatible with `fuse_attr`.
#[derive(Default, Debug, Clone)]
#[repr(C)]
pub struct FileAttr {
    pub ino: Nodeid,
    pub size: u64,
    pub blocks: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub atimensec: u32,
    pub mtimensec: u32,
    pub ctimensec: u32,
    pub mode: FileMode,
    pub nlink: u32,
    pub uid: Uid,
    pub gid: Gid,
    pub rdev: u32,
    pub blksize: u32,
    #[doc(hidden)]
    pub padding: u32,
}

impl From<libc::stat> for FileAttr {
    fn from(attr: libc::stat) -> Self {
        Self {
            ino: Nodeid::from_raw(attr.st_ino),
            size: attr.st_size as u64,
            blocks: attr.st_blocks as u64,
            atime: attr.st_atime as u64,
            mtime: attr.st_mtime as u64,
            ctime: attr.st_ctime as u64,
            atimensec: attr.st_atime_nsec as u32,
            mtimensec: attr.st_mtime_nsec as u32,
            ctimensec: attr.st_ctime_nsec as u32,
            mode: FileMode::from_raw(attr.st_mode),
            nlink: attr.st_nlink as u32,
            uid: Uid::from_raw(attr.st_uid),
            gid: Gid::from_raw(attr.st_gid),
            rdev: attr.st_gid,
            blksize: attr.st_blksize as u32,
            padding: 0,
        }
    }
}

impl FileAttr {
    pub fn atime(&self) -> (u64, u32) {
        (self.atime, self.atimensec)
    }

    pub fn mtime(&self) -> (u64, u32) {
        (self.mtime, self.mtimensec)
    }

    pub fn ctime(&self) -> (u64, u32) {
        (self.ctime, self.ctimensec)
    }
}

/// ABI compatible with `fuse_dirent`.
#[derive(Debug, Default)]
#[repr(C)]
pub struct DirEntry {
    pub ino: Nodeid,
    pub off: u64,
    pub namelen: u32,
    pub typ: u32,
    pub name: [u8; 0],
}

/// ABI compatible with `fuse_direntplus`.
#[derive(Debug, Default)]
#[repr(C)]
pub struct DirEntryPlus {
    pub entry_out: EntryOut,
    pub dirent: DirEntry,
}

/// ABI compatible with `fuse_kstatfs`.
#[derive(Debug, Default, Clone)]
#[repr(C)]
pub struct Statfs {
    pub blocks: u64,
    pub bfree: u64,
    pub bavail: u64,
    pub files: u64,
    pub ffree: u64,
    pub bsize: u32,
    pub namelen: u32,
    pub frsize: u32,
    #[doc(hidden)]
    pub padding: u32,
    #[doc(hidden)]
    pub spare: [u32; 6usize],
}

impl From<libc::statvfs> for Statfs {
    fn from(st: libc::statvfs) -> Self {
        Self {
            bsize: st.f_bsize as u32,
            frsize: st.f_frsize as u32,
            blocks: st.f_blocks,
            bfree: st.f_bfree,
            bavail: st.f_bavail,
            files: st.f_files,
            ffree: st.f_ffree,
            namelen: st.f_namemax as u32,
            padding: 0,
            spare: [0u32; 6],
        }
    }
}

/// ABI compatible with `fuse_file_lock`.
#[derive(Debug, Default, Clone)]
#[repr(C)]
pub struct FileLock {
    pub start: u64,
    pub end: u64,
    pub typ: u32,
    pub pid: Pid,
}

macro_rules! define_opcode {
    ($(
        $(#[$m:meta])*
        $Variant:ident = $val:expr,
    )*) => {
        #[derive(Debug, Copy, Clone, PartialEq, Hash)]
        #[repr(u32)]
        pub enum Opcode {
            $(
                $(#[$m])*
                $Variant = $val,
            )*
        }

        impl TryFrom<u32> for Opcode {
            type Error = UnknownOpcode;

            fn try_from(opcode: u32) -> Result<Self, Self::Error> {
                match opcode {
                    $(
                        $val => Ok(Self::$Variant),
                    )*
                    opcode => Err(UnknownOpcode(opcode)),
                }
            }
        }
    };
}

define_opcode! {
    Lookup = 1,
    Forget = 2,
    Getattr = 3,
    Setattr = 4,
    Readlink = 5,
    Symlink = 6,
    // _ = 7,
    Mknod = 8,
    Mkdir = 9,
    Unlink = 10,
    Rmdir = 11,
    Rename = 12,
    Link = 13,
    Open = 14,
    Read = 15,
    Write = 16,
    Statfs = 17,
    Release = 18,
    // _ = 19,
    Fsync = 20,
    Setxattr = 21,
    Getxattr = 22,
    Listxattr = 23,
    Removexattr = 24,
    Flush = 25,
    Init = 26,
    Opendir = 27,
    Readdir = 28,
    Releasedir = 29,
    Fsyncdir = 30,
    Getlk = 31,
    Setlk = 32,
    Setlkw = 33,
    Access = 34,
    Create = 35,
    Interrupt = 36,
    Bmap = 37,
    Destroy = 38,
    Ioctl = 39,
    Poll = 40,
    NotifyReply = 41,
    BatchForget = 42,
    Fallocate = 43,
    Readdirplus = 44,
    Rename2 = 45,
    Lseek = 46,
    CopyFileRange = 47,

    CuseInit = 4096,
}

#[derive(Debug)]
pub struct UnknownOpcode(u32);

impl fmt::Display for UnknownOpcode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "unknown opcode: {}", self.0)
    }
}

impl error::Error for UnknownOpcode {}

/// ABI compatible with `fuse_in_header`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct InHeader {
    pub len: u32,
    pub opcode: u32,
    pub unique: Unique,
    pub nodeid: Nodeid,
    pub uid: Uid,
    pub gid: Gid,
    pub pid: Pid,
    #[doc(hidden)]
    pub padding: u32,
}

impl InHeader {
    pub fn opcode(&self) -> Option<Opcode> {
        Opcode::try_from(self.opcode).ok()
    }
}

/// ABI compatible with `fuse_init_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct InitIn {
    pub major: u32,
    pub minor: u32,
    pub max_readahead: u32,
    pub flags: CapFlags,
}

bitflags! {
    /// Capability flags.
    #[repr(transparent)]
    pub struct CapFlags: u32 {
        const ASYNC_READ = 1;
        const POSIX_LOCKS = 1 << 1;
        const FILE_OPS = 1 << 2;
        const ATOMIC_O_TRUNC = 1 << 3;
        const EXPORT_SUPPORT = 1 << 4;
        const BIG_WRITES = 1 << 5;
        const DONT_MASK = 1 << 6;
        const SPLICE_WRITE = 1 << 7;
        const SPLICE_MOVE = 1 << 8;
        const SPLICE_READ = 1 << 9;
        const FLOCK_LOCKS = 1 << 10;
        const HAS_IOCTL_DIR = 1 << 11;
        const AUTO_INVAL_DATA = 1 << 12;
        const DO_READDIRPLUS = 1 << 13;
        const READDIRPLUS_AUTO = 1 << 14;
        const ASYNC_DIO = 1 << 15;
        const WRITEBACK_CACHE = 1 << 16;
        const NO_OPEN_SUPPORT = 1 << 17;
        const PARALLEL_DIROPS = 1 << 18;
        const HANDLE_KILLPRIV = 1 << 19;
        const POSIX_ACL = 1 << 20;
        const ABORT_ERROR = 1 << 21;
        const MAX_PAGES = 1 << 22;
        const CACHE_SYMLINKS = 1 << 23;
    }
}

/// ABI compatible with `fuse_forget_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct ForgetIn {
    pub nlookup: u64,
}

/// ABI compatible with `fuse_getattr_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct GetattrIn {
    pub getattr_flags: GetattrFlags,
    #[doc(hidden)]
    pub dummy: u32,
    pub fh: u64,
}

impl GetattrIn {
    pub fn fh(&self) -> Option<u64> {
        if self.getattr_flags.contains(GetattrFlags::FH) {
            Some(self.fh)
        } else {
            None
        }
    }
}

bitflags! {
    /// Getattr flags.
    #[repr(transparent)]
    pub struct GetattrFlags: u32 {
        const FH = 1 << 0;
    }
}

/// ABI compatible with `fuse_setattr_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct SetattrIn {
    pub valid: SetattrFlags,
    #[doc(hidden)]
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
    pub mode: FileMode,
    #[doc(hidden)]
    pub unused4: u32,
    pub uid: Uid,
    pub gid: Gid,
    #[doc(hidden)]
    pub unused5: u32,
}

impl SetattrIn {
    #[inline]
    fn get<R>(&self, flag: SetattrFlags, f: impl FnOnce(&Self) -> R) -> Option<R> {
        if self.valid.contains(flag) {
            Some(f(self))
        } else {
            None
        }
    }

    pub fn fh(&self) -> Option<u64> {
        self.get(SetattrFlags::FH, |this| this.fh)
    }

    pub fn size(&self) -> Option<u64> {
        self.get(SetattrFlags::SIZE, |this| this.size)
    }

    pub fn lock_owner(&self) -> Option<u64> {
        self.get(SetattrFlags::LOCKOWNER, |this| this.lock_owner)
    }

    pub fn atime(&self) -> Option<(u64, u32, bool)> {
        self.get(SetattrFlags::ATIME, |this| {
            (
                this.atime,
                this.atimensec,
                this.valid.contains(SetattrFlags::ATIME_NOW),
            )
        })
    }

    pub fn mtime(&self) -> Option<(u64, u32, bool)> {
        self.get(SetattrFlags::CTIME, |this| {
            (
                this.mtime,
                this.mtimensec,
                this.valid.contains(SetattrFlags::MTIME_NOW),
            )
        })
    }

    pub fn ctime(&self) -> Option<(u64, u32)> {
        self.get(SetattrFlags::CTIME, |this| (this.ctime, this.ctimensec))
    }

    /// Returns the new file if specified.
    pub fn mode(&self) -> Option<FileMode> {
        self.get(SetattrFlags::MODE, |this| this.mode)
    }

    /// Returns the new UID if specified.
    pub fn uid(&self) -> Option<Uid> {
        self.get(SetattrFlags::UID, |this| this.uid)
    }

    /// Returns the new GID if specified.
    pub fn gid(&self) -> Option<Gid> {
        self.get(SetattrFlags::GID, |this| this.gid)
    }
}

bitflags! {
    /// Setattr flags.
    #[repr(transparent)]
    pub struct SetattrFlags: u32 {
        const MODE = 1 << 0;
        const UID = 1 << 1;
        const GID = 1 << 2;
        const SIZE = 1 << 3;
        const ATIME = 1 << 4;
        const MTIME = 1 << 5;
        const FH = 1 << 6;
        const ATIME_NOW = 1 << 7;
        const MTIME_NOW = 1 << 8;
        const LOCKOWNER = 1 << 9;
        const CTIME = 1 << 10;
    }
}

/// ABI compatible with `fuse_mknod_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct MknodIn {
    pub mode: u32,
    pub rdev: u32,
    pub umask: u32,
    #[doc(hidden)]
    pub padding: u32,
}

/// ABI compatible with `fuse_mknod_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct MkdirIn {
    pub mode: u32,
    pub umask: u32,
}

/// ABI compatible with `fuse_rename_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct RenameIn {
    pub newdir: u64,
}

/// ABI compatible with `fuse_link_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct LinkIn {
    pub oldnodeid: u64,
}

/// ABI compatible with `fuse_open_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct OpenIn {
    pub flags: u32,
    #[doc(hidden)]
    pub unused: u32,
}

/// ABI compatible with `fuse_read_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct ReadIn {
    pub fh: u64,
    pub offset: u64,
    pub size: u32,
    pub read_flags: ReadFlags,
    pub lock_owner: u64,
    pub flags: u32,
    #[doc(hidden)]
    pub padding: u32,
}

impl ReadIn {
    pub fn lock_owner(&self) -> Option<u64> {
        if self.read_flags.contains(ReadFlags::LOCKOWNER) {
            Some(self.lock_owner)
        } else {
            None
        }
    }
}

bitflags! {
    /// Read flags.
    #[repr(transparent)]
    pub struct ReadFlags: u32 {
        /// The field `lock_owner` is valid.
        const LOCKOWNER = 1 << 1;
    }
}

/// ABI compatible with `fuse_write_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct WriteIn {
    pub fh: u64,
    pub offset: u64,
    pub size: u32,
    pub write_flags: WriteFlags,
    pub lock_owner: u64,
    pub flags: u32,
    #[doc(hidden)]
    pub padding: u32,
}

impl WriteIn {
    pub fn lock_owner(&self) -> Option<u64> {
        if self.write_flags.contains(WriteFlags::LOCKOWNER) {
            Some(self.lock_owner)
        } else {
            None
        }
    }
}

bitflags! {
    /// Write flags.
    #[repr(transparent)]
    pub struct WriteFlags: u32 {
        /// Delayed write from page cache, file handle is guessed.
        const CACHE = 1 << 0;

        /// The field `lock_owner` is valid.
        const LOCKOWNER = 1 << 1;
    }
}

/// ABI compatible with `fuse_flush_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct FlushIn {
    pub fh: u64,
    #[doc(hidden)]
    pub unused: u32,
    #[doc(hidden)]
    pub padding: u32,
    pub lock_owner: u64,
}

/// ABI compatible with `fuse_release_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct ReleaseIn {
    pub fh: u64,
    pub flags: u32,
    pub release_flags: ReleaseFlags,
    pub lock_owner: u64,
}

impl ReleaseIn {
    pub fn flush(&self) -> bool {
        self.release_flags.contains(ReleaseFlags::FLUSH)
    }
}

bitflags! {
    /// Release flags.
    #[repr(transparent)]
    pub struct ReleaseFlags: u32 {
        const FLUSH = 1 << 0;
        const FLOCK_UNLOCK = 1 << 1;
    }
}

/// ABI compatible with `fuse_fsync_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct FsyncIn {
    pub fh: u64,
    pub fsync_flags: FsyncFlags,
    #[doc(hidden)]
    pub padding: u32,
}

bitflags! {
    #[repr(transparent)]
    pub struct FsyncFlags: u32 {
        const DATASYNC = 1 << 0;
    }
}

/// ABI compatible with `fuse_getxattr_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct GetxattrIn {
    pub size: u32,
    #[doc(hidden)]
    pub padding: u32,
}

/// ABI compatible with `fuse_setxattr_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct SetxattrIn {
    pub size: u32,
    pub flags: u32,
}

/// ABI compatible with `fuse_lk_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct LkIn {
    pub fh: u64,
    pub owner: u64,
    pub lk: FileLock,
    pub lk_flags: LkFlags,
    #[doc(hidden)]
    pub padding: u32,
}

bitflags! {
    /// Lk flags.
    #[repr(transparent)]
    pub struct LkFlags: u32 {
        const FLOCK = 1 << 0;
    }
}

/// ABI compatible with `fuse_access_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct AccessIn {
    pub mask: u32,
    #[doc(hidden)]
    pub padding: u32,
}

/// ABI compatible with `fuse_create_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct CreateIn {
    pub flags: u32,
    pub mode: FileMode,
    pub umask: u32,
    #[doc(hidden)]
    pub padding: u32,
}

/// ABI compatible with `fuse_bmap_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct BmapIn {
    pub block: u64,
    pub blocksize: u32,
    #[doc(hidden)]
    pub padding: u32,
}

/// ABI compatible with `fuse_out_header`.
#[derive(Debug, Default)]
#[repr(C)]
pub struct OutHeader {
    pub len: u32,
    pub error: i32,
    pub unique: Unique,
}

/// ABI compatible with `fuse_attr_out`.
#[derive(Debug, Default, Clone)]
#[repr(C)]
pub struct AttrOut {
    pub attr_valid: u64,
    pub attr_valid_nsec: u32,
    #[doc(hidden)]
    pub dummy: u32,
    pub attr: FileAttr,
}

impl From<FileAttr> for AttrOut {
    fn from(attr: FileAttr) -> Self {
        let mut attr_out = Self::default();
        attr_out.attr = attr.into();
        attr_out
    }
}

impl From<libc::stat> for AttrOut {
    fn from(attr: libc::stat) -> Self {
        Self::from(FileAttr::from(attr))
    }
}

impl AttrOut {
    pub fn attr_valid(&self) -> (u64, u32) {
        (self.attr_valid, self.attr_valid_nsec)
    }

    pub fn set_attr_valid(&mut self, sec: u64, nsec: u32) {
        self.attr_valid = sec;
        self.attr_valid_nsec = nsec;
    }
}

/// ABI compatible with `fuse_entry_out`.
#[derive(Debug, Default)]
#[repr(C)]
pub struct EntryOut {
    pub nodeid: Nodeid,
    pub generation: u64,
    pub entry_valid: u64,
    pub attr_valid: u64,
    pub entry_valid_nsec: u32,
    pub attr_valid_nsec: u32,
    pub attr: FileAttr,
}

impl EntryOut {
    pub fn entry_valid(&self) -> (u64, u32) {
        (self.entry_valid, self.entry_valid_nsec)
    }

    pub fn set_entry_valid(&mut self, sec: u64, nsec: u32) {
        self.entry_valid = sec;
        self.entry_valid_nsec = nsec;
    }

    pub fn attr_valid(&self) -> (u64, u32) {
        (self.attr_valid, self.attr_valid_nsec)
    }

    pub fn set_attr_valid(&mut self, sec: u64, nsec: u32) {
        self.attr_valid = sec;
        self.attr_valid_nsec = nsec;
    }
}

/// ABI compatible with `fuse_init_out`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct InitOut {
    pub major: u32,
    pub minor: u32,
    pub max_readahead: u32,
    pub flags: CapFlags,
    pub max_background: u16,
    pub congestion_threshold: u16,
    pub max_write: u32,
    pub time_gran: u32,
    pub max_pages: u16,
    #[doc(hidden)]
    pub padding: u16,
    #[doc(hidden)]
    pub unused: [u32; 8],
}

impl Default for InitOut {
    fn default() -> Self {
        Self {
            major: crate::abi::consts::KERNEL_VERSION,
            minor: crate::abi::consts::KERNEL_MINOR_VERSION,
            max_readahead: 0,
            flags: CapFlags::empty(),
            max_background: 0,
            congestion_threshold: 0,
            max_write: 0,
            time_gran: 0,
            max_pages: 0,
            padding: 0,
            unused: [0; 8],
        }
    }
}

/// ABI compatible with `fuse_getxattr_out`.
#[derive(Debug, Default)]
#[repr(C)]
pub struct GetxattrOut {
    pub size: u32,
    #[doc(hidden)]
    pub padding: u32,
}

/// ABI compatible with `fuse_open_out`.
#[derive(Debug)]
#[repr(C)]
pub struct OpenOut {
    pub fh: u64,
    pub open_flags: OpenFlags,
    #[doc(hidden)]
    pub padding: u32,
}

impl Default for OpenOut {
    fn default() -> Self {
        Self {
            fh: 0,
            open_flags: OpenFlags::empty(),
            padding: 0,
        }
    }
}

bitflags! {
    /// Open flags.
    #[repr(transparent)]
    pub struct OpenFlags: u32 {
        const DIRECT_IO = 1 << 0;
        const KEEP_CACHE = 1 << 1;
        const NONSEEKABLE = 1 << 2;
        const CACHE_DIR = 1 << 3;
    }
}

/// ABI compatible with `fuse_write_out`.
#[derive(Debug, Default)]
#[repr(C)]
pub struct WriteOut {
    pub size: u32,
    #[doc(hidden)]
    pub padding: u32,
}

/// ABI compatible with `fuse_statfs_out`.
#[derive(Debug, Default)]
#[repr(C)]
pub struct StatfsOut {
    pub st: Statfs,
}

/// ABI compatible with `fuse_lk_out`.
#[derive(Debug, Default)]
#[repr(C)]
pub struct LkOut {
    pub lk: FileLock,
}

/// ABI compatible with `fuse_bmap_out`.
#[derive(Debug, Default)]
#[repr(C)]
pub struct BmapOut {
    pub block: u64,
}

/// ABI compatible with `fuse_ioctl_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct IoctlIn {
    pub fh: u64,
    pub flags: IoctlFlags,
    pub cmd: u32,
    pub arg: u64,
    pub in_size: u32,
    pub out_size: u32,
}

/// ABI compatible with `fuse_ioctl_out`.
#[derive(Debug, Default)]
#[repr(C)]
pub struct IoctlOut {
    pub result: i32,
    pub flags: u32,
    pub in_iovs: u32,
    pub out_iovs: u32,
}

bitflags! {
    /// Ioctl flags.
    #[repr(transparent)]
    pub struct IoctlFlags: u32 {
        const COMPAT = 1 << 0;
        const UNRESTRICTED = 1 << 1;
        const RETRY = 1 << 2;
        const USE_32BIT = 1 << 3;
        const DIR = 1 << 4;
    }
}

/// ABI compatible with `fuse_ioctl_iovec`.
#[derive(Debug, Default)]
#[repr(C)]
pub struct IoctlIovec {
    pub base: u64,
    pub len: u64,
}

/// ABI compatible with `fuse_poll_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct PollIn {
    pub fh: u64,
    pub kh: u64,
    pub flags: u32,
    pub events: u32,
}

/// ABI compatible with `fuse_poll_out`.
#[derive(Debug, Default)]
#[repr(C)]
pub struct PollOut {
    pub revents: u32,
    #[doc(hidden)]
    pub padding: u32,
}

bitflags! {
    /// Poll flags.
    #[repr(transparent)]
    pub struct PollFlags: u32 {
        const SCHEDULE_NOTIFY = 1 << 0;
    }
}

/// ABI compatible with `fuse_interrupt_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct InterruptIn {
    pub unique: Unique,
}

/// ABI compatible with `fuse_fallocate_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct FallocateIn {
    pub fh: u64,
    pub offset: u64,
    pub length: u64,
    pub mode: u32,
    #[doc(hidden)]
    pub padding: u32,
}

/// ABI compatible with `fuse_batch_forget_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct BatchForgetIn {
    pub count: u32,
    #[doc(hidden)]
    pub dummy: u32,
}

/// ABI compatible with `fuse_forget_one`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct ForgetOne {
    pub nodeid: Nodeid,
    pub nlookup: u64,
}

/// ABI compatible with `fuse_rename2_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct Rename2In {
    pub newdir: u64,
    pub flags: u32,
    #[doc(hidden)]
    pub padding: u32,
}

/// ABI compatible with `fuse_lseek_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct LseekIn {
    pub fh: u64,
    pub offset: u64,
    pub whence: u32,
    #[doc(hidden)]
    pub padding: u32,
}

/// ABI compatible with `fuse_lseek_out`.
#[derive(Debug, Default)]
#[repr(C)]
pub struct LseekOut {
    pub offset: u64,
}

/// ABI compatible with `fuse_copy_file_range_in`.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct CopyFileRangeIn {
    pub fh_in: u64,
    pub off_in: u64,
    pub nodeid_out: u64,
    pub fh_out: u64,
    pub off_out: u64,
    pub len: u64,
    pub flags: u64,
}

pub mod notify {
    use crate::abi::{Nodeid, Unique};

    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(u32)]
    pub enum NotifyCode {
        Poll = 1,
        InvalInode = 2,
        InvalEntry = 3,
        Store = 4,
        Retrieve = 5,
        Delete = 6,
    }

    impl NotifyCode {
        pub const MAX: u32 = FUSE_NOTIFY_CODE_MAX;
    }

    pub const FUSE_NOTIFY_CODE_MAX: u32 = 7;

    #[derive(Debug, Default)]
    #[repr(C)]
    pub struct PollWakeupOut {
        pub kh: u64,
    }

    #[derive(Debug, Default)]
    #[repr(C)]
    pub struct InvalInodeOut {
        pub ino: Nodeid,
        pub off: i64,
        pub len: i64,
    }

    #[derive(Debug, Default)]
    #[repr(C)]
    pub struct InvalEntryOut {
        pub parent: Nodeid,
        pub namelen: u32,
        #[doc(hidden)]
        pub padding: u32,
    }

    #[derive(Debug, Default)]
    #[repr(C)]
    pub struct DeleteOut {
        pub parent: Nodeid,
        pub child: Nodeid,
        pub namelen: u32,
        #[doc(hidden)]
        pub padding: u32,
    }

    #[derive(Debug, Default)]
    #[repr(C)]
    pub struct StoreOut {
        pub nodeid: Nodeid,
        pub offset: u64,
        pub size: u32,
        #[doc(hidden)]
        pub padding: u32,
    }

    #[derive(Debug, Default)]
    #[repr(C)]
    pub struct RetrieveOut {
        pub notify_unique: Unique,
        pub nodeid: Nodeid,
        pub offset: u64,
        pub size: u32,
        #[doc(hidden)]
        pub padding: u32,
    }

    #[derive(Debug, Clone)]
    #[repr(C)]
    pub struct RetrieveIn {
        #[doc(hidden)]
        pub dummy1: u64,
        pub offset: u64,
        pub size: u32,
        #[doc(hidden)]
        pub dummy2: u32,
        #[doc(hidden)]
        pub dummy3: u64,
        #[doc(hidden)]
        pub dummy4: u64,
    }
}

/// CUSE
pub mod cuse {
    use bitflags::bitflags;

    /// ABI compatible with `cuse_init_in`.
    #[derive(Debug, Clone)]
    #[repr(C)]
    pub struct InitIn {
        pub major: u32,
        pub minor: u32,
        #[doc(hidden)]
        pub unused: u32,
        pub flags: CuseFlags,
    }

    /// ABI compatible with `cuse_init_out`.
    #[derive(Debug, Clone)]
    #[repr(C)]
    pub struct InitOut {
        pub major: u32,
        pub minor: u32,
        #[doc(hidden)]
        pub unused: u32,
        pub flags: CuseFlags,
        pub max_read: u32,
        pub max_write: u32,
        pub dev_major: u32,
        pub dev_minor: u32,
        #[doc(hidden)]
        pub spare: [u32; 10],
    }

    bitflags! {
        /// CUSE init flags.
        pub struct CuseFlags: u32 {
            /// use unrestricted ioctl.
            const UNRESTRICTED_IOCTL = 1 << 0;
        }
    }
}

macro_rules! impl_as_ref_for_abi {
    ($($t:ty,)*) => {$(
        impl AsRef<[u8]> for $t {
        #[allow(unsafe_code)]
            fn as_ref(&self) -> &[u8] {
                unsafe {
                    std::slice::from_raw_parts(
                        self as *const Self as *const u8,
                        std::mem::size_of::<Self>(),
                    )
                }
            }
        }
    )*}
}

impl_as_ref_for_abi! {
    OutHeader,
    InitOut,
    OpenOut,
    AttrOut,
    EntryOut,
    GetxattrOut,
    WriteOut,
    StatfsOut,
    LkOut,
    BmapOut,
}
