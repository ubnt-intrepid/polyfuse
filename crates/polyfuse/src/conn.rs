use libc::{c_int, c_void, iovec};
use std::{
    cmp,
    ffi::{OsStr, OsString},
    io,
    mem::{self, MaybeUninit},
    os::unix::{net::UnixDatagram, prelude::*},
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    ptr,
};

const FUSERMOUNT_PROG: &str = "/usr/bin/fusermount";
const FUSE_COMMFD_ENV: &str = "_FUSE_COMMFD";

/// A connection with the FUSE kernel driver.
#[derive(Debug)]
pub struct Connection {
    fd: RawFd,
    mountpoint: PathBuf,
}

impl Drop for Connection {
    fn drop(&mut self) {
        unmount(self.fd, &self.mountpoint);
    }
}

impl Connection {
    /// Establish a connection with the FUSE kernel driver.
    pub fn open(mountpoint: &Path, mountopts: &MountOptions) -> io::Result<Self> {
        let fd = mount(mountpoint, mountopts)?;
        Ok(Self {
            fd,
            mountpoint: mountpoint.into(),
        })
    }

    pub fn set_nonblocking(&self) -> io::Result<()> {
        let flags = syscall! { fcntl(self.as_raw_fd(), libc::F_GETFL, 0) };
        syscall! { fcntl(self.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
        Ok(())
    }

    fn read(&self, dst: &mut [u8]) -> io::Result<usize> {
        let len = syscall! {
            read(
                self.fd, //
                dst.as_mut_ptr() as *mut c_void,
                dst.len(),
            )
        };
        Ok(len as usize)
    }

    fn read_vectored(&self, dst: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        let len = syscall! {
            readv(
                self.fd, //
                dst.as_mut_ptr() as *mut iovec,
                cmp::min(dst.len(), c_int::max_value() as usize) as c_int,
            )
        };
        Ok(len as usize)
    }

    fn write(&self, src: &[u8]) -> io::Result<usize> {
        let res = syscall! {
            write(
                self.fd, //
                src.as_ptr() as *const c_void,
                src.len(),
            )
        };
        Ok(res as usize)
    }

    fn write_vectored(&self, src: &[io::IoSlice<'_>]) -> io::Result<usize> {
        let res = syscall! {
            writev(
                self.fd, //
                src.as_ptr() as *const iovec,
                cmp::min(src.len(), c_int::max_value() as usize) as c_int,
            )
        };
        Ok(res as usize)
    }
}

impl AsRawFd for Connection {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl io::Read for Connection {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (*self).read(buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        (*self).read_vectored(bufs)
    }
}

impl io::Read for &Connection {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read(buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        (**self).read_vectored(bufs)
    }
}

impl io::Write for Connection {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (*self).write(buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        (*self).write_vectored(bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Write for &Connection {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (**self).write(buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        (**self).write_vectored(bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ==== mount ====

#[derive(Debug)]
pub struct MountOptions {
    auto_unmount: bool,
    opts: Vec<String>,
    fusermount_path: Option<PathBuf>,
    fuse_comm_fd: Option<OsString>,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            auto_unmount: false,
            opts: vec![],
            fusermount_path: None,
            fuse_comm_fd: None,
        }
    }
}

impl MountOptions {
    #[doc(hidden)] // TODO: dox
    pub fn auto_unmount(&mut self, enabled: bool) -> &mut Self {
        self.auto_unmount = enabled;
        self
    }

    #[doc(hidden)] // TODO: dox
    pub fn opt(&mut self, opt: &str) -> &mut Self {
        self.opts.push(opt.to_owned());
        self
    }

    #[doc(hidden)] // TODO: dox
    pub fn opts<I>(&mut self, opts: I) -> &mut Self
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        self.opts
            .extend(opts.into_iter().map(|opt| opt.as_ref().to_owned()));
        self
    }

    #[doc(hidden)] // TODO: dox
    pub fn fusermount_path(&mut self, program: impl AsRef<OsStr>) -> &mut Self {
        let program = Path::new(program.as_ref());
        assert!(program.is_absolute());
        self.fusermount_path = Some(program.to_owned());
        self
    }

    #[doc(hidden)] // TODO: dox
    pub fn fuse_comm_fd(&mut self, name: impl AsRef<OsStr>) -> &mut Self {
        self.fuse_comm_fd = Some(name.as_ref().to_owned());
        self
    }
}

fn mount(mountpoint: &Path, mountopts: &MountOptions) -> io::Result<RawFd> {
    match unsafe { fork_with_socket_pair()? } {
        ForkResult::Child { output, .. } => {
            let writer = output.into_raw_fd();
            unsafe { libc::fcntl(writer, libc::F_SETFD, 0) };

            let mut opts = mountopts.opts.clone();
            if mountopts.auto_unmount {
                opts.push("auto_unmount".to_owned());
            }

            let opts = opts.into_iter().fold(String::new(), |mut acc, opt| {
                if !acc.is_empty() {
                    acc.push(',');
                }
                acc.push_str(&opt);
                acc
            });

            let mut fusermount = Command::new(
                mountopts
                    .fusermount_path
                    .as_deref()
                    .unwrap_or_else(|| Path::new(FUSERMOUNT_PROG)),
            );
            fusermount.env(
                mountopts
                    .fuse_comm_fd
                    .as_deref()
                    .unwrap_or_else(|| OsStr::new(FUSE_COMMFD_ENV)),
                writer.to_string(),
            );
            fusermount.arg("-o").arg(opts);
            fusermount.arg("--").arg(mountpoint);

            Err(fusermount.exec())
        }

        ForkResult::Parent {
            child_pid, input, ..
        } => {
            // Check if the `fusermount` command is started successfully.
            let mut status = 0;
            syscall! { waitpid(child_pid, &mut status, 0) };
            let status = ExitStatus::from_raw(status);

            if !status.success() {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("fusermount failed with: {}", status),
                ));
            }

            let fd = receive_fd(&input)?;

            // Unmounting is executed when `reader` is dropped and the connection
            // with `fusermount` is closed.
            let _ = input.into_raw_fd();

            Ok(fd)
        }
    }
}

fn unmount(fd: RawFd, mountpoint: &Path) {
    unsafe {
        libc::close(fd);
    }

    let _ = Command::new(FUSERMOUNT_PROG)
        .args(&["-u", "-q", "-z", "--"])
        .arg(&mountpoint)
        .status();
}

fn receive_fd(reader: &UnixDatagram) -> io::Result<RawFd> {
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

// ==== util ====

enum ForkResult {
    Parent {
        child_pid: c_int,
        input: UnixDatagram,
    },
    Child {
        output: UnixDatagram,
    },
}

unsafe fn fork_with_socket_pair() -> io::Result<ForkResult> {
    let (input, output) = UnixDatagram::pair()?;

    let pid = syscall! { fork() };
    if pid == 0 {
        drop(input);

        Ok(ForkResult::Child { output })
    } else {
        drop(output);

        Ok(ForkResult::Parent {
            child_pid: pid,
            input,
        })
    }
}