//! FUSE application binary interface.

#![allow(missing_debug_implementations)]

#[allow(dead_code, nonstandard_style)]
mod bindings {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

pub use crate::bindings::fuse_opcode as Opcode;

use crate::bindings::{
    fuse_access_in, //
    fuse_attr,
    fuse_attr_out,
    fuse_bmap_in,
    fuse_bmap_out,
    fuse_create_in,
    fuse_entry_out,
    fuse_file_lock,
    fuse_flush_in,
    fuse_forget_in,
    fuse_fsync_in,
    fuse_getattr_in,
    fuse_getxattr_in,
    fuse_getxattr_out,
    fuse_in_header,
    fuse_init_in,
    fuse_init_out,
    fuse_kstatfs,
    fuse_link_in,
    fuse_lk_in,
    fuse_lk_out,
    fuse_mkdir_in,
    fuse_mknod_in,
    fuse_open_in,
    fuse_open_out,
    fuse_out_header,
    fuse_read_in,
    fuse_release_in,
    fuse_rename_in,
    fuse_setattr_in,
    fuse_setxattr_in,
    fuse_statfs_out,
    fuse_write_in,
    fuse_write_out,
};
use bitflags::bitflags;
use std::{fmt, mem};

bitflags! {
    pub struct CapFlags: u32 {
        const ASYNC_READ = crate::bindings::FUSE_ASYNC_READ;
        const POSIX_LOCKS = crate::bindings::FUSE_POSIX_LOCKS;
        const FILE_OPS = crate::bindings::FUSE_FILE_OPS;
        const ATOMIC_O_TRUNC = crate::bindings::FUSE_ATOMIC_O_TRUNC;
        const EXPORT_SUPPORT = crate::bindings::FUSE_EXPORT_SUPPORT;
        const BIG_WRITES = crate::bindings::FUSE_BIG_WRITES;
        const DONT_MASK = crate::bindings::FUSE_DONT_MASK;
        const SPLICE_WRITE = crate::bindings::FUSE_SPLICE_WRITE;
        const SPLICE_MOVE = crate::bindings::FUSE_SPLICE_MOVE;
        const SPLICE_READ = crate::bindings::FUSE_SPLICE_READ;
        const FLOCK_LOCKS = crate::bindings::FUSE_FLOCK_LOCKS;
        const HAS_IOCTL_DIR = crate::bindings::FUSE_HAS_IOCTL_DIR;
        const AUTO_INVAL_DATA = crate::bindings::FUSE_AUTO_INVAL_DATA;
        const DO_READDIRPLUS = crate::bindings::FUSE_DO_READDIRPLUS;
        const READDIRPLUS_AUTO = crate::bindings::FUSE_READDIRPLUS_AUTO;
        const ASYNC_DIO = crate::bindings::FUSE_ASYNC_DIO;
        const WRITEBACK_CACHE = crate::bindings::FUSE_WRITEBACK_CACHE;
        const NO_OPEN_SUPPORT = crate::bindings::FUSE_NO_OPEN_SUPPORT;
        const PARALLEL_DIROPS = crate::bindings::FUSE_PARALLEL_DIROPS;
        const HANDLE_KILLPRIV = crate::bindings::FUSE_HANDLE_KILLPRIV;
        const POSIX_ACL = crate::bindings:: FUSE_POSIX_ACL;
        const ABORT_ERROR = crate::bindings::FUSE_ABORT_ERROR;

        // 7.28
        //const MAX_PAGES = crate::bindings::FUSE_MAX_PAGES;
        //const CACHE_SYMLINKS = crate::bindings::FUSE_CACHE_SYMLINKS;

        // 7.29
        //const NO_OPENDIR_SUPPORT = crate::bindings::FUSE_NO_OPENDIR_SUPPORT;
    }
}

#[repr(transparent)]
pub struct FileAttr(pub(crate) fuse_attr);

impl Default for FileAttr {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl fmt::Debug for FileAttr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("FileAttr").finish()
    }
}

