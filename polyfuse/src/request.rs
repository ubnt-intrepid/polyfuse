//! Receives and parses FUSE requests.

use bytes::{Bytes, BytesMut};
use futures::{future::Future, io::AsyncRead, ready};
use polyfuse_sys::kernel::{
    fuse_access_in, //
    fuse_batch_forget_in,
    fuse_bmap_in,
    fuse_copy_file_range_in,
    fuse_create_in,
    fuse_fallocate_in,
    fuse_flush_in,
    fuse_forget_in,
    fuse_forget_one,
    fuse_fsync_in,
    fuse_getattr_in,
    fuse_getxattr_in,
    fuse_in_header,
    fuse_init_in,
    fuse_interrupt_in,
    fuse_link_in,
    fuse_lk_in,
    fuse_mkdir_in,
    fuse_mknod_in,
    fuse_notify_retrieve_in,
    fuse_opcode,
    fuse_open_in,
    fuse_poll_in,
    fuse_read_in,
    fuse_release_in,
    fuse_rename2_in,
    fuse_rename_in,
    fuse_setattr_in,
    fuse_setxattr_in,
    fuse_write_in,
};
use std::{
    convert::TryFrom, //
    ffi::OsStr,
    io::{self, IoSliceMut},
    mem,
    os::unix::ffi::OsStrExt,
    pin::Pin,
    task::{self, Poll},
};

/// Buffer that stores FUSE requests.
pub trait Buffer {
    type Data;

    /// Return a reference to the header part of received request.
    fn header(&self) -> Option<&RequestHeader>;

    /// Prepare the buffer to receive the specified amount of data.
    fn poll_ready(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        bufsize: usize,
    ) -> Poll<io::Result<()>>;

    /// Transfer one request queued in the kernel driver into this buffer.
    fn poll_receive<R: ?Sized>(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        reader: Pin<&mut R>,
    ) -> Poll<io::Result<()>>
    where
        R: AsyncRead;

    /// Extract the content of request from the buffer.
    fn extract(&mut self) -> io::Result<Request<'_, Self::Data>>;
}

pub(crate) trait BufferExt: Buffer {
    fn receive<'a, 'b, R: ?Sized>(
        &'a mut self,
        reader: &'b mut R,
        bufsize: usize,
    ) -> Receive<'a, 'b, Self, R>
    where
        R: AsyncRead,
    {
        Receive {
            buf: self,
            reader,
            is_ready: false,
            bufsize,
        }
    }
}

impl<T: ?Sized> BufferExt for T where T: Buffer {}

#[allow(missing_debug_implementations)]
pub(crate) struct Receive<'a, 'b, T: ?Sized, R: ?Sized> {
    buf: &'a mut T,
    reader: &'b mut R,
    is_ready: bool,
    bufsize: usize,
}

impl<T: ?Sized, R: ?Sized> Future for Receive<'_, '_, T, R>
where
    T: Buffer,
    R: AsyncRead,
{
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context) -> Poll<Self::Output> {
        let me = self.get_mut();
        let mut buf = unsafe { Pin::new_unchecked(&mut *me.buf) };
        let reader = unsafe { Pin::new_unchecked(&mut *me.reader) };

        if !me.is_ready {
            futures::ready!(buf.as_mut().poll_ready(cx, me.bufsize))?;
            me.is_ready = true;
        }

        buf.poll_receive(cx, reader)
    }
}

/// A `Buffer` that stores a FUSE request in memory.
#[derive(Debug)]
pub struct BytesBuffer {
    header: Option<RequestHeader>,
    payload: BytesBufferPayload,
    bufsize: usize,
}

impl BytesBuffer {
    pub fn new(bufsize: usize) -> Self {
        Self {
            header: None,
            payload: BytesBufferPayload::Unique(BytesMut::with_capacity(bufsize)),
            bufsize,
        }
    }
}

impl Buffer for BytesBuffer {
    type Data = Bytes;

    fn header(&self) -> Option<&RequestHeader> {
        self.header.as_ref()
    }

