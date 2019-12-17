//! Receives and parses FUSE requests.

use crate::kernel::{
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
    ffi::OsStr, //
    io,
    mem,
    os::unix::ffi::OsStrExt,
};

/// The kind of FUSE request.
#[doc(hidden)] // TODO: document
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
    },
    Poll {
        arg: &'a fuse_poll_in,
    },
    Unknown,
}

// TODO: add opcodes:
// Ioctl,

#[doc(hidden)] // TODO: document
#[derive(Debug)]
pub struct Parser<'a> {
    bytes: &'a [u8],
    offset: usize,
}

#[doc(hidden)] // TODO: document
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

    pub fn parse(&mut self, opcode: fuse_opcode) -> io::Result<RequestKind<'a>> {
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
                Ok(RequestKind::Write { arg })
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
                Ok(RequestKind::NotifyReply { arg })
            }
            _ => Ok(RequestKind::Unknown),
        }
    }
}
