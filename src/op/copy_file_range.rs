use crate::types::{FileID, NodeID};
use polyfuse_kernel::fuse_copy_file_range_in;
use std::marker::PhantomData;

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

impl<'op> super::Op<'op> for CopyFileRange<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_copy_file_range_in = cx.decoder.fetch()?;
        Ok(CopyFileRange {
            ino_in: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            fh_in: FileID::from_raw(arg.fh_in),
            offset_in: arg.off_in,
            ino_out: NodeID::from_raw(arg.nodeid_out).ok_or(super::Error::InvalidNodeID)?,
            fh_out: FileID::from_raw(arg.fh_out),
            offset_out: arg.off_out,
            length: arg.len,
            flags: arg.flags,
            _marker: PhantomData,
        })
    }
}
