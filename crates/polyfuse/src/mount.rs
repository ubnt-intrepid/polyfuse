use libc::{c_int, c_void};
use std::{
    ffi::{OsStr, OsString},
    io,
    mem::{self, MaybeUninit},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::{net::UnixStream, prelude::*},
    },
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    ptr,
};

use crate::nix::{self, ForkResult};

const FUSERMOUNT_PROG: &str = "/usr/bin/fusermount";
const FUSE_COMMFD_ENV: &str = "_FUSE_COMMFD";

#[derive(Debug, Clone)]
pub struct MountOptions {
    options: Vec<String>,
    auto_unmount: bool,
    fusermount_path: Option<PathBuf>,
    fuse_comm_fd: Option<OsString>,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            options: vec![],
            auto_unmount: true,
            fusermount_path: None,
            fuse_comm_fd: None,
        }
    }
}

impl MountOptions {
    pub fn auto_unmount(&mut self, enabled: bool) -> &mut Self {
        self.auto_unmount = enabled;
        self
    }

    pub fn mount_option(&mut self, option: &str) -> &mut Self {
        for option in option.split(',').map(|s| s.trim()) {
            match option {
                "auto_unmount" => {
                    self.auto_unmount(true);
                }
                option => self.options.push(option.to_owned()),
            }
        }
        self
    }

    pub fn fusermount_path(&mut self, program: impl AsRef<OsStr>) -> &mut Self {
        let program = Path::new(program.as_ref());
        assert!(
            program.is_absolute(),
            "the binary path to `fusermount` must be absolute."
        );
        self.fusermount_path = Some(program.to_owned());
        self
    }

    pub fn fuse_comm_fd(&mut self, name: impl AsRef<OsStr>) -> &mut Self {
        self.fuse_comm_fd = Some(name.as_ref().to_owned());
        self
    }
}

fn fusermount_path(opts: &MountOptions) -> &Path {
    opts.fusermount_path
        .as_deref()
        .unwrap_or_else(|| Path::new(FUSERMOUNT_PROG))
}

#[derive(Debug)]
pub struct Fusermount {
    inner: Option<FusermountInner>,
    mountpoint: PathBuf,
    mountopts: MountOptions,
}

impl Fusermount {
    pub fn unmount(mut self) -> io::Result<()> {
        self.unmount_()
    }

    fn unmount_(&mut self) -> io::Result<()> {
        if let Some(fusermount) = self.inner.take() {
            // この場合、fusermount の終了にともない umount(2) が暗黙的に呼び出される。
            // なので、fd受信用の UnixStream を閉じてバックグラウンドの fusermount を終了する。
            let _st = fusermount.wait()?;
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
struct FusermountInner {
    pid: c_int,
    input: UnixStream,
}

impl FusermountInner {
    fn wait(mut self) -> io::Result<ExitStatus> {
        drop(self.input);
        let mut status = 0;
        syscall! { waitpid(self.pid, &mut status, 0) };
        self.pid = -1;
        Ok(ExitStatus::from_raw(status))
    }
}

/// Acquire the connection to the FUSE kernel driver associated with the specified mountpoint.
pub fn mount(mountpoint: PathBuf, mountopts: MountOptions) -> io::Result<(OwnedFd, Fusermount)> {
    let (input, output) = UnixStream::pair()?;

    let mut fusermount = Command::new(fusermount_path(&mountopts));

    let opts = mountopts
        .options
        .iter()
        .map(|opt| opt.as_str())
        .chain(if mountopts.auto_unmount {
            Some("auto_unmount")
        } else {
            None
        })
        .fold(String::new(), |mut opts, opt| {
            if !opts.is_empty() {
                opts.push(',');
            }
            opts.push_str(&opt);
            opts
        });
    if !opts.is_empty() {
        fusermount.arg("-o").arg(opts);
    }

    fusermount.arg("--").arg(&mountpoint);

    fusermount.env(
        mountopts
            .fuse_comm_fd
            .as_deref()
            .unwrap_or_else(|| OsStr::new(FUSE_COMMFD_ENV)),
        output.as_raw_fd().to_string(),
    );

    match unsafe { nix::fork()? } {
        ForkResult::Child => {
            // Only async-signal-safe functions are allowed to call here.
            // in a multi threaded situation.

            let output = output.into_raw_fd();
            unsafe { libc::fcntl(output, libc::F_SETFD, 0) };

            // Assumes that the UnixStream destructor only calls close(2).
            drop(input);

            let _err = fusermount.exec();

            // Exit immediately since the process may be in a "broken state".
            // https://doc.rust-lang.org/stable/std/os/unix/process/trait.CommandExt.html#notes
            unsafe {
                libc::_exit(1);
            }
        }

        ForkResult::Parent { child_pid, .. } => {
            drop(output);

            let fd = receive_fd(&input)?;

            let mut child = Some(FusermountInner {
                pid: child_pid,
                input,
            });
            if !mountopts.auto_unmount {
                // When auto_unmount is not specified, `fusermount` exits immediately
                // after sending the file descriptor and thus we need to wait until
                // the command is exited.
                let child = child.take().unwrap();
                let _st = child.wait()?;
            }

            Ok((
                fd,
                Fusermount {
                    inner: child,
                    mountpoint,
                    mountopts,
                },
            ))
        }
    }
}

fn receive_fd(reader: &UnixStream) -> io::Result<OwnedFd> {
    let mut buf = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr() as *mut c_void,
        iov_len: 1,
    };

    #[repr(C)]
    struct Cmsg {
        header: libc::cmsghdr,
        fd: c_int,
    }
    let mut cmsg = MaybeUninit::<Cmsg>::uninit();

    let mut msg = libc::msghdr {
        msg_name: ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: cmsg.as_mut_ptr() as *mut c_void,
        msg_controllen: mem::size_of_val(&cmsg),
        msg_flags: 0,
    };

    syscall! { recvmsg(reader.as_raw_fd(), &mut msg, 0) };

    if msg.msg_controllen < mem::size_of_val(&cmsg) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "too short control message length",
        ));
    }
    let cmsg = unsafe { cmsg.assume_init() };

    if cmsg.header.cmsg_type != libc::SCM_RIGHTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "got control message with unknown type",
        ));
    }

    let fd = cmsg.fd;
    syscall! { fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };

    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn unmount(mountpoint: &Path, mountopts: &MountOptions) -> io::Result<()> {
    let _st = Command::new(fusermount_path(mountopts))
        .args(&["-u", "-q", "-z", "--"])
        .arg(mountpoint)
        .status()?;
    Ok(())
}
