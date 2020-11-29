//! Components used when processing FUSE requests.

use crate::{
    op::{self, LockOwner, SetAttrTime},
    session::Session,
    util::Decoder,
    write::ReplySender,
};
use polyfuse_kernel::{self as kernel, fuse_in_header, fuse_opcode};
use std::{convert::TryFrom, ffi::OsStr, fmt, io, marker::PhantomData, sync::Arc, time::Duration};

#[derive(Debug)]
pub struct Error(ErrorKind);

#[derive(Debug)]
enum ErrorKind {
    Decode,
}

impl Error {
    fn decode() -> Self {
        Self(ErrorKind::Decode)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpError").finish()
    }
}

impl std::error::Error for Error {}

/// Context about an incoming FUSE request.
pub struct Request {
    pub(crate) session: Arc<Session>,
    pub(crate) header: fuse_in_header,
    pub(crate) arg: Vec<u8>,
}

impl Request {
    /// Return the unique ID of the request.
    #[inline]
    pub fn unique(&self) -> u64 {
        self.header.unique
    }

    /// Return the user ID of the calling process.
    #[inline]
    pub fn uid(&self) -> u32 {
        self.header.uid
    }

    /// Return the group ID of the calling process.
    #[inline]
    pub fn gid(&self) -> u32 {
        self.header.gid
    }

    /// Return the process ID of the calling process.
    #[inline]
    pub fn pid(&self) -> u32 {
        self.header.pid
    }

    /// Decode the argument of this request.
    pub fn operation(&self) -> Result<Operation<'_>, Error> {
        if self.session.exited() {
            return Ok(Operation::Unknown(UnknownOperation {
                _marker: PhantomData,
            }));
        }

        let header = &self.header;
        let mut decoder = Decoder::new(&self.arg[..]);

        match fuse_opcode::try_from(header.opcode).ok() {
            // Some(fuse_opcode::FUSE_FORGET) => {
            //     let arg = decoder
            //         .fetch::<kernel::fuse_forget_in>()
            //         .map_err(Error::fatal)?;
            //     todo!()
            // }
            // Some(fuse_opcode::FUSE_BATCH_FORGET) => {
            //     let arg = decoder.fetch::<kernel::fuse_batch_forget_in>()?;
            //     let forgets = decoder.fetch_array(arg.count as usize)?;
            //     todo!()
            // }
            // Some(fuse_opcode::FUSE_INTERRUPT) => {
            //     let arg = decoder.fetch()?;
            //     todo!()
            // }
            // Some(fuse_opcode::FUSE_NOTIFY_REPLY) => {
            //     let arg = decoder.fetch()?;
            //     todo!()
            // }
            Some(fuse_opcode::FUSE_LOOKUP) => {
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Lookup(Lookup { header, name }))
            }

