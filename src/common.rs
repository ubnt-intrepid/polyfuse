use polyfuse_sys::kernel::{fuse_attr, fuse_file_lock, fuse_forget_one, fuse_kstatfs};
use std::{convert::TryFrom, error, fmt};

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct FileAttr(fuse_attr);

impl TryFrom<libc::stat> for FileAttr {
    type Error = std::num::TryFromIntError;

    fn try_from(attr: libc::stat) -> Result<Self, Self::Error> {
        Ok(Self(fuse_attr {
            ino: u64::try_from(attr.st_ino)?,
            size: u64::try_from(attr.st_size)?,
            blocks: u64::try_from(attr.st_blocks)?,
            atime: u64::try_from(attr.st_atime)?,
            mtime: u64::try_from(attr.st_mtime)?,
            ctime: u64::try_from(attr.st_ctime)?,
            atimensec: u32::try_from(attr.st_atime_nsec)?,
            mtimensec: u32::try_from(attr.st_mtime_nsec)?,
            ctimensec: u32::try_from(attr.st_ctime_nsec)?,
            mode: u32::try_from(attr.st_mode)?,
            nlink: u32::try_from(attr.st_nlink)?,
            uid: u32::try_from(attr.st_uid)?,
            gid: u32::try_from(attr.st_gid)?,
            rdev: u32::try_from(attr.st_gid)?,
            blksize: u32::try_from(attr.st_blksize)?,
            padding: 0,
        }))
    }
}

impl FileAttr {
    pub(crate) fn into_inner(self) -> fuse_attr {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct FsStatistics(fuse_kstatfs);

impl TryFrom<libc::statvfs> for FsStatistics {
    type Error = std::num::TryFromIntError;

    fn try_from(st: libc::statvfs) -> Result<Self, Self::Error> {
        Ok(Self(fuse_kstatfs {
            bsize: u32::try_from(st.f_bsize)?,
            frsize: u32::try_from(st.f_frsize)?,
            blocks: u64::try_from(st.f_blocks)?,
            bfree: u64::try_from(st.f_bfree)?,
            bavail: u64::try_from(st.f_bavail)?,
            files: u64::try_from(st.f_files)?,
            ffree: u64::try_from(st.f_ffree)?,
            namelen: u32::try_from(st.f_namemax)?,
            padding: 0,
            spare: [0u32; 6],
        }))
    }
}

impl FsStatistics {
    pub(crate) fn into_inner(self) -> fuse_kstatfs {
        self.0
    }
}

#[derive(Debug)]
#[repr(transparent)]
pub struct FileLock(fuse_file_lock);

impl FileLock {
    pub(crate) fn new(attr: &fuse_file_lock) -> &Self {
        unsafe { &*(attr as *const fuse_file_lock as *const Self) }
    }

    pub(crate) fn into_inner(self) -> fuse_file_lock {
        self.0
    }
}

impl TryFrom<libc::flock> for FileLock {
    type Error = InvalidFileLock;

    #[allow(clippy::cast_sign_loss)]
    fn try_from(lk: libc::flock) -> Result<Self, Self::Error> {
        const F_RDLCK: u32 = libc::F_RDLCK as u32;
        const F_WRLCK: u32 = libc::F_WRLCK as u32;
        const F_UNLCK: u32 = libc::F_UNLCK as u32;

        let lock_type = lk.l_type as u32;
        let (start, end) = match lock_type {
            F_UNLCK => (0, 0),
            F_RDLCK | F_WRLCK => {
                let start = lk.l_start as u64;
                let end = if lk.l_len == 0 {
                    std::u64::MAX
                } else {
                    start + (lk.l_len as u64) - 1
                };
                (start, end)
            }
            _ => return Err(InvalidFileLock(())),
        };

        Ok(Self(fuse_file_lock {
            start,
            end,
            typ: lock_type,
            pid: lk.l_pid as u32,
        }))
    }
}

#[doc(hidden)]
#[derive(Debug)]
pub struct InvalidFileLock(());

impl From<std::convert::Infallible> for InvalidFileLock {
    fn from(infallible: std::convert::Infallible) -> Self {
        match infallible {}
    }
}

impl From<std::num::TryFromIntError> for InvalidFileLock {
    fn from(_: std::num::TryFromIntError) -> Self {
        Self(())
    }
}

impl fmt::Display for InvalidFileLock {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "invalid file lock")
    }
}

impl error::Error for InvalidFileLock {}

#[derive(Debug)]
#[repr(transparent)]
pub struct Forget(fuse_forget_one);

impl Forget {
    pub(crate) const fn new(ino: u64, nlookup: u64) -> Self {
        Self(fuse_forget_one {
            nodeid: ino,
            nlookup,
        })
    }

    pub fn ino(&self) -> u64 {
        self.0.nodeid
    }

    pub fn nlookup(&self) -> u64 {
        self.0.nlookup
    }
}
