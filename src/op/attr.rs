use super::{Context, Error};
use crate::types::{FileID, FileMode, LockOwnerID, NodeID};
use polyfuse_kernel::{
    fuse_getattr_in, fuse_setattr_in, FATTR_ATIME, FATTR_ATIME_NOW, FATTR_CTIME, FATTR_FH,
    FATTR_GID, FATTR_LOCKOWNER, FATTR_MODE, FATTR_MTIME, FATTR_MTIME_NOW, FATTR_SIZE, FATTR_UID,
    FUSE_GETATTR_FH,
};
use rustix::fs::{Gid, Uid};
use std::{marker::PhantomData, time::Duration};

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

impl<'op> super::Op<'op> for Getattr<'op> {
    fn decode(cx: &mut Context<'op>) -> Result<Self, Error> {
        let arg: &fuse_getattr_in = cx.decoder.fetch()?;
        Ok(Getattr {
            ino: cx.header.nodeid().ok_or(Error::InvalidNodeID)?,
            fh: (arg.getattr_flags & FUSE_GETATTR_FH != 0).then_some(FileID::from_raw(arg.fh)),
            _marker: PhantomData,
        })
    }
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

impl<'op> super::Op<'op> for Setattr<'op> {
    fn decode(cx: &mut Context<'op>) -> Result<Self, Error> {
        let arg: &fuse_setattr_in = cx.decoder.fetch()?;
        Ok(Setattr {
            ino: cx.header.nodeid().ok_or(Error::InvalidNodeID)?,
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
            ctime: (arg.valid & FATTR_CTIME != 0).then(|| Duration::new(arg.ctime, arg.ctimensec)),
            lock_owner: (arg.valid & FATTR_LOCKOWNER != 0)
                .then(|| LockOwnerID::from_raw(arg.lock_owner)),
            _marker: PhantomData,
        })
    }
}
