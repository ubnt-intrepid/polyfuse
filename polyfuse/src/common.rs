use polyfuse_sys::kernel::{fuse_attr, fuse_file_lock, fuse_forget_one, fuse_kstatfs};
use std::{convert::TryFrom, error, fmt};

/// Attributes about a file.
///
/// This type is ABI-compatible with `fuse_attr`.
#[derive(Debug, Default, Clone, Copy)]
#[repr(transparent)]
pub struct FileAttr(fuse_attr);

macro_rules! define_accessor {
    ($field:ident, $field_mut:ident, $t:ty) => {
        pub fn $field(&self) -> $t {
            (self.0).$field
        }

        pub fn $field_mut(&mut self, value: $t) {
            (self.0).$field = value;
        }
    };
}

impl FileAttr {
    define_accessor!(ino, set_ino, u64);
    define_accessor!(size, set_size, u64);
    define_accessor!(blocks, set_blocks, u64);
    define_accessor!(mode, set_mode, u32);
    define_accessor!(nlink, set_nlink, u32);
    define_accessor!(uid, set_uid, u32);
    define_accessor!(gid, set_gid, u32);
    define_accessor!(rdev, set_rdev, u32);
    define_accessor!(blksize, set_blksize, u32);

    pub fn atime(&self) -> (u64, u32) {
        (self.0.atime, self.0.atimensec)
    }

    pub fn set_atime(&mut self, sec: u64, nsec: u32) {
        self.0.atime = sec;
        self.0.atimensec = nsec;
    }

    pub fn mtime(&self) -> (u64, u32) {
        (self.0.mtime, self.0.mtimensec)
    }

    pub fn set_mtime(&mut self, sec: u64, nsec: u32) {
        self.0.mtime = sec;
        self.0.mtimensec = nsec;
    }

    pub fn ctime(&self) -> (u64, u32) {
        (self.0.ctime, self.0.ctimensec)
    }

    pub fn set_ctime(&mut self, sec: u64, nsec: u32) {
        self.0.ctime = sec;
        self.0.ctimensec = nsec;
    }
}

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

/// File lock information.
///
/// This type is ABI-compatible with `fuse_file_lock`.
#[derive(Debug, Copy, Clone, Default)]
#[repr(transparent)]
pub struct FileLock(fuse_file_lock);

impl FileLock {
    pub(crate) fn new(attr: &fuse_file_lock) -> &Self {
        unsafe { &*(attr as *const fuse_file_lock as *const Self) }
    }

    define_accessor!(start, set_start, u64);
    define_accessor!(end, set_end, u64);
    define_accessor!(typ, set_typ, u32);
    define_accessor!(pid, set_pid, u32);

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

/// Filesystem statistics.
///
/// This type is ABI-compatible with `fuse_kstatfs`.
#[derive(Debug, Clone, Copy, Default)]
#[repr(transparent)]
pub struct StatFs(fuse_kstatfs);

impl StatFs {
    define_accessor!(blocks, set_blocks, u64);
    define_accessor!(bfree, set_bfree, u64);
    define_accessor!(bavail, set_bavail, u64);
    define_accessor!(files, set_files, u64);
    define_accessor!(ffree, set_ffree, u64);
    define_accessor!(bsize, set_bsize, u32);
    define_accessor!(namelen, set_namelen, u32);
    define_accessor!(frsize, set_frsize, u32);

    pub(crate) fn into_inner(self) -> fuse_kstatfs {
        self.0
    }
}

impl TryFrom<libc::statvfs> for StatFs {
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

/// A forget information.
///
/// This type is ABI-compatible with `fuse_forget_one`.
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
