use crate::{
    bytes::{Bytes, POD},
    types::{NodeID, NotifyID, PollWakeupID},
};
use polyfuse_kernel::{
    fuse_notify_code, fuse_notify_delete_out, fuse_notify_inval_entry_out,
    fuse_notify_inval_inode_out, fuse_notify_poll_wakeup_out, fuse_notify_retrieve_out,
    fuse_notify_store_out,
};
use std::{ffi::OsStr, io, os::unix::prelude::*};

pub trait Notifier: Sized {
    /// Send a notification message to the kernel.
    fn send<B>(self, code: fuse_notify_code, arg: B) -> io::Result<()>
    where
        B: Bytes;

    /// Generate a new NotifyID.
    fn new_notify_unique(&self) -> NotifyID;

    fn inval_inode(self, ino: NodeID, off: i64, len: i64) -> io::Result<()> {
        self.send(
            fuse_notify_code::FUSE_NOTIFY_INVAL_INODE,
            POD(fuse_notify_inval_inode_out {
                ino: ino.into_raw(),
                off,
                len,
            }),
        )
    }

    fn inval_entry(self, parent: NodeID, name: impl AsRef<OsStr>) -> io::Result<()> {
        let name = name.as_ref();
        self.send(
            fuse_notify_code::FUSE_NOTIFY_INVAL_ENTRY,
            (
                POD(fuse_notify_inval_entry_out {
                    parent: parent.into_raw(),
                    namelen: name.len() as u32,
                    flags: 0,
                }),
                name.as_bytes(),
                "\0",
            ),
        )
    }

    fn delete(self, parent: NodeID, child: NodeID, name: impl AsRef<OsStr>) -> io::Result<()> {
        let name = name.as_ref();
        self.send(
            fuse_notify_code::FUSE_NOTIFY_DELETE,
            (
                POD(fuse_notify_delete_out {
                    parent: parent.into_raw(),
                    child: child.into_raw(),
                    namelen: name.len() as u32,
                    padding: 0,
                }),
                name.as_bytes(),
                b"\0",
            ),
        )
    }

    fn store<B>(self, ino: NodeID, offset: u64, content: B) -> io::Result<()>
    where
        B: Bytes,
    {
        let size = content.size();
        self.send(
            fuse_notify_code::FUSE_NOTIFY_STORE,
            (
                POD(fuse_notify_store_out {
                    nodeid: ino.into_raw(),
                    offset,
                    size: size as u32,
                    padding: 0,
                }),
                content,
            ),
        )
    }

    fn retrieve(self, ino: NodeID, offset: u64, size: u32) -> io::Result<NotifyID> {
        let notify_unique = self.new_notify_unique();
        self.send(
            fuse_notify_code::FUSE_NOTIFY_RETRIEVE,
            POD(fuse_notify_retrieve_out {
                notify_unique: notify_unique.into_raw(),
                nodeid: ino.into_raw(),
                offset,
                size,
                padding: 0,
            }),
        )?;
        Ok(notify_unique)
    }

    fn poll_wakeup(self, kh: PollWakeupID) -> io::Result<()> {
        self.send(
            fuse_notify_code::FUSE_NOTIFY_POLL,
            POD(fuse_notify_poll_wakeup_out { kh: kh.into_raw() }),
        )
    }
}