            Some(fuse_opcode::FUSE_GETATTR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Getattr(Getattr { header, arg }))
            }

            Some(fuse_opcode::FUSE_SETATTR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Setattr(Setattr { header, arg }))
            }

            Some(fuse_opcode::FUSE_READLINK) => Ok(Operation::Readlink(Readlink { header })),

            Some(fuse_opcode::FUSE_SYMLINK) => {
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                let link = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Symlink(Symlink { header, name, link }))
            }

            Some(fuse_opcode::FUSE_MKNOD) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Mknod(Mknod { header, arg, name }))
            }

            Some(fuse_opcode::FUSE_MKDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Mkdir(Mkdir { header, arg, name }))
            }

            Some(fuse_opcode::FUSE_UNLINK) => {
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Unlink(Unlink { header, name }))
            }

            Some(fuse_opcode::FUSE_RMDIR) => {
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Rmdir(Rmdir { header, name }))
            }

            Some(fuse_opcode::FUSE_RENAME) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                let newname = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Rename(Rename {
                    header,
                    arg: RenameArg::V1(arg),
                    name,
                    newname,
                }))
            }
            Some(fuse_opcode::FUSE_RENAME2) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                let newname = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Rename(Rename {
                    header,
                    arg: RenameArg::V2(arg),
                    name,
                    newname,
                }))
            }

            Some(fuse_opcode::FUSE_LINK) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let newname = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Link(Link {
                    header,
                    arg,
                    newname,
                }))
            }

            Some(fuse_opcode::FUSE_OPEN) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Open(Open { header, arg }))
            }

            Some(fuse_opcode::FUSE_READ) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Read(Read { header, arg }))
            }

            Some(fuse_opcode::FUSE_WRITE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Write(Write { header, arg }))
            }

            Some(fuse_opcode::FUSE_RELEASE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Release(Release { header, arg }))
            }

            Some(fuse_opcode::FUSE_STATFS) => Ok(Operation::Statfs(Statfs { header })),

            Some(fuse_opcode::FUSE_FSYNC) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Fsync(Fsync { header, arg }))
            }

            Some(fuse_opcode::FUSE_SETXATTR) => {
                let arg = decoder
                    .fetch::<kernel::fuse_setxattr_in>()
                    .ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                let value = decoder
                    .fetch_bytes(arg.size as usize)
                    .ok_or_else(Error::decode)?;
                Ok(Operation::Setxattr(Setxattr {
                    header,
                    arg,
                    name,
                    value,
                }))
            }

            Some(fuse_opcode::FUSE_GETXATTR) => {
                let arg: &kernel::fuse_getxattr_in = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Getxattr(Getxattr { header, arg, name }))
            }

            Some(fuse_opcode::FUSE_LISTXATTR) => {
                let arg: &kernel::fuse_getxattr_in = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Listxattr(Listxattr { header, arg }))
            }

            Some(fuse_opcode::FUSE_REMOVEXATTR) => {
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Removexattr(Removexattr { header, name }))
            }

            Some(fuse_opcode::FUSE_FLUSH) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Flush(Flush { header, arg }))
            }

            Some(fuse_opcode::FUSE_OPENDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Opendir(Opendir { header, arg }))
            }

            Some(fuse_opcode::FUSE_READDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Readdir(Readdir {
                    header,
                    arg,
                    is_plus: false,
                }))
            }
            Some(fuse_opcode::FUSE_READDIRPLUS) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Readdir(Readdir {
                    header,
                    arg,
                    is_plus: true,
                }))
            }

            Some(fuse_opcode::FUSE_RELEASEDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Releasedir(Releasedir { header, arg }))
            }

            Some(fuse_opcode::FUSE_FSYNCDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Fsyncdir(Fsyncdir { header, arg }))
            }

            Some(fuse_opcode::FUSE_GETLK) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Getlk(Getlk { header, arg }))
            }

            Some(opcode @ fuse_opcode::FUSE_SETLK) | Some(opcode @ fuse_opcode::FUSE_SETLKW) => {
                let arg: &kernel::fuse_lk_in = decoder.fetch().ok_or_else(Error::decode)?;
                let sleep = match opcode {
                    fuse_opcode::FUSE_SETLK => false,
                    fuse_opcode::FUSE_SETLKW => true,
                    _ => unreachable!(),
                };

                if arg.lk_flags & kernel::FUSE_LK_FLOCK == 0 {
                    Ok(Operation::Setlk(Setlk { header, arg, sleep }))
                } else {
                    let op = convert_to_flock_op(arg.lk.typ, sleep).unwrap_or(0);
                    Ok(Operation::Flock(Flock { header, arg, op }))
                }
            }

            Some(fuse_opcode::FUSE_ACCESS) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Access(Access { header, arg }))
            }

            Some(fuse_opcode::FUSE_CREATE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Create(Create { header, arg, name }))
            }

            Some(fuse_opcode::FUSE_BMAP) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Bmap(Bmap { header, arg }))
            }

            Some(fuse_opcode::FUSE_FALLOCATE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Fallocate(Fallocate { header, arg }))
            }

            Some(fuse_opcode::FUSE_COPY_FILE_RANGE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::CopyFileRange(CopyFileRange { header, arg }))
            }

            Some(fuse_opcode::FUSE_POLL) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Poll(Poll { header, arg }))
            }

            _ => {
                tracing::warn!("unsupported opcode: {}", header.opcode);
                Ok(Operation::Unknown(UnknownOperation {
                    _marker: PhantomData,
                }))
            }
        }
    }

    pub fn reply<W, T>(&self, writer: W, data: T) -> io::Result<()>
    where
        W: io::Write,
        T: crate::write::Bytes,
    {
        ReplySender::new(writer, self.unique()).reply(data)
    }

    pub fn reply_error<W>(&self, writer: W, code: i32) -> io::Result<()>
    where
        W: io::Write,
    {
        ReplySender::new(writer, self.unique()).error(code)
    }
}

