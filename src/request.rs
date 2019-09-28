//! Requests from the kernel.

use fuse_async_abi::{
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

#[derive(Debug)]
pub enum Arg<'a, T = &'a [u8]> {
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
    Write {
        arg: &'a WriteIn,
        data: T,
    },
    Release(&'a ReleaseIn),
    Statfs,
    Fsync(&'a FsyncIn),
    Setxattr {
        arg: &'a SetxattrIn,
        name: &'a OsStr,
        value: T,
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

pub fn parse<'a>(buf: &'a [u8]) -> io::Result<(&'a InHeader, Arg<'a, &'a [u8]>)> {
    let (header, payload) = parse_header(buf)?;
    if buf.len() < header.len as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "received data is too short",
        ));
    }

    let arg = parse_arg(header, payload)?;

    Ok((header, arg))
}

#[allow(clippy::cognitive_complexity)]
pub fn parse_arg<'a>(header: &InHeader, payload: &'a [u8]) -> io::Result<Arg<'a, &'a [u8]>> {
    let arg = match header.opcode {
        Opcode::Init => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Init(arg)
        }
        Opcode::Destroy => {
            debug_assert!(payload.is_empty());
            Arg::Destroy
        }
        Opcode::Lookup => {
            let (name, remains) = fetch_str(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Lookup { name }
        }
        Opcode::Forget => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Forget(arg)
        }
        Opcode::Getattr => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Getattr(arg)
        }
        Opcode::Setattr => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Setattr(arg)
        }
        Opcode::Readlink => {
            debug_assert!(payload.is_empty());
            Arg::Readlink
        }
        Opcode::Symlink => {
            let (name, remains) = fetch_str(payload)?;
            let (link, _remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Arg::Symlink { name, link }
        }
        Opcode::Mknod => {
            let (arg, remains) = fetch(payload)?;
            let (name, remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Arg::Mknod { arg, name }
        }
        Opcode::Mkdir => {
            let (arg, remains) = fetch(payload)?;
            let (name, remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Arg::Mkdir { arg, name }
        }
        Opcode::Unlink => {
            let (name, remains) = fetch_str(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Unlink { name }
        }
        Opcode::Rmdir => {
            let (name, remains) = fetch_str(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Rmdir { name }
        }
        Opcode::Rename => {
            let (arg, remains) = fetch(payload)?;
            let (name, remains) = fetch_str(remains)?;
            let (newname, _remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Arg::Rename { arg, name, newname }
        }
        Opcode::Link => {
            let (arg, remains) = fetch(payload)?;
            let (newname, _remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Arg::Link { arg, newname }
        }
        Opcode::Open => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Open(arg)
        }
        Opcode::Read => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Read(arg)
        }
        Opcode::Write => {
            let (arg, data) = fetch(payload)?;
            Arg::Write { arg, data }
        }
        Opcode::Release => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Release(arg)
        }
        Opcode::Statfs => {
            debug_assert!(payload.is_empty());
            Arg::Statfs
        }
        Opcode::Fsync => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Fsync(arg)
        }
        Opcode::Setxattr => {
            let (arg, remains) = fetch(payload)?;
            let (name, value) = fetch_str(remains)?;
            Arg::Setxattr { arg, name, value }
        }
        Opcode::Getxattr => {
            let (arg, remains) = fetch(payload)?;
            let (name, remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Arg::Getxattr { arg, name }
        }
        Opcode::Listxattr => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Listxattr { arg }
        }
        Opcode::Removexattr => {
            let (name, remains) = fetch_str(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Removexattr { name }
        }
        Opcode::Flush => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Flush(arg)
        }
        Opcode::Opendir => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Opendir(arg)
        }
        Opcode::Readdir => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Readdir(arg)
        }
        Opcode::Releasedir => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Releasedir(arg)
        }
        Opcode::Fsyncdir => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Fsyncdir(arg)
        }
        Opcode::Getlk => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Getlk(arg)
        }
        Opcode::Setlk => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Setlk(arg)
        }
        Opcode::Setlkw => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Setlkw(arg)
        }
        Opcode::Access => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Access(arg)
        }
        Opcode::Create => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Create(arg)
        }
        Opcode::Bmap => {
            let (arg, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Arg::Bmap(arg)
        }
        _ => Arg::Unknown,
    };

    Ok(arg)
}

#[allow(clippy::cast_ptr_alignment)]
fn parse_header<'a>(buf: &'a [u8]) -> io::Result<(&'a InHeader, &'a [u8])> {
    const IN_HEADER_SIZE: usize = mem::size_of::<InHeader>();

    if buf.len() < IN_HEADER_SIZE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "in_header"));
    }
    let (header, remains) = buf.split_at(IN_HEADER_SIZE);

    let header = unsafe { &*(header.as_ptr() as *const InHeader) };

    Ok((header, remains))
}

fn fetch<'a, T>(buf: &'a [u8]) -> io::Result<(&'a T, &'a [u8])> {
    if buf.len() < mem::size_of::<T>() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let (data, remains) = buf.split_at(mem::size_of::<T>());
    Ok((unsafe { &*(data.as_ptr() as *const T) }, remains))
}

fn fetch_str<'a>(buf: &'a [u8]) -> io::Result<(&'a OsStr, &'a [u8])> {
    let pos = buf.iter().position(|&b| b == b'\0');
    let (s, remains) = if let Some(pos) = pos {
        let (s, remains) = buf.split_at(pos);
        let remains = &remains[1..]; // skip '\0'
        (s, remains)
    } else {
        (buf, &[] as &[u8])
    };
    Ok((OsStr::from_bytes(s), remains))
}
