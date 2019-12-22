//! Lowlevel interface to handle FUSE requests.

#![allow(clippy::needless_update)]

use crate::{
    common::StatFs,
    fs::Filesystem,
    init::ConnectionInfo,
    io::{RequestReader, Writer, WriterExt},
    kernel::{
        fuse_notify_code, //
        fuse_notify_delete_out,
        fuse_notify_inval_entry_out,
        fuse_notify_inval_inode_out,
        fuse_notify_poll_wakeup_out,
        fuse_notify_retrieve_out,
        fuse_notify_store_out,
    },
    op::{Operation, OperationKind},
    reply::ReplyWriter,
    util::as_bytes,
};
use futures::io::AsyncRead;
use parking_lot::Mutex;
use smallvec::SmallVec;
use std::{
    collections::HashSet,
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
    retrieves: Mutex<HashSet<u64>>,
}

impl Session {
    pub(crate) fn new(conn: ConnectionInfo, bufsize: usize) -> Self {
        Self {
            conn,
            bufsize,
            exited: AtomicBool::new(false),
            notify_unique: AtomicU64::new(0),
            retrieves: Mutex::default(),
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
    pub async fn process<F: ?Sized, R: ?Sized, W: ?Sized>(
        &self,
        fs: &F,
        reader: &mut R,
        writer: &mut W,
    ) -> io::Result<()>
    where
        F: Filesystem,
        R: AsyncRead + Send + Unpin,
        W: Writer + Send + Unpin,
    {
        if self.exited() {
            tracing::warn!("The sesson has already been exited");
            return Ok(());
        }

        let request = reader.read_request().await?;
        let header = request.header();
        let arg = request.arg()?;

        tracing::debug!(
            "Handle a request: unique={}, opcode={:?}",
            header.unique(),
            header.opcode(),
        );

        let mut writer = ReplyWriter::new(header.unique(), &mut *writer);

        match arg {
            OperationKind::Destroy => {
                self.exit();
                return Ok(());
            }
            OperationKind::Operation(op) => match op {
                op @ Operation::Statfs(..) => {
                    fs.reply(op, &mut *reader, &mut writer).await?;
                    if !writer.replied() {
                        let mut st = StatFs::default();
                        st.set_namelen(255);
                        st.set_bsize(512);
                        let out = crate::reply::ReplyStatfs::new(st);
                        writer
                            .reply_raw(&[unsafe { crate::util::as_bytes(&out) }])
                            .await?;
                    }
                }
                op => {
                    fs.reply(op, &mut *reader, &mut writer).await?;
                    if !writer.replied() {
                        writer.reply_err(libc::ENOSYS).await?;
                    }
                }
            },
            OperationKind::Forget(forgets) => {
                // no reply.
                return fs.forget(forgets.as_ref()).await;
            }
            OperationKind::Interrupt(op) => {
                fs.interrupt(op, &mut writer).await?;
            }
            OperationKind::NotifyReply(op) => {
                if self.retrieves.lock().remove(&op.unique()) {
                    return fs.notify_reply(op, &mut *reader).await;
                }
            }

            OperationKind::Init { .. } => {
                tracing::warn!("ignore an INIT request after initializing the session");
                writer.reply_err(libc::EIO).await?;
            }

            OperationKind::Unknown => {
                tracing::warn!("unsupported opcode: {:?}", header.opcode());
                writer.reply_err(libc::ENOSYS).await?;
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
        self.retrieves.lock().insert(unique);

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
