use crate::types::NodeID;
use polyfuse_kernel::{fuse_getxattr_in, fuse_setxattr_in, fuse_setxattr_in_compat_32};
use std::{ffi::OsStr, marker::PhantomData};

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

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct SetxattrFlags: u32 {
        const CREATE = libc::XATTR_CREATE as u32;
        const REPLACE = libc::XATTR_REPLACE as u32;
    }
}

impl<'op> super::Op<'op> for Setxattr<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        if cx.config.minor <= 32 {
            let arg: &fuse_setxattr_in_compat_32 = cx.decoder.fetch()?;
            let name = cx.decoder.fetch_str()?;
            let value = cx.decoder.fetch_bytes(arg.size as usize)?;
            Ok(Self {
                ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
                name,
                value,
                flags: SetxattrFlags::from_bits_truncate(arg.flags),
            })
        } else {
            // FIXME: treat setxattr_flags
            let arg: &fuse_setxattr_in = cx.decoder.fetch()?;
            let name = cx.decoder.fetch_str()?;
            let value = cx.decoder.fetch_bytes(arg.size as usize)?;
            Ok(Self {
                ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
                name,
                value,
                flags: SetxattrFlags::from_bits_truncate(arg.flags),
            })
        }
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

impl<'op> super::Op<'op> for Getxattr<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_getxattr_in = cx.decoder.fetch()?;
        let name = cx.decoder.fetch_str()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            name,
            size: arg.size,
        })
    }
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

impl<'op> super::Op<'op> for Listxattr<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_getxattr_in = cx.decoder.fetch()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            size: arg.size,
            _marker: PhantomData,
        })
    }
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

impl<'op> super::Op<'op> for Removexattr<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let name = cx.decoder.fetch_str()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            name,
        })
    }
}
