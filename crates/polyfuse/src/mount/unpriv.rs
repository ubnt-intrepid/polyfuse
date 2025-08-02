//! The implementation of unprivileged mount using `fusermount`.

use crate::{
    nix::{self, ForkResult},
    MountOptions,
};
use libc::{c_int, c_void};
use std::{
    ffi::OsStr,
    io,
    mem::{self, MaybeUninit},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::{net::UnixStream, prelude::*},
    },
    path::Path,
    process::{Command, ExitStatus},
    ptr,
};

const FUSERMOUNT_PROG: &str = "/usr/bin/fusermount";
const FUSE_COMMFD_ENV: &str = "_FUSE_COMMFD";

fn fusermount_path(opts: &MountOptions) -> &Path {
    opts.fusermount_path
        .as_deref()
        .unwrap_or_else(|| Path::new(FUSERMOUNT_PROG))
}

#[derive(Debug)]
pub struct Fusermount {
    pid: c_int,
    input: Option<UnixStream>,
}

impl Fusermount {
    fn new(pid: c_int, input: UnixStream) -> Self {
        Self {
            pid,
            input: Some(input),
        }
    }

    pub fn wait(mut self) -> io::Result<ExitStatus> {
        self.wait_()
    }

    fn wait_(&mut self) -> io::Result<ExitStatus> {
        drop(self.input.take());
        let mut status = 0;
        syscall! { waitpid(self.pid, &mut status, 0) };
        self.pid = -1;
        Ok(ExitStatus::from_raw(status))
    }
}

impl Drop for Fusermount {
    fn drop(&mut self) {
        let _ = self.wait_();
    }
}

pub fn mount(
    mountpoint: &Path,
    mountopts: &MountOptions,
) -> io::Result<(OwnedFd, Option<Fusermount>)> {
    let (input, output) = UnixStream::pair()?;

    let mut fusermount = Command::new(fusermount_path(mountopts));

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

    fusermount.arg("--").arg(mountpoint);

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

            let mut child = Some(Fusermount::new(child_pid, input));
            if !mountopts.auto_unmount {
                // When auto_unmount is not specified, `fusermount` exits immediately
                // after sending the file descriptor and thus we need to wait until
                // the command is exited.
                let child = child.take().unwrap();
                let _st = child.wait()?;
            }

            Ok((fd, child))
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

pub fn unmount(mountpoint: &Path, mountopts: &MountOptions) -> io::Result<()> {
    let _st = Command::new(fusermount_path(mountopts))
        .args(&["-u", "-q", "-z", "--"])
        .arg(mountpoint)
        .status()?;
    Ok(())
}