fn convert_to_flock_op(lk_type: u32, sleep: bool) -> Option<u32> {
    const F_RDLCK: u32 = libc::F_RDLCK as u32;
    const F_WRLCK: u32 = libc::F_WRLCK as u32;
    const F_UNLCK: u32 = libc::F_UNLCK as u32;

    let mut op = match lk_type {
        F_RDLCK => libc::LOCK_SH as u32,
        F_WRLCK => libc::LOCK_EX as u32,
        F_UNLCK => libc::LOCK_UN as u32,
        _ => return None,
    };

    if !sleep {
        op |= libc::LOCK_NB as u32;
    }
    Some(op)
}

/// The kind of filesystem operation requested by the kernel.
#[non_exhaustive]
pub enum Operation<'op> {
    Lookup(Lookup<'op>),
    Getattr(Getattr<'op>),
    Setattr(Setattr<'op>),
    Readlink(Readlink<'op>),
    Symlink(Symlink<'op>),
    Mknod(Mknod<'op>),
    Mkdir(Mkdir<'op>),
    Unlink(Unlink<'op>),
    Rmdir(Rmdir<'op>),
    Rename(Rename<'op>),
    Link(Link<'op>),
    Open(Open<'op>),
    Read(Read<'op>),
    Write(Write<'op>),
    Release(Release<'op>),
    Statfs(Statfs<'op>),
    Fsync(Fsync<'op>),
    Setxattr(Setxattr<'op>),
    Getxattr(Getxattr<'op>),
    Listxattr(Listxattr<'op>),
    Removexattr(Removexattr<'op>),
    Flush(Flush<'op>),
    Opendir(Opendir<'op>),
    Readdir(Readdir<'op>),
    Readdirplus(Readdir<'op>),
    Releasedir(Releasedir<'op>),
    Fsyncdir(Fsyncdir<'op>),
    Getlk(Getlk<'op>),
    Setlk(Setlk<'op>),
    Flock(Flock<'op>),
    Access(Access<'op>),
    Create(Create<'op>),
    Bmap(Bmap<'op>),
    Fallocate(Fallocate<'op>),
    CopyFileRange(CopyFileRange<'op>),
    Poll(Poll<'op>),

    #[doc(hidden)]
    Unknown(UnknownOperation<'op>),
}

#[doc(hidden)]
pub struct UnknownOperation<'op> {
    _marker: PhantomData<&'op ()>,
}

// ==== operations ====

pub struct Lookup<'op> {
    header: &'op kernel::fuse_in_header,
    name: &'op OsStr,
}

impl<'op> op::Lookup for Lookup<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }
}

pub struct Getattr<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_getattr_in,
}

impl<'op> op::Getattr for Getattr<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> Option<u64> {
        if self.arg.getattr_flags & kernel::FUSE_GETATTR_FH != 0 {
            Some(self.arg.fh)
        } else {
            None
        }
    }
}

pub struct Setattr<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_setattr_in,
}

impl<'op> Setattr<'op> {
    #[inline(always)]
    fn get<R>(&self, flag: u32, f: impl FnOnce(&kernel::fuse_setattr_in) -> R) -> Option<R> {
        if self.arg.valid & flag != 0 {
            Some(f(&self.arg))
        } else {
            None
        }
    }
}

impl<'op> op::Setattr for Setattr<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    #[inline]
    fn fh(&self) -> Option<u64> {
        self.get(kernel::FATTR_FH, |arg| arg.fh)
    }

    #[inline]
    fn mode(&self) -> Option<u32> {
        self.get(kernel::FATTR_MODE, |arg| arg.mode)
    }

    #[inline]
    fn uid(&self) -> Option<u32> {
        self.get(kernel::FATTR_UID, |arg| arg.uid)
    }

    #[inline]
    fn gid(&self) -> Option<u32> {
        self.get(kernel::FATTR_GID, |arg| arg.gid)
    }

    #[inline]
    fn size(&self) -> Option<u64> {
        self.get(kernel::FATTR_SIZE, |arg| arg.size)
    }

    #[inline]
    fn atime(&self) -> Option<SetAttrTime> {
        self.get(kernel::FATTR_ATIME, |arg| {
            if arg.valid & kernel::FATTR_ATIME_NOW != 0 {
                SetAttrTime::Now
            } else {
                SetAttrTime::Timespec(Duration::new(arg.atime, arg.atimensec))
            }
        })
    }

    #[inline]
    fn mtime(&self) -> Option<SetAttrTime> {
        self.get(kernel::FATTR_MTIME, |arg| {
            if arg.valid & kernel::FATTR_MTIME_NOW != 0 {
                SetAttrTime::Now
            } else {
                SetAttrTime::Timespec(Duration::new(arg.mtime, arg.mtimensec))
            }
        })
    }

    #[inline]
    fn ctime(&self) -> Option<Duration> {
        self.get(kernel::FATTR_CTIME, |arg| {
            Duration::new(arg.ctime, arg.ctimensec)
        })
    }

    #[inline]
    fn lock_owner(&self) -> Option<LockOwner> {
        self.get(kernel::FATTR_LOCKOWNER, |arg| {
            LockOwner::from_raw(arg.lock_owner)
        })
    }
}

pub struct Readlink<'op> {
    header: &'op kernel::fuse_in_header,
}

impl<'op> op::Readlink for Readlink<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }
}

pub struct Symlink<'op> {
    header: &'op kernel::fuse_in_header,
    name: &'op OsStr,
    link: &'op OsStr,
}

impl<'op> op::Symlink for Symlink<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }

    fn link(&self) -> &OsStr {
        self.link
    }
}

pub struct Mknod<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_mknod_in,
    name: &'op OsStr,
}

impl<'op> op::Mknod for Mknod<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }

    fn mode(&self) -> u32 {
        self.arg.mode
    }

    fn rdev(&self) -> u32 {
        self.arg.rdev
    }

    fn umask(&self) -> u32 {
        self.arg.umask
    }
}

pub struct Mkdir<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_mkdir_in,
    name: &'op OsStr,
}

impl<'op> op::Mkdir for Mkdir<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }

    fn mode(&self) -> u32 {
        self.arg.mode
    }

    fn umask(&self) -> u32 {
        self.arg.umask
    }
}

pub struct Unlink<'op> {
    header: &'op kernel::fuse_in_header,
    name: &'op OsStr,
}

impl<'op> op::Unlink for Unlink<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }
}

pub struct Rmdir<'op> {
    header: &'op kernel::fuse_in_header,
    name: &'op OsStr,
}

impl<'op> op::Rmdir for Rmdir<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }
}

pub struct Rename<'op> {
    header: &'op kernel::fuse_in_header,
    arg: RenameArg<'op>,
    name: &'op OsStr,
    newname: &'op OsStr,
}

enum RenameArg<'op> {
    V1(&'op kernel::fuse_rename_in),
    V2(&'op kernel::fuse_rename2_in),
}

impl<'op> op::Rename for Rename<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }

    fn newparent(&self) -> u64 {
        match self.arg {
            RenameArg::V1(arg) => arg.newdir,
            RenameArg::V2(arg) => arg.newdir,
        }
    }

    fn newname(&self) -> &OsStr {
        self.newname
    }

    fn flags(&self) -> u32 {
        match self.arg {
            RenameArg::V1(..) => 0,
            RenameArg::V2(arg) => arg.flags,
        }
    }
}

pub struct Link<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_link_in,
    newname: &'op OsStr,
}

impl<'op> op::Link for Link<'op> {
    fn ino(&self) -> u64 {
        self.arg.oldnodeid
    }

    fn newparent(&self) -> u64 {
        self.header.nodeid
    }

    fn newname(&self) -> &OsStr {
        self.newname
    }
}

pub struct Open<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_open_in,
}

impl<'op> op::Open for Open<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn flags(&self) -> u32 {
        self.arg.flags
    }
}

pub struct Read<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_read_in,
}

impl<'op> op::Read for Read<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn offset(&self) -> u64 {
        self.arg.offset
    }

    fn size(&self) -> u32 {
        self.arg.size
    }

    fn flags(&self) -> u32 {
        self.arg.flags
    }

    fn lock_owner(&self) -> Option<LockOwner> {
        if self.arg.read_flags & kernel::FUSE_READ_LOCKOWNER != 0 {
            Some(LockOwner::from_raw(self.arg.lock_owner))
        } else {
            None
        }
    }
}

pub struct Write<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_write_in,
}

impl<'op> op::Write for Write<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn offset(&self) -> u64 {
        self.arg.offset
    }

    fn size(&self) -> u32 {
        self.arg.size
    }

    fn flags(&self) -> u32 {
        self.arg.flags
    }

    fn lock_owner(&self) -> Option<LockOwner> {
        if self.arg.write_flags & kernel::FUSE_WRITE_LOCKOWNER != 0 {
            Some(LockOwner::from_raw(self.arg.lock_owner))
        } else {
            None
        }
    }
}

pub struct Release<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_release_in,
}

impl<'op> op::Release for Release<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn flags(&self) -> u32 {
        self.arg.flags
    }

    fn lock_owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.lock_owner)
    }

    fn flush(&self) -> bool {
        self.arg.release_flags & kernel::FUSE_RELEASE_FLUSH != 0
    }

    fn flock_release(&self) -> bool {
        self.arg.release_flags & kernel::FUSE_RELEASE_FLOCK_UNLOCK != 0
    }
}

