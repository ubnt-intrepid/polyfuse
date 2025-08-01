use libc::{c_int, c_void, iovec};
use polyfuse_kernel::FUSE_DEV_IOC_CLONE;
use std::{
    cmp,
    ffi::{CStr, OsStr, OsString},
    io,
    mem::{self, MaybeUninit},
    os::unix::{net::UnixStream, prelude::*},
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    ptr,
};

const FUSERMOUNT_PROG: &str = "/usr/bin/fusermount";
const FUSE_COMMFD_ENV: &str = "_FUSE_COMMFD";
const FUSE_DEV_NAME: &CStr = c"/dev/fuse";

macro_rules! syscall {
    ($fn:ident ( $($arg:expr),* $(,)* ) ) => {{
        #[allow(unused_unsafe)]
        let res = unsafe { libc::$fn($($arg),*) };
        if res == -1 {
            return Err(std::io::Error::last_os_error());
        }
        res
    }};
}

/// A connection with the FUSE kernel driver.
#[derive(Debug)]
pub struct Connection {
    fd: OwnedFd,
    _fusermount: Option<Fusermount>,
    _mountpoint: PathBuf,
    _mountopts: MountOptions,
}

impl Connection {
    pub fn try_ioc_clone(&self) -> io::Result<ClonedConnection> {
        Ok(ClonedConnection {
            fd: fuse_ioc_clone(&self.fd)?,
        })
    }
}

impl AsFd for Connection {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl AsRawFd for Connection {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl io::Read for Connection {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        read(&self.fd, buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        read_vectored(&self.fd, bufs)
    }
}

impl io::Write for Connection {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        write(&self.fd, buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        write_vectored(&self.fd, bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Read for &Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        read(&self.fd, buf)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        read_vectored(&self.fd, bufs)
    }
}

impl io::Write for &Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        write(&self.fd, buf)
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        write_vectored(&self.fd, bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct ClonedConnection {
    fd: OwnedFd,
}

impl ClonedConnection {
    pub fn try_ioc_clone(&self) -> io::Result<Self> {
        Ok(Self {
            fd: fuse_ioc_clone(&self.fd)?,
        })
    }
}

impl AsFd for ClonedConnection {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl AsRawFd for ClonedConnection {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl io::Read for ClonedConnection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        read(&self.fd, buf)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        read_vectored(&self.fd, bufs)
    }
}

impl io::Write for ClonedConnection {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        write(&self.fd, buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        write_vectored(&self.fd, bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn fuse_ioc_clone(fd: &impl AsRawFd) -> io::Result<OwnedFd> {
    let newfd = syscall! { open(FUSE_DEV_NAME.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    syscall! { ioctl(newfd, FUSE_DEV_IOC_CLONE, &fd.as_raw_fd()) };
    Ok(unsafe { OwnedFd::from_raw_fd(newfd) })
}

// ==== mount ====

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

    pub fn mount(&self, mountpoint: impl Into<PathBuf>) -> io::Result<Connection> {
        let mountpoint = mountpoint.into();
        let (fd, _fusermount) = fusermount(&mountpoint, self)?;
        Ok(Connection {
            fd,
            _fusermount,
            _mountpoint: mountpoint,
            _mountopts: self.clone(),
        })
    }
}

#[derive(Debug)]
struct Fusermount {
    pid: c_int,
    input: Option<UnixStream>,
    mountpoint: PathBuf,
}

impl Fusermount {
    fn wait(&mut self) -> io::Result<ExitStatus> {
        drop(self.input.take());
        let mut status = 0;
        syscall! { waitpid(self.pid, &mut status, 0) };
        Ok(ExitStatus::from_raw(status))
    }

    fn unmount(&mut self) -> io::Result<()> {
        let _st = Command::new(FUSERMOUNT_PROG)
            .args(&["-u", "-q", "-z", "--"])
            .arg(&self.mountpoint)
            .status()?;
        Ok(())
    }
}

impl Drop for Fusermount {
    fn drop(&mut self) {
        let _ = self.wait();
        let _ = self.unmount();
    }
}

fn fusermount(
    mountpoint: &Path,
    mountopts: &MountOptions,
) -> io::Result<(OwnedFd, Option<Fusermount>)> {
    let (input, output) = UnixStream::pair()?;

    let mut fusermount = Command::new(
        mountopts
            .fusermount_path
            .as_deref()
            .unwrap_or_else(|| Path::new(FUSERMOUNT_PROG)),
    );

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

    match unsafe { fork()? } {
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

            let mut child = Some(Fusermount {
                pid: child_pid,
                input: Some(input),
                mountpoint: mountpoint.to_owned(),
            });

            if !mountopts.auto_unmount {
                // When auto_unmount is not specified, `fusermount` exits immediately
                // after sending the file descriptor and thus we need to wait until
                // the command is exited.
                let mut child = child.take().unwrap();
                let _st = child.wait();
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

// ==== util ====

fn read(fd: &impl AsRawFd, buf: &mut [u8]) -> io::Result<usize> {
    let len = syscall! {
        read(
            fd.as_raw_fd(), //
            buf.as_mut_ptr() as *mut c_void,
            buf.len(),
        )
    };
    Ok(len as usize)
}

fn read_vectored(fd: &impl AsRawFd, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
    let len = syscall! {
        readv(
            fd.as_raw_fd(), //
            bufs.as_mut_ptr() as *mut iovec,
            cmp::min(bufs.len(), c_int::max_value() as usize) as c_int,
        )
    };
    Ok(len as usize)
}

fn write(fd: &impl AsRawFd, buf: &[u8]) -> io::Result<usize> {
    let res = syscall! {
        write(
            fd.as_raw_fd(), //
            buf.as_ptr() as *const c_void,
            buf.len(),
        )
    };
    Ok(res as usize)
}

fn write_vectored(fd: &impl AsRawFd, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
    let res = syscall! {
        writev(
            fd.as_raw_fd(), //
            bufs.as_ptr() as *const iovec,
            cmp::min(bufs.len(), c_int::max_value() as usize) as c_int,
        )
    };
    Ok(res as usize)
}

enum ForkResult {
    Parent { child_pid: c_int },
    Child,
}

unsafe fn fork() -> io::Result<ForkResult> {
    let pid = syscall! { fork() };
    match pid {
        -1 => Err(io::Error::last_os_error()),
        0 => Ok(ForkResult::Child),
        pid => Ok(ForkResult::Parent { child_pid: pid }),
    }
}
