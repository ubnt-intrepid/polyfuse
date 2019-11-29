//! Lowlevel interface to handle FUSE requests.

use crate::{
    common::{Forget, StatFs},
    fs::{Context, Filesystem},
    init::ConnectionInfo,
    kernel::{fuse_forget_one, fuse_opcode},
    notify::Notifier,
    op::{self, Operation},
    request::{Buffer, BufferExt, RequestKind},
};
use futures::{
    channel::oneshot,
    future::{Fuse, FusedFuture, Future, FutureExt},
    io::{AsyncRead, AsyncWrite},
    lock::Mutex,
};
use std::{
    collections::{HashMap, HashSet},
    io,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{self, Poll},
};

/// FUSE session driver.
#[derive(Debug)]
pub struct Session {
    conn: ConnectionInfo,
    bufsize: usize,
    exited: AtomicBool,
    interrupt_state: Mutex<InterruptState>,
}

#[derive(Debug)]
struct InterruptState {
    remains: HashMap<u64, oneshot::Sender<()>>,
    interrupted: HashSet<u64>,
}

impl Session {
    pub(crate) fn new(conn: ConnectionInfo, bufsize: usize) -> Self {
        Self {
            conn,
            bufsize,
            exited: AtomicBool::new(false),
            interrupt_state: Mutex::new(InterruptState {
                remains: HashMap::new(),
                interrupted: HashSet::new(),
            }),
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
    ) -> io::Result<()>
    where
        R: AsyncRead + Unpin,
        B: Buffer,
    {
        loop {
            buf.receive(reader, self.buffer_size()).await?;

            {
                let header = buf.header().unwrap();
                match header.opcode() {
                    Some(fuse_opcode::FUSE_INTERRUPT) | Some(fuse_opcode::FUSE_NOTIFY_REPLY) => (),
                    _ => {
                        // check if the request is already interrupted by the kernel.
                        let mut state = self.interrupt_state.lock().await;
                        if state.interrupted.remove(&header.unique()) {
                            tracing::debug!(
                                "The request was interrupted (unique={})",
                                header.unique()
                            );
                            continue;
                        }

                        return Ok(());
                    }
                }
            }

            let (header, kind) = buf.extract()?.into_inner();
            match kind {
                RequestKind::Interrupt { arg } => {
                    tracing::debug!("Receive INTERRUPT (unique = {:?})", arg.unique);
                    self.send_interrupt(arg.unique).await;
                }
                RequestKind::NotifyReply { arg, data } => {
                    let unique = header.unique();
                    tracing::debug!("Receive NOTIFY_REPLY (notify_unique = {:?})", unique);
                    notifier.send_notify_reply(unique, arg.offset, data).await;
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
        W: AsyncWrite + Send + Unpin,
        B: Buffer,
        B::Data: Send,
    {
        let mut writer = writer;

        if self.exited() {
            tracing::warn!("The sesson has already been exited");
            return Ok(());
        }

        let (header, kind) = buf.extract()?.into_inner();
        let ino = header.nodeid();
        tracing::debug!(
            "Handle a request: unique={}, opcode={:?}",
            header.unique(),
            header.opcode(),
        );

        let mut cx = Context::new(&header, &mut writer, &*self);

        macro_rules! run_op {
            ($op:expr) => {
                fs.call(&mut cx, $op).await?;
            };
        }

        match kind {
            RequestKind::Destroy => {
                self.exit();
                return Ok(());
            }
            RequestKind::Lookup { name } => {
                run_op!(Operation::Lookup(op::Lookup { header, name }));
            }
            RequestKind::Forget { arg } => {
                // no reply.
                return fs
                    .call(&mut cx, Operation::Forget(&[Forget::new(ino, arg.nlookup)]))
                    .await;
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
                return fs
                    .call(&mut cx, Operation::Forget(make_forgets(&*forgets)))
                    .await;
            }
            RequestKind::Getattr { arg } => {
                run_op!(Operation::Getattr(op::Getattr { header, arg }));
            }
            RequestKind::Setattr { arg } => {
                run_op!(Operation::Setattr(op::Setattr { header, arg }));
            }
            RequestKind::Readlink => {
                run_op!(Operation::Readlink(op::Readlink { header }));
            }
            RequestKind::Symlink { name, link } => {
                run_op!(Operation::Symlink(op::Symlink { header, name, link }));
            }
            RequestKind::Mknod { arg, name } => {
                run_op!(Operation::Mknod(op::Mknod { header, arg, name }));
            }
            RequestKind::Mkdir { arg, name } => {
                run_op!(Operation::Mkdir(op::Mkdir { header, arg, name }));
            }
            RequestKind::Unlink { name } => {
                run_op!(Operation::Unlink(op::Unlink { header, name }));
            }
            RequestKind::Rmdir { name } => {
                run_op!(Operation::Rmdir(op::Rmdir { header, name }));
            }
            RequestKind::Rename { arg, name, newname } => {
                run_op!(Operation::Rename(op::Rename {
                    header,
                    arg: arg.into(),
                    name,
                    newname
                }));
            }
            RequestKind::Rename2 { arg, name, newname } => {
                run_op!(Operation::Rename(op::Rename {
                    header,
                    arg: arg.into(),
                    name,
                    newname
                }));
            }
            RequestKind::Link { arg, newname } => {
                run_op!(Operation::Link(op::Link {
                    header,
                    arg,
                    newname
                }));
            }
            RequestKind::Open { arg } => {
                run_op!(Operation::Open(op::Open { header, arg }));
            }
            RequestKind::Read { arg } => {
                run_op!(Operation::Read(op::Read { header, arg }));
            }
            RequestKind::Write { arg, data } => {
                run_op!(Operation::Write(op::Write { header, arg }, data));
            }
            RequestKind::Release { arg } => {
                run_op!(Operation::Release(op::Release { header, arg }));
            }
            RequestKind::Statfs => {
                run_op!(Operation::Statfs(op::Statfs { header }));
            }
            RequestKind::Fsync { arg } => {
                run_op!(Operation::Fsync(op::Fsync { header, arg }));
            }
            RequestKind::Setxattr { arg, name, value } => {
                run_op!(Operation::Setxattr(op::Setxattr {
                    header,
                    arg,
                    name,
                    value
                }));
            }
            RequestKind::Getxattr { arg, name } => {
                run_op!(Operation::Getxattr(op::Getxattr { header, arg, name }));
            }
            RequestKind::Listxattr { arg } => {
                run_op!(Operation::Listxattr(op::Listxattr { header, arg }));
            }
            RequestKind::Removexattr { name } => {
                run_op!(Operation::Removexattr(op::Removexattr { header, name }));
            }
            RequestKind::Flush { arg } => {
                run_op!(Operation::Flush(op::Flush { header, arg }));
            }
            RequestKind::Opendir { arg } => {
                run_op!(Operation::Opendir(op::Opendir { header, arg }));
            }
            RequestKind::Readdir { arg } => {
                run_op!(Operation::Readdir(op::Readdir {
                    header,
                    arg,
                    mode: op::ReaddirMode::Normal
                }));
            }
            RequestKind::Readdirplus { arg } => {
                run_op!(Operation::Readdir(op::Readdir {
                    header,
                    arg,
                    mode: op::ReaddirMode::Plus
                }));
            }
            RequestKind::Releasedir { arg } => {
                run_op!(Operation::Releasedir(op::Releasedir { header, arg }));
            }
            RequestKind::Fsyncdir { arg } => {
                run_op!(Operation::Fsyncdir(op::Fsyncdir { header, arg }));
            }
            RequestKind::Getlk { arg } => {
                run_op!(Operation::Getlk(op::Getlk { header, arg }));
            }
            RequestKind::Setlk { arg, sleep } => {
                if arg.lk_flags & crate::kernel::FUSE_LK_FLOCK != 0 {
                    run_op!(Operation::Flock(op::Flock { header, arg, sleep }));
                } else {
                    run_op!(Operation::Setlk(op::Setlk { header, arg, sleep }));
                }
            }
            RequestKind::Access { arg } => {
                run_op!(Operation::Access(op::Access { header, arg }));
            }
            RequestKind::Create { arg, name } => {
                run_op!(Operation::Create(op::Create { header, arg, name }));
            }
            RequestKind::Bmap { arg } => {
                run_op!(Operation::Bmap(op::Bmap { header, arg }));
            }
            RequestKind::Fallocate { arg } => {
                run_op!(Operation::Fallocate(op::Fallocate { header, arg }));
            }
            RequestKind::CopyFileRange { arg } => {
                run_op!(Operation::CopyFileRange(op::CopyFileRange { header, arg }));
            }
            RequestKind::Poll { arg } => {
                run_op!(Operation::Poll(op::Poll { header, arg }));
            }

            RequestKind::Init { .. } => {
                tracing::warn!("ignore an INIT request after initializing the session");
                cx.reply_err(libc::EIO).await?;
            }

            RequestKind::Interrupt { .. } | RequestKind::NotifyReply { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unexpected request kind",
                ));
            }

            RequestKind::Unknown => {
                tracing::warn!("unsupported opcode: {:?}", header.opcode());
                cx.reply_err(libc::ENOSYS).await?;
            }
        }

        if !cx.is_replied() {
            match header.opcode() {
                Some(fuse_opcode::FUSE_STATFS) => {
                    let mut st = StatFs::default();
                    st.set_namelen(255);
                    st.set_bsize(512);
                    let out = crate::reply::ReplyStatfs::new(st);
                    cx.reply(unsafe { crate::reply::as_bytes(&out) }).await?;
                }
                _ => cx.reply_err(libc::ENOSYS).await?,
            }
        }

        Ok(())
    }

    pub(crate) async fn enable_interrupt(&self, unique: u64) -> Interrupt {
        let (tx, rx) = oneshot::channel();
        let mut state = self.interrupt_state.lock().await;
        state.remains.insert(unique, tx);
        Interrupt(rx.fuse())
    }

    async fn send_interrupt(&self, unique: u64) {
        let mut state = self.interrupt_state.lock().await;
        if let Some(tx) = state.remains.remove(&unique) {
            state.interrupted.insert(unique);
            let _ = tx.send(());
            tracing::debug!("Sent interrupt signal to unique={}", unique);
        }
    }
}

/// A future for awaiting an interrupt signal sent to a request.
#[derive(Debug)]
pub struct Interrupt(Fuse<oneshot::Receiver<()>>);

impl Future for Interrupt {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context) -> Poll<Self::Output> {
        let _res = futures::ready!(self.0.poll_unpin(cx));
        Poll::Ready(())
    }
}

impl FusedFuture for Interrupt {
    fn is_terminated(&self) -> bool {
        self.0.is_terminated()
    }
}
