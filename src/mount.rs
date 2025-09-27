pub mod unprivileged;

use bitflags::{bitflags, bitflags_match};
use std::{borrow::Cow, fmt, path::Path};

pub use unprivileged::mount_unprivileged;

// refs:
// * https://github.com/libfuse/libfuse/blob/fuse-3.10.5/lib/mount.c
// * https://github.com/libfuse/libfuse/blob/fuse-3.10.5/util/fusermount.c
// * https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/fs/fuse/inode.c?h=v6.15.9

bitflags! {
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct MountFlags: u64 {
        // Common mount flags (recognized in `fusermount`).

        /// Specify that the mounted filesystem is read-only.
        ///
        /// This flag is associated with the constant `MS_RDONLY`.
        const RDONLY = libc::MS_RDONLY;

        /// Specify to ignore set-user-ID / set-group-ID bits or capability
        /// flags on files in the mounted filesystem.
        ///
        /// This flag is associated with the constant `MS_NOSUID`.
        const NOSUID = libc::MS_NOSUID;

        /// Specify to disallow access to special files (such as device)
        /// on the mounted filesystem.
        ///
        /// This flag is associated with the constant `MS_NODEV`.
        const NODEV = libc::MS_NODEV;

        /// Specify to disallow programs on the mounted filesystem to be executed.
        ///
        /// This flag is associated with the constant `MS_NOEXEC`.
        const NOEXEC = libc::MS_NOEXEC;

        /// Specify that write operation to files in the mounted filesystem
        /// are performed synchronously.
        ///
        /// This flag is associated with the constant `MS_SYNCHRONOUS`.
        const SYNCHRONOUS = libc::MS_SYNCHRONOUS;

        /// Specify that modification of directories in the mounted file system
        /// are performed synchronously.
        ///
        /// This flag is associated with the constant `MS_DIRSYNC`.
        const DIRSYNC = libc::MS_DIRSYNC;

        /// Specify to disable updating access times of items on the mounted
        /// filesystems.
        ///
        /// This flag is associated with the constant `MS_NOATIME`.
        const NOATIME = libc::MS_NOATIME;

        // FUSE/fusermount-specific flags.

        /// Specify to enable the kernel side permission checks.
        ///
        /// When this flag is enabled, the FUSE kernel will perform access control
        /// based on the file mode stored in the inode cache *before* issuing the
        /// request to the daemon. Otherwise, the FUSE daemon must implement its own
        /// access control mechanism by referencing the UID/GID associated with
        /// the received request.
        const DEFAULT_PERMISSIONS = 1 << 32;

        /// Specify whether the users other than the deamon's owner can access
        /// the mounted filesystem.
        ///
        /// By default, the FUSE kernel driver restricts the access of mounted
        /// filesystem only to the owner, to prevent the side-channel attacks.
        const ALLOW_OTHER = 1 << (32 + 1);

        /// Specify that the mountpoint will be unmounted automatically
        /// when the child `fusermount` is exited.
        ///
        /// When this flag is disabled, the filesystem daemon must explicitly
        /// unmount by calling `umount(2)` (or `fusermount -u`).
        ///
        /// This flag is enabled by default.
        const AUTO_UNMOUNT = 1 << (32 + 2);

        /// Specify to use `fuseblk` as filesystem type.
        const BLKDEV = 1 << (32 + 3);
    }
}

impl Default for MountFlags {
    fn default() -> Self {
        Self::new()
    }
}

impl MountFlags {
    pub const fn new() -> Self {
        Self::AUTO_UNMOUNT
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MountOptions {
    /// The mount flags.
    pub flags: MountFlags,

    pub blksize: Option<u32>,

    pub max_read: Option<u32>,

    /// Specify the subype of this filesystem.
    pub subtype: Option<Cow<'static, str>>,

    /// Specify the name of the mounted filesystem to identify.
    pub fsname: Option<Cow<'static, str>>,

    /// Specify the path to the command `fusermount`.
    pub fusermount_path: Option<Cow<'static, Path>>,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MountOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use std::fmt::Write as _;

        let opts = std::iter::empty()
            .chain(
                self.flags
                    .iter()
                    .flat_map(|flag| {
                        bitflags_match!(flag, {
                            MountFlags::RDONLY => Some("ro"),
                            MountFlags::NOSUID => Some("nosuid"),
                            MountFlags::NODEV => Some("nodev"),
                            MountFlags::NOEXEC => Some("noexec"),
                            MountFlags::SYNCHRONOUS => Some("sync"),
                            MountFlags::DIRSYNC => Some("dirsync"),
                            MountFlags::NOATIME => Some("noatime"),
                            MountFlags::DEFAULT_PERMISSIONS => Some("default_permissions"),
                            MountFlags::ALLOW_OTHER => Some("allow_other"),
                            MountFlags::AUTO_UNMOUNT => Some("auto_unmount"),
                            MountFlags::BLKDEV => Some("blkdev"),
                            _ => None,
                        })
                    })
                    .map(Cow::Borrowed),
            )
            .chain(self.blksize.map(|n| format!("blksize={}", n).into()))
            .chain(self.max_read.map(|n| format!("max_read={}", n).into()))
            .chain(
                self.subtype
                    .as_deref()
                    .map(|s| format!("subtype={}", s).into()),
            )
            .chain(
                self.fsname
                    .as_deref()
                    .map(|fsname| Cow::Owned(format!("fsname={}", fsname))),
            );

        for (i, opts) in opts.enumerate() {
            if i > 0 {
                f.write_char(',')?;
            }
            f.write_str(&opts)?;
        }
        Ok(())
    }
}

impl MountOptions {
    pub const fn new() -> Self {
        Self {
            flags: MountFlags::new(),
            blksize: None,
            max_read: None,
            subtype: None,
            fsname: None,
            fusermount_path: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_opts_encode() {
        let opts = MountOptions::default();
        assert_eq!(opts.to_string(), "auto_unmount");

        let mut opts = MountOptions::new();
        opts.flags = MountFlags::empty();
        assert_eq!(opts.to_string(), "");

        let mut opts = MountOptions::new();
        opts.flags |= MountFlags::BLKDEV;
        opts.fsname = Some("bradbury".into());
        assert_eq!(opts.to_string(), "auto_unmount,blkdev,fsname=bradbury");

        let mut opts = MountOptions::new();
        opts.flags |= MountFlags::RDONLY
            | MountFlags::NOSUID
            | MountFlags::NODEV
            | MountFlags::NOEXEC
            | MountFlags::SYNCHRONOUS
            | MountFlags::DIRSYNC
            | MountFlags::NOATIME
            | MountFlags::DEFAULT_PERMISSIONS;
        assert_eq!(
            opts.to_string(),
            "ro,nosuid,nodev,noexec,sync,dirsync,noatime,default_permissions,auto_unmount"
        );

        let mut opts = MountOptions::new();
        opts.flags |= MountFlags::DEFAULT_PERMISSIONS | MountFlags::ALLOW_OTHER;
        opts.blksize = Some(32);
        opts.max_read = Some(11);
        assert_eq!(
            opts.to_string(),
            "default_permissions,allow_other,auto_unmount,blksize=32,max_read=11"
        );

        let mut opts = MountOptions::new();
        opts.subtype = Some("myfs".into());
        assert_eq!(opts.to_string(), "auto_unmount,subtype=myfs");

        let mut opts = MountOptions::new();
        opts.flags |= MountFlags::RDONLY | MountFlags::DEFAULT_PERMISSIONS;
        assert_eq!(opts.to_string(), "ro,default_permissions,auto_unmount");
    }
}
