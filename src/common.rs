use crate::abi::{fuse_attr, fuse_file_lock, fuse_kstatfs};
use bitflags::bitflags;
use std::{fmt, mem};

bitflags! {
    pub struct CapFlags: u32 {
        const ASYNC_READ = crate::abi::FUSE_ASYNC_READ;
        const POSIX_LOCKS = crate::abi::FUSE_POSIX_LOCKS;
        const FILE_OPS = crate::abi::FUSE_FILE_OPS;
        const ATOMIC_O_TRUNC = crate::abi::FUSE_ATOMIC_O_TRUNC;
        const EXPORT_SUPPORT = crate::abi::FUSE_EXPORT_SUPPORT;
        const BIG_WRITES = crate::abi::FUSE_BIG_WRITES;
        const DONT_MASK = crate::abi::FUSE_DONT_MASK;
        const SPLICE_WRITE = crate::abi::FUSE_SPLICE_WRITE;
        const SPLICE_MOVE = crate::abi::FUSE_SPLICE_MOVE;
        const SPLICE_READ = crate::abi::FUSE_SPLICE_READ;
        const FLOCK_LOCKS = crate::abi::FUSE_FLOCK_LOCKS;
        const HAS_IOCTL_DIR = crate::abi::FUSE_HAS_IOCTL_DIR;
        const AUTO_INVAL_DATA = crate::abi::FUSE_AUTO_INVAL_DATA;
        const DO_READDIRPLUS = crate::abi::FUSE_DO_READDIRPLUS;
        const READDIRPLUS_AUTO = crate::abi::FUSE_READDIRPLUS_AUTO;
        const ASYNC_DIO = crate::abi::FUSE_ASYNC_DIO;
        const WRITEBACK_CACHE = crate::abi::FUSE_WRITEBACK_CACHE;
        const NO_OPEN_SUPPORT = crate::abi::FUSE_NO_OPEN_SUPPORT;
        const PARALLEL_DIROPS = crate::abi::FUSE_PARALLEL_DIROPS;
        const HANDLE_KILLPRIV = crate::abi::FUSE_HANDLE_KILLPRIV;
        const POSIX_ACL = crate::abi:: FUSE_POSIX_ACL;
        const ABORT_ERROR = crate::abi::FUSE_ABORT_ERROR;

        // 7.28
        //const MAX_PAGES = crate::abi::FUSE_MAX_PAGES;
        //const CACHE_SYMLINKS = crate::abi::FUSE_CACHE_SYMLINKS;

        // 7.29
        //const NO_OPENDIR_SUPPORT = crate::abi::FUSE_NO_OPENDIR_SUPPORT;
    }
}

#[repr(transparent)]
pub struct Attr(pub(crate) fuse_attr);

impl Default for Attr {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl From<libc::stat> for Attr {
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
