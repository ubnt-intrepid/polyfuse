use polyfuse_kernel::{self as kernel, fuse_opcode};
use polyfuse_types::types::FileLock;
use std::{convert::TryFrom, ffi::OsStr, fmt, io, marker::PhantomData, mem, os::unix::prelude::*};

pub struct Request<'a> {
    pub header: &'a kernel::fuse_in_header,
    pub arg: Arg<'a>,
}

impl<'a> Request<'a> {
    pub fn parse(mut buf: &'a [u8]) -> anyhow::Result<Self> {
        if buf.len() < mem::size_of::<kernel::fuse_in_header>() {
            anyhow::bail!("request is too short");
        }

        let header = unsafe { &*(buf.as_ptr().cast::<kernel::fuse_in_header>()) };
        buf = &buf[mem::size_of::<kernel::fuse_in_header>()..];

        let arg = Parser::new(header, buf).parse()?;

        Ok(Self { header, arg })
    }
}

pub enum Arg<'a> {
    Init {
        arg: &'a kernel::fuse_init_in,
    },
    Destroy,
    Forget {
        arg: &'a kernel::fuse_forget_in,
    },
    BatchForget {
        forgets: &'a [kernel::fuse_forget_one],
    },
    Interrupt(Interrupt<'a>),
    NotifyReply(NotifyReply<'a>),
    Lookup(Lookup<'a>),
    Getattr(Getattr<'a>),
    Setattr(Setattr<'a>),
    Readlink(Readlink<'a>),
    Symlink(Symlink<'a>),
    Mknod(Mknod<'a>),
    Mkdir(Mkdir<'a>),
    Unlink(Unlink<'a>),
    Rmdir(Rmdir<'a>),
    Rename(Rename<'a>),
    Rename2(Rename2<'a>),
    Link(Link<'a>),
    Open(Open<'a>),
    Read(Read<'a>),
    Write(Write<'a>),
    Release(Release<'a>),
    Statfs(Statfs<'a>),
    Fsync(Fsync<'a>),
    Setxattr(Setxattr<'a>),
    Getxattr(Getxattr<'a>),
    Listxattr(Listxattr<'a>),
    Removexattr(Removexattr<'a>),
    Flush(Flush<'a>),
    Opendir(Opendir<'a>),
    Readdir(Readdir<'a>),
    Readdirplus(Readdirplus<'a>),
    Releasedir(Releasedir<'a>),
    Fsyncdir(Fsyncdir<'a>),
    Getlk(Getlk<'a>),
    Setlk(Setlk<'a>),
    Flock(Flock<'a>),
    Access(Access<'a>),
    Create(Create<'a>),
    Bmap(Bmap<'a>),
    Fallocate(Fallocate<'a>),
    CopyFileRange(CopyFileRange<'a>),
    Poll(Poll<'a>),

    Unknown,
}

impl fmt::Debug for Arg<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Arg").finish()
    }
}

pub struct Interrupt<'a> {
    pub arg: &'a kernel::fuse_interrupt_in,
}

pub struct NotifyReply<'a> {
    pub arg: &'a kernel::fuse_notify_retrieve_in,
}

pub struct Lookup<'a> {
    pub name: &'a OsStr,
}

pub struct Getattr<'a> {
    pub arg: &'a kernel::fuse_getattr_in,
}

pub struct Setattr<'a> {
    pub arg: &'a kernel::fuse_setattr_in,
}

pub struct Readlink<'a> {
    _marker: PhantomData<&'a ()>,
}

pub struct Symlink<'a> {
    pub name: &'a OsStr,
    pub link: &'a OsStr,
}

pub struct Mknod<'a> {
    pub arg: &'a kernel::fuse_mknod_in,
    pub name: &'a OsStr,
}

pub struct Mkdir<'a> {
    pub arg: &'a kernel::fuse_mkdir_in,
    pub name: &'a OsStr,
}

pub struct Unlink<'a> {
    pub name: &'a OsStr,
}

pub struct Rmdir<'a> {
    pub name: &'a OsStr,
}

pub struct Rename<'a> {
    pub arg: &'a kernel::fuse_rename_in,
    pub name: &'a OsStr,
    pub newname: &'a OsStr,
}

pub struct Rename2<'a> {
    pub arg: &'a kernel::fuse_rename2_in,
    pub name: &'a OsStr,
    pub newname: &'a OsStr,
}

pub struct Link<'a> {
    pub arg: &'a kernel::fuse_link_in,
    pub newname: &'a OsStr,
}

pub struct Open<'a> {
    pub arg: &'a kernel::fuse_open_in,
}

pub struct Read<'a> {
    pub arg: &'a kernel::fuse_read_in,
}

pub struct Write<'a> {
    pub arg: &'a kernel::fuse_write_in,
}

pub struct Release<'a> {
    pub arg: &'a kernel::fuse_release_in,
}

pub struct Statfs<'a> {
    _marker: PhantomData<&'a ()>,
}

pub struct Fsync<'a> {
    pub arg: &'a kernel::fuse_fsync_in,
}

pub struct Setxattr<'a> {
    pub arg: &'a kernel::fuse_setxattr_in,
    pub name: &'a OsStr,
    pub value: &'a [u8],
}

pub struct Getxattr<'a> {
    pub arg: &'a kernel::fuse_getxattr_in,
    pub name: &'a OsStr,
}

pub struct Listxattr<'a> {
    pub arg: &'a kernel::fuse_getxattr_in,
}

pub struct Removexattr<'a> {
    pub name: &'a OsStr,
}

pub struct Flush<'a> {
    pub arg: &'a kernel::fuse_flush_in,
}

pub struct Opendir<'a> {
    pub arg: &'a kernel::fuse_open_in,
}

pub struct Readdir<'a> {
    pub arg: &'a kernel::fuse_read_in,
}

pub struct Readdirplus<'a> {
    pub arg: &'a kernel::fuse_read_in,
}

pub struct Releasedir<'a> {
    pub arg: &'a kernel::fuse_release_in,
}

pub struct Fsyncdir<'a> {
    pub arg: &'a kernel::fuse_fsync_in,
}

pub struct Getlk<'a> {
    pub arg: &'a kernel::fuse_lk_in,
    pub lk: FileLock,
}

pub struct Setlk<'a> {
    pub arg: &'a kernel::fuse_lk_in,
    pub lk: FileLock,
    pub sleep: bool,
}

pub struct Flock<'a> {
    pub arg: &'a kernel::fuse_lk_in,
    pub op: u32,
}

pub struct Access<'a> {
    pub arg: &'a kernel::fuse_access_in,
}

pub struct Create<'a> {
    pub arg: &'a kernel::fuse_create_in,
    pub name: &'a OsStr,
}

pub struct Bmap<'a> {
    pub arg: &'a kernel::fuse_bmap_in,
}

