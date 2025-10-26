mod priv_mount;
mod unpriv_mount;

use self::{priv_mount::PrivMount, unpriv_mount::UnprivMount};
use crate::Connection;
use bitflags::{bitflags, bitflags_match};
use rustix::io::Errno;
use std::{borrow::Cow, io, mem, path::Path};

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

    fn iter_flags(&self, unpriv: bool) -> impl Iterator<Item = Cow<'_, str>> + '_ {
        self.flags
            .iter()
            .flat_map(move |flag| {
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
                    MountFlags::AUTO_UNMOUNT => unpriv.then_some("auto_unmount"), // ignored
                    MountFlags::BLKDEV => unpriv.then_some("blkdev"), // handled by source/fstype
                    _ => None,
                })
            })
            .map(Cow::Borrowed)
    }

    fn iter_common_opts(&self) -> impl Iterator<Item = Cow<'_, str>> + '_ {
        std::iter::empty()
            .chain(self.blksize.map(|n| format!("blksize={}", n).into()))
            .chain(self.max_read.map(|n| format!("max_read={}", n).into()))
    }
}

#[derive(Debug)]
#[must_use]
pub struct Mount {
    kind: MountKind,
    mountpoint: Cow<'static, Path>,
    mountopts: MountOptions,
}

#[derive(Debug)]
enum MountKind {
    Priv(PrivMount),
    Unpriv(UnprivMount),
    Gone,
}

impl Mount {
    #[inline]
    pub fn is_priviledged(&self) -> bool {
        matches!(self.kind, MountKind::Priv(..))
    }

    #[inline]
    pub fn mountpoint(&self) -> &Path {
        &self.mountpoint
    }

    #[inline]
    pub fn mountopts(&self) -> &MountOptions {
        &self.mountopts
    }

    #[inline]
    pub fn unmount(mut self) -> io::Result<()> {
        self.unmount_()
    }

    fn unmount_(&mut self) -> io::Result<()> {
        match mem::replace(&mut self.kind, MountKind::Gone) {
            MountKind::Priv(mount) => mount.unmount(&self.mountpoint, &self.mountopts),
            MountKind::Unpriv(mount) => mount.unmount(&self.mountpoint, &self.mountopts),
            MountKind::Gone => {
                tracing::warn!("unmounted twice");
                Ok(())
            }
        }
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        let _ = self.unmount_();
    }
}

pub(crate) fn mount(
    mountpoint: Cow<'static, Path>,
    mountopts: MountOptions,
) -> io::Result<(Connection, Mount)> {
    tracing::debug!("Mount information:");
    tracing::debug!("  mountpoint: {:?}", mountpoint);
    tracing::debug!("  opts: {:?}", mountopts);

    if !mountpoint.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "The specified mountpoint does not exist",
        ));
    }

    let stat = rustix::fs::stat(&*mountpoint)?;

    let (fd, kind) = match PrivMount::mount(&mountpoint, &stat, &mountopts) {
        Ok((fd, mount)) => {
            tracing::trace!("use privileged mount");
            (fd, MountKind::Priv(mount))
        }
        Err(err) => match Errno::from_io_error(&err) {
            Some(Errno::PERM) => {
                tracing::warn!("The privileged mount is failed. Fallback to unprivileged mount...");
                let (fd, fusermount) = UnprivMount::mount(&mountpoint, &mountopts)?;
                (fd, MountKind::Unpriv(fusermount))
            }
            _ => return Err(err),
        },
    };

    Ok((
        Connection::from(fd),
        Mount {
            kind,
            mountpoint,
            mountopts,
        },
    ))
}
