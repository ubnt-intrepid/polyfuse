use crate::bytes::{Bytes, Collector};
use polyfuse_kernel::*;
use std::{convert::TryFrom as _, ffi::OsStr, mem, os::unix::prelude::*};
use zerocopy::AsBytes as _;

/// Notify the cache invalidation about an inode to the kernel.
pub struct InvalInode {
    header: fuse_out_header,
    arg: fuse_notify_inval_inode_out,
}

impl InvalInode {
    #[inline]
    pub fn new(ino: u64, off: i64, len: i64) -> Self {
        let total_len = u32::try_from(
            mem::size_of::<fuse_out_header>() + mem::size_of::<fuse_notify_inval_inode_out>(),
        )
        .unwrap();
        Self {
            header: fuse_out_header {
                len: total_len,
                error: fuse_notify_code::FUSE_NOTIFY_INVAL_INODE as i32,
                unique: 0,
            },
            arg: fuse_notify_inval_inode_out { ino, off, len },
        }
    }
}

impl Bytes for InvalInode {
    fn size(&self) -> usize {
        mem::size_of::<fuse_out_header>() + mem::size_of::<fuse_notify_inval_inode_out>()
    }

    fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>) {
        collector.append(self.header.as_bytes());
        collector.append(self.arg.as_bytes());
    }
}

/// Notify the invalidation about a directory entry to the kernel.
pub struct InvalEntry<T>
where
    T: AsRef<OsStr>,
{
    header: fuse_out_header,
    arg: fuse_notify_inval_entry_out,
    name: T,
}

impl<T> InvalEntry<T>
where
    T: AsRef<OsStr>,
{
    #[inline]
    pub fn new(parent: u64, name: T) -> Self {
        let namelen = u32::try_from(name.as_ref().len()).expect("provided name is too long");

        let total_len = u32::try_from(
            mem::size_of::<fuse_out_header>()
                + mem::size_of::<fuse_notify_inval_entry_out>()
                + name.as_ref().len(),
        )
        .unwrap();

        Self {
            header: fuse_out_header {
                len: total_len,
                error: fuse_notify_code::FUSE_NOTIFY_INVAL_ENTRY as i32,
                unique: 0,
            },
            arg: fuse_notify_inval_entry_out {
                parent,
                namelen: namelen + 1,
                padding: 0,
            },
            name,
        }
    }
}

impl<T> Bytes for InvalEntry<T>
where
    T: AsRef<OsStr>,
{
    fn size(&self) -> usize {
        self.header.len as usize
    }

    fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>) {
        collector.append(self.header.as_bytes());
        collector.append(self.arg.as_bytes());
        collector.append(self.name.as_ref().as_bytes());
        collector.append(&[0]); // null terminator
    }
}

/// Notify the invalidation about a directory entry to the kernel.
///
/// The role of this notification is similar to `notify_inval_entry`.
/// Additionally, when the provided `child` inode matches the inode
/// in the dentry cache, the inotify will inform the deletion to
/// watchers if exists.
pub struct Delete<T>
where
    T: AsRef<OsStr>,
{
    header: fuse_out_header,
    arg: fuse_notify_delete_out,
    name: T,
}

impl<T> Delete<T>
where
    T: AsRef<OsStr>,
{
    #[inline]
    pub fn new(parent: u64, child: u64, name: T) -> Self {
        let namelen = u32::try_from(name.as_ref().len()).expect("provided name is too long");

        let total_len = u32::try_from(
            mem::size_of::<fuse_out_header>()
                + mem::size_of::<fuse_notify_delete_out>()
                + name.as_ref().len(),
        )
        .expect("payload is too long");

        Self {
            header: fuse_out_header {
                len: total_len,
                error: fuse_notify_code::FUSE_NOTIFY_DELETE as i32,
                unique: 0,
            },
            arg: fuse_notify_delete_out {
                parent,
                child,
                namelen: namelen + 1,
                padding: 0,
            },
            name,
        }
    }
}

