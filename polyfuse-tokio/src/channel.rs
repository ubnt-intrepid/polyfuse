//! Establish connection with FUSE kernel driver.

#![allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use bytes::{Bytes, BytesMut};
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
use polyfuse::io::{Buffer, InHeader, OutHeader, Reader, Writer};
use smallvec::SmallVec;
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

impl Reader for Channel {
    type Buffer = ChannelBuffer;

    fn poll_receive_msg(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut Self::Buffer,
    ) -> Poll<io::Result<()>> {
        buf.poll_receive_from(cx, self.get_mut())
    }
}

impl Writer for Channel {
    fn poll_write_msg(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        header: &OutHeader,
        data: &[&[u8]],
    ) -> Poll<io::Result<()>> {
        // Unfortunately, IoSlice<'_> does not implement Send and
        // the data vector must be created in `poll` function.
        let vec: SmallVec<[_; 4]> = Some(IoSlice::new(header.as_ref()))
            .into_iter()
            .chain(data.iter().map(|t| IoSlice::new(&*t)))
            .collect();

        let count = ready!(self.poll_write_vectored(cx, &*vec))?;
        if count < header.len() as usize {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::Other,
                "written data is too short",
            )));
        }

        Poll::Ready(Ok(()))
    }
}

#[derive(Debug)]
pub struct ChannelBuffer {
    header: InHeader,
    data: ChannelBufferData,
    bufsize: usize,
}

impl ChannelBuffer {
    pub fn new(bufsize: usize) -> Self {
        let bufsize = bufsize - mem::size_of::<InHeader>();
        Self {
            header: unsafe { mem::zeroed() },
            data: ChannelBufferData::Unique(BytesMut::with_capacity(bufsize)),
            bufsize,
        }
    }

    fn poll_receive_from(
        &mut self,
        cx: &mut task::Context<'_>,
        reader: &mut Channel,
    ) -> Poll<io::Result<()>> {
        let data = self.data.make_unique(self.bufsize);
        unsafe {
            data.set_len(data.capacity());
        }

        let mut vec = [
            IoSliceMut::new(self.header.as_mut()),
            IoSliceMut::new(data.as_mut()),
        ];

        loop {
            match ready!(Pin::new(&mut *reader).poll_read_vectored(cx, &mut vec[..])) {
                Ok(len) => {
                    if len < mem::size_of::<InHeader>() {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "the received data from the kernel is too short",
                        )));
                    }
                    if self.header.len() as usize != len {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "the payload length is mismatched to the header value",
                        )));
                    }

                    unsafe {
                        data.set_len(len - mem::size_of::<InHeader>());
                    }

                    return Poll::Ready(Ok(()));
                }

                Err(err) => match err.raw_os_error() {
                    Some(libc::ENOENT) | Some(libc::EINTR) => {
                        tracing::debug!("continue reading from the kernel");
                        continue;
                    }
                    _ => {
                        tracing::error!("receive error: {}", err);
                        return Poll::Ready(Err(err));
                    }
                },
            }
        }
    }
}

impl Buffer for ChannelBuffer {
    type Data = Bytes;

    fn header(&self) -> &InHeader {
        &self.header
    }

    fn extract(&mut self) -> (&InHeader, &[u8], Option<Self::Data>) {
        let header = &self.header;
        let mut data = None;
        let arg = if header.arg_len() < self.data.len() {
            let bytes = self.data.make_shared();
            data.replace(bytes.slice(header.arg_len()..));
            &bytes[..header.arg_len()]
        } else {
            &self.data.as_slice()[..header.arg_len()]
        };
        (header, arg, data)
    }
}

#[derive(Debug)]
enum ChannelBufferData {
    Unique(BytesMut),
    Shared(Bytes),
    Empty,
}

impl ChannelBufferData {
    fn make_unique(&mut self, bufsize: usize) -> &mut BytesMut {
        if let Self::Unique(bytes) = self {
            let cap = bytes.capacity();
            if cap < bufsize {
                bytes.reserve(bufsize - cap);
            }
            return bytes;
        }

        *self = Self::Unique(BytesMut::with_capacity(bufsize));
        match self {
            Self::Unique(bytes) => bytes,
            _ => unreachable!(),
        }
    }

    fn make_shared(&mut self) -> &Bytes {
        loop {
            match self {
                Self::Shared(bytes) => return bytes,
                Self::Unique(..) => match mem::replace(self, Self::Empty) {
                    Self::Unique(bytes) => {
                        *self = Self::Shared(bytes.freeze());
                        continue;
                    }
                    _ => unreachable!(),
                },
                _ => unreachable!(),
            }
        }
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Unique(bytes) => bytes.as_ref(),
            Self::Shared(bytes) => bytes.as_ref(),
            Self::Empty => unreachable!(),
        }
    }

    fn len(&self) -> usize {
        self.as_slice().len()
    }
}