    fn poll_ready(
        self: Pin<&mut Self>,
        _: &mut task::Context,
        bufsize: usize,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        let payload = me.payload.make_unique(bufsize);
        unsafe {
            let capacity = payload.capacity();
            payload.set_len(capacity);
        }
        Poll::Ready(Ok(()))
    }

    fn poll_receive<R: ?Sized>(
        self: Pin<&mut Self>,
        cx: &mut task::Context,
        reader: Pin<&mut R>,
    ) -> Poll<io::Result<()>>
    where
        R: AsyncRead,
    {
        let me = self.get_mut();
        let mut reader = reader;

        let mut header = mem::MaybeUninit::<RequestHeader>::uninit();
        let payload = me.payload.get_unique();

        let mut vec = [
            IoSliceMut::new(unsafe {
                std::slice::from_raw_parts_mut(
                    header.as_mut_ptr() as *mut u8,
                    mem::size_of::<fuse_in_header>(),
                )
            }),
            IoSliceMut::new(payload.as_mut()),
        ];

        loop {
            match ready!(reader.as_mut().poll_read_vectored(cx, &mut vec[..])) {
                Ok(len) => {
                    if len < mem::size_of::<fuse_in_header>() {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "the received data from the kernel is too short",
                        )));
                    }
                    let header = unsafe { header.assume_init() };

                    if header.len() as usize != len {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "the payload length is mismatched to the header value",
                        )));
                    }

                    me.header = Some(header);
                    unsafe {
                        payload.set_len(len - mem::size_of::<fuse_in_header>());
                    }

                    return Poll::Ready(Ok(()));
                }
                Err(err) => match err.raw_os_error() {
                    Some(libc::ENOENT) | Some(libc::EINTR) => {
                        tracing::debug!("continue reading from the kernel");
                        continue;
                    }
                    _ => return Poll::Ready(Err(err)),
                },
            }
        }
    }

    fn extract(&mut self) -> io::Result<Request<'_, Self::Data>> {
        let header = self.header.as_ref().expect("the header is not read");
        let opcode = header
            .opcode()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid opcode"))?;

        let kind = match opcode {
            fuse_opcode::FUSE_WRITE | fuse_opcode::FUSE_NOTIFY_REPLY => {
                let payload = self.payload.make_shared();
                Parser::new(payload).parse(opcode, |parser| payload.slice_from(parser.offset()))?
            }
            _ => {
                let payload = self.payload.as_slice();
                Parser::new(payload).parse(opcode, |_| Bytes::new())?
            }
        };

        Ok(Request::new(header, kind))
    }
}

#[derive(Debug)]
enum BytesBufferPayload {
    Unique(BytesMut),
    Shared(Bytes),
    Empty,
}

impl BytesBufferPayload {
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

    fn get_unique(&mut self) -> &mut BytesMut {
        match self {
            Self::Unique(bytes) => bytes,
            Self::Shared(..) => panic!("unexpected get_unique"),
            _ => unreachable!(),
        }
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Unique(bytes) => bytes.as_ref(),
            Self::Shared(bytes) => bytes.as_ref(),
            Self::Empty => unreachable!(),
        }
    }
}

/// A FUSE request.
#[derive(Debug)]
pub struct Request<'a, T> {
    header: &'a RequestHeader,
    kind: RequestKind<'a, T>,
}

impl<'a, T> Request<'a, T> {
    /// Create a new `Request` from the provided components.
    pub fn new(header: &'a RequestHeader, kind: RequestKind<'a, T>) -> Self {
        Self { header, kind }
    }

    pub(crate) fn into_inner(self) -> (&'a RequestHeader, RequestKind<'a, T>) {
        (self.header, self.kind)
    }
}

/// The header part of a FUSE request.
///
/// This type is ABI-compatible with `fuse_in_header`.
#[derive(Debug)]
#[repr(transparent)]
pub struct RequestHeader(fuse_in_header);