pub struct Fallocate<'a> {
    pub arg: &'a kernel::fuse_fallocate_in,
}

pub struct CopyFileRange<'a> {
    pub arg: &'a kernel::fuse_copy_file_range_in,
}

pub struct Poll<'a> {
    pub arg: &'a kernel::fuse_poll_in,
}

// ==== parser ====

struct Parser<'a> {
    header: &'a kernel::fuse_in_header,
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Parser<'a> {
    fn new(header: &'a kernel::fuse_in_header, bytes: &'a [u8]) -> Self {
        Self {
            header,
            bytes,
            offset: 0,
        }
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
        self.fetch_bytes(len).map(|s| {
            self.offset = std::cmp::min(self.bytes.len(), self.offset + 1);
            OsStr::from_bytes(s)
        })
    }

    fn fetch<T>(&mut self) -> io::Result<&'a T> {
        self.fetch_bytes(mem::size_of::<T>())
            .map(|data| unsafe { &*(data.as_ptr() as *const T) })
    }

    fn parse(&mut self) -> io::Result<Arg<'a>> {
        let header = self.header;
        match fuse_opcode::try_from(header.opcode).ok() {
            Some(fuse_opcode::FUSE_INIT) => {
                let arg = self.fetch()?;
                Ok(Arg::Init { arg })
            }
            Some(fuse_opcode::FUSE_DESTROY) => Ok(Arg::Destroy),
            Some(fuse_opcode::FUSE_FORGET) => {
                let arg = self.fetch::<kernel::fuse_forget_in>()?;
                Ok(Arg::Forget { arg })
            }
            Some(fuse_opcode::FUSE_BATCH_FORGET) => {
                let arg = self.fetch::<kernel::fuse_batch_forget_in>()?;
                let forgets = self.fetch_array(arg.count as usize)?;
                Ok(Arg::BatchForget { forgets })
            }
            Some(fuse_opcode::FUSE_INTERRUPT) => {
                let arg = self.fetch()?;
                Ok(Arg::Interrupt(Interrupt { arg }))
            }
            Some(fuse_opcode::FUSE_NOTIFY_REPLY) => {
                let arg = self.fetch()?;
                Ok(Arg::NotifyReply(NotifyReply { arg }))
            }

            Some(fuse_opcode::FUSE_LOOKUP) => {
                let name = self.fetch_str()?;
                Ok(Arg::Lookup(Lookup { name }))
            }
            Some(fuse_opcode::FUSE_GETATTR) => {
                let arg = self.fetch()?;
                Ok(Arg::Getattr(Getattr { arg }))
            }
            Some(fuse_opcode::FUSE_SETATTR) => {
                let arg = self.fetch()?;
                Ok(Arg::Setattr(Setattr { arg }))
            }
            Some(fuse_opcode::FUSE_READLINK) => Ok(Arg::Readlink(Readlink {
                _marker: PhantomData,
            })),
            Some(fuse_opcode::FUSE_SYMLINK) => {
                let name = self.fetch_str()?;
                let link = self.fetch_str()?;
                Ok(Arg::Symlink(Symlink { name, link }))
            }
            Some(fuse_opcode::FUSE_MKNOD) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(Arg::Mknod(Mknod { arg, name }))
            }
            Some(fuse_opcode::FUSE_MKDIR) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(Arg::Mkdir(Mkdir { arg, name }))
            }
            Some(fuse_opcode::FUSE_UNLINK) => {
                let name = self.fetch_str()?;
                Ok(Arg::Unlink(Unlink { name }))
            }
            Some(fuse_opcode::FUSE_RMDIR) => {
                let name = self.fetch_str()?;
                Ok(Arg::Rmdir(Rmdir { name }))
            }

            Some(fuse_opcode::FUSE_RENAME) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                let newname = self.fetch_str()?;
                Ok(Arg::Rename(Rename { arg, name, newname }))
            }
            Some(fuse_opcode::FUSE_LINK) => {
                let arg = self.fetch()?;
                let newname = self.fetch_str()?;
                Ok(Arg::Link(Link { arg, newname }))
            }
            Some(fuse_opcode::FUSE_OPEN) => {
                let arg = self.fetch()?;
                Ok(Arg::Open(Open { arg }))
            }
            Some(fuse_opcode::FUSE_READ) => {
                let arg = self.fetch()?;
                Ok(Arg::Read(Read { arg }))
            }
            Some(fuse_opcode::FUSE_WRITE) => {
                let arg = self.fetch()?;
                Ok(Arg::Write(Write { arg }))
            }
            Some(fuse_opcode::FUSE_RELEASE) => {
                let arg = self.fetch()?;
                Ok(Arg::Release(Release { arg }))
            }
            Some(fuse_opcode::FUSE_STATFS) => Ok(Arg::Statfs(Statfs {
                _marker: PhantomData,
            })),
            Some(fuse_opcode::FUSE_FSYNC) => {
                let arg = self.fetch()?;
                Ok(Arg::Fsync(Fsync { arg }))
            }
            Some(fuse_opcode::FUSE_SETXATTR) => {
                let arg = self.fetch::<kernel::fuse_setxattr_in>()?;
                let name = self.fetch_str()?;
                let value = self.fetch_bytes(arg.size as usize)?;
                Ok(Arg::Setxattr(Setxattr { arg, name, value }))
            }
            Some(fuse_opcode::FUSE_GETXATTR) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(Arg::Getxattr(Getxattr { arg, name }))
            }
            Some(fuse_opcode::FUSE_LISTXATTR) => {
                let arg = self.fetch()?;
                Ok(Arg::Listxattr(Listxattr { arg }))
            }
            Some(fuse_opcode::FUSE_REMOVEXATTR) => {
                let name = self.fetch_str()?;
                Ok(Arg::Removexattr(Removexattr { name }))
            }
            Some(fuse_opcode::FUSE_FLUSH) => {
                let arg = self.fetch()?;
                Ok(Arg::Flush(Flush { arg }))
            }
            Some(fuse_opcode::FUSE_OPENDIR) => {
                let arg = self.fetch()?;
                Ok(Arg::Opendir(Opendir { arg }))
            }
            Some(fuse_opcode::FUSE_READDIR) => {
                let arg = self.fetch()?;
                Ok(Arg::Readdir(Readdir { arg }))
            }
            Some(fuse_opcode::FUSE_RELEASEDIR) => {
                let arg = self.fetch()?;
                Ok(Arg::Releasedir(Releasedir { arg }))
            }
            Some(fuse_opcode::FUSE_FSYNCDIR) => {
                let arg = self.fetch()?;
                Ok(Arg::Fsyncdir(Fsyncdir { arg }))
            }
            Some(fuse_opcode::FUSE_GETLK) => {
                let arg = self.fetch()?;
                let lk = fill_lock(&arg);
                Ok(Arg::Getlk(Getlk { arg, lk }))
            }
            Some(fuse_opcode::FUSE_SETLK) => {
                let arg = self.fetch()?;
                let lk = fill_lock(&arg);
                parse_lock_arg(arg, lk, false)
            }
            Some(fuse_opcode::FUSE_SETLKW) => {
                let arg = self.fetch()?;
                let lk = fill_lock(&arg);
                parse_lock_arg(arg, lk, true)
            }
            Some(fuse_opcode::FUSE_ACCESS) => {
                let arg = self.fetch()?;
                Ok(Arg::Access(Access { arg }))
            }
            Some(fuse_opcode::FUSE_CREATE) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(Arg::Create(Create { arg, name }))
            }
            Some(fuse_opcode::FUSE_BMAP) => {
                let arg = self.fetch()?;
                Ok(Arg::Bmap(Bmap { arg }))
            }
            Some(fuse_opcode::FUSE_FALLOCATE) => {
                let arg = self.fetch()?;
                Ok(Arg::Fallocate(Fallocate { arg }))
            }
            Some(fuse_opcode::FUSE_READDIRPLUS) => {
                let arg = self.fetch()?;
                Ok(Arg::Readdirplus(Readdirplus { arg }))
            }
            Some(fuse_opcode::FUSE_RENAME2) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                let newname = self.fetch_str()?;
                Ok(Arg::Rename2(Rename2 { arg, name, newname }))
            }
            Some(fuse_opcode::FUSE_COPY_FILE_RANGE) => {
                let arg = self.fetch()?;
                Ok(Arg::CopyFileRange(CopyFileRange { arg }))
            }
            Some(fuse_opcode::FUSE_POLL) => {
                let arg = self.fetch()?;
                Ok(Arg::Poll(Poll { arg }))
            }
            _ => Ok(Arg::Unknown),
        }
    }
}

