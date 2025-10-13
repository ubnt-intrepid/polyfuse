use super::file::OpenOptions;
use crate::types::{FileMode, FilePermissions, NodeID};
use polyfuse_kernel::fuse_create_in;
use std::ffi::OsStr;

#[cfg(doc)]
use super::{file::Open, inode::Mknod};

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

impl<'op> super::Op<'op> for Create<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_create_in = cx.decoder.fetch()?;
        let name = cx.decoder.fetch_str()?;
        Ok(Self {
            parent: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            name,
            mode: FileMode::from_raw(arg.mode),
            umask: FilePermissions::from_bits_truncate(arg.umask) & FilePermissions::MASK,
            open_options: OpenOptions::from_raw(arg.flags),
        })
    }
}