#[allow(clippy::len_without_is_empty)]
impl RequestHeader {
    pub fn len(&self) -> u32 {
        self.0.len
    }

    pub fn unique(&self) -> u64 {
        self.0.unique
    }

    pub fn opcode(&self) -> Option<fuse_opcode> {
        fuse_opcode::try_from(self.0.opcode).ok()
    }

    pub fn nodeid(&self) -> u64 {
        self.0.nodeid
    }

    pub fn uid(&self) -> u32 {
        self.0.uid
    }

    pub fn gid(&self) -> u32 {
        self.0.gid
    }

    pub fn pid(&self) -> u32 {
        self.0.pid
    }
}

/// The kind of FUSE request.
#[derive(Debug)]
pub enum RequestKind<'a, T> {
    Init {
        arg: &'a fuse_init_in,
    },
    Destroy,
    Lookup {
        name: &'a OsStr,
    },
    Forget {
        arg: &'a fuse_forget_in,
    },
    Getattr {
        arg: &'a fuse_getattr_in,
    },
    Setattr {
        arg: &'a fuse_setattr_in,
    },
    Readlink,
    Symlink {
        name: &'a OsStr,
        link: &'a OsStr,
    },
    Mknod {
        arg: &'a fuse_mknod_in,
        name: &'a OsStr,
    },
    Mkdir {
        arg: &'a fuse_mkdir_in,
        name: &'a OsStr,
    },
    Unlink {
        name: &'a OsStr,
    },
    Rmdir {
        name: &'a OsStr,
    },
    Rename {
        arg: &'a fuse_rename_in,
        name: &'a OsStr,
        newname: &'a OsStr,
    },
    Link {
        arg: &'a fuse_link_in,
        newname: &'a OsStr,
    },
    Open {
        arg: &'a fuse_open_in,
    },
    Read {
        arg: &'a fuse_read_in,
    },
    Write {
        arg: &'a fuse_write_in,
        data: T,
    },
    Release {
        arg: &'a fuse_release_in,
    },
    Statfs,
    Fsync {
        arg: &'a fuse_fsync_in,
    },
    Setxattr {
        arg: &'a fuse_setxattr_in,
        name: &'a OsStr,
        value: &'a [u8],
    },
    Getxattr {
        arg: &'a fuse_getxattr_in,
        name: &'a OsStr,
    },
    Listxattr {
        arg: &'a fuse_getxattr_in,
    },
    Removexattr {
        name: &'a OsStr,
    },
    Flush {
        arg: &'a fuse_flush_in,
    },
    Opendir {
        arg: &'a fuse_open_in,
    },
    Readdir {
        arg: &'a fuse_read_in,
    },
    Readdirplus {
        arg: &'a fuse_read_in,
    },
    Releasedir {
        arg: &'a fuse_release_in,
    },
    Fsyncdir {
        arg: &'a fuse_fsync_in,
    },
    Getlk {
        arg: &'a fuse_lk_in,
    },
    Setlk {
        arg: &'a fuse_lk_in,
        sleep: bool,
    },
    Access {
        arg: &'a fuse_access_in,
    },
    Create {
        arg: &'a fuse_create_in,
        name: &'a OsStr,
    },
    Interrupt {
        arg: &'a fuse_interrupt_in,
    },
    Bmap {
        arg: &'a fuse_bmap_in,
    },
    Fallocate {
        arg: &'a fuse_fallocate_in,
    },
    Rename2 {
        arg: &'a fuse_rename2_in,
        name: &'a OsStr,
        newname: &'a OsStr,
    },
    CopyFileRange {
        arg: &'a fuse_copy_file_range_in,
    },
    BatchForget {
        arg: &'a fuse_batch_forget_in,
        forgets: &'a [fuse_forget_one],
    },
    NotifyReply {
        arg: &'a fuse_notify_retrieve_in,
        data: T,
    },
    Poll {
        arg: &'a fuse_poll_in,
    },
    Unknown,
}

// TODO: add opcodes:
// Ioctl,

