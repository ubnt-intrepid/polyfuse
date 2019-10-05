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
use std::{ffi::OsStr, io, mem, os::unix::ffi::OsStrExt};

const IN_HEADER_SIZE: usize = mem::size_of::<InHeader>();

#[derive(Debug)]
pub enum Arg<'a> {
    Init(&'a InitIn),
    Destroy,
    Lookup {
        name: &'a OsStr,
    },
    Forget(&'a ForgetIn),
    Getattr(&'a GetattrIn),
    Setattr(&'a SetattrIn),
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
    Open(&'a OpenIn),
    Read(&'a ReadIn),
    Write(&'a WriteIn),
    Release(&'a ReleaseIn),
    Statfs,
    Fsync(&'a FsyncIn),
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
    Flush(&'a FlushIn),
    Opendir(&'a OpenIn),
    Readdir(&'a ReadIn),
    Releasedir(&'a ReleaseIn),
    Fsyncdir(&'a FsyncIn),
    Getlk(&'a LkIn),
    Setlk(&'a LkIn),
    Setlkw(&'a LkIn),
    Access(&'a AccessIn),
    Create(&'a CreateIn),
    Bmap(&'a BmapIn),
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

pub fn parse<'a>(buf: &'a [u8]) -> io::Result<(&'a InHeader, Arg<'a>, usize)> {
    let header = parse_header(buf)?;
    if buf.len() < header.len as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "received data is too short",
        ));
    }

    let (arg, arg_len) = parse_arg(header, &buf[IN_HEADER_SIZE..])?;

    Ok((header, arg, IN_HEADER_SIZE + arg_len))
}

#[allow(clippy::cast_ptr_alignment)]
pub fn parse_header<'a>(buf: &'a [u8]) -> io::Result<&'a InHeader> {
    let header = buf
        .get(..IN_HEADER_SIZE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "in_header"))?;
    Ok(unsafe { &*(header.as_ptr() as *const InHeader) })
}

pub fn parse_arg<'a>(header: &InHeader, payload: &'a [u8]) -> io::Result<(Arg<'a>, usize)> {
    let mut cursor = Cursor::new(payload);
    let arg = match header.opcode {
        Opcode::Init => {
            let arg = cursor.fetch()?;
            Arg::Init(arg)
        }
        Opcode::Destroy => Arg::Destroy,
        Opcode::Lookup => {
            let name = cursor.fetch_str()?;
            Arg::Lookup { name }
        }
        Opcode::Forget => {
            let arg = cursor.fetch()?;
            Arg::Forget(arg)
        }
        Opcode::Getattr => {
            let arg = cursor.fetch()?;
            Arg::Getattr(arg)
        }
        Opcode::Setattr => {
            let arg = cursor.fetch()?;
            Arg::Setattr(arg)
        }
        Opcode::Readlink => Arg::Readlink,
        Opcode::Symlink => {
            let name = cursor.fetch_str()?;
            let link = cursor.fetch_str()?;
            Arg::Symlink { name, link }
        }
        Opcode::Mknod => {
            let arg = cursor.fetch()?;
            let name = cursor.fetch_str()?;
            Arg::Mknod { arg, name }
        }
        Opcode::Mkdir => {
            let arg = cursor.fetch()?;
            let name = cursor.fetch_str()?;
            Arg::Mkdir { arg, name }
        }
        Opcode::Unlink => {
            let name = cursor.fetch_str()?;
            Arg::Unlink { name }
        }
        Opcode::Rmdir => {
            let name = cursor.fetch_str()?;
            Arg::Rmdir { name }
        }
        Opcode::Rename => {
            let arg = cursor.fetch()?;
            let name = cursor.fetch_str()?;
            let newname = cursor.fetch_str()?;
            Arg::Rename { arg, name, newname }
        }
        Opcode::Link => {
            let arg = cursor.fetch()?;
            let newname = cursor.fetch_str()?;
            Arg::Link { arg, newname }
        }
        Opcode::Open => {
            let arg = cursor.fetch()?;
            Arg::Open(arg)
        }
        Opcode::Read => {
            let arg = cursor.fetch()?;
            Arg::Read(arg)
        }
        Opcode::Write => {
            let arg = cursor.fetch()?;
            Arg::Write(arg)
        }
        Opcode::Release => {
            let arg = cursor.fetch()?;
            Arg::Release(arg)
        }
        Opcode::Statfs => Arg::Statfs,
        Opcode::Fsync => {
            let arg = cursor.fetch()?;
            Arg::Fsync(arg)
        }
        Opcode::Setxattr => {
            let arg: &SetxattrIn = cursor.fetch()?;
            let name = cursor.fetch_str()?;
            let value = cursor.fetch_bytes(arg.size as usize)?;
            Arg::Setxattr { arg, name, value }
        }
        Opcode::Getxattr => {
            let arg = cursor.fetch()?;
            let name = cursor.fetch_str()?;
            Arg::Getxattr { arg, name }
        }
        Opcode::Listxattr => {
            let arg = cursor.fetch()?;
            Arg::Listxattr { arg }
        }
        Opcode::Removexattr => {
            let name = cursor.fetch_str()?;
            Arg::Removexattr { name }
        }
        Opcode::Flush => {
            let arg = cursor.fetch()?;
            Arg::Flush(arg)
        }
        Opcode::Opendir => {
            let arg = cursor.fetch()?;
            Arg::Opendir(arg)
        }
        Opcode::Readdir => {
            let arg = cursor.fetch()?;
            Arg::Readdir(arg)
        }
        Opcode::Releasedir => {
            let arg = cursor.fetch()?;
            Arg::Releasedir(arg)
        }
        Opcode::Fsyncdir => {
            let arg = cursor.fetch()?;
            Arg::Fsyncdir(arg)
        }
        Opcode::Getlk => {
            let arg = cursor.fetch()?;
            Arg::Getlk(arg)
        }
        Opcode::Setlk => {
            let arg = cursor.fetch()?;
            Arg::Setlk(arg)
        }
        Opcode::Setlkw => {
            let arg = cursor.fetch()?;
            Arg::Setlkw(arg)
        }
        Opcode::Access => {
            let arg = cursor.fetch()?;
            Arg::Access(arg)
        }
        Opcode::Create => {
            let arg = cursor.fetch()?;
            Arg::Create(arg)
        }
        Opcode::Bmap => {
            let arg = cursor.fetch()?;
            Arg::Bmap(arg)
        }
        _ => Arg::Unknown,
    };

    Ok((arg, cursor.offset))
}

struct Cursor<'a> {
    buf: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, offset: 0 }
    }

    fn fetch<T>(&mut self) -> io::Result<&'a T> {
        self.fetch_bytes(mem::size_of::<T>())
            .map(|data| unsafe { &*(data.as_ptr() as *const T) })
    }

    fn fetch_str(&mut self) -> io::Result<&'a OsStr> {
        let len = self.buf[self.offset..]
            .iter()
            .position(|&b| b == b'\0')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "fetch_str: missing \\0"))?;
        self.fetch_bytes(len).map(OsStr::from_bytes)
    }

    fn fetch_bytes(&mut self, count: usize) -> io::Result<&'a [u8]> {
        if self.buf.len() < self.offset + count {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "fetch"));
        }

        let data = &self.buf[self.offset..self.offset + count];
        self.offset += count;

        Ok(data)
    }
}
