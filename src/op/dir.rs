use polyfuse_kernel::{
    fuse_fsync_in, fuse_opcode, fuse_open_in, fuse_read_in, fuse_release_in, FUSE_FSYNC_FDATASYNC,
};

use super::file::OpenOptions;
use crate::types::{FileID, NodeID};
use std::marker::PhantomData;

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

impl<'op> super::Op<'op> for Opendir<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_open_in = cx.decoder.fetch()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            options: OpenOptions::from_raw(arg.flags),
            _marker: PhantomData,
        })
    }
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

impl<'op> super::Op<'op> for Readdir<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_read_in = cx.decoder.fetch()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            fh: FileID::from_raw(arg.fh),
            offset: arg.offset,
            size: arg.size,
            mode: if cx.opcode == fuse_opcode::FUSE_READDIRPLUS {
                ReaddirMode::Plus
            } else {
                ReaddirMode::Normal
            },
            _marker: PhantomData,
        })
    }
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

impl<'op> super::Op<'op> for Releasedir<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_release_in = cx.decoder.fetch()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            fh: FileID::from_raw(arg.fh),
            options: OpenOptions::from_raw(arg.flags),
            _marker: PhantomData,
        })
    }
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

impl<'op> super::Op<'op> for Fsyncdir<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_fsync_in = cx.decoder.fetch()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            fh: FileID::from_raw(arg.fh),
            datasync: arg.fsync_flags & FUSE_FSYNC_FDATASYNC != 0,
            _marker: PhantomData,
        })
    }
}
