//! Requests from the kernel.

use crate::abi::{
    AccessIn, //
    BmapIn,
    CreateIn,
    FlushIn,
    ForgetIn,
    FsyncIn,
    GetattrIn,
    GetxattrIn,
    InHeader,
    InitIn,
    LinkIn,
    LkIn,
    MkdirIn,
    MknodIn,
    Opcode,
    OpenIn,
    ReadIn,
    ReleaseIn,
    RenameIn,
    SetattrIn,
    SetxattrIn,
    WriteIn,
};
use futures::{future::poll_fn, io::AsyncRead};
use std::{
    ffi::OsStr,
    io, mem,
    os::unix::ffi::OsStrExt,
    pin::Pin,
    task::{self, Poll},
};

pub const MAX_WRITE_SIZE: u32 = 16 * 1024 * 1024;

pub const DEFAULT_RECV_BUF_SIZE: u32 = MAX_WRITE_SIZE + 4096;

#[derive(Debug)]
pub struct Request<'a> {
    pub header: &'a InHeader,
    pub arg: Arg<'a>,
    _p: (),
}

#[derive(Debug)]
pub enum Arg<'a> {
    Init {
        arg: &'a InitIn,
    },
    Destroy,
    Lookup {
        name: &'a OsStr,
    },
    Forget {
        arg: &'a ForgetIn,
    },
    Getattr {
        arg: &'a GetattrIn,
    },
    Setattr {
        arg: &'a SetattrIn,
    },
    Readlink,
    Symlink {
        name: &'a OsStr,
        link: &'a OsStr,
    },
    Mknod {
        arg: &'a MknodIn,
        name: &'a OsStr,
    },
    Mkdir {
        arg: &'a MkdirIn,
        name: &'a OsStr,
    },
    Unlink {
        name: &'a OsStr,
    },
    Rmdir {
        name: &'a OsStr,
    },
    Rename {
        arg: &'a RenameIn,
        name: &'a OsStr,
        newname: &'a OsStr,
    },
    Link {
        arg: &'a LinkIn,
        newname: &'a OsStr,
    },
    Open {
        arg: &'a OpenIn,
    },
    Read {
        arg: &'a ReadIn,
    },
    Write {
        arg: &'a WriteIn,
    },
    Release {
        arg: &'a ReleaseIn,
    },
    Statfs,
    Fsync {
        arg: &'a FsyncIn,
    },
    Setxattr {
        arg: &'a SetxattrIn,
        name: &'a OsStr,
        value: &'a [u8],
    },
    Getxattr {
        arg: &'a GetxattrIn,
        name: &'a OsStr,
    },
    Listxattr {
        arg: &'a GetxattrIn,
    },
    Removexattr {
        name: &'a OsStr,
    },
    Flush {
        arg: &'a FlushIn,
    },
    Opendir {
        arg: &'a OpenIn,
    },
    Readdir {
        arg: &'a ReadIn,
    },
    Releasedir {
        arg: &'a ReleaseIn,
    },
    Fsyncdir {
        arg: &'a FsyncIn,
    },
    Getlk {
        arg: &'a LkIn,
    },
    Setlk {
        arg: &'a LkIn,
    },
    Setlkw {
        arg: &'a LkIn,
    },
    Access {
        arg: &'a AccessIn,
    },
    Create {
        arg: &'a CreateIn,
    },
    Bmap {
        arg: &'a BmapIn,
    },
    // Interrupt,
    // Ioctl,
    // Poll,
    // NotifyReply,
    // BatchForget,
    // Fallocate,
    // Readdirplus,
    // Rename2,
    // Lseek,
    // CopyFileRange,
    Unknown,
}

pub trait FromBytes<'a> {
    const SIZE: usize;

    unsafe fn from_bytes(bytes: &'a [u8]) -> &'a Self;
}

macro_rules! impl_from_bytes {
    ($($t:ty,)*) => {$(
        impl<'a> FromBytes<'a> for $t {
            const SIZE: usize = mem::size_of::<Self>();

            unsafe fn from_bytes(bytes: &'a [u8]) -> &'a Self {
                debug_assert_eq!(bytes.len(), Self::SIZE);
                &*(bytes.as_ptr() as *const Self)
            }
        }
    )*};
}

impl_from_bytes! {
    InHeader,
    InitIn,
    ForgetIn,
    GetattrIn,
    SetattrIn,
    MknodIn,
    MkdirIn,
    RenameIn,
    LinkIn,
    OpenIn,
    ReadIn,
    WriteIn,
    ReleaseIn,
    FsyncIn,
    SetxattrIn,
    GetxattrIn,
    FlushIn,
    LkIn,
    AccessIn,
    CreateIn,
    BmapIn,
}

#[derive(Debug)]
pub struct Parser<'a> {
    buf: &'a [u8],
    offset: usize,
}