fn fill_lock(arg: &kernel::fuse_lk_in) -> FileLock {
    FileLock {
        typ: arg.lk.typ,
        start: arg.lk.start,
        end: arg.lk.end,
        pid: arg.lk.pid,
        ..Default::default()
    }
}

fn parse_lock_arg(arg: &kernel::fuse_lk_in, lk: FileLock, sleep: bool) -> io::Result<Arg<'_>> {
    if arg.lk_flags & kernel::FUSE_LK_FLOCK == 0 {
        // POSIX file lock.
        return Ok(Arg::Setlk(Setlk { arg, lk, sleep }));
    }

    const F_RDLCK: u32 = libc::F_RDLCK as u32;
    const F_WRLCK: u32 = libc::F_WRLCK as u32;
    const F_UNLCK: u32 = libc::F_UNLCK as u32;

    let mut op = match arg.lk.typ {
        F_RDLCK => libc::LOCK_SH as u32,
        F_WRLCK => libc::LOCK_EX as u32,
        F_UNLCK => libc::LOCK_UN as u32,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "unknown lock operation is specified",
            ))
        }
    };

    if !sleep {
        op |= libc::LOCK_NB as u32;
    }

    Ok(Arg::Flock(Flock { arg, op }))
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn parse_lookup() {
        let name = CString::new("foo").unwrap();
        let parent = 1;

        let header = kernel::fuse_in_header {
            len: (mem::size_of::<kernel::fuse_in_header>() + name.as_bytes_with_nul().len()) as u32,
            opcode: kernel::FUSE_LOOKUP,
            unique: 2,
            nodeid: parent,
            uid: 1,
            gid: 1,
            pid: 42,
            padding: 0,
        };
        let mut payload = vec![];
        payload.extend_from_slice(name.as_bytes_with_nul());

        let mut parser = Parser::new(&header, &payload[..]);
        let op = parser.parse().unwrap();
        match op {
            Arg::Lookup(op) => {
                assert_eq!(op.name.as_bytes(), name.as_bytes());
            }
            _ => panic!("incorret operation is returned"),
        }
    }

    #[test]
    fn parse_symlink() {
        let name = CString::new("foo").unwrap();
        let link = CString::new("bar").unwrap();
        let parent = 1;

        let header = kernel::fuse_in_header {
            len: (mem::size_of::<kernel::fuse_in_header>()
                + name.as_bytes_with_nul().len()
                + link.as_bytes_with_nul().len()) as u32,
            opcode: kernel::FUSE_SYMLINK,
            unique: 2,
            nodeid: parent,
            uid: 1,
            gid: 1,
            pid: 42,
            padding: 0,
        };
        let mut payload = vec![];
        payload.extend_from_slice(name.as_bytes_with_nul());
        payload.extend_from_slice(link.as_bytes_with_nul());

        let mut parser = Parser::new(&header, &payload[..]);
        let op = parser.parse().unwrap();
        match op {
            Arg::Symlink(op) => {
                assert_eq!(op.name.as_bytes(), name.as_bytes());
                assert_eq!(op.link.as_bytes(), link.as_bytes());
            }
            _ => panic!("incorret operation is returned"),
        }
    }
}