impl From<libc::stat> for FileAttr {
    fn from(attr: libc::stat) -> Self {
        Self(fuse_attr {
            ino: attr.st_ino,
            mode: attr.st_mode,
            nlink: attr.st_nlink as u32,
            uid: attr.st_uid,
            gid: attr.st_gid,
            rdev: attr.st_gid,
            size: attr.st_size as u64,
            blksize: attr.st_blksize as u32,
            blocks: attr.st_blocks as u64,
            atime: attr.st_atime as u64,
            mtime: attr.st_mtime as u64,
            ctime: attr.st_ctime as u64,
            atimensec: attr.st_atime_nsec as u32,
            mtimensec: attr.st_mtime_nsec as u32,
            ctimensec: attr.st_ctime_nsec as u32,
            padding: 0,
        })
    }
}

#[repr(transparent)]
pub struct FileLock(pub(crate) fuse_file_lock);

impl fmt::Debug for FileLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileLock")
            .field("start", &self.start())
            .field("end", &self.end())
            .field("type", &self.type_())
            .field("pid", &self.pid())
            .finish()
    }
}

impl Clone for FileLock {
    fn clone(&self) -> Self {
        Self(fuse_file_lock {
            start: self.0.start,
            end: self.0.end,
            type_: self.0.type_,
            pid: self.0.pid,
        })
    }
}

impl FileLock {
    pub fn start(&self) -> u64 {
        self.0.start
    }

    pub fn set_start(&mut self, start: u64) {
        self.0.start = start;
    }

    pub fn end(&self) -> u64 {
        self.0.end
    }

    pub fn set_end(&mut self, end: u64) {
        self.0.end = end;
    }

    pub fn type_(&self) -> u32 {
        self.0.type_
    }

    pub fn set_type(&mut self, type_: u32) {
        self.0.type_ = type_;
    }

    pub fn pid(&self) -> u32 {
        self.0.pid
    }

    pub fn set_pid(&mut self, pid: u32) {
        self.0.pid = pid;
    }
}

#[repr(transparent)]
pub struct Statfs(pub(crate) fuse_kstatfs);

impl fmt::Debug for Statfs {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Statfs").finish()
    }
}

impl From<libc::statvfs> for Statfs {
    fn from(st: libc::statvfs) -> Self {
        Self(fuse_kstatfs {
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
        })
    }
}

#[repr(transparent)]
pub struct InHeader(fuse_in_header);

impl fmt::Debug for InHeader {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("InHeader")
            .field("len", &self.len())
            .field("opcode", &self.opcode())
            .field("unique", &self.unique())
            .field("nodeid", &self.nodeid())
            .field("uid", &self.uid())
            .field("gid", &self.gid())
            .field("pid", &self.pid())
            .finish()
    }
}

impl InHeader {
    pub fn len(&self) -> u32 {
        self.0.len
    }

    pub fn opcode(&self) -> Opcode {
        unsafe { mem::transmute(self.0.opcode) }
    }

    pub fn unique(&self) -> u64 {
        self.0.unique
    }

    pub fn nodeid(&self) -> u64 {
        self.0.nodeid
    }

    pub fn uid(&self) -> u32 {
        self.0.uid
    }

    pub fn gid(&self) -> u32 {
        self.0.gid
    }

    pub fn pid(&self) -> u32 {
        self.0.pid
    }
}

#[repr(transparent)]
pub struct OpInit(fuse_init_in);

impl fmt::Debug for OpInit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpInit")
            .field("major", &self.major())
            .field("minor", &self.minor())
            .field("max_readahead", &self.max_readahead())
            .field("flags", &self.flags())
            .finish()
    }
}

impl OpInit {
    pub fn major(&self) -> u32 {
        self.0.major
    }

    pub fn minor(&self) -> u32 {
        self.0.minor
    }

    pub fn max_readahead(&self) -> u32 {
        self.0.max_readahead
    }

    pub fn flags(&self) -> CapFlags {
        CapFlags::from_bits_truncate(self.0.flags)
    }
}

#[repr(transparent)]
pub struct OpForget(fuse_forget_in);

impl fmt::Debug for OpForget {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpForget")
            .field("nlookup", &self.nlookup())
            .finish()
    }
}

impl OpForget {
    pub fn nlookup(&self) -> u64 {
        self.0.nlookup
    }
}

#[repr(transparent)]
pub struct OpGetattr(fuse_getattr_in);