impl<'a> Parser<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, offset: 0 }
    }

    fn fetch_bytes(&mut self, count: usize) -> io::Result<&'a [u8]> {
        if self.buf.len() < self.offset + count {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "fetch"));
        }

        let data = &self.buf[self.offset..self.offset + count];
        self.offset += count;

        Ok(data)
    }

    fn fetch_str(&mut self) -> io::Result<&'a OsStr> {
        let len = self.buf[self.offset..]
            .iter()
            .position(|&b| b == b'\0')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "fetch_str: missing \\0"))?;
        self.fetch_bytes(len).map(OsStr::from_bytes)
    }

    fn fetch<T: FromBytes<'a>>(&mut self) -> io::Result<&'a T> {
        self.fetch_bytes(T::SIZE)
            .map(|data| unsafe { T::from_bytes(data) })
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn parse(&mut self) -> io::Result<(Request<'a>, usize)> {
        let header = self.parse_header()?;
        let arg = self.parse_arg(header)?;
        Ok((
            Request {
                header,
                arg,
                _p: (),
            },
            self.offset(),
        ))
    }

    #[allow(clippy::cast_ptr_alignment)]
    fn parse_header(&mut self) -> io::Result<&'a InHeader> {
        let header = self.fetch::<InHeader>()?;

        if self.buf.len() < header.len as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "received data is too short",
            ));
        }

        Ok(header)
    }

    fn parse_arg(&mut self, header: &'a InHeader) -> io::Result<Arg<'a>> {
        match header.opcode() {
            Some(Opcode::Init) => {
                let arg = self.fetch()?;
                Ok(Arg::Init { arg })
            }
            Some(Opcode::Destroy) => Ok(Arg::Destroy),
            Some(Opcode::Lookup) => {
                let name = self.fetch_str()?;
                Ok(Arg::Lookup { name })
            }
            Some(Opcode::Forget) => {
                let arg = self.fetch()?;
                Ok(Arg::Forget { arg })
            }
            Some(Opcode::Getattr) => {
                let arg = self.fetch()?;
                Ok(Arg::Getattr { arg })
            }
            Some(Opcode::Setattr) => {
                let arg = self.fetch()?;
                Ok(Arg::Setattr { arg })
            }
            Some(Opcode::Readlink) => Ok(Arg::Readlink),
            Some(Opcode::Symlink) => {
                let name = self.fetch_str()?;
                let link = self.fetch_str()?;
                Ok(Arg::Symlink { name, link })
            }
            Some(Opcode::Mknod) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(Arg::Mknod { arg, name })
            }
            Some(Opcode::Mkdir) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(Arg::Mkdir { arg, name })
            }
            Some(Opcode::Unlink) => {
                let name = self.fetch_str()?;
                Ok(Arg::Unlink { name })
            }
            Some(Opcode::Rmdir) => {
                let name = self.fetch_str()?;
                Ok(Arg::Rmdir { name })
            }
            Some(Opcode::Rename) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                let newname = self.fetch_str()?;
                Ok(Arg::Rename { arg, name, newname })
            }
            Some(Opcode::Link) => {
                let arg = self.fetch()?;
                let newname = self.fetch_str()?;
                Ok(Arg::Link { arg, newname })
            }
            Some(Opcode::Open) => {
                let arg = self.fetch()?;
                Ok(Arg::Open { arg })
            }
            Some(Opcode::Read) => {
                let arg = self.fetch()?;
                Ok(Arg::Read { arg })
            }
            Some(Opcode::Write) => {
                let arg = self.fetch()?;
                Ok(Arg::Write { arg })
            }
            Some(Opcode::Release) => {
                let arg = self.fetch()?;
                Ok(Arg::Release { arg })
            }
            Some(Opcode::Statfs) => Ok(Arg::Statfs),
            Some(Opcode::Fsync) => {
                let arg = self.fetch()?;
                Ok(Arg::Fsync { arg })
            }
            Some(Opcode::Setxattr) => {
                let arg: &SetxattrIn = self.fetch()?;
                let name = self.fetch_str()?;
                let value = self.fetch_bytes(arg.size as usize)?;
                Ok(Arg::Setxattr { arg, name, value })
            }
            Some(Opcode::Getxattr) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(Arg::Getxattr { arg, name })
            }
            Some(Opcode::Listxattr) => {
                let arg = self.fetch()?;
                Ok(Arg::Listxattr { arg })
            }
            Some(Opcode::Removexattr) => {
                let name = self.fetch_str()?;
                Ok(Arg::Removexattr { name })
            }
            Some(Opcode::Flush) => {
                let arg = self.fetch()?;
                Ok(Arg::Flush { arg })
            }
            Some(Opcode::Opendir) => {
                let arg = self.fetch()?;
                Ok(Arg::Opendir { arg })
            }
            Some(Opcode::Readdir) => {
                let arg = self.fetch()?;
                Ok(Arg::Readdir { arg })
            }
            Some(Opcode::Releasedir) => {
                let arg = self.fetch()?;
                Ok(Arg::Releasedir { arg })
            }
            Some(Opcode::Fsyncdir) => {
                let arg = self.fetch()?;
                Ok(Arg::Fsyncdir { arg })
            }
            Some(Opcode::Getlk) => {
                let arg = self.fetch()?;
                Ok(Arg::Getlk { arg })
            }
            Some(Opcode::Setlk) => {
                let arg = self.fetch()?;
                Ok(Arg::Setlk { arg })
            }
            Some(Opcode::Setlkw) => {
                let arg = self.fetch()?;
                Ok(Arg::Setlkw { arg })
            }
            Some(Opcode::Access) => {
                let arg = self.fetch()?;
                Ok(Arg::Access { arg })
            }
            Some(Opcode::Create) => {
                let arg = self.fetch()?;
                Ok(Arg::Create { arg })
            }
            Some(Opcode::Bmap) => {
                let arg = self.fetch()?;
                Ok(Arg::Bmap { arg })
            }
            _ => Ok(Arg::Unknown),
        }
    }
}

