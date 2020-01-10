use crate::{
    kernel::{fuse_attr, fuse_file_lock, fuse_forget_one, fuse_kstatfs},
    util::{make_raw_time, make_system_time},
};
use std::{
    convert::TryFrom, //
    error,
    fmt,
    time::SystemTime,
};

/// Attributes about a file.
///
/// This type is ABI-compatible with `fuse_attr`.
#[derive(Default, Clone, Copy)]
#[repr(transparent)]
pub struct FileAttr(fuse_attr);

impl fmt::Debug for FileAttr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileAttr")
            .field("ino", &self.ino())
            .field("size", &self.size())
            .field("mode", &self.mode())
            .field("nlink", &self.nlink())
            .field("uid", &self.uid())
            .field("gid", &self.gid())
            .field("rdev", &self.rdev())
            .field("blksize", &self.blksize())
            .field("blocks", &self.blocks())
            .field("atime", &self.atime())
            .field("mtime", &self.mtime())
            .field("ctime", &self.ctime())
            .finish()
    }
}

impl FileAttr {
    /// Return the inode number.
    pub fn ino(&self) -> u64 {
        self.0.ino
    }

    /// Set the inode number.
    pub fn set_ino(&mut self, ino: u64) {
        self.0.ino = ino;
    }

    /// Return the size of content.
    pub fn size(&self) -> u64 {
        self.0.size
    }

    /// Set the size of content.
    pub fn set_size(&mut self, size: u64) {
        self.0.size = size;
    }

    /// Return the permission of the inode.
    pub fn mode(&self) -> u32 {
        self.0.mode
    }

    /// Set the permission of the inode.
    pub fn set_mode(&mut self, mode: u32) {
        self.0.mode = mode;
    }

    /// Return the number of hard links.
    pub fn nlink(&self) -> u32 {
        self.0.nlink
    }

    /// Set the number of hard links.
    pub fn set_nlink(&mut self, nlink: u32) {
        self.0.nlink = nlink
    }

    /// Return the user ID.
    pub fn uid(&self) -> u32 {
        self.0.uid
    }

    /// Set the user ID.
    pub fn set_uid(&mut self, uid: u32) {
        self.0.uid = uid;
    }

    /// Return the group ID.
    pub fn gid(&self) -> u32 {
        self.0.gid
    }

    /// Set the group ID.
    pub fn set_gid(&mut self, gid: u32) {
        self.0.gid = gid;
    }

    /// Return the device ID.
    pub fn rdev(&self) -> u32 {
        self.0.rdev
    }

    /// Set the device ID.
    pub fn set_rdev(&mut self, rdev: u32) {
        self.0.rdev = rdev;
    }

    /// Return the block size.
    pub fn blksize(&self) -> u32 {
        self.0.blksize
    }

    /// Set the block size.
    pub fn set_blksize(&mut self, blksize: u32) {
        self.0.blksize = blksize;
    }

    /// Return the number of allocated blocks.
    pub fn blocks(&self) -> u64 {
        self.0.blocks
    }

    /// Set the number of allocated blocks.
    pub fn set_blocks(&mut self, blocks: u64) {
        self.0.blocks = blocks;
    }

    /// Return the last accessed time.
    pub fn atime(&self) -> SystemTime {
        make_system_time(self.atime_raw())
    }

    /// Return the last accessed time in raw form.
    pub fn atime_raw(&self) -> (u64, u32) {
        (self.0.atime, self.0.atimensec)
    }

    /// Set the last accessed time.
    pub fn set_atime(&mut self, time: SystemTime) {
        self.set_atime_raw(make_raw_time(time))
    }

    /// Set the last accessed time by raw form.
    pub fn set_atime_raw(&mut self, (sec, nsec): (u64, u32)) {
        self.0.atime = sec;
        self.0.atimensec = nsec;
    }

    /// Return the last modification time.
    pub fn mtime(&self) -> SystemTime {
        make_system_time(self.mtime_raw())
    }

    /// Return the last modification time in raw form.
    pub fn mtime_raw(&self) -> (u64, u32) {
        (self.0.mtime, self.0.mtimensec)
    }

    /// Set the last modification time.
    pub fn set_mtime(&mut self, time: SystemTime) {
        self.set_mtime_raw(make_raw_time(time))
    }

    /// Set the last modification time by raw form.
    pub fn set_mtime_raw(&mut self, (sec, nsec): (u64, u32)) {
        self.0.mtime = sec;
        self.0.mtimensec = nsec;
    }

    /// Return the last created time.
    pub fn ctime(&self) -> SystemTime {
        make_system_time(self.ctime_raw())
    }

    /// Return the last created time in raw form.
    pub fn ctime_raw(&self) -> (u64, u32) {
        (self.0.ctime, self.0.ctimensec)
    }

