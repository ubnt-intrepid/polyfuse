use bytes::{Bytes, BytesMut};
use futures::{future::poll_fn, io::AsyncRead, ready};
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
    marker::PhantomData,
    mem,
    ops::Deref,
    os::unix::ffi::OsStrExt,
    pin::Pin,
    task::Poll,
};

/// An incoming FUSE request received from the kernel.
#[derive(Debug)]
pub struct Request {
    pub(crate) header: fuse_in_header,
    pub(crate) kind: RequestKind,
    _p: (),
}

impl Request {
    pub(crate) fn opcode(&self) -> Option<fuse_opcode> {
        fuse_opcode::try_from(self.header.opcode).ok()
    }
}

#[derive(Debug)]
pub enum RequestKind {
    Init {
        arg: Shared<fuse_init_in>,
    },
    Destroy,
    Lookup {
        name: SharedOsStr,
    },
    Forget {
        arg: Shared<fuse_forget_in>,
    },
    Getattr {
        arg: Shared<fuse_getattr_in>,
    },
    Setattr {
        arg: Shared<fuse_setattr_in>,
    },
    Readlink,
    Symlink {
        name: SharedOsStr,
        link: SharedOsStr,
    },
    Mknod {
        arg: Shared<fuse_mknod_in>,
        name: SharedOsStr,
    },
    Mkdir {
        arg: Shared<fuse_mkdir_in>,
        name: SharedOsStr,
    },
    Unlink {
        name: SharedOsStr,
    },
    Rmdir {
        name: SharedOsStr,
    },
    Rename {
        arg: Shared<fuse_rename_in>,
        name: SharedOsStr,
        newname: SharedOsStr,
    },
    Link {
        arg: Shared<fuse_link_in>,
        newname: SharedOsStr,
    },
    Open {
        arg: Shared<fuse_open_in>,
    },
    Read {
        arg: Shared<fuse_read_in>,
    },
    Write {
        arg: Shared<fuse_write_in>,
        data: Bytes,
    },
    Release {
        arg: Shared<fuse_release_in>,
    },
    Statfs,
    Fsync {
        arg: Shared<fuse_fsync_in>,
    },
    Setxattr {
        arg: Shared<fuse_setxattr_in>,
        name: SharedOsStr,
        value: Bytes,
    },
    Getxattr {
        arg: Shared<fuse_getxattr_in>,
        name: SharedOsStr,
    },
    Listxattr {
        arg: Shared<fuse_getxattr_in>,
    },
    Removexattr {
        name: SharedOsStr,
    },
    Flush {
        arg: Shared<fuse_flush_in>,
    },
    Opendir {
        arg: Shared<fuse_open_in>,
    },
    Readdir {
        arg: Shared<fuse_read_in>,
        plus: bool,
    },
    Releasedir {
        arg: Shared<fuse_release_in>,
    },
    Fsyncdir {
        arg: Shared<fuse_fsync_in>,
    },
    Getlk {
        arg: Shared<fuse_lk_in>,
    },
    Setlk {
        arg: Shared<fuse_lk_in>,
        sleep: bool,
    },
    Access {
        arg: Shared<fuse_access_in>,
    },
    Create {
        arg: Shared<fuse_create_in>,
        name: SharedOsStr,
    },
    Interrupt {
        arg: Shared<fuse_interrupt_in>,
    },
    Bmap {
        arg: Shared<fuse_bmap_in>,
    },
    Fallocate {
        arg: Shared<fuse_fallocate_in>,
    },
    Rename2 {
        arg: Shared<fuse_rename2_in>,
        name: SharedOsStr,
        newname: SharedOsStr,
    },
    CopyFileRange {
        arg: Shared<fuse_copy_file_range_in>,
    },
    BatchForget {
        arg: Shared<fuse_batch_forget_in>,
        forgets: SharedSlice<fuse_forget_one>,
    },
    NotifyReply {
        arg: Shared<fuse_notify_retrieve_in>,
        data: Bytes,
    },
    Poll {
        arg: Shared<fuse_poll_in>,
    },
    Unknown,
}

// TODO: add opcodes:
// Ioctl,

#[derive(Debug)]
pub struct Parser<'a> {
    buf: &'a mut Bytes,
}

impl<'a> Parser<'a> {
    pub fn new(buf: &'a mut Bytes) -> Self {
        Self { buf }
    }

