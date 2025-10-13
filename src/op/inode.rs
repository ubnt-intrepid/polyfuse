use crate::types::{DeviceID, FileMode, FilePermissions, NodeID};
use bitflags::bitflags;
use polyfuse_kernel::{
    fuse_access_in, fuse_bmap_in, fuse_link_in, fuse_mkdir_in, fuse_mknod_in,
    fuse_mknod_in_compat_11, fuse_opcode, fuse_rename2_in, fuse_rename_in,
};
use std::{ffi::OsStr, marker::PhantomData};

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

impl<'op> super::Op<'op> for Lookup<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let name = cx.decoder.fetch_str()?;
        Ok(Lookup {
            parent: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            name,
        })
    }
}

/// Read a symbolic link.
#[derive(Debug)]
#[non_exhaustive]
pub struct Readlink<'op> {
    /// The inode number to be read the link value.
    pub ino: NodeID,

    _marker: PhantomData<&'op ()>,
}

impl<'op> super::Op<'op> for Readlink<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            _marker: PhantomData,
        })
    }
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

impl<'op> super::Op<'op> for Symlink<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let parent = cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?;
        let name = cx.decoder.fetch_str()?;
        let link = cx.decoder.fetch_str()?;
        Ok(Self { parent, name, link })
    }
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

impl<'op> super::Op<'op> for Mknod<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        if cx.config.minor <= 11 {
            let arg: &fuse_mknod_in_compat_11 = cx.decoder.fetch()?;
            let name = cx.decoder.fetch_str()?;
            Ok(Self {
                parent: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
                name,
                mode: FileMode::from_raw(arg.mode),
                rdev: DeviceID::from_kernel_dev(arg.rdev),
                umask: FilePermissions::empty(),
            })
        } else {
            let arg: &fuse_mknod_in = cx.decoder.fetch()?;
            let name = cx.decoder.fetch_str()?;
            Ok(Self {
                parent: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
                name,
                mode: FileMode::from_raw(arg.mode),
                rdev: DeviceID::from_kernel_dev(arg.rdev),
                umask: FilePermissions::from_bits_truncate(arg.umask) & FilePermissions::MASK,
            })
        }
    }
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

impl<'op> super::Op<'op> for Mkdir<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_mkdir_in = cx.decoder.fetch()?;
        let name = cx.decoder.fetch_str()?;
        Ok(Self {
            parent: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            name,
            permissions: FilePermissions::from_bits_truncate(arg.mode),
            umask: FilePermissions::from_bits_truncate(arg.umask) & FilePermissions::MASK,
        })
    }
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

impl<'op> super::Op<'op> for Unlink<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let name = cx.decoder.fetch_str()?;
        Ok(Self {
            parent: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            name,
        })
    }
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

impl<'op> super::Op<'op> for Rmdir<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let name = cx.decoder.fetch_str()?;
        Ok(Self {
            parent: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            name,
        })
    }
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

impl<'op> super::Op<'op> for Rename<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        match cx.opcode {
            fuse_opcode::FUSE_RENAME => {
                let arg: &fuse_rename_in = cx.decoder.fetch()?;
                let name = cx.decoder.fetch_str()?;
                let newname = cx.decoder.fetch_str()?;
                Ok(Self {
                    parent: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
                    name,
                    newparent: NodeID::from_raw(arg.newdir).ok_or(super::Error::InvalidNodeID)?,
                    newname,
                    flags: RenameFlags::empty(),
                })
            }

            fuse_opcode::FUSE_RENAME2 => {
                let arg: &fuse_rename2_in = cx.decoder.fetch()?;
                let name = cx.decoder.fetch_str()?;
                let newname = cx.decoder.fetch_str()?;
                Ok(Self {
                    parent: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
                    name,
                    newparent: NodeID::from_raw(arg.newdir).ok_or(super::Error::InvalidNodeID)?,
                    newname,
                    flags: RenameFlags::from_bits_truncate(arg.flags),
                })
            }

            _ => unreachable!(),
        }
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

impl<'op> super::Op<'op> for Link<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_link_in = cx.decoder.fetch()?;
        let newname = cx.decoder.fetch_str()?;
        Ok(Self {
            ino: NodeID::from_raw(arg.oldnodeid).ok_or(super::Error::InvalidNodeID)?,
            newparent: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            newname,
        })
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

impl<'op> super::Op<'op> for Statfs<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            _marker: PhantomData,
        })
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

impl<'op> super::Op<'op> for Access<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_access_in = cx.decoder.fetch()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            mask: FilePermissions::from_bits_truncate(arg.mask) & FilePermissions::MASK,
            _marker: PhantomData,
        })
    }
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

impl<'op> super::Op<'op> for Bmap<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_bmap_in = cx.decoder.fetch()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            block: arg.block,
            blocksize: arg.blocksize,
            _marker: PhantomData,
        })
    }
}