/// A buffer to hold request data from the kernel.
#[derive(Debug)]
pub struct Buffer {
    recv_buf: Vec<u8>,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new(DEFAULT_RECV_BUF_SIZE as usize)
    }
}

impl Buffer {
    /// Create a new `Buffer`.
    pub fn new(bufsize: usize) -> Self {
        Self {
            recv_buf: Vec::with_capacity(bufsize),
        }
    }

    /// Acquires an incoming request from the kernel.
    ///
    /// The received data is stored in the internal buffer, and could be
    /// retrieved using `parse`.
    pub fn poll_receive<I: ?Sized>(
        &mut self,
        cx: &mut task::Context,
        io: &mut I,
    ) -> Poll<io::Result<bool>>
    where
        I: AsyncRead + Unpin,
    {
        let old_len = self.recv_buf.len();
        unsafe {
            let capacity = self.recv_buf.capacity();
            self.recv_buf.set_len(capacity);
        }

        loop {
            match Pin::new(&mut *io).poll_read(cx, &mut self.recv_buf[..]) {
                Poll::Pending => {
                    unsafe {
                        self.recv_buf.set_len(old_len);
                    }
                    return Poll::Pending;
                }
                Poll::Ready(Ok(count)) => {
                    unsafe {
                        self.recv_buf.set_len(count);
                    }
                    return Poll::Ready(Ok(false));
                }
                Poll::Ready(Err(err)) => match err.raw_os_error() {
                    Some(libc::ENOENT) | Some(libc::EINTR) => {
                        log::debug!("continue reading from the kernel");
                        continue;
                    }
                    Some(libc::ENODEV) => {
                        log::debug!("the connection was closed by the kernel");
                        return Poll::Ready(Ok(true));
                    }
                    _ => return Poll::Ready(Err(err)),
                },
            }
        }
    }

    /// Receive a request from the kernel asynchronously.
    ///
    /// This method is a helper to call `poll_receive` in async functions.
    pub async fn receive<I: ?Sized>(&mut self, io: &mut I) -> io::Result<bool>
    where
        I: AsyncRead + Unpin,
    {
        poll_fn(move |cx| self.poll_receive(cx, io)).await
    }

    /// Extract the last incoming request.
    pub fn extract(&mut self) -> io::Result<(Request<'_>, Option<&[u8]>)> {
        let (request, offset) = Parser::new(&self.recv_buf[..]).parse()?;
        let data = match request.arg {
            Arg::Write { arg: write_in } => {
                let size = write_in.size as usize;
                if offset + size < self.recv_buf.len() {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "receive_write"));
                }
                Some(&self.recv_buf[offset..offset + size])
            }
            _ => None,
        };
        Ok((request, data))
    }
}
