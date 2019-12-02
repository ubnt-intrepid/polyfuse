//! Establish connection with FUSE kernel driver.

#![allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use futures::{
    io::{AsyncRead, AsyncWrite},
    ready,
    task::{self, Poll},
};
use libc::{c_int, c_void, iovec};
use mio::{
    unix::{EventedFd, UnixReady},
    Evented, PollOpt, Ready, Token,
};
use std::{
    cmp,
    ffi::OsStr,
    io::{self, IoSlice, IoSliceMut},
    mem::{self, MaybeUninit},
    os::unix::{
        io::{AsRawFd, IntoRawFd, RawFd},
        net::UnixDatagram,
        process::CommandExt,
    },
    path::{Path, PathBuf},
    pin::Pin,
    process::Command,
    ptr,
};
use tokio::io::PollEvented;

const FUSERMOUNT_PROG: &str = "fusermount";
const FUSE_COMMFD_ENV: &str = "_FUSE_COMMFD";

macro_rules! syscall {
    ($fn:ident ( $($arg:expr),* $(,)* ) ) => {{
        let res = unsafe { libc::$fn($($arg),*) };
        if res == -1 {
            return Err(io::Error::last_os_error());
        }
        res
    }};
}

/// A connection with the FUSE kernel driver.
#[derive(Debug)]
struct Connection {
    fd: RawFd,
    mountpoint: Option<PathBuf>,
}

impl Connection {
    fn try_clone(&self) -> io::Result<Self> {
        let clonefd = syscall! { dup(self.fd) };

        Ok(Self {
            fd: clonefd,
            mountpoint: None,
        })
    }

    fn unmount(&mut self) -> io::Result<()> {
        if let Some(mountpoint) = self.mountpoint.take() {
            Command::new(FUSERMOUNT_PROG)
                .args(&["-u", "-q", "-z", "--"])
                .arg(&mountpoint)
                .status()?;
        }
        Ok(())
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _e = self.unmount();
        unsafe {
            libc::close(self.fd);
        }
    }
}

impl AsRawFd for Connection {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl Evented for Connection {
    fn register(
        &self,
        poll: &mio::Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.fd).register(poll, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &mio::Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.fd).reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &mio::Poll) -> io::Result<()> {
        EventedFd(&self.fd).deregister(poll)
    }
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = syscall! { fcntl(fd, libc::F_GETFL, 0) };
    syscall! { fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    Ok(())
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

// ==== Channel ====

/// Asynchronous I/O object that communicates with the FUSE kernel driver.
#[derive(Debug)]
pub struct Channel(PollEvented<Connection>);

impl Channel {
    /// Establish a connection with the FUSE kernel driver.
    pub fn open(mountpoint: &Path, mountopts: &[&OsStr]) -> io::Result<Self> {
        let (_pid, mut reader) = exec_fusermount(mountpoint, mountopts)?;

        let fd = receive_fd(&mut reader)?;
        set_nonblocking(fd)?;

        // Unmounting is executed when `reader` is dropped and the connection
        // with `fusermount` is closed.
        let _ = reader.into_raw_fd();

        let conn = PollEvented::new(Connection {
            fd,
            mountpoint: Some(mountpoint.into()),
        })?;

        Ok(Self(conn))
    }

    fn poll_read_with<F, R>(&mut self, cx: &mut task::Context<'_>, f: F) -> Poll<io::Result<R>>
    where
        F: FnOnce(&mut Connection) -> io::Result<R>,
    {
        let mut ready = Ready::readable();
        ready.insert(UnixReady::error());
        ready!(self.0.poll_read_ready(cx, ready))?;

        match f(self.0.get_mut()) {
            Ok(ret) => Poll::Ready(Ok(ret)),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.0.clear_read_ready(cx, ready)?;
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_write_with<F, R>(&mut self, cx: &mut task::Context<'_>, f: F) -> Poll<io::Result<R>>
    where
        F: FnOnce(&mut Connection) -> io::Result<R>,
    {
        ready!(self.0.poll_write_ready(cx))?;

        match f(self.0.get_mut()) {
            Ok(ret) => Poll::Ready(Ok(ret)),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.0.clear_write_ready(cx)?;
                Poll::Pending
            }
            Err(e) => {
                tracing::debug!("write error: {}", e);
                Poll::Ready(Err(e))
            }
        }
    }

    /// Attempt to create a clone of this channel.
    pub fn try_clone(&self) -> io::Result<Self> {
        let conn = self.0.get_ref().try_clone()?;
        Ok(Self(PollEvented::new(conn)?))
    }
}

impl AsyncRead for Channel {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_with(cx, |conn| {
            let len = syscall! {
                read(
                    conn.as_raw_fd(), //
                    dst.as_mut_ptr() as *mut c_void,
                    dst.len(),
                )
            };
            Ok(len as usize)
        })
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [IoSliceMut],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_with(cx, |conn| {
            let len = syscall! {
                readv(
                    conn.as_raw_fd(), //
                    dst.as_mut_ptr() as *mut iovec,
                    cmp::min(dst.len(), c_int::max_value() as usize) as c_int,
                )
            };
            Ok(len as usize)
        })
    }
}

impl AsyncWrite for Channel {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_with(cx, |conn| {
            let res = syscall! {
                write(
                    conn.as_raw_fd(), //
                    src.as_ptr() as *const c_void,
                    src.len(),
                )
            };
            Ok(res as usize)
        })
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[IoSlice],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_with(cx, |conn| {
            let res = syscall! {
                writev(
                    conn.as_raw_fd(), //
                    src.as_ptr() as *const iovec,
                    cmp::min(src.len(), c_int::max_value() as usize) as c_int,
                )
            };
            Ok(res as usize)
        })
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