    /// Set the last created time.
    pub fn set_ctime(&mut self, time: SystemTime) {
        self.set_ctime_raw(make_raw_time(time))
    }

    /// Set the last created time by raw form.
    pub fn set_ctime_raw(&mut self, (sec, nsec): (u64, u32)) {
        self.0.ctime = sec;
        self.0.ctimensec = nsec;
    }
}

// #[cfg(target_os = "linux")]
mod impl_try_from_for_file_attr {
    use super::*;
    use std::{
        convert::{TryFrom, TryInto},
        fs::Metadata,
        mem,
        os::linux::fs::MetadataExt,
    };

    impl TryFrom<libc::stat> for FileAttr {
        // FIXME: switch to the custom error type.
        type Error = std::num::TryFromIntError;

        #[allow(clippy::cast_sign_loss)]
        fn try_from(attr: libc::stat) -> Result<Self, Self::Error> {
            let mut ret: fuse_attr = unsafe { mem::MaybeUninit::zeroed().assume_init() };

            ret.ino = attr.st_ino;
            ret.mode = attr.st_mode;
            ret.nlink = attr.st_nlink.try_into()?; // FUSE does not support 64bit nlinks.
            ret.uid = attr.st_uid;
            ret.gid = attr.st_gid;
            ret.rdev = attr.st_rdev.try_into()?; // FUSE does not support 64bit devide ID.

            // In the FUSE kernel driver, the type of this field is treated as `loff_t`.
            ret.size = attr.st_size as u64;

            ret.atime = attr.st_atime as u64;
            ret.atimensec = attr.st_atime_nsec.try_into()?;
            ret.mtime = attr.st_mtime as u64;
            ret.mtimensec = attr.st_mtime_nsec.try_into()?;
            ret.ctime = attr.st_ctime as u64;
            ret.ctimensec = attr.st_ctime_nsec.try_into()?;

            ret.blksize = attr.st_blksize.try_into()?;
            ret.blocks = attr.st_blocks as u64;

            Ok(Self(ret))
        }
    }

    impl TryFrom<Metadata> for FileAttr {
        type Error = std::num::TryFromIntError;

        #[allow(clippy::cast_sign_loss)]
        fn try_from(attr: Metadata) -> Result<Self, Self::Error> {
            let mut ret: fuse_attr = unsafe { mem::MaybeUninit::zeroed().assume_init() };

            ret.ino = attr.st_ino();
            ret.mode = attr.st_mode();
            ret.nlink = attr.st_nlink().try_into()?; // FUSE does not support 64bit nlinks.
            ret.uid = attr.st_uid();
            ret.gid = attr.st_gid();
            ret.rdev = attr.st_rdev().try_into()?; // FUSE does not support 64bit devide ID.

            // In the FUSE kernel driver, the type of this field is treated as `loff_t`.
            ret.size = attr.st_size() as u64;

            ret.atime = attr.st_atime() as u64;
            ret.atimensec = attr.st_atime_nsec().try_into()?;
            ret.mtime = attr.st_mtime() as u64;
            ret.mtimensec = attr.st_mtime_nsec().try_into()?;
            ret.ctime = attr.st_ctime() as u64;
            ret.ctimensec = attr.st_ctime_nsec().try_into()?;

            ret.blksize = attr.st_blksize().try_into()?;
            ret.blocks = attr.st_blocks() as u64;

            Ok(Self(ret))
        }
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
#[derive(Copy, Clone, Default)]
#[repr(transparent)]
pub struct FileLock(fuse_file_lock);

impl fmt::Debug for FileLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileLock")
            .field("typ", &self.typ())
            .field("start", &self.start())
            .field("end", &self.end())
            .field("pid", &self.pid())
            .finish()
    }
}

impl FileLock {
    pub(crate) fn new(attr: &fuse_file_lock) -> &Self {
        unsafe { &*(attr as *const fuse_file_lock as *const Self) }
    }

    /// Return the type of lock.
    pub fn typ(&self) -> u32 {
        self.0.typ
    }

    /// Set the type of lock.
    pub fn set_typ(&mut self, typ: u32) {
        self.0.typ = typ;
    }

    /// Return the starting offset for lock.
    pub fn start(&self) -> u64 {
        self.0.start
    }

    /// Set the starting offset for lock.
    pub fn set_start(&mut self, start: u64) {
        self.0.start = start;
    }

    /// Return the ending offset for lock.
    pub fn end(&self) -> u64 {
        self.0.end
    }

    /// Set the ending offset for lock.
    pub fn set_end(&mut self, end: u64) {
        self.0.end = end;
    }

    /// Return the process ID blocking the lock.
    pub fn pid(&self) -> u32 {
        self.0.pid
    }

