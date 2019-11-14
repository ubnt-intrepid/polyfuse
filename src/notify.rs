#![allow(clippy::needless_update)]

use crate::{
    reply::{send_msg, Payload},
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

#[derive(Debug)]
#[allow(clippy::type_complexity)]
pub struct Notifier<T> {
    notify_unique: AtomicU64,
    notify_remains: Mutex<HashMap<u64, oneshot::Sender<(u64, T)>>>,
}

impl<T> Default for Notifier<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Notifier<T> {
    pub fn new() -> Self {
        Self {
            notify_unique: AtomicU64::new(0),
            notify_remains: Mutex::new(HashMap::new()),
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
            &[out.as_bytes()],
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
            &[out.as_bytes(), name.as_bytes()],
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
            &[out.as_bytes(), name.as_bytes()],
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
        let data: SmallVec<[_; 4]> = Some(out.as_bytes())
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

        let notify_unique = self.notify_unique.fetch_add(1, Ordering::SeqCst);

        let (tx, rx) = oneshot::channel();
        self.notify_remains.lock().await.insert(notify_unique, tx);

        let out = fuse_notify_retrieve_out {
            notify_unique,
            nodeid: ino,
            offset,
            size,
            ..Default::default()
        };
        send_notify(
            writer,
            fuse_notify_code::FUSE_NOTIFY_RETRIEVE,
            &[out.as_bytes()],
        )
        .await?;

        Ok(RetrieveHandle {
            unique: notify_unique,
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
            &[out.as_bytes()],
        )
        .await
    }

    pub(crate) async fn send_notify_reply(&self, unique: u64, offset: u64, data: T) {
        if let Some(tx) = self.notify_remains.lock().await.remove(&unique) {
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