/// A parser for FUSE request.
#[derive(Debug)]
pub struct Parser<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Parser<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn fetch_bytes(&mut self, count: usize) -> io::Result<&'a [u8]> {
        if self.bytes.len() < self.offset + count {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "fetch"));
        }
        let bytes = &self.bytes[self.offset..self.offset + count];
        self.offset += count;
        Ok(bytes)
    }

    fn fetch_array<T>(&mut self, count: usize) -> io::Result<&'a [T]> {
        self.fetch_bytes(mem::size_of::<T>() * count)
            .map(|bytes| unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const T, count) })
    }

    fn fetch_str(&mut self) -> io::Result<&'a OsStr> {
        let len = self.bytes[self.offset..]
            .iter()
            .position(|&b| b == b'\0')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "fetch_str: missing \\0"))?;
        self.fetch_bytes(len).map(OsStr::from_bytes)
    }

    fn fetch<T>(&mut self) -> io::Result<&'a T> {
        self.fetch_bytes(mem::size_of::<T>())
            .map(|data| unsafe { &*(data.as_ptr() as *const T) })
    }

    /// Return the length of consumed bytes.
    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn parse<F, T>(&mut self, opcode: fuse_opcode, f: F) -> io::Result<RequestKind<'a, T>>
    where
        F: FnOnce(&mut Self) -> T,
    {
        match opcode {
            fuse_opcode::FUSE_INIT => {
                let arg = self.fetch()?;
                Ok(RequestKind::Init { arg })
            }
            fuse_opcode::FUSE_DESTROY => Ok(RequestKind::Destroy),
            fuse_opcode::FUSE_LOOKUP => {
                let name = self.fetch_str()?;
                Ok(RequestKind::Lookup { name })
            }
            fuse_opcode::FUSE_FORGET => {
                let arg = self.fetch()?;
                Ok(RequestKind::Forget { arg })
            }
            fuse_opcode::FUSE_GETATTR => {
                let arg = self.fetch()?;
                Ok(RequestKind::Getattr { arg })
            }
            fuse_opcode::FUSE_SETATTR => {
                let arg = self.fetch()?;
                Ok(RequestKind::Setattr { arg })
            }
            fuse_opcode::FUSE_READLINK => Ok(RequestKind::Readlink),
            fuse_opcode::FUSE_SYMLINK => {
                let name = self.fetch_str()?;
                let link = self.fetch_str()?;
                Ok(RequestKind::Symlink { name, link })
            }
            fuse_opcode::FUSE_MKNOD => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(RequestKind::Mknod { arg, name })
            }
            fuse_opcode::FUSE_MKDIR => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(RequestKind::Mkdir { arg, name })
            }
            fuse_opcode::FUSE_UNLINK => {
                let name = self.fetch_str()?;
                Ok(RequestKind::Unlink { name })
            }
            fuse_opcode::FUSE_RMDIR => {
                let name = self.fetch_str()?;
                Ok(RequestKind::Rmdir { name })
            }
            fuse_opcode::FUSE_RENAME => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                let newname = self.fetch_str()?;
                Ok(RequestKind::Rename { arg, name, newname })
            }
            fuse_opcode::FUSE_LINK => {
                let arg = self.fetch()?;
                let newname = self.fetch_str()?;
                Ok(RequestKind::Link { arg, newname })
            }
            fuse_opcode::FUSE_OPEN => {
                let arg = self.fetch()?;
                Ok(RequestKind::Open { arg })
            }
            fuse_opcode::FUSE_READ => {
                let arg = self.fetch()?;
                Ok(RequestKind::Read { arg })
            }
            fuse_opcode::FUSE_WRITE => {
                let arg = self.fetch()?;
                let data = f(&mut *self);
                Ok(RequestKind::Write { arg, data })
            }
            fuse_opcode::FUSE_RELEASE => {
                let arg = self.fetch()?;
                Ok(RequestKind::Release { arg })
            }
            fuse_opcode::FUSE_STATFS => Ok(RequestKind::Statfs),
            fuse_opcode::FUSE_FSYNC => {
                let arg = self.fetch()?;
                Ok(RequestKind::Fsync { arg })
            }
            fuse_opcode::FUSE_SETXATTR => {
                let arg = self.fetch::<fuse_setxattr_in>()?;
                let name = self.fetch_str()?;
                let value = self.fetch_bytes(arg.size as usize)?;
                Ok(RequestKind::Setxattr { arg, name, value })
            }
            fuse_opcode::FUSE_GETXATTR => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(RequestKind::Getxattr { arg, name })
            }
            fuse_opcode::FUSE_LISTXATTR => {
                let arg = self.fetch()?;
                Ok(RequestKind::Listxattr { arg })
            }
            fuse_opcode::FUSE_REMOVEXATTR => {
                let name = self.fetch_str()?;
                Ok(RequestKind::Removexattr { name })
            }
            fuse_opcode::FUSE_FLUSH => {
                let arg = self.fetch()?;
                Ok(RequestKind::Flush { arg })
            }
            fuse_opcode::FUSE_OPENDIR => {
                let arg = self.fetch()?;
                Ok(RequestKind::Opendir { arg })
            }
            fuse_opcode::FUSE_READDIR => {
                let arg = self.fetch()?;
                Ok(RequestKind::Readdir { arg })
            }
            fuse_opcode::FUSE_RELEASEDIR => {
                let arg = self.fetch()?;
                Ok(RequestKind::Releasedir { arg })
            }
            fuse_opcode::FUSE_FSYNCDIR => {
                let arg = self.fetch()?;
                Ok(RequestKind::Fsyncdir { arg })
            }
            fuse_opcode::FUSE_GETLK => {
                let arg = self.fetch()?;
                Ok(RequestKind::Getlk { arg })
            }
            fuse_opcode::FUSE_SETLK => {
                let arg = self.fetch()?;
                Ok(RequestKind::Setlk { arg, sleep: false })
            }
            fuse_opcode::FUSE_SETLKW => {
                let arg = self.fetch()?;
                Ok(RequestKind::Setlk { arg, sleep: true })
            }
            fuse_opcode::FUSE_ACCESS => {
                let arg = self.fetch()?;
                Ok(RequestKind::Access { arg })
            }
            fuse_opcode::FUSE_CREATE => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(RequestKind::Create { arg, name })
            }
            fuse_opcode::FUSE_INTERRUPT => {
                let arg = self.fetch()?;
                Ok(RequestKind::Interrupt { arg })
            }
            fuse_opcode::FUSE_BMAP => {
                let arg = self.fetch()?;
                Ok(RequestKind::Bmap { arg })
            }
            fuse_opcode::FUSE_FALLOCATE => {
                let arg = self.fetch()?;
                Ok(RequestKind::Fallocate { arg })
            }
            fuse_opcode::FUSE_READDIRPLUS => {
                let arg = self.fetch()?;
                Ok(RequestKind::Readdirplus { arg })
            }
            fuse_opcode::FUSE_RENAME2 => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                let newname = self.fetch_str()?;
                Ok(RequestKind::Rename2 { arg, name, newname })
            }
            fuse_opcode::FUSE_COPY_FILE_RANGE => {
                let arg = self.fetch()?;
                Ok(RequestKind::CopyFileRange { arg })
            }
            fuse_opcode::FUSE_POLL => {
                let arg = self.fetch()?;
                Ok(RequestKind::Poll { arg })
            }
            fuse_opcode::FUSE_BATCH_FORGET => {
                let arg = self.fetch::<fuse_batch_forget_in>()?;
                let forgets = self.fetch_array(arg.count as usize)?;
                Ok(RequestKind::BatchForget { arg, forgets })
            }
            fuse_opcode::FUSE_NOTIFY_REPLY => {
                let arg = self.fetch()?;
                let data = f(&mut *self);
                Ok(RequestKind::NotifyReply { arg, data })
            }
            _ => Ok(RequestKind::Unknown),
        }
    }
}