impl fmt::Debug for OpGetattr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpGetattr")
            .field("flags", &self.flags())
            .field("fh", &self.fh())
            .finish()
    }
}

impl OpGetattr {
    pub fn flags(&self) -> GetattrFlags {
        GetattrFlags::from_bits_truncate(self.0.getattr_flags)
    }

    pub fn fh(&self) -> u64 {
        self.0.fh
    }
}

bitflags! {
    pub struct GetattrFlags: u32 {
        const FH = crate::bindings::FUSE_GETATTR_FH;
    }
}

#[repr(transparent)]
pub struct OpSetattr(fuse_setattr_in);

impl fmt::Debug for OpSetattr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpSetattr")
            .field("valid", &self.valid())
            .field("fh", &self.fh())
            .field("size", &self.size())
            .field("lock_owner", &self.lock_owner())
            .field("atime", &self.atime())
            .field("mtime", &self.mtime())
            .field("ctime", &self.ctime())
            .field("mode", &self.mode())
            .field("uid", &self.uid())
            .field("gid", &self.gid())
            .finish()
    }
}

impl OpSetattr {
    pub fn valid(&self) -> SetattrFlags {
        SetattrFlags::from_bits_truncate(self.0.valid)
    }

    pub fn fh(&self) -> u64 {
        self.0.fh
    }

    pub fn size(&self) -> u64 {
        self.0.size
    }

    pub fn lock_owner(&self) -> u64 {
        self.0.lock_owner
    }

    pub fn atime(&self) -> (u64, u32) {
        (self.0.atime, self.0.atimensec)
    }

    pub fn mtime(&self) -> (u64, u32) {
        (self.0.mtime, self.0.mtimensec)
    }

    pub fn ctime(&self) -> (u64, u32) {
        (self.0.ctime, self.0.ctimensec)
    }

    pub fn mode(&self) -> u32 {
        self.0.mode
    }

    pub fn uid(&self) -> u32 {
        self.0.uid
    }

    pub fn gid(&self) -> u32 {
        self.0.gid
    }
}

bitflags! {
    pub struct SetattrFlags: u32 {
        const MODE = crate::bindings::FATTR_MODE;
        const UID = crate::bindings::FATTR_UID;
        const GID = crate::bindings::FATTR_GID;
        const SIZE = crate::bindings::FATTR_SIZE;
        const ATIME = crate::bindings::FATTR_ATIME;
        const MTIME = crate::bindings::FATTR_MTIME;
        const FH = crate::bindings::FATTR_FH;
        const ATIME_NOW = crate::bindings::FATTR_ATIME_NOW;
        const MTIME_NOW = crate::bindings::FATTR_MTIME_NOW;
        const LOCKOWNER = crate::bindings::FATTR_LOCKOWNER;
        const CTIME = crate::bindings::FATTR_CTIME;
    }
}

#[repr(transparent)]
pub struct OpMknod(fuse_mknod_in);

impl fmt::Debug for OpMknod {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpMknod")
            .field("mode", &self.mode())
            .field("rdev", &self.rdev())
            .field("umask", &self.umask())
            .finish()
    }
}

impl OpMknod {
    pub fn mode(&self) -> u32 {
        self.0.mode
    }

    pub fn rdev(&self) -> u32 {
        self.0.mode
    }

    pub fn umask(&self) -> u32 {
        self.0.mode
    }
}

#[repr(transparent)]
pub struct OpMkdir(fuse_mkdir_in);

impl fmt::Debug for OpMkdir {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpMkdir")
            .field("mode", &self.mode())
            .field("umask", &self.umask())
            .finish()
    }
}

impl OpMkdir {
    pub fn mode(&self) -> u32 {
        self.0.mode
    }

    pub fn umask(&self) -> u32 {
        self.0.mode
    }
}

#[repr(transparent)]
pub struct OpRename(fuse_rename_in);

impl fmt::Debug for OpRename {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpRename")
            .field("newdir", &self.newdir())
            .finish()
    }
}

impl OpRename {
    pub fn newdir(&self) -> u64 {
        self.0.newdir
    }
}

#[repr(transparent)]
pub struct OpLink(fuse_link_in);

