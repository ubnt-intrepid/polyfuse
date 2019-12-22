//! Lowlevel interface to handle FUSE requests.

#![allow(clippy::needless_update)]

use crate::{
    common::StatFs,
    context::Context,
    fs::Filesystem,
    init::ConnectionInfo,
    io::{Reader, ReaderExt, Writer, WriterExt},
    kernel::{
        fuse_notify_code, //
        fuse_notify_delete_out,
        fuse_notify_inval_entry_out,
        fuse_notify_inval_inode_out,
        fuse_notify_poll_wakeup_out,
        fuse_notify_retrieve_out,
        fuse_notify_store_out,
        fuse_opcode,
    },
    op::{Operation, OperationKind},
    util::as_bytes,
};
use smallvec::SmallVec;
use std::{
    convert::TryFrom,
    ffi::OsStr,
    io, mem,
    os::unix::ffi::OsStrExt,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

/// FUSE session driver.
#[derive(Debug)]
pub struct Session {
    conn: ConnectionInfo,
    bufsize: usize,
    exited: AtomicBool,
    notify_unique: AtomicU64,
}

impl Session {
    pub(crate) fn new(conn: ConnectionInfo, bufsize: usize) -> Self {
        Self {
            conn,
            bufsize,
            exited: AtomicBool::new(false),
            notify_unique: AtomicU64::new(0),
        }
    }

    fn exit(&self) {
        self.exited.store(true, Ordering::SeqCst);
    }

    pub(crate) fn exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    /// Returns the information about the FUSE connection.
    pub fn connection_info(&self) -> &ConnectionInfo {
        &self.conn
    }

    /// Returns the buffer size required to receive one request.
    pub fn buffer_size(&self) -> usize {
        self.bufsize
    }

    /// Process an incoming request using the specified filesystem operations.
    #[allow(clippy::cognitive_complexity)]
    pub async fn process<F: ?Sized, T: ?Sized>(&self, fs: &F, io: &mut T) -> io::Result<()>
    where
        F: Filesystem,
        T: Reader + Writer + Send + Unpin,
    {
        if self.exited() {
            tracing::warn!("The sesson has already been exited");
            return Ok(());
        }

        let request = io.read_request().await?;
        let header = request.header();
        let arg = request.arg()?;

        tracing::debug!(
            "Handle a request: unique={}, opcode={:?}",
            header.unique,
            fuse_opcode::try_from(header.opcode).ok(),
        );

        let mut cx = Context::new(header, &mut *io);

        match arg {
            OperationKind::Destroy => {
                self.exit();
                return Ok(());
            }
            OperationKind::Operation(op) => match op {
                op @ Operation::Forget(..) => {
                    cx.disable_writer();
                    fs.call(&mut cx, op).await?;
                }
                op @ Operation::Interrupt(..) => {
                    fs.call(&mut cx, op).await?;
                }
                op @ Operation::NotifyReply(..) => {
                    cx.disable_writer();
                    fs.call(&mut cx, op).await?;
                }
                op @ Operation::Statfs(..) => {
                    fs.call(&mut cx, op).await?;
                    if !cx.replied() {
                        let mut st = StatFs::default();
                        st.set_namelen(255);
                        st.set_bsize(512);
                        let out = crate::reply::ReplyStatfs::new(st);
                        cx.reply_raw(&[unsafe { crate::util::as_bytes(&out) }])
                            .await?;
                    }
                }
                op => {
                    fs.call(&mut cx, op).await?;
                    if !cx.replied() {
                        cx.reply_err(libc::ENOSYS).await?;
                    }
                }
            },

            OperationKind::Init { .. } => {
                tracing::warn!("ignore an INIT request after initializing the session");
                cx.reply_err(libc::EIO).await?;
            }
        }

        Ok(())
    }

    /// Notify the cache invalidation about an inode to the kernel.
    pub async fn notify_inval_inode<W: ?Sized>(
        &self,
        writer: &mut W,
        ino: u64,
        off: i64,
        len: i64,
    ) -> io::Result<()>
    where
        W: Writer + Unpin,
    {
        if self.exited() {
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
    pub async fn notify_inval_entry<W: ?Sized>(
        &self,
        writer: &mut W,
        parent: u64,
        name: impl AsRef<OsStr>,
    ) -> io::Result<()>
    where
        W: Writer + Unpin,
    {
        if self.exited() {
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
            &[unsafe { as_bytes(&out) }, name.as_bytes(), &[0]],
        )
        .await
    }

    /// Notify the invalidation about a directory entry to the kernel.
    ///
    /// The role of this notification is similar to `notify_inval_entry`.
    /// Additionally, when the provided `child` inode matches the inode
    /// in the dentry cache, the inotify will inform the deletion to
    /// watchers if exists.
    pub async fn notify_delete<W: ?Sized>(
        &self,
        writer: &mut W,
        parent: u64,
        child: u64,
        name: impl AsRef<OsStr>,
    ) -> io::Result<()>
    where
        W: Writer + Unpin,
    {
        if self.exited() {
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
            &[unsafe { as_bytes(&out) }, name.as_bytes(), &[0]],
        )
        .await
    }

    /// Push the data in an inode for updating the kernel cache.
    pub async fn notify_store<W: ?Sized>(
        &self,
        writer: &mut W,
        ino: u64,
        offset: u64,
        data: &[&[u8]],
    ) -> io::Result<()>
    where
        W: Writer + Unpin,
    {
        if self.exited() {
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
    pub async fn notify_retrieve<W: ?Sized>(
        &self,
        writer: &mut W,
        ino: u64,
        offset: u64,
        size: u32,
    ) -> io::Result<u64>
    where
        W: Writer + Unpin,
    {
        if self.exited() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "session is closed",
            ));
        }

        let unique = self.notify_unique.fetch_add(1, Ordering::SeqCst);
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

        Ok(unique)
    }

    /// Send I/O readiness to the kernel.
    pub async fn notify_poll_wakeup<W: ?Sized>(&self, writer: &mut W, kh: u64) -> io::Result<()>
    where
        W: Writer + Unpin,
    {
        if self.exited() {
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
}

#[inline]
async fn send_notify<W: ?Sized>(
    writer: &mut W,
    code: fuse_notify_code,
    data: &[&[u8]],
) -> io::Result<()>
where
    W: Writer + Unpin,
{
    let code = unsafe { mem::transmute::<_, i32>(code) };
    writer.send_msg(0, code, data).await
}