    fn fetch_bytes(&mut self, count: usize) -> io::Result<Bytes> {
        if self.buf.len() < count {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "fetch"));
        }
        Ok(self.buf.split_to(count))
    }

    fn fetch_array<T>(&mut self, count: usize) -> io::Result<SharedSlice<T>> {
        self.fetch_bytes(mem::size_of::<T>() * count)
            .map(|bytes| unsafe { SharedSlice::from_bytes_unchecked(bytes, count) })
    }

    fn fetch_str(&mut self) -> io::Result<SharedOsStr> {
        let len = self
            .buf
            .as_ref()
            .iter()
            .position(|&b| b == b'\0')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "fetch_str: missing \\0"))?;
        self.fetch_bytes(len).map(SharedOsStr::from_bytes)
    }

    fn fetch<T>(&mut self) -> io::Result<Shared<T>> {
        self.fetch_bytes(mem::size_of::<T>())
            .map(|data| unsafe { Shared::new_unchecked(data) })
    }

    pub fn parse(&mut self, opcode: fuse_opcode) -> io::Result<RequestKind> {
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
                let arg = self.fetch::<fuse_write_in>()?;
                let data = self.fetch_bytes(arg.size as usize)?;
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
                Ok(RequestKind::Readdir { arg, plus: false })
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
                Ok(RequestKind::Readdir { arg, plus: true })
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
                let arg = self.fetch::<fuse_notify_retrieve_in>()?;
                let data = self.fetch_bytes(arg.size as usize)?;
                Ok(RequestKind::NotifyReply { arg, data })
            }
            _ => Ok(RequestKind::Unknown),
        }
    }
}

#[derive(Debug)]
pub struct Shared<T>(Bytes, PhantomData<T>);

impl<T> Shared<T> {
    #[inline]
    unsafe fn new_unchecked(bytes: Bytes) -> Self {
        debug_assert_eq!(bytes.len(), mem::size_of::<T>());
        Self(bytes, PhantomData)
    }
}

impl<T> Deref for Shared<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*(self.0.as_ref().as_ptr() as *const T) }
    }
}

#[derive(Debug)]
pub struct SharedOsStr(Bytes);

impl SharedOsStr {
    #[inline]
    fn from_bytes(bytes: Bytes) -> Self {
        Self(bytes)
    }
}

impl Deref for SharedOsStr {
    type Target = OsStr;

    fn deref(&self) -> &Self::Target {
        OsStr::from_bytes(self.0.as_ref())
    }
}

#[derive(Debug)]
pub struct SharedSlice<T> {
    bytes: Bytes,
    count: usize,
    _marker: PhantomData<T>,
}

impl<T> SharedSlice<T> {
    #[inline]
    unsafe fn from_bytes_unchecked(bytes: Bytes, count: usize) -> Self {
        debug_assert_eq!(bytes.len(), mem::size_of::<T>() * count);
        Self {
            bytes,
            count,
            _marker: PhantomData,
        }
    }
}

impl<T> Deref for SharedSlice<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        unsafe {
            std::slice::from_raw_parts(
                self.bytes.as_ref().as_ptr() as *const T, //
                self.count,
            )
        }
    }
}

pub(crate) async fn receive_msg<R: ?Sized>(
    reader: &mut R,
    bufsize: usize,
) -> io::Result<Option<Request>>
where
    R: AsyncRead + Unpin,
{
    let mut header = mem::MaybeUninit::<fuse_in_header>::uninit();
    let mut buf = BytesMut::with_capacity(bufsize);
    unsafe {
        buf.set_len(bufsize);
    }

    let terminated = poll_fn(|cx| {
        let mut vec = [
            IoSliceMut::new(unsafe {
                std::slice::from_raw_parts_mut(
                    header.as_mut_ptr() as *mut u8,
                    mem::size_of::<fuse_in_header>(),
                )
            }),
            IoSliceMut::new(buf.as_mut()),
        ];

        loop {
            match ready!(Pin::new(&mut *reader).poll_read_vectored(cx, &mut vec[..])) {
                Ok(len) => {
                    if len < mem::size_of::<fuse_in_header>() {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "the received data from the kernel is too short",
                        )));
                    }
                    unsafe {
                        buf.set_len(len - mem::size_of::<fuse_in_header>());
                    }
                    return Poll::Ready(Ok(false));
                }
                Err(err) => match err.raw_os_error() {
                    Some(libc::ENOENT) | Some(libc::EINTR) => {
                        log::debug!("continue reading from the kernel");
                        continue;
                    }
                    Some(libc::ENODEV) => return Poll::Ready(Ok(true)),
                    _ => return Poll::Ready(Err(err)),
                },
            }
        }
    })
    .await?;

    if terminated {
        return Ok(None);
    }

    let header = unsafe { header.assume_init() };
    let mut buf = buf.freeze();

    if header.len as usize != mem::size_of::<fuse_in_header>() + buf.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the payload length is mismatched to the header value",
        ));
    }

    let opcode = fuse_opcode::try_from(header.opcode)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let kind = Parser::new(&mut buf).parse(opcode)?;

    Ok(Some(Request {
        header,
        kind,
        _p: (),
    }))
}
