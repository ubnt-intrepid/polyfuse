use super::conn::Connection;
use rustix::{
    io::FdFlags,
    net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags},
};
use std::{
    borrow::Cow,
    fmt, io,
    mem::{ManuallyDrop, MaybeUninit},
    os::{
        fd::OwnedFd,
        unix::{net::UnixStream, prelude::*},
    },
    path::{Path, PathBuf},
    process::{Child, Command},
};

// refs:
// * https://github.com/libfuse/libfuse/blob/fuse-3.10.5/lib/mount.c
// * https://github.com/libfuse/libfuse/blob/fuse-3.10.5/util/fusermount.c
// * https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/fs/fuse/inode.c?h=v6.15.9

const FUSERMOUNT_PROG: &str = "/usr/bin/fusermount";
const FUSE_COMMFD_ENV: &str = "_FUSE_COMMFD";

#[derive(Debug, Clone)]
pub struct MountOptions {
    // Common mount flags (recognized in `fusermount`).
    /// Specify that the mounted filesystem is read-only.
    ///
    /// This flag is associated with the constant `MS_RDONLY`.
    pub ro: bool,

    /// Specify to ignore set-user-ID / set-group-ID bits or capability
    /// flags on files in the mounted filesystem.
    ///
    /// This flag is associated with the constant `MS_NOSUID`.
    pub nosuid: bool,

    /// Specify to disallow access to special files (such as device)
    /// on the mounted filesystem.
    ///
    /// This flag is associated with the constant `MS_NODEV`.
    pub nodev: bool,

    /// Specify to disallow programs on the mounted filesystem to be executed.
    ///
    /// This flag is associated with the constant `MS_NOEXEC`.
    pub noexec: bool,

    /// Specify that write operation to files in the mounted filesystem
    /// are performed synchronously.
    ///
    /// This flag is associated with the constant `MS_SYNCHRONOUS`.
    pub sync: bool,

    /// Specify that modification of directories in the mounted file system
    /// are performed synchronously.
    ///
    /// This flag is associated with the constant `MS_DIRSYNC`.
    pub dirsync: bool,

    /// Specify to disable updating access times of items on the mounted
    /// filesystems.
    ///
    /// This flag is associated with the constant `MS_NOATIME`.
    pub noatime: bool,

    // FUSE-specific options
    pub default_permissions: bool,
    pub allow_other: bool,
    pub blksize: Option<u32>,
    pub max_read: Option<u32>,

    /// Specify the subype of this filesystem.
    pub subtype: Option<String>,

    // fusermount-specific options
    /// Specify that the mountpoint will be unmounted automatically
    /// when the child `fusermount` is exited.
    ///
    /// When this flag is disabled, the filesystem daemon must explicitly
    /// unmount by calling `umount(2)` (or calling `fusermount -u`).
    ///
    /// This flag is enabled by default.
    pub auto_unmount: bool,

    /// Specify to use `fuseblk` as filesystem type.
    pub blkdev: bool,

    /// Specify the name of the mounted filesystem to identify.
    pub fsname: Option<String>,

    /// Specify the path to the command `fusermount`.
    pub fusermount_path: Option<PathBuf>,

    #[doc(hidden)]
    pub _dummy: (),
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            ro: false,
            nosuid: false,
            nodev: false,
            noexec: false,
            sync: false,
            dirsync: false,
            noatime: false,
            default_permissions: false,
            allow_other: false,
            blksize: None,
            max_read: None,
            subtype: None,
            auto_unmount: true,
            blkdev: false,
            fsname: None,
            fusermount_path: None,
            _dummy: (),
        }
    }
}

impl fmt::Display for MountOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use std::fmt::Write as _;

        let opts = std::iter::empty()
            .chain(self.ro.then_some("ro".into()))
            .chain(self.nosuid.then_some("nosuid".into()))
            .chain(self.nodev.then_some("nodev".into()))
            .chain(self.noexec.then_some("noexec".into()))
            .chain(self.sync.then_some("sync".into()))
            .chain(self.dirsync.then_some("dirsync".into()))
            .chain(self.noatime.then_some("noatime".into()))
            .chain(
                self.default_permissions
                    .then_some(Cow::Borrowed("default_permissions")),
            )
            .chain(self.allow_other.then_some(Cow::Borrowed("allow_other")))
            .chain(self.blksize.map(|n| format!("blksize={}", n).into()))
            .chain(self.max_read.map(|n| format!("max_read={}", n).into()))
            .chain(
                self.subtype
                    .as_deref()
                    .map(|s| format!("subtype={}", s).into()),
            )
            .chain(self.auto_unmount.then_some(Cow::Borrowed("auto_unmount")))
            .chain(self.blkdev.then_some(Cow::Borrowed("blkdev")))
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

fn fusermount_path(opts: &MountOptions) -> &Path {
    opts.fusermount_path
        .as_deref()
        .unwrap_or_else(|| Path::new(FUSERMOUNT_PROG))
}

#[derive(Debug)]
pub struct Fusermount {
    child: Option<PipedChild>,
    mountpoint: PathBuf,
    mountopts: MountOptions,
}

impl Fusermount {
    pub fn unmount(mut self) -> io::Result<()> {
        self.unmount_()
    }