pub struct Statfs<'op> {
    header: &'op kernel::fuse_in_header,
}

impl<'op> op::Statfs for Statfs<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }
}

pub struct Fsync<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_fsync_in,
}

impl<'op> op::Fsync for Fsync<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn datasync(&self) -> bool {
        self.arg.fsync_flags & kernel::FUSE_FSYNC_FDATASYNC != 0
    }
}

pub struct Setxattr<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_setxattr_in,
    name: &'op OsStr,
    value: &'op [u8],
}

impl<'op> op::Setxattr for Setxattr<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }

    fn value(&self) -> &[u8] {
        self.value
    }

    fn flags(&self) -> u32 {
        self.arg.flags
    }
}

pub struct Getxattr<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_getxattr_in,
    name: &'op OsStr,
}

impl<'op> op::Getxattr for Getxattr<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }

    fn size(&self) -> u32 {
        self.arg.size
    }
}

pub struct Listxattr<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_getxattr_in,
}

impl<'op> op::Listxattr for Listxattr<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn size(&self) -> u32 {
        self.arg.size
    }
}

pub struct Removexattr<'op> {
    header: &'op kernel::fuse_in_header,
    name: &'op OsStr,
}

impl<'op> op::Removexattr for Removexattr<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }
}

pub struct Flush<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_flush_in,
}

