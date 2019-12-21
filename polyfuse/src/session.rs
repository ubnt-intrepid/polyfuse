//! Lowlevel interface to handle FUSE requests.

use crate::{
    common::StatFs,
    fs::{Context, Filesystem},
    init::ConnectionInfo,
    io::{Buffer, Reader, ReaderExt, Writer},
    kernel::fuse_opcode,
    notify::Notifier,
    op::{Operation, OperationKind},
    reply::ReplyWriter,
};
use std::{
    io,
    sync::atomic::{AtomicBool, Ordering},
};

/// FUSE session driver.
#[derive(Debug)]
pub struct Session {
    conn: ConnectionInfo,
    bufsize: usize,
    exited: AtomicBool,
}

impl Session {
    pub(crate) fn new(conn: ConnectionInfo, bufsize: usize) -> Self {
        Self {
            conn,
            bufsize,
            exited: AtomicBool::new(false),
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

    /// Receive one or more requests from the kernel.
    pub async fn receive<R: ?Sized, B: ?Sized>(
        &self,
        reader: &mut R,
        buf: &mut B,
        notifier: &Notifier<B::Data>,
    ) -> io::Result<Vec<Interrupt>>
    where
        R: Reader<Buffer = B> + Unpin,
        B: Buffer + Unpin,
    {
        let mut interrupts = vec![];
        loop {
            reader.receive_msg(buf).await?;
            match buf.header().opcode() {
                Some(fuse_opcode::FUSE_INTERRUPT) | Some(fuse_opcode::FUSE_NOTIFY_REPLY) => (),
                _ => return Ok(interrupts),
            }

            let (header, kind, data) = buf.extract();
            let kind = OperationKind::parse(header, kind, data)?;
            match kind {
                OperationKind::Interrupt { arg } => {
                    tracing::debug!("Receive INTERRUPT (unique = {:?})", arg.unique);
                    interrupts.push(Interrupt {
                        unique: arg.unique,
                        interrupt_unique: header.unique(),
                    });
                }
                OperationKind::NotifyReply { arg, data } => {
                    let unique = header.unique();
                    tracing::debug!("Receive NOTIFY_REPLY (notify_unique = {:?})", unique);
                    notifier.send_notify_reply(unique, arg.offset, data).await;
                }
                _ => unreachable!(),
            }
        }
    }

    /// Process an incoming request using the specified filesystem operations.
    pub async fn process<F: ?Sized, W: ?Sized, B: ?Sized>(
        &self,
        fs: &F,
        buf: &mut B,
        writer: &mut W,
    ) -> io::Result<()>
    where
        F: Filesystem<B::Data>,
        W: Writer + Send + Unpin,
        B: Buffer + Unpin,
        B::Data: Send,
    {
        if self.exited() {
            tracing::warn!("The sesson has already been exited");
            return Ok(());
        }

        let (header, kind, data) = buf.extract();
        let kind = OperationKind::parse(header, kind, data)?;

        tracing::debug!(
            "Handle a request: unique={}, opcode={:?}",
            header.unique(),
            header.opcode(),
        );

        let mut cx = Context::new(&header);
        let mut writer = ReplyWriter::new(header.unique(), &mut *writer);

        match kind {
            OperationKind::Destroy => {
                self.exit();
                return Ok(());
            }
            OperationKind::Forget(forgets) => {
                // no reply.
                return fs.forget(&mut cx, forgets.as_ref()).await;
            }
            OperationKind::Operation(op) => match op {
                op @ Operation::Statfs(..) => {
                    fs.reply(&mut cx, op, &mut writer).await?;
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
                    fs.reply(&mut cx, op, &mut writer).await?;
                    if !writer.replied() {
                        writer.reply_err(libc::ENOSYS).await?;
                    }
                }
            },

            OperationKind::Init { .. } => {
                tracing::warn!("ignore an INIT request after initializing the session");
                writer.reply_err(libc::EIO).await?;
            }
            OperationKind::Interrupt { .. } | OperationKind::NotifyReply { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unexpected request kind",
                ));
            }

            OperationKind::Unknown => {
                tracing::warn!("unsupported opcode: {:?}", header.opcode());
                writer.reply_err(libc::ENOSYS).await?;
            }
        }

        Ok(())
    }
}
/// Information about an interrupt request.
#[derive(Debug, Copy, Clone)]
pub struct Interrupt {
    unique: u64,
    interrupt_unique: u64,
}

impl Interrupt {
    /// Return the unique ID of the interrupted request.
    ///
    /// The implementation of FUSE filesystem deamon library should
    /// handle this interrupt request by sending a reply with an EINTR error,
    /// and terminate the background task processing the corresponding request.
    #[inline]
    pub fn unique(&self) -> u64 {
        self.unique
    }

    /// Return the unique ID of the interrupt request itself.
    ///
    /// If the filesystem daemon is not ready to handle this interrupt
    /// request, sending a reply using this unique with EAGAIN error causes
    /// the kernel to requeue its interrupt request.
    #[inline]
    pub fn interrupt_unique(&self) -> u64 {
        self.interrupt_unique
    }
}