impl<T> Bytes for Delete<T>
where
    T: AsRef<OsStr>,
{
    fn size(&self) -> usize {
        self.header.len as usize
    }

    fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>) {
        collector.append(self.header.as_bytes());
        collector.append(self.arg.as_bytes());
        collector.append(self.name.as_ref().as_bytes());
        collector.append(&[0]); // null terminator
    }
}

/// Push the data in an inode for updating the kernel cache.
pub struct Store<T>
where
    T: Bytes,
{
    header: fuse_out_header,
    arg: fuse_notify_store_out,
    data: T,
}

impl<T> Store<T>
where
    T: Bytes,
{
    #[inline]
    pub fn new(ino: u64, offset: u64, data: T) -> Self {
        let size = u32::try_from(data.size()).expect("provided data is too large");

        let total_len = u32::try_from(
            mem::size_of::<fuse_out_header>() + mem::size_of::<fuse_notify_store_out>(),
        )
        .expect("payload is too long");

        Self {
            header: fuse_out_header {
                len: total_len,
                error: fuse_notify_code::FUSE_NOTIFY_STORE as i32,
                unique: 0,
            },
            arg: fuse_notify_store_out {
                nodeid: ino,
                offset,
                size,
                padding: 0,
            },
            data,
        }
    }
}

impl<T> Bytes for Store<T>
where
    T: Bytes,
{
    fn size(&self) -> usize {
        self.header.len as usize
    }

    fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>) {
        collector.append(self.header.as_bytes());
        collector.append(self.arg.as_bytes());
        self.data.collect(collector);
    }
}

/// Retrieve data in an inode from the kernel cache.
pub struct Retrieve {
    header: fuse_out_header,
    arg: fuse_notify_retrieve_out,
}

impl Retrieve {
    #[inline]
    pub fn new(unique: u64, ino: u64, offset: u64, size: u32) -> Self {
        let total_len = u32::try_from(
            mem::size_of::<fuse_out_header>() + mem::size_of::<fuse_notify_retrieve_out>(),
        )
        .unwrap();
        Self {
            header: fuse_out_header {
                len: total_len,
                error: fuse_notify_code::FUSE_NOTIFY_RETRIEVE as i32,
                unique: 0,
            },
            arg: fuse_notify_retrieve_out {
                nodeid: ino,
                offset,
                size,
                notify_unique: unique,
                padding: 0,
            },
        }
    }
}

impl Bytes for Retrieve {
    fn size(&self) -> usize {
        self.header.len as usize
    }

    fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>) {
        collector.append(self.header.as_bytes());
        collector.append(self.arg.as_bytes());
    }
}

/// Send I/O readiness to the kernel.
pub struct PollWakeup {
    header: fuse_out_header,
    arg: fuse_notify_poll_wakeup_out,
}

impl PollWakeup {
    #[inline]
    pub fn new(kh: u64) -> Self {
        let total_len = u32::try_from(
            mem::size_of::<fuse_out_header>() + mem::size_of::<fuse_notify_poll_wakeup_out>(),
        )
        .unwrap();
        Self {
            header: fuse_out_header {
                len: total_len,
                error: fuse_notify_code::FUSE_NOTIFY_POLL as i32,
                unique: 0,
            },
            arg: fuse_notify_poll_wakeup_out { kh },
        }
    }
}

impl Bytes for PollWakeup {
    fn size(&self) -> usize {
        self.header.len as usize
    }

    fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>) {
        collector.append(self.header.as_bytes());
        collector.append(self.arg.as_bytes());
    }
}

// pub fn notify_poll_wakeup<W>(&self, writer: W, kh: u64) -> io::Result<()>
// where
//     W: io::Write,
// {
//     self.ensure_session_is_alived()?;

//     let out = fuse_notify_poll_wakeup_out { kh };

//     write_bytes(
//         writer,
//         Reply::notify(fuse_notify_code::FUSE_NOTIFY_POLL, out.as_bytes()),
//     )
// }
