use super::{MountFlags, MountOptions};
use crate::util::IteratorJoinExt as _;
use rustix::{
    io::{Errno, FdFlags},
    net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags},
};
use std::{
    borrow::Cow,
    io,
    mem::{ManuallyDrop, MaybeUninit},
    os::unix::{net::UnixStream, prelude::*},
    path::Path,
    process::{Child, Command, Stdio},
};

const FUSERMOUNT_PROG: &str = "/usr/bin/fusermount";
const FUSE_COMMFD_ENV: &str = "_FUSE_COMMFD";

fn fusermount_path(opts: &MountOptions) -> &Path {
    opts.fusermount_path
        .as_ref()
        .map_or(Path::new(FUSERMOUNT_PROG), |path| &**path)
}

/// The object that manages the mount status of the FUSE daemon.
#[derive(Debug)]
pub struct UnprivMount {
    mode: UmountMode,
}

#[derive(Debug)]
enum UmountMode {
    Implicit { child: Child, input: UnixStream },
    Explicit,
}

impl UnprivMount {
    pub(crate) fn mount(
        mountpoint: &Path,
        mountopts: &MountOptions,
    ) -> io::Result<(OwnedFd, Self)> {
        let fusermount_path = fusermount_path(mountopts);
        tracing::debug!(
            "The path to excutable `fusermount` is expanded to: {}",
            fusermount_path.display()
        );

        let mut fusermount = Command::new(fusermount_path);

        fusermount.stdin(Stdio::null());
        fusermount.stdout(Stdio::piped());
        fusermount.stderr(Stdio::piped());

        let opts = encode_unpriv_options(mountopts);
        if !opts.is_empty() {
            fusermount.arg("-o").arg(opts);
        }

        fusermount.arg("--").arg(mountpoint);

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

        let mode = if mountopts.flags.contains(MountFlags::AUTO_UNMOUNT) {
            UmountMode::Implicit { child, input }
        } else {
            // When auto_unmount is not specified, `fusermount` exits immediately
            // after sending the file descriptor and thus we need to wait until
            // the command is exited.
            let output = child.wait_with_output()?;
            if !output.status.success() {
                tracing::error!("The child `fusermount` exists with failure");
                tracing::error!("  stdout: {:?}", String::from_utf8_lossy(&output.stdout));
                tracing::error!("  stderr: {:?}", String::from_utf8_lossy(&output.stderr));
                umount(mountpoint, fusermount_path)?;
                return Err(Errno::INVAL.into());
            }

            UmountMode::Explicit
        };

        Ok((fd, UnprivMount { mode }))
    }

    pub(crate) fn unmount(self, mountpoint: &Path, mountopts: &MountOptions) -> io::Result<()> {
        match self.mode {
            UmountMode::Implicit { mut child, input } => {
                // この場合、fusermount の終了にともない umount(2) が暗黙的に呼び出される。
                // なので、fd受信用の UnixStream を閉じてバックグラウンドの fusermount を終了する。
                drop(input);
                let _st = child.wait()?;
            }
            UmountMode::Explicit => {
                // fusermount は fd を受信した直後に終了しているので、明示的に umount(2) を呼ぶ必要がある。
                // 非特権プロセスなので `fusermount -u /path/to/mountpoint` を呼ぶことで間接的にアンマウントを行う
                umount(mountpoint, fusermount_path(mountopts))?;
            }
        }
        Ok(())
    }
}

fn umount(mountpoint: &Path, fusermount_path: &Path) -> io::Result<()> {
    let _st = Command::new(fusermount_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .args(["-u", "-q", "-z", "--"])
        .arg(mountpoint)
        .status()?;
    Ok(())
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
    )?;

    let fd = cmsg_buffer
        .drain()
        .flat_map(|msg| match msg {
            RecvAncillaryMessage::ScmRights(mut fds) => fds.next(),
            _ => None,
        })
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "recv_fd"))?;

    rustix::io::fcntl_setfd(&fd, FdFlags::CLOEXEC)?;

    Ok(fd)
}

fn encode_unpriv_options(opts: &MountOptions) -> String {
    opts.iter_flags(true)
        .chain(opts.iter_common_opts())
        .chain(
            opts.subtype
                .as_deref()
                .map(|s| format!("subtype={}", s).into()),
        )
        .chain(
            opts.fsname
                .as_deref()
                .map(|fsname| Cow::Owned(format!("fsname={}", fsname))),
        )
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_opts_encode_unprivileged() {
        let opts = MountOptions::new();
        assert_eq!(encode_unpriv_options(&opts), "auto_unmount");

        let mut opts = MountOptions::new();
        opts.flags = MountFlags::empty();
        assert_eq!(encode_unpriv_options(&opts), "");

        let mut opts = MountOptions::new();
        opts.flags |= MountFlags::BLKDEV;
        opts.fsname = Some("bradbury".into());
        assert_eq!(
            encode_unpriv_options(&opts),
            "auto_unmount,blkdev,fsname=bradbury"
        );

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
            encode_unpriv_options(&opts),
            "ro,nosuid,nodev,noexec,sync,dirsync,noatime,default_permissions,auto_unmount"
        );

        let mut opts = MountOptions::new();
        opts.flags |= MountFlags::DEFAULT_PERMISSIONS | MountFlags::ALLOW_OTHER;
        opts.blksize = Some(32);
        opts.max_read = Some(11);
        assert_eq!(
            encode_unpriv_options(&opts),
            "default_permissions,allow_other,auto_unmount,blksize=32,max_read=11"
        );

        let mut opts = MountOptions::new();
        opts.subtype = Some("myfs".into());
        assert_eq!(encode_unpriv_options(&opts), "auto_unmount,subtype=myfs");

        let mut opts = MountOptions::new();
        opts.flags |= MountFlags::RDONLY | MountFlags::DEFAULT_PERMISSIONS;
        assert_eq!(
            encode_unpriv_options(&opts),
            "ro,default_permissions,auto_unmount"
        );
    }
}
