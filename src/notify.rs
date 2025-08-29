use std::ffi::OsStr;

use crate::bytes::{Bytes, POD};
use polyfuse_kernel::{
    fuse_notify_code, fuse_notify_delete_out, fuse_notify_inval_entry_out,
    fuse_notify_inval_inode_out, fuse_notify_poll_wakeup_out, fuse_notify_retrieve_out,
    fuse_notify_store_out,
};

pub trait Notify {
    const NOTIFY_CODE: fuse_notify_code;
    fn bytes(&self) -> impl Bytes;
}

/// Notify the cache invalidation about an inode to the kernel.
pub struct InvalNode {
    out: POD<fuse_notify_inval_inode_out>,
}

impl InvalNode {
    pub const fn new(ino: u64, off: i64, len: i64) -> Self {
        Self {
            out: POD(fuse_notify_inval_inode_out { ino, off, len }),
        }
    }
}

impl Notify for InvalNode {
    const NOTIFY_CODE: fuse_notify_code = fuse_notify_code::FUSE_NOTIFY_INVAL_INODE;

    fn bytes(&self) -> impl Bytes {
        &self.out
    }
}

/// Notify the invalidation about a directory entry to the kernel.
pub struct InvalEntry<'a> {
    out: POD<fuse_notify_inval_entry_out>,
    name: &'a OsStr,
}

impl<'a> InvalEntry<'a> {
    pub fn new(parent: u64, name: &'a OsStr) -> Self {
        let namelen = name.len().try_into().expect("provided name is too long");
        Self {
            out: POD(fuse_notify_inval_entry_out {
                parent,
                namelen,
                padding: 0,
            }),
            name,
        }
    }
}

impl Notify for InvalEntry<'_> {
    const NOTIFY_CODE: fuse_notify_code = fuse_notify_code::FUSE_NOTIFY_INVAL_ENTRY;

    fn bytes(&self) -> impl Bytes {
        (&self.out, self.name, b"\0")
    }
}

/// Notify the invalidation about a directory entry to the kernel.
///
/// The role of this notification is similar to `InvalEntry`.
/// Additionally, when the provided `child` inode matches the inode
/// in the dentry cache, the inotify will inform the deletion to
/// watchers if exists.
pub struct Delete<'a> {
    out: POD<fuse_notify_delete_out>,
    name: &'a OsStr,
}

impl<'a> Delete<'a> {
    pub fn new(parent: u64, child: u64, name: &'a OsStr) -> Self {
        let namelen = name.len().try_into().expect("provided name is too long");
        Self {
            out: POD(fuse_notify_delete_out {
                parent,
                child,
                namelen,
                padding: 0,
            }),
            name,
        }
    }
}

impl Notify for Delete<'_> {
    const NOTIFY_CODE: fuse_notify_code = fuse_notify_code::FUSE_NOTIFY_DELETE;

    fn bytes(&self) -> impl Bytes {
        (&self.out, self.name, b"\0")
    }
}

/// Push the data in an inode for updating the kernel cache.
pub struct Store<B: Bytes> {
    out: POD<fuse_notify_store_out>,
    data: B,
}

impl<B> Store<B>
where
    B: Bytes,
{
    pub fn new(ino: u64, offset: u64, data: B) -> Self {
        let size = data.size().try_into().expect("provided data is too large");
        Self {
            out: POD(fuse_notify_store_out {
                nodeid: ino,
                offset,
                size,
                padding: 0,
            }),
            data,
        }
    }
}

impl<B> Notify for Store<B>
where
    B: Bytes,
{
    const NOTIFY_CODE: fuse_notify_code = fuse_notify_code::FUSE_NOTIFY_STORE;

    fn bytes(&self) -> impl Bytes {
        (&self.out, &self.data)
    }
}

/// Retrieve data in an inode from the kernel cache.
#[repr(transparent)]
pub struct Retrieve {
    inner: POD<fuse_notify_retrieve_out>,
}

impl Retrieve {
    pub const fn new(unique: u64, ino: u64, offset: u64, size: u32) -> Self {
        Self {
            inner: POD(fuse_notify_retrieve_out {
                notify_unique: unique,
                nodeid: ino,
                offset,
                size,
                padding: 0,
            }),
        }
    }
}

impl Notify for Retrieve {
    const NOTIFY_CODE: fuse_notify_code = fuse_notify_code::FUSE_NOTIFY_RETRIEVE;

    fn bytes(&self) -> impl Bytes {
        &self.inner
    }
}

/// Send I/O readiness to the kernel.
#[repr(transparent)]
pub struct PollWakeup {
    inner: POD<fuse_notify_poll_wakeup_out>,
}

impl PollWakeup {
    pub const fn new(kh: u64) -> Self {
        Self {
            inner: POD(fuse_notify_poll_wakeup_out { kh }),
        }
    }
}

impl Notify for PollWakeup {
    const NOTIFY_CODE: fuse_notify_code = fuse_notify_code::FUSE_NOTIFY_POLL;
    fn bytes(&self) -> impl Bytes {
        &self.inner
    }
}
