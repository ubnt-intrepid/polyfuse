use libc::{c_int, c_void};
use std::{
    ffi::OsStr,
    io,
    mem::{self, MaybeUninit},
    os::unix::{net::UnixDatagram, prelude::*},
    path::Path,
    process::{Command, ExitStatus},
    ptr,
};

const FUSERMOUNT_PROG: &str = "fusermount";
const FUSE_COMMFD_ENV: &str = "_FUSE_COMMFD";

pub fn mount(mountpoint: &Path, mountopts: &[&OsStr]) -> io::Result<RawFd> {
    let (pid, mut reader) = exec_fusermount(mountpoint, mountopts)?;

    // Check if the `fusermount` command is started successfully.
    let mut status = 0;
    syscall! { waitpid(pid, &mut status, 0) };
    let status = ExitStatus::from_raw(status);
    if !status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("fusermount failed with: {}", status),
        ));
    }

    let fd = receive_fd(&mut reader)?;

    // Unmounting is executed when `reader` is dropped and the connection
    // with `fusermount` is closed.
    let _ = reader.into_raw_fd();

    Ok(fd)
}

pub(crate) fn unmount(fd: RawFd, mountpoint: &Path) {
    unsafe {
        libc::close(fd);
    }

    let _ = Command::new(FUSERMOUNT_PROG)
        .args(&["-u", "-q", "-z", "--"])
        .arg(&mountpoint)
        .status();
}

fn exec_fusermount(mountpoint: &Path, mountopts: &[&OsStr]) -> io::Result<(c_int, UnixDatagram)> {
    let (reader, writer) = UnixDatagram::pair()?;

    let pid = syscall! { fork() };
    if pid == 0 {
        drop(reader);
        let writer = writer.into_raw_fd();
        unsafe { libc::fcntl(writer, libc::F_SETFD, 0) };

        let mut fusermount = Command::new(FUSERMOUNT_PROG);
        fusermount.env(FUSE_COMMFD_ENV, writer.to_string());
        fusermount.args(mountopts);
        fusermount.arg("--").arg(mountpoint);

        return Err(fusermount.exec());
    }

    Ok((pid, reader))
}

fn receive_fd(reader: &mut UnixDatagram) -> io::Result<RawFd> {
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

    Ok(cmsg.fd)
}
