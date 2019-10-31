use polyfuse_sys::abi::{
    fuse_access_in, //
    fuse_bmap_in,
    fuse_create_in,
    fuse_flush_in,
    fuse_forget_in,
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
    fuse_opcode,
    fuse_open_in,
    fuse_read_in,
    fuse_release_in,
    fuse_rename_in,
    fuse_setattr_in,
    fuse_setxattr_in,
    fuse_write_in,
};
use std::{convert::TryFrom, ffi::OsStr, io, mem, os::unix::ffi::OsStrExt};

/// An incoming FUSE request received from the kernel.
#[derive(Debug)]
pub struct Request<'a> {
    pub(crate) header: &'a fuse_in_header,
    pub(crate) kind: RequestKind<'a>,
    _p: (),
}

impl<'a> Request<'a> {
    pub(crate) const fn new(header: &'a fuse_in_header, kind: RequestKind<'a>) -> Self {
        Self {
            header,
            kind,
            _p: (),
        }
    }
}

#[derive(Debug)]
pub enum RequestKind<'a> {
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

impl Request<'_> {
    pub fn unique(&self) -> u64 {
        self.header.unique
    }

    pub fn opcode(&self) -> Option<fuse_opcode> {
        fuse_opcode::try_from(self.header.opcode).ok()
    }
}

trait FromBytes<'a> {
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
    fuse_in_header,
    fuse_init_in,
    fuse_forget_in,
    fuse_getattr_in,
    fuse_setattr_in,
    fuse_mknod_in,
    fuse_mkdir_in,
    fuse_rename_in,
    fuse_link_in,
    fuse_open_in,
    fuse_read_in,
    fuse_write_in,
    fuse_release_in,
    fuse_fsync_in,
    fuse_setxattr_in,
    fuse_getxattr_in,
    fuse_flush_in,
    fuse_lk_in,
    fuse_access_in,
    fuse_create_in,
    fuse_interrupt_in,
    fuse_bmap_in,
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

    pub fn parse(&mut self) -> io::Result<(&'a fuse_in_header, RequestKind<'a>, usize)> {
        let header = self.parse_header()?;
        let arg = self.parse_arg(header)?;
        Ok((header, arg, self.offset()))
    }

    #[allow(clippy::cast_ptr_alignment)]
    fn parse_header(&mut self) -> io::Result<&'a fuse_in_header> {
        let header = self.fetch::<fuse_in_header>()?;

        if self.buf.len() < header.len as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "received data is too short",
            ));
        }

        Ok(header)
    }

    fn parse_arg(&mut self, header: &'a fuse_in_header) -> io::Result<RequestKind<'a>> {
        match fuse_opcode::try_from(header.opcode).ok() {
            Some(fuse_opcode::FUSE_INIT) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Init { arg })
            }
            Some(fuse_opcode::FUSE_DESTROY) => Ok(RequestKind::Destroy),
            Some(fuse_opcode::FUSE_LOOKUP) => {
                let name = self.fetch_str()?;
                Ok(RequestKind::Lookup { name })
            }
            Some(fuse_opcode::FUSE_FORGET) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Forget { arg })
            }
            Some(fuse_opcode::FUSE_GETATTR) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Getattr { arg })
            }
            Some(fuse_opcode::FUSE_SETATTR) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Setattr { arg })
            }
            Some(fuse_opcode::FUSE_READLINK) => Ok(RequestKind::Readlink),
            Some(fuse_opcode::FUSE_SYMLINK) => {
                let name = self.fetch_str()?;
                let link = self.fetch_str()?;
                Ok(RequestKind::Symlink { name, link })
            }
            Some(fuse_opcode::FUSE_MKNOD) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(RequestKind::Mknod { arg, name })
            }
            Some(fuse_opcode::FUSE_MKDIR) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(RequestKind::Mkdir { arg, name })
            }
            Some(fuse_opcode::FUSE_UNLINK) => {
                let name = self.fetch_str()?;
                Ok(RequestKind::Unlink { name })
            }
            Some(fuse_opcode::FUSE_RMDIR) => {
                let name = self.fetch_str()?;
                Ok(RequestKind::Rmdir { name })
            }
            Some(fuse_opcode::FUSE_RENAME) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                let newname = self.fetch_str()?;
                Ok(RequestKind::Rename { arg, name, newname })
            }
            Some(fuse_opcode::FUSE_LINK) => {
                let arg = self.fetch()?;
                let newname = self.fetch_str()?;
                Ok(RequestKind::Link { arg, newname })
            }
            Some(fuse_opcode::FUSE_OPEN) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Open { arg })
            }
            Some(fuse_opcode::FUSE_READ) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Read { arg })
            }
            Some(fuse_opcode::FUSE_WRITE) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Write { arg })
            }
            Some(fuse_opcode::FUSE_RELEASE) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Release { arg })
            }
            Some(fuse_opcode::FUSE_STATFS) => Ok(RequestKind::Statfs),
            Some(fuse_opcode::FUSE_FSYNC) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Fsync { arg })
            }
            Some(fuse_opcode::FUSE_SETXATTR) => {
                let arg: &fuse_setxattr_in = self.fetch()?;
                let name = self.fetch_str()?;
                let value = self.fetch_bytes(arg.size as usize)?;
                Ok(RequestKind::Setxattr { arg, name, value })
            }
            Some(fuse_opcode::FUSE_GETXATTR) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(RequestKind::Getxattr { arg, name })
            }
            Some(fuse_opcode::FUSE_LISTXATTR) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Listxattr { arg })
            }
            Some(fuse_opcode::FUSE_REMOVEXATTR) => {
                let name = self.fetch_str()?;
                Ok(RequestKind::Removexattr { name })
            }
            Some(fuse_opcode::FUSE_FLUSH) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Flush { arg })
            }
            Some(fuse_opcode::FUSE_OPENDIR) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Opendir { arg })
            }
            Some(fuse_opcode::FUSE_READDIR) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Readdir { arg })
            }
            Some(fuse_opcode::FUSE_RELEASEDIR) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Releasedir { arg })
            }
            Some(fuse_opcode::FUSE_FSYNCDIR) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Fsyncdir { arg })
            }
            Some(fuse_opcode::FUSE_GETLK) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Getlk { arg })
            }
            Some(fuse_opcode::FUSE_SETLK) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Setlk { arg, sleep: false })
            }
            Some(fuse_opcode::FUSE_SETLKW) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Setlk { arg, sleep: true })
            }
            Some(fuse_opcode::FUSE_ACCESS) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Access { arg })
            }
            Some(fuse_opcode::FUSE_CREATE) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(RequestKind::Create { arg, name })
            }
            Some(fuse_opcode::FUSE_INTERRUPT) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Interrupt { arg })
            }
            Some(fuse_opcode::FUSE_BMAP) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Bmap { arg })
            }
            _ => Ok(RequestKind::Unknown),
        }
    }
}