    /// Set the process ID blocking the lock.
    pub fn set_pid(&mut self, pid: u32) {
        self.0.pid = pid;
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

/// Filesystem statistics.
///
/// This type is ABI-compatible with `fuse_kstatfs`.
#[derive(Clone, Copy, Default)]
#[repr(transparent)]
pub struct StatFs(fuse_kstatfs);

impl fmt::Debug for StatFs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Statfs")
            .field("bsize", &self.bsize())
            .field("frsize", &self.frsize())
            .field("blocks", &self.blocks())
            .field("bfree", &self.bfree())
            .field("bavail", &self.bavail())
            .field("files", &self.files())
            .field("ffree", &self.ffree())
            .field("namelen", &self.namelen())
            .finish()
    }
}

impl StatFs {
    /// Return the block size.
    pub fn bsize(&self) -> u32 {
        self.0.bsize
    }

    /// Set the block size.
    pub fn set_bsize(&mut self, bsize: u32) {
        self.0.bsize = bsize;
    }

    /// Return the fragment size.
    pub fn frsize(&self) -> u32 {
        self.0.frsize
    }

    /// Set the fragment size.
    pub fn set_frsize(&mut self, frsize: u32) {
        self.0.frsize = frsize;
    }

    /// Return the number of blocks in the filesystem.
    pub fn blocks(&self) -> u64 {
        self.0.blocks
    }

    /// Return the number of blocks in the filesystem.
    pub fn set_blocks(&mut self, blocks: u64) {
        self.0.blocks = blocks;
    }

    /// Return the number of free blocks.
    pub fn bfree(&self) -> u64 {
        self.0.bfree
    }

    /// Return the number of free blocks.
    pub fn set_bfree(&mut self, bfree: u64) {
        self.0.bfree = bfree;
    }

    /// Return the number of free blocks for non-priviledge users.
    pub fn bavail(&self) -> u64 {
        self.0.bavail
    }

    /// Set the number of free blocks for non-priviledge users.
    pub fn set_bavail(&mut self, bavail: u64) {
        self.0.bavail = bavail;
    }

    /// Return the number of inodes.
    pub fn files(&self) -> u64 {
        self.0.files
    }

    /// Set the number of inodes.
    pub fn set_files(&mut self, files: u64) {
        self.0.files = files;
    }

    /// Return the number of free inodes.
    pub fn ffree(&self) -> u64 {
        self.0.ffree
    }

    /// Set the number of free inodes.
    pub fn set_ffree(&mut self, ffree: u64) {
        self.0.ffree = ffree;
    }

    /// Return the maximum length of file names.
    pub fn namelen(&self) -> u32 {
        self.0.namelen
    }

    /// Return the maximum length of file names.
    pub fn set_namelen(&mut self, namelen: u32) {
        self.0.namelen = namelen;
    }

    pub(crate) fn into_inner(self) -> fuse_kstatfs {
        self.0
    }
}

impl TryFrom<libc::statvfs> for StatFs {
    type Error = std::num::TryFromIntError;

    fn try_from(st: libc::statvfs) -> Result<Self, Self::Error> {
        Ok(Self(fuse_kstatfs {
            bsize: u32::try_from(st.f_bsize).map_err(Self::Error::from)?,
            frsize: u32::try_from(st.f_frsize).map_err(Self::Error::from)?,
            blocks: u64::try_from(st.f_blocks).map_err(Self::Error::from)?,
            bfree: u64::try_from(st.f_bfree).map_err(Self::Error::from)?,
            bavail: u64::try_from(st.f_bavail).map_err(Self::Error::from)?,
            files: u64::try_from(st.f_files).map_err(Self::Error::from)?,
            ffree: u64::try_from(st.f_ffree).map_err(Self::Error::from)?,
            namelen: u32::try_from(st.f_namemax).map_err(Self::Error::from)?,
            padding: 0,
            spare: [0u32; 6],
        }))
    }
}

/// A forget information.
///
/// This type is ABI-compatible with `fuse_forget_one`.
#[repr(transparent)]
pub struct Forget(fuse_forget_one);

impl fmt::Debug for Forget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Forget")
            .field("ino", &self.ino())
            .field("nlookup", &self.nlookup())
            .finish()
    }
}

impl Forget {
    pub(crate) const fn new(ino: u64, nlookup: u64) -> Self {
        Self(fuse_forget_one {
            nodeid: ino,
            nlookup,
        })
    }

    /// Return the inode number of the target inode.
    pub fn ino(&self) -> u64 {
        self.0.nodeid
    }

    /// Return the released lookup count of the target inode.
    pub fn nlookup(&self) -> u64 {
        self.0.nlookup
    }
}

/// The identifier for locking operations.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct LockOwner(u64);

impl fmt::Debug for LockOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LockOwner {{ .. }}")
    }
}

impl LockOwner {
    pub(crate) const fn from_raw(id: u64) -> Self {
        Self(id)
    }
}
