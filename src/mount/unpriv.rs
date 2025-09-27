use super::{MountFlags, MountOptions};
use crate::conn::Connection;
use rustix::{
    io::{Errno, FdFlags},
    net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags},
};
use std::{
    borrow::Cow,
    io,
    mem::{self, ManuallyDrop, MaybeUninit},
    os::unix::{net::UnixStream, prelude::*},
    path::Path,
    process::{Child, Command, Stdio},
};

const FUSERMOUNT_PROG: &str = "/usr/bin/fusermount";
const FUSE_COMMFD_ENV: &str = "_FUSE_COMMFD";

/// The object that manages the mount status of the FUSE daemon.
#[derive(Debug)]
pub struct Fusermount {
    mode: UmountMode,
}

#[derive(Debug)]
enum UmountMode {
    Implicit {
        child: Child,
        input: UnixStream,
    },
    Explicit {
        mountpoint: Cow<'static, Path>,
        fusermount_path: Cow<'static, Path>,
    },
    Gone,
}

impl Fusermount {
    /// Unmount the filesystem.
    pub fn unmount(mut self) -> io::Result<()> {
        self.unmount_()
    }

    fn unmount_(&mut self) -> io::Result<()> {
        match mem::replace(&mut self.mode, UmountMode::Gone) {
            UmountMode::Implicit { mut child, input } => {
                // この場合、fusermount の終了にともない umount(2) が暗黙的に呼び出される。
                // なので、fd受信用の UnixStream を閉じてバックグラウンドの fusermount を終了する。
                drop(input);
                let _st = child.wait()?;
            }
            UmountMode::Explicit {
                mountpoint,
                fusermount_path,
            } => {
                // fusermount は fd を受信した直後に終了しているので、明示的に umount(2) を呼ぶ必要がある。
                // 非特権プロセスなので `fusermount -u /path/to/mountpoint` を呼ぶことで間接的にアンマウントを行う
                umount(&mountpoint, &fusermount_path)?;
            }
            UmountMode::Gone => (),
        }
        Ok(())
    }
}

impl Drop for Fusermount {
    fn drop(&mut self) {
        let _ = self.unmount_();
    }
}

/// Establish a connection with the FUSE kernel driver in non-privileged mode.
pub fn mount_unpriv(
    mountpoint: Cow<'static, Path>,
    mountopts: &MountOptions,
) -> io::Result<(Connection, Fusermount)> {
    tracing::debug!("Mount information:");
    tracing::debug!("  mountpoint: {:?}", mountpoint);
    tracing::debug!("  opts: {:?}", mountopts);

    if !mountpoint.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "The specified mountpoint is not exist",
        ));
    }

    let fusermount_path = mountopts
        .fusermount_path
        .clone()
        .unwrap_or(Path::new(FUSERMOUNT_PROG).into());

    let mut fusermount = Command::new(&*fusermount_path);

    fusermount.stdin(Stdio::null());
    fusermount.stdout(Stdio::piped());
    fusermount.stderr(Stdio::piped());

    let opts = mountopts.to_string();
    if !opts.is_empty() {
        fusermount.arg("-o").arg(opts);
    }

    fusermount.arg("--").arg(&*mountpoint);

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
            umount(&mountpoint, &fusermount_path)?;
            return Err(Errno::INVAL.into());
        }

        UmountMode::Explicit {
            mountpoint: mountpoint.clone(),
            fusermount_path,
        }
    };

    Ok((Connection::from(fd), Fusermount { mode }))
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
