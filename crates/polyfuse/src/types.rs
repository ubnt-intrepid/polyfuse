//! Common types used in the filesystem representation.

use self::non_exhaustive::NonExhaustive;
use std::{borrow::Cow, ffi::OsStr, fmt};

/// Attributes about a file.
pub trait FileAttr {
    /// Return the inode number.
    fn ino(&self) -> u64;

    /// Return the size of content.
    fn size(&self) -> u64;

    /// Return the permission of the inode.
    fn mode(&self) -> u32;

    /// Return the number of hard links.
    fn nlink(&self) -> u32;

    /// Return the user ID.
    fn uid(&self) -> u32;

    /// Return the group ID.
    fn gid(&self) -> u32;

    /// Return the device ID.
    fn rdev(&self) -> u32;

    /// Return the block size.
    fn blksize(&self) -> u32;

    /// Return the number of allocated blocks.
    fn blocks(&self) -> u64;

    /// Return the last accessed time.
    fn atime(&self) -> u64;

    /// Return the last accessed time in nanoseconds since `atime`.
    fn atime_nsec(&self) -> u32;

    /// Return the last modification time.
    fn mtime(&self) -> u64;

    /// Return the last modification time in nanoseconds since `mtime`.
    fn mtime_nsec(&self) -> u32;

    /// Return the last created time.
    fn ctime(&self) -> u64;

    /// Return the last created time in nanoseconds since `ctime`.    
    fn ctime_nsec(&self) -> u32;
}

mod impl_file_attr {
    use super::FileAttr;
    use std::{convert::TryInto as _, fs::Metadata, os::unix::fs::MetadataExt};

    macro_rules! passthrough {
        ($m:ident) => {
            $m! {
                ino: u64,
                size: u64,
                mode: u32,
                nlink: u32,
                uid: u32,
                gid: u32,
                rdev: u32,
                blksize: u32,
                blocks: u64,
                atime: u64,
                atime_nsec: u32,
                mtime: u64,
                mtime_nsec: u32,
                ctime: u64,
                ctime_nsec: u32,
            }
        };
    }

    macro_rules! refs {
        ($( $name:ident : $ret:ty, )*) => {$(
            #[inline]
            fn $name(&self) -> $ret {
                (**self).$name()
            }
        )*};
    }

    impl<T: ?Sized> FileAttr for &T
    where
        T: FileAttr,
    {
        passthrough!(refs);
    }

    impl<T: ?Sized> FileAttr for Box<T>
    where
        T: FileAttr,
    {
        passthrough!(refs);
    }

    impl<T: ?Sized> FileAttr for std::rc::Rc<T>
    where
        T: FileAttr,
    {
        passthrough!(refs);
    }

    impl<T: ?Sized> FileAttr for std::sync::Arc<T>
    where
        T: FileAttr,
    {
        passthrough!(refs);
    }

    macro_rules! stat {
        ($( $name:ident : $ret:ty, )*) => {
            paste::paste!{$(
                #[inline]
                fn $name(&self) -> $ret {
                    self. [< st_ $name >]
                        .try_into()
                        .expect(concat!("st_", stringify!($name)))
                }
            )*}
        };
    }

    impl FileAttr for libc::stat {
        passthrough!(stat);
    }

    macro_rules! metadata_ext {
        ($( $name:ident : $ret:ty, )*) => {$(
            #[inline]
            fn $name(&self) -> $ret {
                MetadataExt::$name(self)
                    .try_into()
                    .expect(concat!("MetadataExt::", stringify!($name)))
            }
        )*};
    }

    impl FileAttr for Metadata {
        passthrough!(metadata_ext);
    }
}

/// Filesystem statistics.
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

mod impl_fs_stat {
    use super::FsStatistics;

    macro_rules! passthrough {
        ($m:ident) => {
            $m! {
                bsize: u32,
                frsize: u32,
                blocks: u64,
                bfree: u64,
                bavail: u64,
                files: u64,
                ffree: u64,
                namelen: u32,
            }
        };
    }

    macro_rules! refs {
        ($( $name:ident : $ret:ty, )*) => {$(
            #[inline]
            fn $name(&self) -> $ret {
                (**self).$name()
            }
        )*};
    }

    impl<T: ?Sized> FsStatistics for &T
    where
        T: FsStatistics,
    {
        passthrough!(refs);
    }

    impl<T: ?Sized> FsStatistics for Box<T>
    where
        T: FsStatistics,
    {
        passthrough!(refs);
    }

    impl<T: ?Sized> FsStatistics for std::rc::Rc<T>
    where
        T: FsStatistics,
    {
        passthrough!(refs);
    }

    impl<T: ?Sized> FsStatistics for std::sync::Arc<T>
    where
        T: FsStatistics,
    {
        passthrough!(refs);
    }

    impl FsStatistics for libc::statvfs {
        #[inline]
        fn bsize(&self) -> u32 {
            self.f_bsize as u32
        }

        #[inline]
        fn frsize(&self) -> u32 {
            self.f_frsize as u32
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
            self.f_namemax as u32
        }
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
    /// Create a `LockOwner` from the raw value.
    #[inline]
    pub const fn from_raw(id: u64) -> Self {
        Self(id)
    }

    /// Take the raw value of this identifier.
    #[inline]
    pub const fn into_raw(self) -> u64 {
        self.0
    }
}

/// A directory entry replied to the kernel.
#[derive(Clone, Debug)]
pub struct DirEntry {
    /// Return the inode number of this entry.
    pub ino: u64,

    /// Return the offset value of this entry.
    pub offset: u64,

    /// Return the type of this entry.
    pub typ: u32,

    /// Returns the name of this entry.
    pub name: Cow<'static, OsStr>,

    #[doc(hidden)] // non_exhaustive
    pub __non_exhaustive: NonExhaustive,
}

impl Default for DirEntry {
    #[inline]
    fn default() -> Self {
        Self {
            ino: 0,
            offset: 0,
            typ: 0,
            name: Cow::Borrowed("".as_ref()),

            __non_exhaustive: NonExhaustive::INIT,
        }
    }
}

mod non_exhaustive {
    #[derive(Copy, Clone, Debug)]
    pub struct NonExhaustive(());

    impl NonExhaustive {
        pub(crate) const INIT: Self = Self(());
    }
}