impl fmt::Debug for OpLink {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpLink")
            .field("oldnodeid", &self.oldnodeid())
            .finish()
    }
}

impl OpLink {
    pub fn oldnodeid(&self) -> u64 {
        self.0.oldnodeid
    }
}

#[repr(transparent)]
pub struct OpOpen(fuse_open_in);

impl fmt::Debug for OpOpen {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpOpen")
            .field("flags", &self.flags())
            .finish()
    }
}

impl OpOpen {
    pub fn flags(&self) -> u32 {
        self.0.flags
    }
}

#[repr(transparent)]
pub struct OpRead(fuse_read_in);

impl fmt::Debug for OpRead {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpRead")
            .field("fh", &self.fh())
            .field("offset", &self.offset())
            .field("size", &self.size())
            .field("read_flags", &self.read_flags())
            .field("lock_owner", &self.lock_owner())
            .field("flags", &self.flags())
            .finish()
    }
}

impl OpRead {
    pub fn fh(&self) -> u64 {
        self.0.fh
    }

    pub fn offset(&self) -> u64 {
        self.0.offset
    }

    pub fn size(&self) -> u32 {
        self.0.size
    }

    pub fn read_flags(&self) -> ReadFlags {
        ReadFlags::from_bits_truncate(self.0.read_flags)
    }

    pub fn lock_owner(&self) -> u64 {
        self.0.lock_owner
    }

    pub fn flags(&self) -> u32 {
        self.0.flags
    }
}

bitflags! {
    pub struct ReadFlags: u32 {
        const LOCKOWNER = crate::bindings::FUSE_READ_LOCKOWNER;
    }
}

#[repr(transparent)]
pub struct OpWrite(fuse_write_in);

impl fmt::Debug for OpWrite {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpWrite")
            .field("fh", &self.fh())
            .field("offset", &self.offset())
            .field("size", &self.size())
            .field("write_flags", &self.write_flags())
            .field("lock_owner", &self.lock_owner())
            .field("flags", &self.flags())
            .finish()
    }
}

impl OpWrite {
    pub fn fh(&self) -> u64 {
        self.0.fh
    }

    pub fn offset(&self) -> u64 {
        self.0.offset
    }

    pub fn size(&self) -> u32 {
        self.0.size
    }

    pub fn write_flags(&self) -> WriteFlags {
        WriteFlags::from_bits_truncate(self.0.write_flags)
    }

    pub fn lock_owner(&self) -> u64 {
        self.0.lock_owner
    }

    pub fn flags(&self) -> u32 {
        self.0.flags
    }
}

bitflags! {
    pub struct WriteFlags: u32 {
        const CACHE = crate::bindings::FUSE_WRITE_CACHE;
        const LOCKOWNER = crate::bindings::FUSE_WRITE_LOCKOWNER;
    }
}

#[repr(transparent)]
pub struct OpFlush(fuse_flush_in);

impl fmt::Debug for OpFlush {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpFlush")
            .field("fh", &self.fh())
            .field("lock_owner", &self.lock_owner())
            .finish()
    }
}

impl OpFlush {
    pub fn fh(&self) -> u64 {
        self.0.fh
    }

    pub fn lock_owner(&self) -> u64 {
        self.0.lock_owner
    }
}

#[repr(transparent)]
pub struct OpRelease(fuse_release_in);

impl fmt::Debug for OpRelease {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpRelease")
            .field("fh", &self.fh())
            .field("flags", &self.flags())
            .field("release_flags", &self.release_flags())
            .field("lock_owner", &self.lock_owner())
            .finish()
    }
}

impl OpRelease {
    pub fn fh(&self) -> u64 {
        self.0.fh
    }

    pub fn flags(&self) -> u32 {
        self.0.flags
    }

    pub fn release_flags(&self) -> ReleaseFlags {
        ReleaseFlags::from_bits_truncate(self.0.release_flags)
    }

    pub fn lock_owner(&self) -> u64 {
        self.0.lock_owner
    }
}

bitflags! {
    pub struct ReleaseFlags: u32 {
        const FLUSH = crate::bindings::FUSE_RELEASE_FLUSH;
        const FLOCK_UNLOCK = crate::bindings::FUSE_RELEASE_FLOCK_UNLOCK;
    }
}

