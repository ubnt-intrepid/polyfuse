//! Send notification to the kernel.

#![allow(clippy::needless_update)]

use crate::{
    reply::{as_bytes, send_msg},
    session::Session,
};
use futures::{
    channel::oneshot,
    future::{Fuse, FusedFuture, Future, FutureExt},
    io::AsyncWrite,
    lock::Mutex,
};
use polyfuse_sys::kernel::{
    fuse_notify_code, //
    fuse_notify_delete_out,
    fuse_notify_inval_entry_out,
    fuse_notify_inval_inode_out,
    fuse_notify_poll_wakeup_out,
    fuse_notify_retrieve_out,
    fuse_notify_store_out,
};
use smallvec::SmallVec;
use std::{
    collections::HashMap,
    convert::TryFrom,
    ffi::OsStr,
    io, mem,
    os::unix::ffi::OsStrExt,
    pin::Pin,
    sync::atomic::{AtomicU64, Ordering},
    task::{self, Poll},
};

/// Notification sender.
#[derive(Debug)]
pub struct Notifier<T> {
    unique: AtomicU64,
    in_flights: Mutex<HashMap<u64, oneshot::Sender<(u64, T)>>>,
}

impl<T> Default for Notifier<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Notifier<T> {
    /// Create a new `Notifier`.
    pub fn new() -> Self {
        Self {
            unique: AtomicU64::new(0),
            in_flights: Mutex::new(HashMap::new()),
        }
    }

    /// Notify the cache invalidation about an inode to the kernel.
    pub async fn inval_inode<W: ?Sized>(
        &self,
        writer: &mut W,
        session: &Session,
        ino: u64,
        off: i64,
        len: i64,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        if session.exited() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "session is closed",
            ));
        }

        let out = fuse_notify_inval_inode_out {
            ino,
            off,
            len,
            ..Default::default()
        };
        send_notify(
            writer,
            fuse_notify_code::FUSE_NOTIFY_INVAL_INODE,
            &[unsafe { as_bytes(&out) }],
        )
        .await
    }

    /// Notify the invalidation about a directory entry to the kernel.
    pub async fn inval_entry<W: ?Sized>(
        &self,
        writer: &mut W,
        session: &Session,
        parent: u64,
        name: impl AsRef<OsStr>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        if session.exited() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "session is closed",
            ));
        }

        let name = name.as_ref();
        let namelen = u32::try_from(name.len()).unwrap();
        let out = fuse_notify_inval_entry_out {
            parent,
            namelen,
            ..Default::default()
        };
        send_notify(
            writer,
            fuse_notify_code::FUSE_NOTIFY_INVAL_ENTRY,
            &[unsafe { as_bytes(&out) }, name.as_bytes()],
        )
        .await
    }

    /// Notify the invalidation about a directory entry to the kernel.
    ///
    /// The role of this notification is similar to `notify_inval_entry`.
    /// Additionally, when the provided `child` inode matches the inode
    /// in the dentry cache, the inotify will inform the deletion to
    /// watchers if exists.
    pub async fn delete<W: ?Sized>(
        &self,
        writer: &mut W,
        session: &Session,
        parent: u64,
        child: u64,
        name: impl AsRef<OsStr>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        if session.exited() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "session is closed",
            ));
        }

        let name = name.as_ref();
        let namelen = u32::try_from(name.len()).unwrap();
        let out = fuse_notify_delete_out {
            parent,
            child,
            namelen,
            ..Default::default()
        };
        send_notify(
            writer,
            fuse_notify_code::FUSE_NOTIFY_DELETE,
            &[unsafe { as_bytes(&out) }, name.as_bytes()],
        )
        .await
    }

    /// Push the data in an inode for updating the kernel cache.
    pub async fn store<W: ?Sized>(
        &self,
        writer: &mut W,
        session: &Session,
        ino: u64,
        offset: u64,
        data: &[&[u8]],
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        if session.exited() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "session is closed",
            ));
        }

        let size = u32::try_from(data.iter().map(|t| t.len()).sum::<usize>()).unwrap();
        let out = fuse_notify_store_out {
            nodeid: ino,
            offset,
            size,
            ..Default::default()
        };
        let data: SmallVec<[_; 4]> = Some(unsafe { as_bytes(&out) })
            .into_iter()
            .chain(data.iter().copied())
            .collect();
        send_notify(writer, fuse_notify_code::FUSE_NOTIFY_STORE, &*data).await
    }

    /// Retrieve data in an inode from the kernel cache.
    pub async fn retrieve<W: ?Sized>(
        &self,
        writer: &mut W,
        session: &Session,
        ino: u64,
        offset: u64,
        size: u32,
    ) -> io::Result<RetrieveHandle<T>>
    where
        W: AsyncWrite + Unpin,
    {
        if session.exited() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "session is closed",
            ));
        }

        let unique = self.unique.fetch_add(1, Ordering::SeqCst);

        let (tx, rx) = oneshot::channel();
        self.in_flights.lock().await.insert(unique, tx);

        let out = fuse_notify_retrieve_out {
            notify_unique: unique,
            nodeid: ino,
            offset,
            size,
            ..Default::default()
        };
        send_notify(
            writer,
            fuse_notify_code::FUSE_NOTIFY_RETRIEVE,
            &[unsafe { as_bytes(&out) }],
        )
        .await?;

        Ok(RetrieveHandle {
            unique,
            rx: rx.fuse(),
        })
    }

    /// Send I/O readiness to the kernel.
    pub async fn poll_wakeup<W: ?Sized>(
        &self,
        writer: &mut W,
        session: &Session,
        kh: u64,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        if session.exited() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "session is closed",
            ));
        }

        let out = fuse_notify_poll_wakeup_out {
            kh,
            ..Default::default()
        };
        send_notify(
            writer,
            fuse_notify_code::FUSE_NOTIFY_POLL,
            &[unsafe { as_bytes(&out) }],
        )
        .await
    }

    pub(crate) async fn send_notify_reply(&self, unique: u64, offset: u64, data: T) {
        if let Some(tx) = self.in_flights.lock().await.remove(&unique) {
            let _ = tx.send((offset, data));
        }
    }
}

/// A handle for awaiting a result of `notify_retrieve`.
#[derive(Debug)]
pub struct RetrieveHandle<T> {
    unique: u64,
    rx: Fuse<oneshot::Receiver<(u64, T)>>,
}

impl<T> RetrieveHandle<T> {
    /// Return the unique ID of the notification.
    pub fn unique(&self) -> u64 {
        self.unique
    }
}

impl<T> Future for RetrieveHandle<T> {
    type Output = (u64, T);

    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context) -> Poll<Self::Output> {
        self.rx.poll_unpin(cx).map(|res| res.expect("canceled"))
    }
}

impl<T> FusedFuture for RetrieveHandle<T> {
    fn is_terminated(&self) -> bool {
        self.rx.is_terminated()
    }
}

#[inline]
async fn send_notify<W: ?Sized>(
    writer: &mut W,
    code: fuse_notify_code,
    data: &[&[u8]],
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let code = unsafe { mem::transmute::<_, i32>(code) };
    send_msg(writer, 0, code, data).await
}
