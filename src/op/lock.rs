use polyfuse_kernel::{fuse_lk_in, fuse_opcode, FUSE_LK_FLOCK};
use rustix::thread::Pid;

use crate::types::{FileID, FileLock, LockOwnerID, NodeID};
use std::marker::PhantomData;

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

impl<'op> super::Op<'op> for Getlk<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_lk_in = cx.decoder.fetch()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            fh: FileID::from_raw(arg.fh),
            owner: LockOwnerID::from_raw(arg.owner),
            file_lock: FileLock {
                typ: arg.lk.typ,
                start: arg.lk.start,
                end: arg.lk.end,
                pid: Pid::from_raw(arg.lk.pid as i32).ok_or(super::Error::InvalidPid)?,
            },
            _marker: PhantomData,
        })
    }
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

#[derive(Debug)]
#[non_exhaustive]
pub(super) enum SetlkKind<'op> {
    Posix(Setlk<'op>),
    Flock(Flock<'op>),
}

impl<'op> super::Op<'op> for SetlkKind<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_lk_in = cx.decoder.fetch()?;
        let sleep = match cx.opcode {
            fuse_opcode::FUSE_SETLK => false,
            fuse_opcode::FUSE_SETLKW => true,
            _ => unreachable!(),
        };

        if arg.lk_flags & FUSE_LK_FLOCK == 0 {
            Ok(Self::Posix(Setlk {
                ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
                fh: FileID::from_raw(arg.fh),
                file_lock: FileLock {
                    typ: arg.lk.typ,
                    start: arg.lk.start,
                    end: arg.lk.end,
                    pid: Pid::from_raw(arg.lk.pid as i32).ok_or(super::Error::InvalidPid)?,
                },
                sleep,
                _marker: PhantomData,
            }))
        } else {
            Ok(Self::Flock(Flock {
                ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
                fh: FileID::from_raw(arg.fh),
                owner: LockOwnerID::from_raw(arg.owner),
                op: match arg.lk.typ as i32 {
                    libc::F_RDLCK if sleep => FlockOp::Shared,
                    libc::F_RDLCK => FlockOp::SharedNonblock,
                    libc::F_WRLCK if sleep => FlockOp::Exclusive,
                    libc::F_WRLCK => FlockOp::ExclusiveNonblock,
                    libc::F_UNLCK => FlockOp::Unlock,
                    _ => return Err(super::Error::InvalidFlockType),
                },
                _marker: PhantomData,
            }))
        }
    }
}
