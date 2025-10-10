use crate::{
    bytes::{DecodeError, Decoder},
    request::RequestHeader,
    session::KernelConfig,
    types::{
        DeviceID, FileID, FileLock, FileMode, FilePermissions, LockOwnerID, NodeID, NotifyID,
        PollEvents, PollWakeupID, RequestID,
    },
};
use bitflags::bitflags;
use polyfuse_kernel::*;
use rustix::{
    fs::{Gid, Uid},
    process::Pid,
};
use std::{
    ffi::OsStr, fmt, marker::PhantomData, ops::Deref, os::unix::fs::OpenOptionsExt, slice,
    time::Duration,
};

const FUSE_INT_REQ_BIT: u64 = 1;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("during decoding: {}", _0)]
    Decode(#[from] DecodeError),

    #[error("unsupported opcode")]
    UnsupportedOpcode,

    #[error("invalid nodeid")]
    InvalidNodeID,

    #[error("invalid `flock(2)` operation type")]
    InvalidFlockType,

    #[error("invalid PID value")]
    InvalidPid,

    #[error("unknown `lseek(2)` whence")]
    UnknownLseekWhence,
}

/// The kind of filesystem operation requested by the kernel.
#[derive(Debug)]
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
    Lseek(Lseek<'op>),

    Forget(Forgets<'op>),
    Interrupt(Interrupt<'op>),
    NotifyReply(NotifyReply<'op>),
}

impl<'op> Operation<'op> {
    pub fn decode(
        config: &KernelConfig,
        header: &'op RequestHeader,
        arg: &'op [u8],
    ) -> Result<Self, Error> {
        let opcode = header.opcode().map_err(|_| Error::UnsupportedOpcode)?;

        let mut decoder = Decoder::new(arg);

        match opcode {
            fuse_opcode::FUSE_FORGET => {
                let arg: &fuse_forget_in = decoder.fetch()?;
                let forget = Forget {
                    raw: fuse_forget_one {
                        nodeid: header.nodeid().ok_or(Error::InvalidNodeID)?.into_raw(),
                        nlookup: arg.nlookup,
                    },
                };
                Ok(Operation::Forget(Forgets::Single([forget; 1])))
            }

            fuse_opcode::FUSE_BATCH_FORGET => {
                let arg: &fuse_batch_forget_in = decoder.fetch()?;
                let forgets = decoder.fetch_array::<fuse_forget_one>(arg.count as usize)?;
                let forgets = unsafe {
                    // Safety: `Forget` has the same layout with `fuse_forget_one`.
                    slice::from_raw_parts(forgets.as_ptr().cast(), forgets.len())
                };
                Ok(Operation::Forget(Forgets::Batch(forgets)))
            }

            fuse_opcode::FUSE_INTERRUPT => {
                let arg: &fuse_interrupt_in = decoder.fetch()?;
                Ok(Operation::Interrupt(Interrupt {
                    unique: RequestID::from_raw(arg.unique & !FUSE_INT_REQ_BIT),
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_NOTIFY_REPLY => {
                let arg: &fuse_notify_retrieve_in = decoder.fetch()?;
                Ok(Operation::NotifyReply(NotifyReply {
                    unique: NotifyID::from_raw(header.unique().into_raw()),
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    offset: arg.offset,
                    size: arg.size,
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_LOOKUP => {
                let name = decoder.fetch_str()?;
                Ok(Operation::Lookup(Lookup {
                    parent: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    name,
                }))
            }

            fuse_opcode::FUSE_GETATTR => {
                let arg: &fuse_getattr_in = decoder.fetch()?;
                Ok(Operation::Getattr(Getattr {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: (arg.getattr_flags & FUSE_GETATTR_FH != 0)
                        .then_some(FileID::from_raw(arg.fh)),
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_SETATTR => {
                let arg: &fuse_setattr_in = decoder.fetch()?;
                Ok(Operation::Setattr(Setattr {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: (arg.valid & FATTR_FH != 0).then(|| FileID::from_raw(arg.fh)),
                    mode: (arg.valid & FATTR_MODE != 0).then(|| FileMode::from_raw(arg.mode)),
                    uid: (arg.valid & FATTR_UID != 0).then(|| Uid::from_raw(arg.uid)),
                    gid: (arg.valid & FATTR_GID != 0).then(|| Gid::from_raw(arg.gid)),
                    size: (arg.valid & FATTR_SIZE != 0).then_some(arg.size),
                    atime: (arg.valid & FATTR_ATIME != 0).then(|| {
                        if arg.valid & FATTR_ATIME_NOW != 0 {
                            SetAttrTime::Now
                        } else {
                            SetAttrTime::Timespec(Duration::new(arg.atime, arg.atimensec))
                        }
                    }),
                    mtime: (arg.valid & FATTR_MTIME != 0).then(|| {
                        if arg.valid & FATTR_MTIME_NOW != 0 {
                            SetAttrTime::Now
                        } else {
                            SetAttrTime::Timespec(Duration::new(arg.mtime, arg.mtimensec))
                        }
                    }),
                    ctime: (arg.valid & FATTR_CTIME != 0)
                        .then(|| Duration::new(arg.ctime, arg.ctimensec)),
                    lock_owner: (arg.valid & FATTR_LOCKOWNER != 0)
                        .then(|| LockOwnerID::from_raw(arg.lock_owner)),
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_READLINK => Ok(Operation::Readlink(Readlink {
                ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                _marker: PhantomData,
            })),

            fuse_opcode::FUSE_SYMLINK => {
                let name = decoder.fetch_str()?;
                let link = decoder.fetch_str()?;
                Ok(Operation::Symlink(Symlink {
                    parent: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    name,
                    link,
                }))
            }

            fuse_opcode::FUSE_MKNOD if config.minor <= 11 => {
                let arg: &fuse_mknod_in_compat_11 = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                Ok(Operation::Mknod(Mknod {
                    parent: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    name,
                    mode: FileMode::from_raw(arg.mode),
                    rdev: DeviceID::from_kernel_dev(arg.rdev),
                    umask: FilePermissions::empty(),
                }))
            }

            fuse_opcode::FUSE_MKNOD => {
                let arg: &fuse_mknod_in = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                Ok(Operation::Mknod(Mknod {
                    parent: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    name,
                    mode: FileMode::from_raw(arg.mode),
                    rdev: DeviceID::from_kernel_dev(arg.rdev),
                    umask: FilePermissions::from_bits_truncate(arg.umask) & FilePermissions::MASK,
                }))
            }

            fuse_opcode::FUSE_MKDIR => {
                let arg: &fuse_mkdir_in = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                Ok(Operation::Mkdir(Mkdir {
                    parent: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    name,
                    permissions: FilePermissions::from_bits_truncate(arg.mode),
                    umask: FilePermissions::from_bits_truncate(arg.umask) & FilePermissions::MASK,
                }))
            }

            fuse_opcode::FUSE_UNLINK => {
                let name = decoder.fetch_str()?;
                Ok(Operation::Unlink(Unlink {
                    parent: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    name,
                }))
            }

            fuse_opcode::FUSE_RMDIR => {
                let name = decoder.fetch_str()?;
                Ok(Operation::Rmdir(Rmdir {
                    parent: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    name,
                }))
            }

            fuse_opcode::FUSE_RENAME => {
                let arg: &fuse_rename_in = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                let newname = decoder.fetch_str()?;
                Ok(Operation::Rename(Rename {
                    parent: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    name,
                    newparent: NodeID::from_raw(arg.newdir).ok_or(Error::InvalidNodeID)?,
                    newname,
                    flags: RenameFlags::empty(),
                }))
            }

            fuse_opcode::FUSE_RENAME2 => {
                let arg: &fuse_rename2_in = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                let newname = decoder.fetch_str()?;
                Ok(Operation::Rename(Rename {
                    parent: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    name,
                    newparent: NodeID::from_raw(arg.newdir).ok_or(Error::InvalidNodeID)?,
                    newname,
                    flags: RenameFlags::from_bits_truncate(arg.flags),
                }))
            }

            fuse_opcode::FUSE_LINK => {
                let arg: &fuse_link_in = decoder.fetch()?;
                let newname = decoder.fetch_str()?;
                Ok(Operation::Link(Link {
                    ino: NodeID::from_raw(arg.oldnodeid).ok_or(Error::InvalidNodeID)?,
                    newparent: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    newname,
                }))
            }

            fuse_opcode::FUSE_OPEN => {
                let arg: &fuse_open_in = decoder.fetch()?;
                Ok(Operation::Open(Open {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    options: OpenOptions::from_raw(arg.flags),
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_READ => {
                let arg: &fuse_read_in = decoder.fetch()?;
                Ok(Operation::Read(Read {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: FileID::from_raw(arg.fh),
                    offset: arg.offset,
                    size: arg.size,
                    options: OpenOptions::from_raw(arg.flags),
                    lock_owner: (arg.read_flags & FUSE_READ_LOCKOWNER != 0)
                        .then(|| LockOwnerID::from_raw(arg.lock_owner)),
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_WRITE if config.minor <= 8 => {
                let arg: &fuse_write_in_compat_8 = decoder.fetch()?;
                Ok(Operation::Write(Write {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: FileID::from_raw(arg.fh),
                    offset: arg.offset,
                    size: arg.size,
                    options: OpenOptions::from_raw(0),
                    lock_owner: None,
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_WRITE => {
                let arg: &fuse_write_in = decoder.fetch()?;
                Ok(Operation::Write(Write {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: FileID::from_raw(arg.fh),
                    offset: arg.offset,
                    size: arg.size,
                    options: OpenOptions::from_raw(arg.flags),
                    lock_owner: (arg.write_flags & FUSE_WRITE_LOCKOWNER != 0)
                        .then(|| LockOwnerID::from_raw(arg.lock_owner)),
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_RELEASE => {
                let arg: &fuse_release_in = decoder.fetch()?;
                Ok(Operation::Release(Release {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: FileID::from_raw(arg.fh),
                    options: OpenOptions::from_raw(arg.flags),
                    lock_owner: LockOwnerID::from_raw(arg.lock_owner),
                    flags: ReleaseFlags::from_bits_truncate(arg.release_flags),
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_STATFS => Ok(Operation::Statfs(Statfs {
                ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                _marker: PhantomData,
            })),

            fuse_opcode::FUSE_FSYNC => {
                let arg: &fuse_fsync_in = decoder.fetch()?;
                Ok(Operation::Fsync(Fsync {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: FileID::from_raw(arg.fh),
                    datasync: arg.fsync_flags & FUSE_FSYNC_FDATASYNC != 0,
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_SETXATTR if config.minor <= 32 => {
                let arg: &fuse_setxattr_in_compat_32 = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                let value = decoder.fetch_bytes(arg.size as usize)?;
                Ok(Operation::Setxattr(Setxattr {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    name,
                    value,
                    flags: SetxattrFlags::from_bits_truncate(arg.flags),
                }))
            }

            fuse_opcode::FUSE_SETXATTR => {
                // FIXME: treat setxattr_flags
                let arg: &fuse_setxattr_in = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                let value = decoder.fetch_bytes(arg.size as usize)?;
                Ok(Operation::Setxattr(Setxattr {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    name,
                    value,
                    flags: SetxattrFlags::from_bits_truncate(arg.flags),
                }))
            }

            fuse_opcode::FUSE_GETXATTR => {
                let arg: &fuse_getxattr_in = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                Ok(Operation::Getxattr(Getxattr {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    name,
                    size: arg.size,
                }))
            }

            fuse_opcode::FUSE_LISTXATTR => {
                let arg: &fuse_getxattr_in = decoder.fetch()?;
                Ok(Operation::Listxattr(Listxattr {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    size: arg.size,
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_REMOVEXATTR => {
                let name = decoder.fetch_str()?;
                Ok(Operation::Removexattr(Removexattr {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    name,
                }))
            }

            fuse_opcode::FUSE_FLUSH => {
                let arg: &fuse_flush_in = decoder.fetch()?;
                Ok(Operation::Flush(Flush {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: FileID::from_raw(arg.fh),
                    lock_owner: LockOwnerID::from_raw(arg.lock_owner),
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_OPENDIR => {
                let arg: &fuse_open_in = decoder.fetch()?;
                Ok(Operation::Opendir(Opendir {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    options: OpenOptions::from_raw(arg.flags),
                    _marker: PhantomData,
                }))
            }

            opcode @ fuse_opcode::FUSE_READDIR | opcode @ fuse_opcode::FUSE_READDIRPLUS => {
                let arg: &fuse_read_in = decoder.fetch()?;
                Ok(Operation::Readdir(Readdir {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: FileID::from_raw(arg.fh),
                    offset: arg.offset,
                    size: arg.size,
                    mode: if opcode == fuse_opcode::FUSE_READDIRPLUS {
                        ReaddirMode::Plus
                    } else {
                        ReaddirMode::Normal
                    },
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_RELEASEDIR => {
                let arg: &fuse_release_in = decoder.fetch()?;
                Ok(Operation::Releasedir(Releasedir {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: FileID::from_raw(arg.fh),
                    options: OpenOptions::from_raw(arg.flags),
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_FSYNCDIR => {
                let arg: &fuse_fsync_in = decoder.fetch()?;
                Ok(Operation::Fsyncdir(Fsyncdir {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: FileID::from_raw(arg.fh),
                    datasync: arg.fsync_flags & FUSE_FSYNC_FDATASYNC != 0,
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_GETLK => {
                let arg: &fuse_lk_in = decoder.fetch()?;
                Ok(Operation::Getlk(Getlk {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: FileID::from_raw(arg.fh),
                    owner: LockOwnerID::from_raw(arg.owner),
                    file_lock: FileLock {
                        typ: arg.lk.typ,
                        start: arg.lk.start,
                        end: arg.lk.end,
                        pid: Pid::from_raw(arg.lk.pid as i32).ok_or(Error::InvalidPid)?,
                    },
                    _marker: PhantomData,
                }))
            }

            opcode @ fuse_opcode::FUSE_SETLK | opcode @ fuse_opcode::FUSE_SETLKW => {
                let arg: &fuse_lk_in = decoder.fetch()?;
                let sleep = match opcode {
                    fuse_opcode::FUSE_SETLK => false,
                    fuse_opcode::FUSE_SETLKW => true,
                    _ => unreachable!(),
                };

                if arg.lk_flags & FUSE_LK_FLOCK == 0 {
                    Ok(Operation::Setlk(Setlk {
                        ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                        fh: FileID::from_raw(arg.fh),
                        file_lock: FileLock {
                            typ: arg.lk.typ,
                            start: arg.lk.start,
                            end: arg.lk.end,
                            pid: Pid::from_raw(arg.lk.pid as i32).ok_or(Error::InvalidPid)?,
                        },
                        sleep,
                        _marker: PhantomData,
                    }))
                } else {
                    Ok(Operation::Flock(Flock {
                        ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                        fh: FileID::from_raw(arg.fh),
                        owner: LockOwnerID::from_raw(arg.owner),
                        op: match arg.lk.typ as i32 {
                            libc::F_RDLCK if sleep => FlockOp::Shared,
                            libc::F_RDLCK => FlockOp::SharedNonblock,
                            libc::F_WRLCK if sleep => FlockOp::Exclusive,
                            libc::F_WRLCK => FlockOp::ExclusiveNonblock,
                            libc::F_UNLCK => FlockOp::Unlock,
                            _ => return Err(Error::InvalidFlockType),
                        },
                        _marker: PhantomData,
                    }))
                }
            }

            fuse_opcode::FUSE_ACCESS => {
                let arg: &fuse_access_in = decoder.fetch()?;
                Ok(Operation::Access(Access {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    mask: FilePermissions::from_bits_truncate(arg.mask) & FilePermissions::MASK,
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_CREATE => {
                let arg: &fuse_create_in = decoder.fetch()?;
                let name = decoder.fetch_str()?;
                Ok(Operation::Create(Create {
                    parent: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    name,
                    mode: FileMode::from_raw(arg.mode),
                    umask: FilePermissions::from_bits_truncate(arg.umask) & FilePermissions::MASK,
                    open_options: OpenOptions::from_raw(arg.flags),
                }))
            }

            fuse_opcode::FUSE_BMAP => {
                let arg: &fuse_bmap_in = decoder.fetch()?;
                Ok(Operation::Bmap(Bmap {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    block: arg.block,
                    blocksize: arg.blocksize,
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_FALLOCATE => {
                let arg: &fuse_fallocate_in = decoder.fetch()?;
                Ok(Operation::Fallocate(Fallocate {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: FileID::from_raw(arg.fh),
                    offset: arg.offset,
                    length: arg.length,
                    mode: FallocateFlags::from_bits_truncate(arg.mode),
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_COPY_FILE_RANGE => {
                let arg: &fuse_copy_file_range_in = decoder.fetch()?;
                Ok(Operation::CopyFileRange(CopyFileRange {
                    ino_in: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh_in: FileID::from_raw(arg.fh_in),
                    offset_in: arg.off_in,
                    ino_out: NodeID::from_raw(arg.nodeid_out).ok_or(Error::InvalidNodeID)?,
                    fh_out: FileID::from_raw(arg.fh_out),
                    offset_out: arg.off_out,
                    length: arg.len,
                    flags: arg.flags,
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_POLL => {
                let arg: &fuse_poll_in = decoder.fetch()?;
                Ok(Operation::Poll(Poll {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: FileID::from_raw(arg.fh),
                    events: PollEvents::from_bits_truncate(arg.events),
                    kh: (arg.flags & FUSE_POLL_SCHEDULE_NOTIFY != 0)
                        .then(|| PollWakeupID::from_raw(arg.kh)),
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_LSEEK => {
                let arg: &fuse_lseek_in = decoder.fetch()?;
                Ok(Operation::Lseek(Lseek {
                    ino: header.nodeid().ok_or(Error::InvalidNodeID)?,
                    fh: FileID::from_raw(arg.fh),
                    offset: arg.offset,
                    whence: match arg.whence as i32 {
                        libc::SEEK_SET => Whence::Set,
                        libc::SEEK_CUR => Whence::Current,
                        libc::SEEK_END => Whence::End,
                        libc::SEEK_DATA => Whence::Data,
                        libc::SEEK_HOLE => Whence::Hole,
                        _ => return Err(Error::UnknownLseekWhence),
                    },
                    _marker: PhantomData,
                }))
            }

            fuse_opcode::FUSE_INIT | fuse_opcode::FUSE_DESTROY => {
                // should be handled by the upstream process.
                unreachable!()
            }

            fuse_opcode::FUSE_IOCTL
            | fuse_opcode::FUSE_SETUPMAPPING
            | fuse_opcode::FUSE_REMOVEMAPPING
            | fuse_opcode::FUSE_SYNCFS
            | fuse_opcode::FUSE_TMPFILE
            | fuse_opcode::FUSE_STATX
            | fuse_opcode::CUSE_INIT => Err(Error::UnsupportedOpcode),
        }
    }
}

/// A set of forget information removed from the kernel's internal caches.
pub enum Forgets<'op> {
    Single([Forget; 1]),
    Batch(&'op [Forget]),
}

impl fmt::Debug for Forgets<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.as_ref()).finish()
    }
}

impl<'op> Deref for Forgets<'op> {
    type Target = [Forget];

    #[inline]
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Single(forget) => forget,
            Self::Batch(forgets) => forgets,
        }
    }
}

/// A forget information.
#[repr(transparent)]
pub struct Forget {
    raw: fuse_forget_one,
}

impl fmt::Debug for Forget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Forget")
            .field("ino", &self.ino())
            .field("nlookup", &self.nlookup())
            .finish()
    }
}

impl Forget {
    /// Return the inode number of the target inode.
    #[inline]
    pub fn ino(&self) -> NodeID {
        NodeID::from_raw(self.raw.nodeid).expect("invalid nodeid")
    }

    /// Return the released lookup count of the target inode.
    #[inline]
    pub fn nlookup(&self) -> u64 {
        self.raw.nlookup
    }
}

/// A reply to a `NOTIFY_RETRIEVE` notification.
#[derive(Debug)]
#[non_exhaustive]
pub struct NotifyReply<'op> {
    /// The unique ID of the corresponding notification message.
    pub unique: NotifyID,

    /// The inode number corresponding with the cache data.
    pub ino: NodeID,

    /// The starting position of the cache data.
    pub offset: u64,

    /// The length of the retrieved cache data.
    pub size: u32,

    _marker: PhantomData<&'op ()>,
}

/// Interrupt a previous FUSE request.
#[derive(Debug)]
#[non_exhaustive]
pub struct Interrupt<'op> {
    /// Return the target unique ID to be interrupted.
    pub unique: RequestID,

    _marker: PhantomData<&'op ()>,
}

/// Lookup a directory entry by name.
///
/// If a matching entry is found, the filesystem replies to the kernel
/// with its attribute using `ReplyEntry`.  In addition, the lookup count
/// of the corresponding inode is incremented on success.
///
/// See also the documentation of `ReplyEntry` for tuning the reply parameters.
#[derive(Debug)]
#[non_exhaustive]
pub struct Lookup<'op> {
    /// The inode number of the parent directory.
    pub parent: NodeID,

    /// The name of the entry to be looked up.
    pub name: &'op OsStr,
}

/// Get file attributes.
///
/// The obtained attribute values are replied using `ReplyAttr`.
///
/// If writeback caching is enabled, the kernel might ignore
/// some of the attribute values, such as `st_size`.
#[derive(Debug)]
#[non_exhaustive]
pub struct Getattr<'op> {
    /// The inode number for obtaining the attribute value.
    pub ino: NodeID,

    /// The handle of opened file, if specified.
    pub fh: Option<FileID>,

    _marker: PhantomData<&'op ()>,
}

/// Set file attributes.
///
/// When the setting of attribute values succeeds, the filesystem replies its value
/// to the kernel using `ReplyAttr`.
#[derive(Debug)]
#[non_exhaustive]
pub struct Setattr<'op> {
    /// The inode number to be set the attribute values.
    pub ino: NodeID,

    /// The handle of opened file, if specified.
    pub fh: Option<FileID>,

    /// The file mode to be set.
    pub mode: Option<FileMode>,

    /// The user id to be set.
    pub uid: Option<Uid>,

    /// The group id to be set.
    pub gid: Option<Gid>,

    /// The size of the file content to be set.
    pub size: Option<u64>,

    /// The last accessed time to be set.
    pub atime: Option<SetAttrTime>,

    /// The last modified time to be set.
    pub mtime: Option<SetAttrTime>,

    /// The last creation time to be set.
    pub ctime: Option<Duration>,

    /// The identifier of lock owner.
    pub lock_owner: Option<LockOwnerID>,

    _marker: PhantomData<&'op ()>,
}

/// The time value requested to be set.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SetAttrTime {
    /// Set the specified time value.
    Timespec(Duration),

    /// Set the current time.
    Now,
}

/// Read a symbolic link.
#[derive(Debug)]
#[non_exhaustive]
pub struct Readlink<'op> {
    /// The inode number to be read the link value.
    pub ino: NodeID,

    _marker: PhantomData<&'op ()>,
}

/// Create a symbolic link.
///
/// When the link is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
#[derive(Debug)]
#[non_exhaustive]
pub struct Symlink<'op> {
    /// The inode number of the parent directory.
    pub parent: NodeID,

    /// The name of the symbolic link to create.
    pub name: &'op OsStr,

    /// The contents of the symbolic link.
    pub link: &'op OsStr,
}

/// Create a file node.
///
/// When the file node is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
#[derive(Debug)]
#[non_exhaustive]
pub struct Mknod<'op> {
    /// The inode number of the parent directory.
    pub parent: NodeID,

    /// The file name to create.
    pub name: &'op OsStr,

    /// The file type and permissions used when creating the new file.
    pub mode: FileMode,

    /// The device number for special file.
    ///
    /// This value is meaningful only if the created node is a device file
    /// (i.e. the file type is specified either `S_IFCHR` or `S_IFBLK`).
    pub rdev: DeviceID,

    /// The mask of permissions for the node to be created.
    pub umask: FilePermissions,
}

/// Create a directory node.
///
/// When the directory is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
#[derive(Debug)]
#[non_exhaustive]
pub struct Mkdir<'op> {
    /// The inode number of the parent directory where the directory is created.
    pub parent: NodeID,

    /// The name of the directory to be created.
    pub name: &'op OsStr,

    /// The file permissions used when creating the new directory.
    pub permissions: FilePermissions,

    /// The mask of permissions for the directory to be created.
    pub umask: FilePermissions,
}

// TODO: description about lookup count.

/// Remove a file.
#[derive(Debug)]
#[non_exhaustive]
pub struct Unlink<'op> {
    /// The inode number of the parent directory.
    pub parent: NodeID,

    /// The file name to be removed.
    pub name: &'op OsStr,
}

/// Remove a directory.
#[derive(Debug)]
#[non_exhaustive]
pub struct Rmdir<'op> {
    /// The inode number of the parent directory.
    pub parent: NodeID,

    /// The directory name to be removed.
    pub name: &'op OsStr,
}

/// Rename a file.
#[derive(Debug)]
#[non_exhaustive]
pub struct Rename<'op> {
    /// The inode number of the old parent directory.
    pub parent: NodeID,

    /// The old name of the target node.
    pub name: &'op OsStr,

    /// The inode number of the new parent directory.
    pub newparent: NodeID,

    /// The new name of the target node.
    pub newname: &'op OsStr,

    /// The rename flags.
    pub flags: RenameFlags,
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct RenameFlags: u32 {
        const EXCHANGE = libc::RENAME_EXCHANGE;
        const NOREPLACE = libc::RENAME_NOREPLACE;
        const WHITEOUT = libc::RENAME_WHITEOUT;
    }
}

/// Create a hard link.
///
/// When the link is successfully created, the filesystem must send
/// its attribute values using `ReplyEntry`.
#[derive(Debug)]
#[non_exhaustive]
pub struct Link<'op> {
    /// The *original* inode number which links to the created hard link.
    pub ino: NodeID,

    /// The inode number of the parent directory where the hard link is created.
    pub newparent: NodeID,

    /// The name of the hard link to be created.
    pub newname: &'op OsStr,
}

/// Open a file.
///
/// If the file is successfully opened, the filesystem must send the identifier
/// of the opened file handle to the kernel using `ReplyOpen`. This parameter is
/// set to a series of requests, such as `read` and `write`, until releasing
/// the file, and is able to be utilized as a "pointer" to the state during
/// handling the opened file.
///
/// See also the documentation of `ReplyOpen` for tuning the reply parameters.
// TODO: Description of behavior when writeback caching is enabled.
#[derive(Debug)]
#[non_exhaustive]
pub struct Open<'op> {
    /// The inode number to be opened.
    pub ino: NodeID,

    /// The access mode and creation/status flags of the opened file.
    pub options: OpenOptions,

    _marker: PhantomData<&'op ()>,
}

/// The compound type of the access mode and auxiliary flags of opened files.
///
/// * If the mount option contains `-o default_permissions`, the access mode
///   flags (`O_RDONLY`, `O_WRONLY` and `O_RDWR`) might be handled by the kernel
///   and in that case, these flags are omitted before issuing the request.
///   Otherwise, the filesystem should handle these flags and return an `EACCES`
///   error when provided access mode is invalid.
/// * Some parts of the creating flags (`O_CREAT`, `O_EXCL` and `O_NOCTTY`) are
///   removed and these flags are handled by the kernel.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct OpenOptions {
    raw: u32,
}

impl fmt::Debug for OpenOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenOptions")
            .field("access_mode", &self.access_mode())
            .field("flags", &self.flags())
            .finish()
    }
}

impl OpenOptions {
    const fn from_raw(raw: u32) -> Self {
        Self { raw }
    }

    pub const fn access_mode(self) -> Option<AccessMode> {
        match self.raw as i32 & libc::O_ACCMODE {
            libc::O_RDONLY => Some(AccessMode::ReadOnly),
            libc::O_WRONLY => Some(AccessMode::WriteOnly),
            libc::O_RDWR => Some(AccessMode::ReadWrite),
            _ => None,
        }
    }

    pub const fn flags(self) -> OpenFlags {
        OpenFlags::from_bits_truncate(self.raw)
    }

    pub fn set_flags(&mut self, flags: OpenFlags) {
        self.raw &= !OpenFlags::all().bits();
        self.raw |= flags.bits();
    }
}

impl From<OpenOptions> for std::fs::OpenOptions {
    fn from(src: OpenOptions) -> Self {
        let mut options = std::fs::OpenOptions::new();
        match src.access_mode() {
            Some(AccessMode::ReadOnly) => {
                options.read(true);
            }
            Some(AccessMode::WriteOnly) => {
                options.write(true);
            }
            Some(AccessMode::ReadWrite) => {
                options.read(true).write(true);
            }
            _ => (),
        }
        options.custom_flags(src.flags().bits() as i32);
        options
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AccessMode {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

impl AccessMode {
    pub const fn into_raw(self) -> i32 {
        match self {
            Self::ReadOnly => libc::O_RDONLY,
            Self::WriteOnly => libc::O_WRONLY,
            Self::ReadWrite => libc::O_RDWR,
        }
    }
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct OpenFlags: u32 {
        // file creation flags.
        const CLOEXEC = libc::O_CLOEXEC as u32;
        const DIRECTORY = libc::O_DIRECTORY as u32;
        const NOFOLLOW = libc::O_NOFOLLOW as u32;
        const TMPFILE = libc::O_TMPFILE as u32;
        const TRUNC = libc::O_TRUNC as u32;

        // file status flags.
        const APPEND = libc::O_APPEND as u32;
        const ASYNC = libc::O_ASYNC as u32;
        const DIRECT = libc::O_DIRECT as u32;
        const DSYNC = libc::O_DSYNC as u32;
        const LARGEFILE = libc::O_LARGEFILE as u32;
        const NOATIME = libc::O_NOATIME as u32;
        const NONBLOCK = libc::O_NONBLOCK as u32;
        const NDELAY = libc::O_NDELAY as u32;
        const PATH = libc::O_PATH as u32;
        const SYNC = libc::O_SYNC as u32;
    }
}

/// Read data from a file.
///
/// The total amount of the replied data must be within `size`.
///
/// When the file is opened in `direct_io` mode, the result replied will be
/// reflected in the caller's result of `read` syscall.
///
/// When the file is not opened in `direct_io` mode (i.e. the page caching is enabled),
/// the filesystem should send *exactly* the specified range of file content to the
/// kernel. If the length of the passed data is shorter than `size`, the rest of
/// the data will be substituted with zeroes.
#[derive(Debug)]
#[non_exhaustive]
pub struct Read<'op> {
    /// The inode number to be read.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    /// The starting position of the content to be read.
    pub offset: u64,

    /// The length of the data to be read.
    pub size: u32,

    /// The flags specified at opening the file.
    pub options: OpenOptions,

    /// The identifier of lock owner.
    pub lock_owner: Option<LockOwnerID>,

    _marker: PhantomData<&'op ()>,
}

/// Write data to a file.
///
/// If the data is successfully written, the filesystem must send the amount of the written
/// data using `ReplyWrite`.
///
/// When the file is opened in `direct_io` mode, the result replied will be reflected
/// in the caller's result of `write` syscall.
///
/// When the file is not opened in `direct_io` mode (i.e. the page caching is enabled),
/// the filesystem should receive *exactly* the specified range of file content from the kernel.
#[derive(Debug)]
#[non_exhaustive]
pub struct Write<'op> {
    /// The inode number to be written.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    /// The starting position of contents to be written.
    pub offset: u64,

    /// The length of contents to be written.
    pub size: u32,

    /// The flags specified at opening the file.
    pub options: OpenOptions,

    /// The identifier of lock owner.
    pub lock_owner: Option<LockOwnerID>,

    _marker: PhantomData<&'op ()>,
}

/// Release an opened file.
#[derive(Debug)]
#[non_exhaustive]
pub struct Release<'op> {
    /// The inode number of opened file.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    /// The flags specified at opening the file.
    pub options: OpenOptions,

    /// The identifier of lock owner.
    pub lock_owner: LockOwnerID,

    /// The flags of release operation.
    pub flags: ReleaseFlags,

    _marker: PhantomData<&'op ()>,
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct ReleaseFlags: u32 {
        /// Indicates whether the operation indicates a flush.
        const FLUSH = FUSE_RELEASE_FLUSH;

        /// Indicates whether the `flock` locks for this file should be released.
        const FLOCK_UNLOCK = FUSE_RELEASE_FLOCK_UNLOCK;
    }
}

/// Get the filesystem statistics.
///
/// The obtained statistics must be sent to the kernel using `ReplyStatfs`.
#[derive(Debug)]
#[non_exhaustive]
pub struct Statfs<'op> {
    /// The inode number or `0` which means "undefined".
    pub ino: NodeID,

    _marker: PhantomData<&'op ()>,
}

/// Synchronize the file contents.
#[derive(Debug)]
#[non_exhaustive]
pub struct Fsync<'op> {
    /// The inode number to be synchronized.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    /// Indicates whether to synchronize only the file contents.
    ///
    /// When this method returns `true`, the metadata does not have to be flushed.
    pub datasync: bool,

    _marker: PhantomData<&'op ()>,
}

/// Set an extended attribute.
#[derive(Debug)]
#[non_exhaustive]
pub struct Setxattr<'op> {
    /// The inode number to set the value of extended attribute.
    pub ino: NodeID,

    /// The name of extended attribute to be set.
    pub name: &'op OsStr,

    /// The value of extended attribute.
    pub value: &'op [u8],

    /// The flags that specifies the meanings of this operation.
    pub flags: SetxattrFlags,
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct SetxattrFlags: u32 {
        const CREATE = libc::XATTR_CREATE as u32;
        const REPLACE = libc::XATTR_REPLACE as u32;
    }
}

/// Get an extended attribute.
///
/// This operation needs to switch the reply value according to the
/// value of `size`:
///
/// * When `size` is zero, the filesystem must send the length of the
///   attribute value for the specified name using `ReplyXattr`.
///
/// * Otherwise, returns the attribute value with the specified name.
///   The filesystem should send an `ERANGE` error if the specified
///   size is too small for the attribute value.
#[derive(Debug)]
#[non_exhaustive]
pub struct Getxattr<'op> {
    /// The inode number to be get the extended attribute.
    pub ino: NodeID,

    /// The name of the extend attribute.
    pub name: &'op OsStr,

    /// The maximum length of the attribute value to be replied.
    pub size: u32,
}

/// List extended attribute names.
///
/// Each element of the attribute names list must be null-terminated.
/// As with `Getxattr`, the filesystem must send the data length of the attribute
/// names using `ReplyXattr` if `size` is zero.
#[derive(Debug)]
#[non_exhaustive]
pub struct Listxattr<'op> {
    /// The inode number to be obtained the attribute names.
    pub ino: NodeID,

    /// The maximum length of the attribute names to be replied.
    pub size: u32,

    _marker: PhantomData<&'op ()>,
}

/// Remove an extended attribute.
#[derive(Debug)]
#[non_exhaustive]
pub struct Removexattr<'op> {
    /// The inode number to remove the extended attribute.
    pub ino: NodeID,

    /// The name of extended attribute to be removed.
    pub name: &'op OsStr,
}

/// Close a file descriptor.
///
/// This operation is issued on each `close(2)` syscall
/// for a file descriptor.
///
/// Do not confuse this operation with `Release`.
/// Since the file descriptor could be duplicated, the multiple
/// flush operations might be issued for one `Open`.
/// Also, it is not guaranteed that flush will always be issued
/// after some writes.
#[derive(Debug)]
#[non_exhaustive]
pub struct Flush<'op> {
    /// The inode number of target file.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    /// The identifier of lock owner.
    pub lock_owner: LockOwnerID,

    _marker: PhantomData<&'op ()>,
}

/// Open a directory.
///
/// If the directory is successfully opened, the filesystem must send
/// the identifier to the opened directory handle using `ReplyOpen`.
#[derive(Debug)]
#[non_exhaustive]
pub struct Opendir<'op> {
    /// The inode number to be opened.
    pub ino: NodeID,

    /// The open flags.
    pub options: OpenOptions,

    _marker: PhantomData<&'op ()>,
}

/// Read contents from an opened directory.
#[derive(Debug)]
#[non_exhaustive]
pub struct Readdir<'op> {
    /// The inode number to be read.
    pub ino: NodeID,

    /// The handle of opened directory.
    pub fh: FileID,

    /// The *offset* value to continue reading the directory stream.
    pub offset: u64,

    /// The maximum length of returned data.
    pub size: u32,

    pub mode: ReaddirMode,

    _marker: PhantomData<&'op ()>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReaddirMode {
    Normal,
    Plus,
}

/// Release an opened directory.
#[derive(Debug)]
#[non_exhaustive]
pub struct Releasedir<'op> {
    /// The inode number of opened directory.
    pub ino: NodeID,

    /// The handle of opened directory.
    pub fh: FileID,

    /// The open flags.
    pub options: OpenOptions,

    _marker: PhantomData<&'op ()>,
}

/// Synchronize the directory contents.
#[derive(Debug)]
#[non_exhaustive]
pub struct Fsyncdir<'op> {
    /// The inode number to be synchronized.
    pub ino: NodeID,

    /// The handle of opened directory.
    pub fh: FileID,

    /// Indicates whether to synchronize only the directory contents.
    ///
    /// When this method returns `true`, the metadata does not have to be flushed.
    pub datasync: bool,

    _marker: PhantomData<&'op ()>,
}

/// Test for a POSIX file lock.
///
/// The lock result must be replied using `ReplyLk`.
#[derive(Debug)]
#[non_exhaustive]
pub struct Getlk<'op> {
    /// The inode number to be tested the lock.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    /// The identifier of lock owner.
    pub owner: LockOwnerID,

    pub file_lock: FileLock,

    _marker: PhantomData<&'op ()>,
}

/// Acquire, modify or release a POSIX file lock.
#[derive(Debug)]
#[non_exhaustive]
pub struct Setlk<'op> {
    /// The inode number to be obtained the lock.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    pub file_lock: FileLock,

    /// Indicates whether the locking operation might sleep until a lock is obtained.
    pub sleep: bool,

    _marker: PhantomData<&'op ()>,
}

/// Acquire, modify or release a BSD file lock.
#[derive(Debug)]
#[non_exhaustive]
pub struct Flock<'op> {
    /// The target inode number.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    /// The identifier of lock owner.
    pub owner: LockOwnerID,

    /// Return the locking operation.
    ///
    /// See [`flock(2)`][flock] for details.
    ///
    /// [flock]: http://man7.org/linux/man-pages/man2/flock.2.html
    pub op: FlockOp,

    _marker: PhantomData<&'op ()>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[repr(i32)]
pub enum FlockOp {
    Shared = libc::LOCK_SH,
    SharedNonblock = libc::LOCK_SH | libc::LOCK_NB,
    Exclusive = libc::LOCK_EX,
    ExclusiveNonblock = libc::LOCK_EX | libc::LOCK_NB,
    Unlock = libc::LOCK_UN,
}

impl FlockOp {
    pub const fn into_raw(self) -> i32 {
        self as i32
    }
}

/// Check file access permissions.
#[derive(Debug)]
#[non_exhaustive]
pub struct Access<'op> {
    /// The inode number subject to the access permission check.
    pub ino: NodeID,

    /// The requested access mode.
    pub mask: FilePermissions,

    _marker: PhantomData<&'op ()>,
}

/// Create and open a file.
///
/// This operation is a combination of `Mknod` and `Open`. If an `ENOSYS` error is returned
/// for this operation, those operations will be used instead.
///
/// If the file is successfully created and opened, a pair of `ReplyEntry` and `ReplyOpen`
/// with the corresponding attribute values and the file handle must be sent to the kernel.
#[derive(Debug)]
#[non_exhaustive]
pub struct Create<'op> {
    /// The inode number of the parent directory.
    ///
    /// See also [`Mknod::parent`].
    pub parent: NodeID,

    /// The file name to crate.
    ///
    /// See also [`Mknod::name`].
    pub name: &'op OsStr,

    /// The file type and permissions used when creating the new file.
    ///
    /// See also [`Mknod::mode`].
    pub mode: FileMode,

    /// The mask of permissions for the file to be created.
    ///
    /// See also [`Mknod::umask`].
    pub umask: FilePermissions,

    /// The open flags.
    ///
    /// See also [`Open::options`].
    pub open_options: OpenOptions,
}

/// Map block index within a file to block index within device.
///
/// The mapping result must be replied using `ReplyBmap`.
///
/// This operation makes sense only for filesystems that use
/// block devices, and is called only when the mount options
/// contains `blkdev`.
#[derive(Debug)]
#[non_exhaustive]
pub struct Bmap<'op> {
    /// The inode number of the file node to be mapped.
    pub ino: NodeID,

    /// The block index to be mapped.
    pub block: u64,

    /// The unit of block index.
    pub blocksize: u32,

    _marker: PhantomData<&'op ()>,
}

/// Allocate requested space.
///
/// If this operation is successful, the filesystem shall not report
/// the error caused by the lack of free spaces to subsequent write
/// requests.
#[derive(Debug)]
#[non_exhaustive]
pub struct Fallocate<'op> {
    /// The number of target inode to be allocated the space.
    pub ino: NodeID,

    /// The handle for opened file.
    pub fh: FileID,

    /// The starting point of region to be allocated.
    pub offset: u64,

    /// The length of region to be allocated.
    pub length: u64,

    /// The mode that specifies how to allocate the region.
    ///
    /// See [`fallocate(2)`][fallocate] for details.
    ///
    /// [fallocate]: http://man7.org/linux/man-pages/man2/fallocate.2.html
    pub mode: FallocateFlags,

    _marker: PhantomData<&'op ()>,
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct FallocateFlags: u32 {
        const KEEP_SIZE = libc::FALLOC_FL_KEEP_SIZE as u32;
        const UNSHARE_RANGE = libc::FALLOC_FL_UNSHARE_RANGE as u32;
        const PUNCH_HOLE = libc::FALLOC_FL_PUNCH_HOLE as u32;
        const COLLAPSE_RANGE = libc::FALLOC_FL_COLLAPSE_RANGE as u32;
        const ZERO_RANGE = libc::FALLOC_FL_ZERO_RANGE as u32;
        const INSERT_RANGE = libc::FALLOC_FL_INSERT_RANGE as u32;
    }
}

/// Copy a range of data from an opened file to another.
///
/// The length of copied data must be replied using `ReplyWrite`.
#[derive(Debug)]
#[non_exhaustive]
pub struct CopyFileRange<'op> {
    /// The inode number of source file.
    pub ino_in: NodeID,

    /// The file handle of source file.
    pub fh_in: FileID,

    /// The starting point of source file where the data should be read.
    pub offset_in: u64,

    /// The inode number of target file.
    pub ino_out: NodeID,

    /// The file handle of target file.
    pub fh_out: FileID,

    /// The starting point of target file where the data should be written.
    pub offset_out: u64,

    /// The maximum size of data to copy.
    pub length: u64,

    /// The flag value for `copy_file_range` syscall.
    pub flags: u64,

    _marker: PhantomData<&'op ()>,
}

/// Poll for readiness.
///
/// The mask of ready poll events must be replied using `ReplyPoll`.
#[derive(Debug)]
#[non_exhaustive]
pub struct Poll<'op> {
    /// The inode number to check the I/O readiness.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    /// The requested poll events.
    pub events: PollEvents,

    /// The handle to this poll.
    ///
    /// If the returned value is not `None`, the filesystem should send the notification
    /// when the corresponding I/O will be ready.
    pub kh: Option<PollWakeupID>,

    _marker: PhantomData<&'op ()>,
}

/// Reposition the offset of read/write operations.
///
/// See [`lseek(2)`][lseek] for details.
///
/// [lseek]: https://man7.org/linux/man-pages/man2/lseek.2.html
#[derive(Debug)]
#[non_exhaustive]
pub struct Lseek<'op> {
    pub ino: NodeID,
    pub fh: FileID,
    pub offset: u64,
    pub whence: Whence,
    _marker: PhantomData<&'op ()>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Whence {
    Set,
    Current,
    End,
    Data,
    Hole,
}