    fn unmount_(&mut self) -> io::Result<()> {
        if let Some(child) = self.child.take() {
            // この場合、fusermount の終了にともない umount(2) が暗黙的に呼び出される。
            // なので、fd受信用の UnixStream を閉じてバックグラウンドの fusermount を終了する。
            child.wait()?;
        } else {
            // fusermount は fd を受信した直後に終了しているので、明示的に umount(2) を呼ぶ必要がある。
            // 非特権プロセスなので `fusermount -u /path/to/mountpoint` を呼ぶことで間接的にアンマウントを行う
            unmount(&self.mountpoint, &self.mountopts)?;
        }
        Ok(())
    }
}

impl Drop for Fusermount {
    fn drop(&mut self) {
        let _ = self.unmount_();
    }
}

#[derive(Debug)]
struct PipedChild {
    child: Child,
    input: UnixStream,
}

impl PipedChild {
    fn wait(mut self) -> io::Result<()> {
        drop(self.input);
        let _st = self.child.wait()?;
        Ok(())
    }
}

/// Acquire the connection to the FUSE kernel driver associated with the specified mountpoint.
pub fn mount(mountpoint: PathBuf, mountopts: MountOptions) -> io::Result<(Connection, Fusermount)> {
    tracing::debug!("Mount information:");
    tracing::debug!("  mountpoint: {:?}", mountpoint);
    tracing::debug!("  opts: {:?}", mountopts);

    let mut fusermount = Command::new(fusermount_path(&mountopts));

    let opts = mountopts.to_string();
    if !opts.is_empty() {
        fusermount.arg("-o").arg(opts);
    }

    fusermount.arg("--").arg(&mountpoint);

    let (input, output) = UnixStream::pair()?;

    fusermount.env(FUSE_COMMFD_ENV, output.as_raw_fd().to_string());

    unsafe {
        let output = ManuallyDrop::new(output);
        fusermount.pre_exec(move || {
            rustix::io::fcntl_setfd(&*output, FdFlags::empty()).map_err(Into::into)
        });
    }

    let child = fusermount.spawn()?;

    let fd = receive_fd(&input)?;

    let mut child = Some(PipedChild { child, input });
    if !mountopts.auto_unmount {
        // When auto_unmount is not specified, `fusermount` exits immediately
        // after sending the file descriptor and thus we need to wait until
        // the command is exited.
        let child = child.take().unwrap();
        child.wait()?;
    }

    Ok((
        Connection::from(fd),
        Fusermount {
            child,
            mountpoint,
            mountopts,
        },
    ))
}

fn receive_fd(reader: &UnixStream) -> io::Result<OwnedFd> {
    let mut buf = [0u8; 1];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut cmsg_buffer = RecvAncillaryBuffer::new(&mut space);

    let _msg = rustix::net::recvmsg(
        reader,
        &mut [io::IoSliceMut::new(&mut buf[..])],
        &mut cmsg_buffer,
        RecvFlags::empty(),
    );

    let fd = cmsg_buffer
        .drain()
        .flat_map(|msg| match msg {
            RecvAncillaryMessage::ScmRights(fds) => Some(fds),
            _ => None,
        })
        .flatten()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "recv_fd"))?;

    rustix::io::fcntl_setfd(&fd, FdFlags::CLOEXEC)?;

    Ok(fd)
}

fn unmount(mountpoint: &Path, mountopts: &MountOptions) -> io::Result<()> {
    let _st = Command::new(fusermount_path(mountopts))
        .args(["-u", "-q", "-z", "--"])
        .arg(mountpoint)
        .status()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_opts_encode() {
        let opts = MountOptions::default();
        assert_eq!(opts.to_string(), "auto_unmount");

        let opts = MountOptions {
            auto_unmount: false,
            ..Default::default()
        };
        assert_eq!(opts.to_string(), "");

        let opts = MountOptions {
            blkdev: true,
            fsname: Some("bradbury".into()),
            ..Default::default()
        };
        assert_eq!(opts.to_string(), "auto_unmount,blkdev,fsname=bradbury");

        let opts = MountOptions {
            ro: true,
            nosuid: true,
            nodev: true,
            noexec: true,
            sync: true,
            dirsync: true,
            noatime: true,
            default_permissions: true,
            ..Default::default()
        };
        assert_eq!(
            opts.to_string(),
            "ro,nosuid,nodev,noexec,sync,dirsync,noatime,default_permissions,auto_unmount"
        );

        let opts = MountOptions {
            default_permissions: true,
            allow_other: true,
            blksize: Some(32),
            max_read: Some(11),
            ..Default::default()
        };
        assert_eq!(
            opts.to_string(),
            "default_permissions,allow_other,blksize=32,max_read=11,auto_unmount"
        );

        let opts = MountOptions {
            subtype: Some("myfs".into()),
            ..Default::default()
        };
        assert_eq!(opts.to_string(), "subtype=myfs,auto_unmount");

        let opts = MountOptions {
            ro: true,
            default_permissions: true,
            ..Default::default()
        };
        assert_eq!(opts.to_string(), "ro,default_permissions,auto_unmount");
    }
}
