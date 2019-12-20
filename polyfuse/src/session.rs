//! Lowlevel interface to handle FUSE requests.

use crate::{
    common::{Forget, StatFs},
    fs::{Context, Filesystem},
    init::ConnectionInfo,
    io::{Buffer, Reader, ReaderExt, Writer},
    kernel::{fuse_forget_one, fuse_opcode},
    notify::Notifier,
    op::{self, Operation},
    reply::ReplyWriter,
    request::RequestKind,
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
            let kind = crate::request::Parser::new(kind).parse(header.opcode().unwrap())?;
            match kind {
                RequestKind::Interrupt { arg } => {
                    tracing::debug!("Receive INTERRUPT (unique = {:?})", arg.unique);
                    interrupts.push(Interrupt {
                        unique: arg.unique,
                        interrupt_unique: header.unique(),
                    });
                }
                RequestKind::NotifyReply { arg } => {
                    let unique = header.unique();
                    tracing::debug!("Receive NOTIFY_REPLY (notify_unique = {:?})", unique);
                    notifier
                        .send_notify_reply(unique, arg.offset, data.expect("empty data"))
                        .await;
                }
                _ => unreachable!(),
            }
        }
    }

    /// Process an incoming request using the specified filesystem operations.
    #[allow(clippy::cognitive_complexity)]
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
        let kind = crate::request::Parser::new(kind).parse(header.opcode().unwrap())?;

        let ino = header.nodeid();
        tracing::debug!(
            "Handle a request: unique={}, opcode={:?}",
            header.unique(),
            header.opcode(),
        );

        let mut cx = Context::new(&header);
        let mut writer = ReplyWriter::new(header.unique(), &mut *writer);

        macro_rules! do_reply {
            ($op:expr) => {
                do_reply!($op, {
                    writer.reply_err(libc::ENOSYS).await?;
                })
            };
            ($op:expr, $after:stmt) => {
                fs.reply(&mut cx, $op, &mut writer).await?;
                if !writer.replied() {
                    $after
                }
            };
        }

        match kind {
            RequestKind::Destroy => {
                self.exit();
                return Ok(());
            }
            RequestKind::Lookup { name } => {
                do_reply!(Operation::Lookup(op::Lookup { header, name }));
            }
            RequestKind::Forget { arg } => {
                // no reply.
                return fs.forget(&mut cx, &[Forget::new(ino, arg.nlookup)]).await;
            }
            RequestKind::BatchForget { forgets, .. } => {
                #[inline(always)]
                fn make_forgets(forgets: &[fuse_forget_one]) -> &[Forget] {
                    unsafe {
                        std::slice::from_raw_parts(
                            forgets.as_ptr() as *const Forget, //
                            forgets.len(),
                        )
                    }
                }

                // no reply.
                return fs.forget(&mut cx, make_forgets(&*forgets)).await;
            }
            RequestKind::Getattr { arg } => {
                do_reply!(Operation::Getattr(op::Getattr { header, arg }));
            }
            RequestKind::Setattr { arg } => {
                do_reply!(Operation::Setattr(op::Setattr { header, arg }));
            }
            RequestKind::Readlink => {
                do_reply!(Operation::Readlink(op::Readlink { header }));
            }
            RequestKind::Symlink { name, link } => {
                do_reply!(Operation::Symlink(op::Symlink { header, name, link }));
            }
            RequestKind::Mknod { arg, name } => {
                do_reply!(Operation::Mknod(op::Mknod { header, arg, name }));
            }
            RequestKind::Mkdir { arg, name } => {
                do_reply!(Operation::Mkdir(op::Mkdir { header, arg, name }));
            }
            RequestKind::Unlink { name } => {
                do_reply!(Operation::Unlink(op::Unlink { header, name }));
            }
            RequestKind::Rmdir { name } => {
                do_reply!(Operation::Rmdir(op::Rmdir { header, name }));
            }
            RequestKind::Rename { arg, name, newname } => {
                do_reply!(Operation::Rename(op::Rename {
                    header,
                    arg: arg.into(),
                    name,
                    newname,
                }));
            }
            RequestKind::Rename2 { arg, name, newname } => {
                do_reply!(Operation::Rename(op::Rename {
                    header,
                    arg: arg.into(),
                    name,
                    newname,
                }));
            }
            RequestKind::Link { arg, newname } => {
                do_reply!(Operation::Link(op::Link {
                    header,
                    arg,
                    newname,
                }));
            }
            RequestKind::Open { arg } => {
                do_reply!(Operation::Open(op::Open { header, arg }));
            }
            RequestKind::Read { arg } => {
                do_reply!(Operation::Read(op::Read { header, arg }));
            }
            RequestKind::Write { arg } => {
                do_reply!(Operation::Write(
                    op::Write { header, arg },
                    data.expect("empty remains")
                ));
            }
            RequestKind::Release { arg } => {
                do_reply!(Operation::Release(op::Release { header, arg }));
            }
            RequestKind::Statfs => {
                do_reply!(Operation::Statfs(op::Statfs { header }), {
                    let mut st = StatFs::default();
                    st.set_namelen(255);
                    st.set_bsize(512);
                    let out = crate::reply::ReplyStatfs::new(st);
                    writer
                        .reply_raw(&[unsafe { crate::util::as_bytes(&out) }])
                        .await?;
                });
            }
            RequestKind::Fsync { arg } => {
                do_reply!(Operation::Fsync(op::Fsync { header, arg }));
            }
            RequestKind::Setxattr { arg, name, value } => {
                do_reply!(Operation::Setxattr(op::Setxattr {
                    header,
                    arg,
                    name,
                    value,
                }));
            }
            RequestKind::Getxattr { arg, name } => {
                do_reply!(Operation::Getxattr(op::Getxattr { header, arg, name }));
            }
            RequestKind::Listxattr { arg } => {
                do_reply!(Operation::Listxattr(op::Listxattr { header, arg }));
            }
            RequestKind::Removexattr { name } => {
                do_reply!(Operation::Removexattr(op::Removexattr { header, name }));
            }
            RequestKind::Flush { arg } => {
                do_reply!(Operation::Flush(op::Flush { header, arg }));
            }
            RequestKind::Opendir { arg } => {
                do_reply!(Operation::Opendir(op::Opendir { header, arg }));
            }
            RequestKind::Readdir { arg } => {
                do_reply!(Operation::Readdir(op::Readdir {
                    header,
                    arg,
                    mode: op::ReaddirMode::Normal,
                }));
            }
            RequestKind::Readdirplus { arg } => {
                do_reply!(Operation::Readdir(op::Readdir {
                    header,
                    arg,
                    mode: op::ReaddirMode::Plus,
                }));
            }
            RequestKind::Releasedir { arg } => {
                do_reply!(Operation::Releasedir(op::Releasedir { header, arg }));
            }
            RequestKind::Fsyncdir { arg } => {
                do_reply!(Operation::Fsyncdir(op::Fsyncdir { header, arg }));
            }
            RequestKind::Getlk { arg } => {
                do_reply!(Operation::Getlk(op::Getlk { header, arg }));
            }
            RequestKind::Setlk { arg, sleep } => {
                if arg.lk_flags & crate::kernel::FUSE_LK_FLOCK != 0 {
                    do_reply!(Operation::Flock(op::Flock { header, arg, sleep }));
                } else {
                    do_reply!(Operation::Setlk(op::Setlk { header, arg, sleep }));
                }
            }
            RequestKind::Access { arg } => {
                do_reply!(Operation::Access(op::Access { header, arg }));
            }
            RequestKind::Create { arg, name } => {
                do_reply!(Operation::Create(op::Create { header, arg, name }));
            }
            RequestKind::Bmap { arg } => {
                do_reply!(Operation::Bmap(op::Bmap { header, arg }));
            }
            RequestKind::Fallocate { arg } => {
                do_reply!(Operation::Fallocate(op::Fallocate { header, arg }));
            }
            RequestKind::CopyFileRange { arg } => {
                do_reply!(Operation::CopyFileRange(op::CopyFileRange { header, arg }));
            }
            RequestKind::Poll { arg } => {
                do_reply!(Operation::Poll(op::Poll { header, arg }));
            }

            RequestKind::Init { .. } => {
                tracing::warn!("ignore an INIT request after initializing the session");
                writer.reply_err(libc::EIO).await?;
            }

            RequestKind::Interrupt { .. } | RequestKind::NotifyReply { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unexpected request kind",
                ));
            }

            RequestKind::Unknown => {
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