#[repr(transparent)]
pub struct OpFsync(fuse_fsync_in);

impl fmt::Debug for OpFsync {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpFsync")
            .field("fh", &self.fh())
            .field("fsync_flags", &self.fsync_flags())
            .finish()
    }
}

impl OpFsync {
    pub fn fh(&self) -> u64 {
        self.0.fh
    }

    pub fn fsync_flags(&self) -> u32 {
        self.0.fsync_flags
    }
}

#[repr(transparent)]
pub struct OpGetxattr(fuse_getxattr_in);

impl fmt::Debug for OpGetxattr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpGetxattr")
            .field("size", &self.size())
            .finish()
    }
}

impl OpGetxattr {
    pub fn size(&self) -> u32 {
        self.0.size
    }
}

#[repr(transparent)]
pub struct OpSetxattr(fuse_setxattr_in);

impl fmt::Debug for OpSetxattr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpSetxattr")
            .field("size", &self.size())
            .field("flags", &self.flags())
            .finish()
    }
}

impl OpSetxattr {
    pub fn size(&self) -> u32 {
        self.0.size
    }

    pub fn flags(&self) -> u32 {
        self.0.flags
    }
}

#[repr(transparent)]
pub struct OpLk(fuse_lk_in);

impl fmt::Debug for OpLk {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpLk")
            .field("fh", &self.fh())
            .field("owner", &self.owner())
            .field("lk", &self.lk())
            .field("fh", &self.fh())
            .finish()
    }
}

impl OpLk {
    pub fn fh(&self) -> u64 {
        self.0.fh
    }

    pub fn owner(&self) -> u64 {
        self.0.owner
    }

    pub fn lk(&self) -> &FileLock {
        unsafe { mem::transmute(&self.0.lk) }
    }

    pub fn lk_flags(&self) -> LkFlags {
        LkFlags::from_bits_truncate(self.0.lk_flags)
    }
}

bitflags! {
    pub struct LkFlags: u32 {
        const FLOCK = crate::bindings::FUSE_LK_FLOCK;
    }
}

#[repr(transparent)]
pub struct OpAccess(fuse_access_in);

impl fmt::Debug for OpAccess {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpAccess")
            .field("mask", &self.mask())
            .finish()
    }
}

impl OpAccess {
    pub fn mask(&self) -> u32 {
        self.0.mask
    }
}

#[repr(transparent)]
pub struct OpCreate(fuse_create_in);

impl fmt::Debug for OpCreate {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpCreate").finish()
    }
}

#[repr(transparent)]
pub struct OpBmap(fuse_bmap_in);

impl fmt::Debug for OpBmap {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpCreate").finish()
    }
}

#[repr(transparent)]
pub struct OutHeader(fuse_out_header);

impl OutHeader {
    pub fn new(unique: u64, error: i32, data_len: usize) -> Self {
        Self(fuse_out_header {
            unique,
            error: -error,
            len: (mem::size_of::<fuse_out_header>() + data_len) as u32,
        })
    }
}

#[repr(transparent)]
pub struct AttrOut(fuse_attr_out);

impl fmt::Debug for AttrOut {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("AttrOut").finish()
    }
}

impl Default for AttrOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl From<FileAttr> for AttrOut {
    fn from(attr: FileAttr) -> Self {
        let mut attr_out = Self::default();
        attr_out.set_attr(attr);
        attr_out
    }
}

impl From<libc::stat> for AttrOut {
    fn from(attr: libc::stat) -> Self {
        Self::from(FileAttr::from(attr))
    }
}

impl AttrOut {
    pub fn set_attr(&mut self, attr: impl Into<FileAttr>) {
        self.0.attr = attr.into().0;
    }

    pub fn set_attr_valid(&mut self, sec: u64, nsec: u32) {
        self.0.attr_valid = sec;
        self.0.attr_valid_nsec = nsec;
    }
}

#[repr(transparent)]
pub struct EntryOut(pub(crate) fuse_entry_out);

impl fmt::Debug for EntryOut {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EntryOut").finish()
    }
}