impl<'op> op::Flush for Flush<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn lock_owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.lock_owner)
    }
}

pub struct Opendir<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_open_in,
}

impl<'op> op::Opendir for Opendir<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn flags(&self) -> u32 {
        self.arg.flags
    }
}

pub struct Readdir<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_read_in,
    is_plus: bool,
}

impl Readdir<'_> {
    pub fn is_plus(&self) -> bool {
        self.is_plus
    }
}

impl<'op> op::Readdir for Readdir<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn offset(&self) -> u64 {
        self.arg.offset
    }

    fn size(&self) -> u32 {
        self.arg.size
    }
}

pub struct Releasedir<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_release_in,
}

impl<'op> op::Releasedir for Releasedir<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn flags(&self) -> u32 {
        self.arg.flags
    }
}

pub struct Fsyncdir<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_fsync_in,
}

impl<'op> op::Fsyncdir for Fsyncdir<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn datasync(&self) -> bool {
        self.arg.fsync_flags & kernel::FUSE_FSYNC_FDATASYNC != 0
    }
}

pub struct Getlk<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_lk_in,
}

impl<'op> op::Getlk for Getlk<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.owner)
    }

    fn typ(&self) -> u32 {
        self.arg.lk.typ
    }

    fn start(&self) -> u64 {
        self.arg.lk.start
    }

    fn end(&self) -> u64 {
        self.arg.lk.end
    }

    fn pid(&self) -> u32 {
        self.arg.lk.pid
    }
}

