use std::{fmt, fs::Metadata, num::NonZeroU64, os::unix::prelude::*, time::Duration};

pub type RequestID = u64;
pub type RetrieveID = u64;
pub type UID = u32;
pub type GID = u32;
pub type PID = u32;
pub type PollID = u64;
pub type FileHandle = u64;

pub type FileMode = u32;
pub type OpenFlags = u32;
pub type LockType = u32;

#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeID(NonZeroU64);

impl NodeID {
    pub const ROOT: Self = Self(unsafe { NonZeroU64::new_unchecked(1) });

    pub const fn from_raw(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(ino) => Some(Self(ino)),
            None => None,
        }
    }

    pub const unsafe fn from_raw_unchecked(value: u64) -> Self {
        Self(NonZeroU64::new_unchecked(value))
    }

    pub const fn into_raw(self) -> u64 {
        self.0.get()
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
    #[inline]
    pub(crate) const fn from_raw(id: u64) -> Self {
        Self(id)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct DeviceID {
    pub major: u32,
    pub minor: u32,
}

impl DeviceID {
    pub const fn to_userspace_dev_t(self) -> u64 {
        libc::makedev(self.major, self.minor)
    }

    pub const fn to_kernel_dev_t(self) -> u32 {
        debug_assert!(self.major < 0x1000, "invalid DeviceID.major");
        debug_assert!(self.minor < 0x100000, "invalid DeviceID.minor");
        (self.major << 20) | (self.minor & 0x000f_ffff)
    }
}

/// The collection of attributes corresponding to an inode entries.
///
/// This trait is the abstraction of both `fuse_attr` and `fuse_statx`.
pub trait FileAttr {
    /// Return the inode number.
    fn ino(&self) -> Option<NodeID>;

    /// Return the size of content.
    fn size(&self) -> u64;

    /// Return the permission of the inode.
    fn mode(&self) -> FileMode;

    /// Return the number of hard links.
    fn nlink(&self) -> u32;

    /// Return the user ID.
    fn uid(&self) -> UID;

    /// Return the group ID.
    fn gid(&self) -> GID;

    /// Return the device ID.
    fn rdev(&self) -> DeviceID;

    /// Return the block size.
    fn blksize(&self) -> u32;

    /// Return the number of allocated blocks.
    fn blocks(&self) -> u64;

    /// Return the last accessed time.
    fn atime(&self) -> Duration;

    /// Return the last modification time.
    fn mtime(&self) -> Duration;

    /// Return the last created time.
    fn ctime(&self) -> Duration;
}

macro_rules! impl_file_attr_fields {
    (@all $( $name:ident => $ret:ty, )*) => {$(
        #[inline]
        fn $name(&self) -> $ret {
            (**self).$name()
        }
    )*};
    () => {
        impl_file_attr_fields!(@all
            ino => Option<NodeID>,
            size => u64,
            mode => FileMode,
            nlink => u32,
            uid => UID,
            gid => GID,
            rdev => DeviceID,
            blksize => u32,
            blocks => u64,
            atime => Duration,
            mtime => Duration,
            ctime => Duration,
        );
    };
}

impl<T: ?Sized> FileAttr for &T
where
    T: FileAttr,
{
    impl_file_attr_fields!();
}

impl<T: ?Sized> FileAttr for &mut T
where
    T: FileAttr,
{
    impl_file_attr_fields!();
}

impl<T: ?Sized> FileAttr for Box<T>
where
    T: FileAttr,
{
    impl_file_attr_fields!();
}

impl<T: ?Sized> FileAttr for std::rc::Rc<T>
where
    T: FileAttr,
{
    impl_file_attr_fields!();
}

impl<T: ?Sized> FileAttr for std::sync::Arc<T>
where
    T: FileAttr,
{
    impl_file_attr_fields!();
}

impl FileAttr for libc::stat {
    fn ino(&self) -> Option<NodeID> {
        NodeID::from_raw(self.st_ino)
    }

    fn size(&self) -> u64 {
        self.st_size as _
    }

    fn mode(&self) -> FileMode {
        self.st_mode
    }

    fn nlink(&self) -> u32 {
        self.st_nlink.try_into().expect("st_nlink")
    }

    fn uid(&self) -> UID {
        self.st_uid
    }

    fn gid(&self) -> GID {
        self.st_gid
    }

    fn rdev(&self) -> DeviceID {
        DeviceID {
            major: libc::major(self.st_rdev),
            minor: libc::minor(self.st_rdev),
        }
    }

    fn blksize(&self) -> u32 {
        self.st_blksize.try_into().expect("st_blksize")
    }

    fn blocks(&self) -> u64 {
        self.st_blocks as _
    }

    fn atime(&self) -> Duration {
        Duration::new(
            self.st_atime as _,
            self.st_atime_nsec.try_into().expect("st_atime_nsec"),
        )
    }

    fn mtime(&self) -> Duration {
        Duration::new(
            self.st_mtime as _,
            self.st_mtime_nsec.try_into().expect("st_mtime_nsec"),
        )
    }

    fn ctime(&self) -> Duration {
        Duration::new(
            self.st_ctime as _,
            self.st_ctime_nsec.try_into().expect("st_ctime_nsec"),
        )
    }
}

impl FileAttr for Metadata {
    fn ino(&self) -> Option<NodeID> {
        NodeID::from_raw(MetadataExt::ino(self))
    }

    fn size(&self) -> u64 {
        MetadataExt::size(self) as _
    }

    fn mode(&self) -> FileMode {
        MetadataExt::mode(self)
    }

    fn nlink(&self) -> u32 {
        MetadataExt::nlink(self).try_into().expect("metadata.nlink")
    }

    fn uid(&self) -> UID {
        MetadataExt::uid(self)
    }

    fn gid(&self) -> GID {
        MetadataExt::gid(self)
    }

    fn rdev(&self) -> DeviceID {
        DeviceID {
            major: libc::major(MetadataExt::rdev(self)),
            minor: libc::minor(MetadataExt::rdev(self)),
        }
    }

    fn blksize(&self) -> u32 {
        MetadataExt::blksize(self)
            .try_into()
            .expect("metadata.blksize")
    }

    fn blocks(&self) -> u64 {
        MetadataExt::blocks(self)
    }

    fn atime(&self) -> Duration {
        Duration::new(
            MetadataExt::atime(self) as _,
            MetadataExt::atime_nsec(self) as _,
        )
    }

    fn mtime(&self) -> Duration {
        Duration::new(
            MetadataExt::atime(self) as _,
            MetadataExt::atime_nsec(self) as _,
        )
    }

    fn ctime(&self) -> Duration {
        Duration::new(
            MetadataExt::atime(self) as _,
            MetadataExt::atime_nsec(self) as _,
        )
    }
}

pub trait FsStatistics {
    /// Return the block size.
    fn bsize(&self) -> u32;

    /// Return the fragment size.
    fn frsize(&self) -> u32;

    /// Return the number of blocks in the filesystem.
    fn blocks(&self) -> u64;

    /// Return the number of free blocks.
    fn bfree(&self) -> u64;

    /// Return the number of free blocks for non-priviledge users.
    fn bavail(&self) -> u64;

    /// Return the number of inodes.
    fn files(&self) -> u64;

    /// Return the number of free inodes.
    fn ffree(&self) -> u64;

    /// Return the maximum length of file names.
    fn namelen(&self) -> u32;
}

macro_rules! impl_fs_statistics_fields {
    (@all $( $name:ident => $ret:ty, )*) => {$(
        #[inline]
        fn $name(&self) -> $ret {
            (**self).$name()
        }
    )*};
    () => {
        impl_fs_statistics_fields!(@all
            bsize => u32,
            frsize => u32,
            blocks => u64,
            bfree => u64,
            bavail => u64,
            files => u64,
            ffree => u64,
            namelen => u32,
        );
    };
}

impl<T: ?Sized> FsStatistics for &T
where
    T: FsStatistics,
{
    impl_fs_statistics_fields!();
}

impl<T: ?Sized> FsStatistics for &mut T
where
    T: FsStatistics,
{
    impl_fs_statistics_fields!();
}

impl<T: ?Sized> FsStatistics for Box<T>
where
    T: FsStatistics,
{
    impl_fs_statistics_fields!();
}

impl<T: ?Sized> FsStatistics for std::rc::Rc<T>
where
    T: FsStatistics,
{
    impl_fs_statistics_fields!();
}

impl<T: ?Sized> FsStatistics for std::sync::Arc<T>
where
    T: FsStatistics,
{
    impl_fs_statistics_fields!();
}

impl FsStatistics for libc::statvfs {
    #[inline]
    fn bsize(&self) -> u32 {
        self.f_bsize.try_into().expect("bsize")
    }

    #[inline]
    fn frsize(&self) -> u32 {
        self.f_frsize.try_into().expect("frsize")
    }

    #[inline]
    fn blocks(&self) -> u64 {
        self.f_blocks
    }

    #[inline]
    fn bfree(&self) -> u64 {
        self.f_bfree
    }

    #[inline]
    fn bavail(&self) -> u64 {
        self.f_bavail
    }

    #[inline]
    fn files(&self) -> u64 {
        self.f_files
    }

    #[inline]
    fn ffree(&self) -> u64 {
        self.f_ffree
    }

    #[inline]
    fn namelen(&self) -> u32 {
        self.f_namemax.try_into().expect("f_namemax")
    }
}

pub trait FileLock {
    /// Return the type of this lock.
    fn typ(&self) -> LockType;

    /// Return the starting offset to be locked.
    fn start(&self) -> u64;

    /// Return the ending offset to be locked.
    fn end(&self) -> u64;

    /// Return the process ID.
    fn pid(&self) -> PID;
}

macro_rules! impl_file_lock_fields {
    (@all $( $name:ident => $ret:ty, )*) => {$(
        #[inline]
        fn $name(&self) -> $ret {
            (**self).$name()
        }
    )*};
    () => {
        impl_file_lock_fields!(@all
            typ => LockType,
            start => u64,
            end => u64,
            pid => PID,
        );
    };
}

impl<T: ?Sized> FileLock for &T
where
    T: FileLock,
{
    impl_file_lock_fields!();
}

impl<T: ?Sized> FileLock for &mut T
where
    T: FileLock,
{
    impl_file_lock_fields!();
}

impl<T: ?Sized> FileLock for Box<T>
where
    T: FileLock,
{
    impl_file_lock_fields!();
}

impl<T: ?Sized> FileLock for std::rc::Rc<T>
where
    T: FileLock,
{
    impl_file_lock_fields!();
}

impl<T: ?Sized> FileLock for std::sync::Arc<T>
where
    T: FileLock,
{
    impl_file_lock_fields!();
}