impl Default for EntryOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl EntryOut {
    pub fn set_nodeid(&mut self, nodeid: u64) {
        self.0.nodeid = nodeid;
    }

    pub fn set_generation(&mut self, generation: u64) {
        self.0.generation = generation;
    }

    pub fn set_entry_valid(&mut self, sec: u64, nsec: u32) {
        self.0.entry_valid = sec;
        self.0.entry_valid_nsec = nsec;
    }

    pub fn set_attr_valid(&mut self, sec: u64, nsec: u32) {
        self.0.attr_valid = sec;
        self.0.attr_valid_nsec = nsec;
    }

    pub fn set_attr(&mut self, attr: impl Into<FileAttr>) {
        self.0.attr = attr.into().0;
    }
}

#[repr(transparent)]
pub struct InitOut(fuse_init_out);

impl fmt::Debug for InitOut {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("InitOut").finish()
    }
}

impl Default for InitOut {
    fn default() -> Self {
        let mut init_out: fuse_init_out = unsafe { mem::zeroed() };
        init_out.major = crate::bindings::FUSE_KERNEL_VERSION;
        init_out.minor = crate::bindings::FUSE_KERNEL_MINOR_VERSION;
        Self(init_out)
    }
}

impl InitOut {
    pub fn set_flags(&mut self, flags: CapFlags) {
        self.0.flags = flags.bits();
    }

    pub fn max_readahead(&self) -> u32 {
        self.0.max_readahead
    }

    pub fn set_max_readahead(&mut self, max_readahead: u32) {
        self.0.max_readahead = max_readahead;
    }

    pub fn max_write(&self) -> u32 {
        self.0.max_write
    }

    pub fn set_max_write(&mut self, max_write: u32) {
        self.0.max_write = max_write;
    }
}

#[repr(transparent)]
pub struct GetxattrOut(fuse_getxattr_out);

impl fmt::Debug for GetxattrOut {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("GetxattrOut").finish()
    }
}

impl Default for GetxattrOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl GetxattrOut {
    pub fn set_size(&mut self, size: u32) {
        self.0.size = size;
    }
}

#[repr(transparent)]
pub struct OpenOut(fuse_open_out);

impl fmt::Debug for OpenOut {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpenOut").finish()
    }
}

impl Default for OpenOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl OpenOut {
    pub fn set_fh(&mut self, fh: u64) {
        self.0.fh = fh;
    }

    pub fn set_flags(&mut self, flags: OpenFlags) {
        self.0.open_flags = flags.bits();
    }
}

bitflags! {
    pub struct OpenFlags: u32 {
        const DIRECT_IO = crate::bindings::FOPEN_DIRECT_IO;
        const KEEP_CACHE = crate::bindings::FOPEN_KEEP_CACHE;
        const NONSEEKABLE = crate::bindings::FOPEN_NONSEEKABLE;
        //const CACHE_DIR = crate::abi::FOPEN_CACHE_DIR;
    }
}

#[repr(transparent)]
pub struct WriteOut(fuse_write_out);

impl fmt::Debug for WriteOut {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("WriteOut").finish()
    }
}

impl Default for WriteOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl WriteOut {
    pub fn set_size(&mut self, size: u32) {
        self.0.size = size;
    }
}

#[repr(transparent)]
pub struct StatfsOut(fuse_statfs_out);

impl fmt::Debug for StatfsOut {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("StatfsOut").finish()
    }
}

impl Default for StatfsOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl StatfsOut {
    pub fn set_st(&mut self, st: impl Into<Statfs>) {
        self.0.st = st.into().0;
    }
}

#[repr(transparent)]
pub struct LkOut(fuse_lk_out);

impl fmt::Debug for LkOut {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("LkOut").finish()
    }
}

impl Default for LkOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl LkOut {
    pub fn set_lk(&mut self, lk: impl Into<FileLock>) {
        self.0.lk = lk.into().0;
    }
}

#[repr(transparent)]
pub struct BmapOut(fuse_bmap_out);

impl fmt::Debug for BmapOut {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("BmapOut").finish()
    }
}

impl Default for BmapOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl BmapOut {
    pub fn set_block(&mut self, block: u64) {
        self.0.block = block;
    }
}
