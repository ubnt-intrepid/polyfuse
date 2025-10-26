mod priv_mount;
mod unpriv_mount;

use self::{priv_mount::PrivMount, unpriv_mount::UnprivMount};
use crate::Connection;
use bitflags::{bitflags, bitflags_match};
use rustix::{io::Errno, mount::MountFlags};
use std::{borrow::Cow, io, mem, path::Path};

// refs:
// * https://github.com/libfuse/libfuse/blob/fuse-3.10.5/lib/mount.c
// * https://github.com/libfuse/libfuse/blob/fuse-3.10.5/util/fusermount.c
// * https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/fs/fuse/inode.c?h=v6.15.9

// mount(2) flags recognized in FUSE/fusermount.
const ALLOWED_MOUNT_FLAGS: MountFlags = MountFlags::empty()
    .union(MountFlags::RDONLY)
    .union(MountFlags::NOSUID)
    .union(MountFlags::NODEV)
    .union(MountFlags::NOEXEC)
    .union(MountFlags::SYNCHRONOUS)
    .union(MountFlags::DIRSYNC)
    .union(MountFlags::NOATIME);

bitflags! {
    /// FUSE/fusermount-specific mount flags.
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct FuseFlags: u32 {
        /// Specify to enable the kernel side permission checks.
        ///
        /// When this flag is enabled, the FUSE kernel will perform access control
        /// based on the file mode stored in the inode cache *before* issuing the
        /// request to the daemon. Otherwise, the FUSE daemon must implement its own
        /// access control mechanism by referencing the UID/GID associated with
        /// the received request.
        const DEFAULT_PERMISSIONS = 1 << 0;

        /// Specify whether the users other than the deamon's owner can access
        /// the mounted filesystem.
        ///
        /// By default, the FUSE kernel driver restricts the access of mounted
        /// filesystem only to the owner, to prevent the side-channel attacks.
        const ALLOW_OTHER = 1 << 1;

        /// Specify that the mountpoint will be unmounted automatically
        /// when the child `fusermount` is exited.
        ///
        /// When this flag is disabled, the filesystem daemon must explicitly
        /// unmount by calling `umount(2)` (or `fusermount -u`).
        ///
        /// This flag is enabled by default.
        const AUTO_UNMOUNT = 1 << 2;

        /// Specify to use `fuseblk` as filesystem type.
        const BLKDEV = 1 << 3;
    }
}

impl Default for FuseFlags {
    fn default() -> Self {
        Self::new()
    }
}

impl FuseFlags {
    pub const fn new() -> Self {
        Self::AUTO_UNMOUNT
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct MountOptions {
    /// The flags for mount(2) syscall.
    pub mount_flags: MountFlags,

    /// The FUSE-specific mount flags.
    pub fuse_flags: FuseFlags,

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
            mount_flags: MountFlags::empty(),
            fuse_flags: FuseFlags::new(),
            blksize: None,
            max_read: None,
            subtype: None,
            fsname: None,
            fusermount_path: None,
        }
    }

    fn iter_flags(&self, unpriv: bool) -> impl Iterator<Item = Cow<'_, str>> + '_ {
        let mount_flags = self.mount_flags.iter().flat_map(|flag| {
            bitflags_match!(flag, {
                MountFlags::RDONLY => Some("ro"),
                MountFlags::NOSUID => Some("nosuid"),
                MountFlags::NODEV => Some("nodev"),
                MountFlags::NOEXEC => Some("noexec"),
                MountFlags::SYNCHRONOUS => Some("sync"),
                MountFlags::DIRSYNC => Some("dirsync"),
                MountFlags::NOATIME => Some("noatime"),
                _ => None,
            })
        });
        let fuse_flags = self.fuse_flags.iter().flat_map(move |flag| {
            bitflags_match!(flag, {
                FuseFlags::DEFAULT_PERMISSIONS => Some("default_permissions"),
                FuseFlags::ALLOW_OTHER => Some("allow_other"),
                FuseFlags::AUTO_UNMOUNT => unpriv.then_some("auto_unmount"), // ignored
                FuseFlags::BLKDEV => unpriv.then_some("blkdev"), // handled by source/fstype
                _ => None,
            })
        });
        mount_flags.chain(fuse_flags).map(Cow::Borrowed)
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
    mut mountopts: MountOptions,
) -> io::Result<(Connection, Mount)> {
    mountopts.mount_flags &= ALLOWED_MOUNT_FLAGS;

    tracing::debug!("Mount information:");
    tracing::debug!("  mountpoint: {:?}", mountpoint);
    tracing::debug!("  opts: {:?}", mountopts);

    if !mountpoint.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "The specified mountpoint does not exist",
        ));
    }

    let (fd, kind) = match PrivMount::mount(&mountpoint, &mountopts) {
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
