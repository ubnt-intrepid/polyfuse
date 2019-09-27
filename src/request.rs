//! Requests from the kernel.

use std::{ffi::OsStr, io, mem, os::unix::ffi::OsStrExt};
use tokio_fuse_abi::{
    InHeader,
    OpAccess, //
    OpBmap,
    OpCreate,
    OpFlush,
    OpForget,
    OpFsync,
    OpGetattr,
    OpGetxattr,
    OpInit,
    OpLink,
    OpLk,
    OpMkdir,
    OpMknod,
    OpOpen,
    OpRead,
    OpRelease,
    OpRename,
    OpSetattr,
    OpSetxattr,
    OpWrite,
    Opcode,
};

#[derive(Debug)]
pub enum Op<'a> {
    Init(&'a OpInit),
    Destroy,
    Lookup {
        name: &'a OsStr,
    },
    Forget(&'a OpForget),
    Getattr(&'a OpGetattr),
    Setattr(&'a OpSetattr),
    Readlink,
    Symlink {
        name: &'a OsStr,
        link: &'a OsStr,
    },
    Mknod {
        op: &'a OpMknod,
        name: &'a OsStr,
    },
    Mkdir {
        op: &'a OpMkdir,
        name: &'a OsStr,
    },
    Unlink {
        name: &'a OsStr,
    },
    Rmdir {
        name: &'a OsStr,
    },
    Rename {
        op: &'a OpRename,
        name: &'a OsStr,
        newname: &'a OsStr,
    },
    Link {
        op: &'a OpLink,
        newname: &'a OsStr,
    },
    Open(&'a OpOpen),
    Read(&'a OpRead),
    Write {
        op: &'a OpWrite,
        data: &'a [u8],
    },
    Release(&'a OpRelease),
    Statfs,
    Fsync(&'a OpFsync),
    Setxattr {
        op: &'a OpSetxattr,
        name: &'a OsStr,
        value: &'a [u8],
    },
    Getxattr {
        op: &'a OpGetxattr,
        name: &'a OsStr,
    },
    Listxattr {
        op: &'a OpGetxattr,
    },
    Removexattr {
        name: &'a OsStr,
    },
    Flush(&'a OpFlush),
    Opendir(&'a OpOpen),
    Readdir(&'a OpRead),
    Releasedir(&'a OpRelease),
    Fsyncdir(&'a OpFsync),
    Getlk(&'a OpLk),
    Setlk(&'a OpLk),
    Setlkw(&'a OpLk),
    Access(&'a OpAccess),
    Create(&'a OpCreate),
    Bmap(&'a OpBmap),
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
    Unknown {
        opcode: Opcode,
        payload: &'a [u8],
    },
}

pub fn parse<'a>(buf: &'a [u8]) -> io::Result<(&'a InHeader, Op<'a>)> {
    let (header, payload) = parse_header(buf)?;
    if buf.len() < header.len() as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "received data is too short",
        ));
    }

    let op = match header.opcode() {
        Opcode::FUSE_INIT => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Init(op)
        }
        Opcode::FUSE_DESTROY => {
            debug_assert!(payload.is_empty());
            Op::Destroy
        }
        Opcode::FUSE_LOOKUP => {
            let (name, remains) = fetch_str(payload)?;
            debug_assert!(remains.is_empty());
            Op::Lookup { name }
        }
        Opcode::FUSE_FORGET => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Forget(op)
        }
        Opcode::FUSE_GETATTR => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Getattr(op)
        }
        Opcode::FUSE_SETATTR => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Setattr(op)
        }
        Opcode::FUSE_READLINK => {
            debug_assert!(payload.is_empty());
            Op::Readlink
        }
        Opcode::FUSE_SYMLINK => {
            let (name, remains) = fetch_str(payload)?;
            let (link, _remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Op::Symlink { name, link }
        }
        Opcode::FUSE_MKNOD => {
            let (op, remains) = fetch(payload)?;
            let (name, remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Op::Mknod { op, name }
        }
        Opcode::FUSE_MKDIR => {
            let (op, remains) = fetch(payload)?;
            let (name, remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Op::Mkdir { op, name }
        }
        Opcode::FUSE_UNLINK => {
            let (name, remains) = fetch_str(payload)?;
            debug_assert!(remains.is_empty());
            Op::Unlink { name }
        }
        Opcode::FUSE_RMDIR => {
            let (name, remains) = fetch_str(payload)?;
            debug_assert!(remains.is_empty());
            Op::Rmdir { name }
        }
        Opcode::FUSE_RENAME => {
            let (op, remains) = fetch(payload)?;
            let (name, remains) = fetch_str(remains)?;
            let (newname, _remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Op::Rename { op, name, newname }
        }
        Opcode::FUSE_LINK => {
            let (op, remains) = fetch(payload)?;
            let (newname, _remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Op::Link { op, newname }
        }
        Opcode::FUSE_OPEN => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Open(op)
        }
        Opcode::FUSE_READ => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Read(op)
        }
        Opcode::FUSE_WRITE => {
            let (op, data) = fetch(payload)?;
            Op::Write { op, data }
        }
        Opcode::FUSE_RELEASE => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Release(op)
        }
        Opcode::FUSE_STATFS => {
            debug_assert!(payload.is_empty());
            Op::Statfs
        }
        Opcode::FUSE_FSYNC => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Fsync(op)
        }
        Opcode::FUSE_SETXATTR => {
            let (op, remains) = fetch(payload)?;
            let (name, value) = fetch_str(remains)?;
            Op::Setxattr { op, name, value }
        }
        Opcode::FUSE_GETXATTR => {
            let (op, remains) = fetch(payload)?;
            let (name, remains) = fetch_str(remains)?;
            debug_assert!(remains.is_empty());
            Op::Getxattr { op, name }
        }
        Opcode::FUSE_LISTXATTR => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Listxattr { op }
        }
        Opcode::FUSE_REMOVEXATTR => {
            let (name, remains) = fetch_str(payload)?;
            debug_assert!(remains.is_empty());
            Op::Removexattr { name }
        }
        Opcode::FUSE_FLUSH => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Flush(op)
        }
        Opcode::FUSE_OPENDIR => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Opendir(op)
        }
        Opcode::FUSE_READDIR => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Readdir(op)
        }
        Opcode::FUSE_RELEASEDIR => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Releasedir(op)
        }
        Opcode::FUSE_FSYNCDIR => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Fsyncdir(op)
        }
        Opcode::FUSE_GETLK => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Getlk(op)
        }
        Opcode::FUSE_SETLK => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Setlk(op)
        }
        Opcode::FUSE_SETLKW => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Setlkw(op)
        }
        Opcode::FUSE_ACCESS => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Access(op)
        }
        Opcode::FUSE_CREATE => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Create(op)
        }
        Opcode::FUSE_BMAP => {
            let (op, remains) = fetch(payload)?;
            debug_assert!(remains.is_empty());
            Op::Bmap(op)
        }
        // Opcode::FUSE_INTERRUPT => unimplemented!(),
        // Opcode::FUSE_IOCTL => unimplemented!(),
        // Opcode::FUSE_POLL => unimplemented!(),
        // Opcode::FUSE_NOTIFY_REPLY => unimplemented!(),
        // Opcode::FUSE_BATCH_FORGET => unimplemented!(),
        // Opcode::FUSE_FALLOCATE => unimplemented!(),
        // Opcode::FUSE_READDIRPLUS => unimplemented!(),
        // Opcode::FUSE_RENAME2 => unimplemented!(),
        // Opcode::FUSE_LSEEK => unimplemented!(),
        // Opcode::FUSE_COPY_FILE_RANGE => unimplemented!(),
        opcode => Op::Unknown { opcode, payload },
    };
    Ok((header, op))
}

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