pub struct Setlk<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_lk_in,
    sleep: bool,
}

impl<'op> op::Setlk for Setlk<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.owner)
    }

    fn typ(&self) -> u32 {
        self.arg.lk.typ
    }

    fn start(&self) -> u64 {
        self.arg.lk.start
    }

    fn end(&self) -> u64 {
        self.arg.lk.end
    }

    fn pid(&self) -> u32 {
        self.arg.lk.pid
    }

    fn sleep(&self) -> bool {
        self.sleep
    }
}

pub struct Flock<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_lk_in,
    op: u32,
}

impl<'op> op::Flock for Flock<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.owner)
    }

    fn op(&self) -> Option<u32> {
        Some(self.op)
    }
}

pub struct Access<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_access_in,
}

impl<'op> op::Access for Access<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn mask(&self) -> u32 {
        self.arg.mask
    }
}

pub struct Create<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_create_in,
    name: &'op OsStr,
}

impl<'op> op::Create for Create<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }

    fn mode(&self) -> u32 {
        self.arg.mode
    }

    fn open_flags(&self) -> u32 {
        self.arg.flags
    }

    fn umask(&self) -> u32 {
        self.arg.umask
    }
}

pub struct Bmap<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_bmap_in,
}

impl<'op> op::Bmap for Bmap<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn block(&self) -> u64 {
        self.arg.block
    }

    fn blocksize(&self) -> u32 {
        self.arg.blocksize
    }
}

pub struct Fallocate<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_fallocate_in,
}

impl<'op> op::Fallocate for Fallocate<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn offset(&self) -> u64 {
        self.arg.offset
    }

    fn length(&self) -> u64 {
        self.arg.length
    }

    fn mode(&self) -> u32 {
        self.arg.mode
    }
}

pub struct CopyFileRange<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_copy_file_range_in,
}

impl<'op> op::CopyFileRange for CopyFileRange<'op> {
    fn ino_in(&self) -> u64 {
        self.header.nodeid
    }

    fn fh_in(&self) -> u64 {
        self.arg.fh_in
    }

    fn offset_in(&self) -> u64 {
        self.arg.off_in
    }

    fn ino_out(&self) -> u64 {
        self.arg.nodeid_out
    }

    fn fh_out(&self) -> u64 {
        self.arg.fh_out
    }

    fn offset_out(&self) -> u64 {
        self.arg.off_out
    }

    fn length(&self) -> u64 {
        self.arg.len
    }

    fn flags(&self) -> u64 {
        self.arg.flags
    }
}

pub struct Poll<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_poll_in,
}

impl<'op> op::Poll for Poll<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn events(&self) -> u32 {
        self.arg.events
    }

    fn kh(&self) -> Option<u64> {
        if self.arg.flags & kernel::FUSE_POLL_SCHEDULE_NOTIFY != 0 {
            Some(self.arg.kh)
        } else {
            None
        }
    }
}
