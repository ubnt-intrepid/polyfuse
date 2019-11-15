//! Lowlevel interface to handle FUSE requests.

use crate::{
    common::{FileLock, Forget},
    fs::{Context, Filesystem, Operation},
    init::ConnectionInfo,
    notify::Notifier,
    reply::{
        ReplyAttr, //
        ReplyBmap,
        ReplyCreate,
        ReplyData,
        ReplyEmpty,
        ReplyEntry,
        ReplyLk,
        ReplyOpen,
        ReplyOpendir,
        ReplyPoll,
        ReplyReadlink,
        ReplyStatfs,
        ReplyWrite,
        ReplyXattr,
    },
    request::{Buffer, BufferExt, RequestKind},
};
use futures::{
    channel::oneshot,
    future::{Fuse, FusedFuture, Future, FutureExt},
    io::{AsyncRead, AsyncWrite},
    lock::Mutex,
};
use polyfuse_sys::kernel::{fuse_forget_one, fuse_opcode};
use std::{
    collections::{HashMap, HashSet},
    convert::TryFrom,
    io, mem,
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
                run_op!(Operation::Lookup {
                    parent: ino,
                    name: &*name,
                    reply: ReplyEntry::new(),
                });
            }
            RequestKind::Forget { arg } => {
                // no reply.
                return fs
                    .call(
                        &mut cx,
                        Operation::Forget {
                            forgets: &[Forget::new(ino, arg.nlookup)],
                        },
                    )
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
                    .call(
                        &mut cx,
                        Operation::Forget {
                            forgets: make_forgets(&*forgets),
                        },
                    )
                    .await;
            }
            RequestKind::Getattr { arg } => {
                run_op!(Operation::Getattr {
                    ino,
                    fh: arg.fh(),
                    reply: ReplyAttr::new(),
                });
            }
            RequestKind::Setattr { arg } => {
                run_op!(Operation::Setattr {
                    ino,
                    fh: arg.fh(),
                    mode: arg.mode(),
                    uid: arg.uid(),
                    gid: arg.gid(),
                    size: arg.size(),
                    atime: arg.atime(),
                    mtime: arg.mtime(),
                    ctime: arg.ctime(),
                    lock_owner: arg.lock_owner(),
                    reply: ReplyAttr::new(),
                });
            }
            RequestKind::Readlink => {
                run_op!(Operation::Readlink {
                    ino,
                    reply: ReplyReadlink::new(),
                });
            }
            RequestKind::Symlink { name, link } => {
                run_op!(Operation::Symlink {
                    parent: ino,
                    name: &*name,
                    link: &*link,
                    reply: ReplyEntry::new(),
                });
            }
            RequestKind::Mknod { arg, name } => {
                run_op!(Operation::Mknod {
                    parent: ino,
                    name: &*name,
                    mode: arg.mode,
                    rdev: arg.rdev,
                    umask: Some(arg.umask),
                    reply: ReplyEntry::new(),
                });
            }
            RequestKind::Mkdir { arg, name } => {
                run_op!(Operation::Mkdir {
                    parent: ino,
                    name: &*name,
                    mode: arg.mode,
                    umask: Some(arg.umask),
                    reply: ReplyEntry::new(),
                });
            }
            RequestKind::Unlink { name } => {
                run_op!(Operation::Unlink {
                    parent: ino,
                    name: &*name,
                    reply: ReplyEmpty::new(),
                });
            }
            RequestKind::Rmdir { name } => {
                run_op!(Operation::Rmdir {
                    parent: ino,
                    name: &*name,
                    reply: ReplyEmpty::new(),
                });
            }
            RequestKind::Rename { arg, name, newname } => {
                run_op!(Operation::Rename {
                    parent: ino,
                    name: &*name,
                    newparent: arg.newdir,
                    newname: &*newname,
                    flags: 0,
                    reply: ReplyEmpty::new(),
                });
            }
            RequestKind::Rename2 { arg, name, newname } => {
                run_op!(Operation::Rename {
                    parent: ino,
                    name: &*name,
                    newparent: arg.newdir,
                    newname: &*newname,
                    flags: arg.flags,
                    reply: ReplyEmpty::new(),
                });
            }
            RequestKind::Link { arg, newname } => {
                run_op!(Operation::Link {
                    ino: arg.oldnodeid,
                    newparent: ino,
                    newname: &*newname,
                    reply: ReplyEntry::new(),
                });
            }
            RequestKind::Open { arg } => {
                run_op!(Operation::Open {
                    ino,
                    flags: arg.flags,
                    reply: ReplyOpen::new(),
                });
            }
            RequestKind::Read { arg } => {
                run_op!(Operation::Read {
                    ino,
                    fh: arg.fh,
                    offset: arg.offset,
                    flags: arg.flags,
                    lock_owner: arg.lock_owner(),
                    reply: ReplyData::new(arg.size),
                });
            }
            RequestKind::Write { arg, data } => {
                // debug_assert_eq!(data.len(), arg.size as usize);

                run_op!(Operation::Write {
                    ino,
                    fh: arg.fh,
                    offset: arg.offset,
                    data,
                    flags: arg.flags,
                    lock_owner: arg.lock_owner(),
                    reply: ReplyWrite::new(),
                });
            }
            RequestKind::Release { arg } => {
                let mut flush = false;
                let mut flock_release = false;
                let mut lock_owner = None;
                if self.conn.proto_minor() >= 8 {
                    flush = arg.release_flags & polyfuse_sys::kernel::FUSE_RELEASE_FLUSH != 0;
                    lock_owner.get_or_insert_with(|| arg.lock_owner);
                }
                if arg.release_flags & polyfuse_sys::kernel::FUSE_RELEASE_FLOCK_UNLOCK != 0 {
                    flock_release = true;
                    lock_owner.get_or_insert_with(|| arg.lock_owner);
                }
                run_op!(Operation::Release {
                    ino,
                    fh: arg.fh,
                    flags: arg.flags,
                    lock_owner,
                    flush,
                    flock_release,
                    reply: ReplyEmpty::new(),
                });
            }
            RequestKind::Statfs => {
                run_op!(Operation::Statfs {
                    ino,
                    reply: ReplyStatfs::new(),
                });

                if !cx.is_replied() {
                    let mut st: libc::statvfs = unsafe { mem::zeroed() };
                    st.f_namemax = 255;
                    st.f_bsize = 512;
                    let st = TryFrom::try_from(st).unwrap();
                    ReplyStatfs::new().stat(&mut cx, st).await?;
                }
            }
            RequestKind::Fsync { arg } => {
                run_op!(Operation::Fsync {
                    ino,
                    fh: arg.fh,
                    datasync: arg.datasync(),
                    reply: ReplyEmpty::new(),
                });
            }
            RequestKind::Setxattr { arg, name, value } => {
                run_op!(Operation::Setxattr {
                    ino,
                    name: &*name,
                    value: &*value,
                    flags: arg.flags,
                    reply: ReplyEmpty::new(),
                });
            }
            RequestKind::Getxattr { arg, name } => {
                run_op!(Operation::Getxattr {
                    ino,
                    name: &*name,
                    size: arg.size,
                    reply: ReplyXattr::new(),
                });
            }
            RequestKind::Listxattr { arg } => {
                run_op!(Operation::Listxattr {
                    ino,
                    size: arg.size,
                    reply: ReplyXattr::new(),
                });
            }
            RequestKind::Removexattr { name } => {
                run_op!(Operation::Removexattr {
                    ino,
                    name: &*name,
                    reply: ReplyEmpty::new(),
                });
            }
            RequestKind::Flush { arg } => {
                run_op!(Operation::Flush {
                    ino,
                    fh: arg.fh,
                    lock_owner: arg.lock_owner,
                    reply: ReplyEmpty::new(),
                });
            }
            RequestKind::Opendir { arg } => {
                run_op!(Operation::Opendir {
                    ino,
                    flags: arg.flags,
                    reply: ReplyOpendir::new(),
                });
            }
            RequestKind::Readdir { arg, plus } => {
                run_op!(Operation::Readdir {
                    ino,
                    fh: arg.fh,
                    offset: arg.offset,
                    plus,
                    reply: ReplyData::new(arg.size),
                });
            }
            RequestKind::Releasedir { arg } => {
                run_op!(Operation::Releasedir {
                    ino,
                    fh: arg.fh,
                    flags: arg.flags,
                    reply: ReplyEmpty::new(),
                });
            }
            RequestKind::Fsyncdir { arg } => {
                run_op!(Operation::Fsyncdir {
                    ino,
                    fh: arg.fh,
                    datasync: arg.datasync(),
                    reply: ReplyEmpty::new(),
                });
            }
            RequestKind::Getlk { arg } => {
                run_op!(Operation::Getlk {
                    ino,
                    fh: arg.fh,
                    owner: arg.owner,
                    lk: FileLock::new(&arg.lk),
                    reply: ReplyLk::new(),
                });
            }
            RequestKind::Setlk { arg, sleep } => {
                if arg.lk_flags & polyfuse_sys::kernel::FUSE_LK_FLOCK != 0 {
                    const F_RDLCK: u32 = libc::F_RDLCK as u32;
                    const F_WRLCK: u32 = libc::F_WRLCK as u32;
                    const F_UNLCK: u32 = libc::F_UNLCK as u32;
                    #[allow(clippy::cast_possible_wrap)]
                    let mut op = match arg.lk.typ {
                        F_RDLCK => libc::LOCK_SH as u32,
                        F_WRLCK => libc::LOCK_EX as u32,
                        F_UNLCK => libc::LOCK_UN as u32,
                        _ => return cx.reply_err(libc::EIO).await,
                    };
                    if !sleep {
                        op |= libc::LOCK_NB as u32;
                    }
                    run_op!(Operation::Flock {
                        ino,
                        fh: arg.fh,
                        owner: arg.owner,
                        op,
                        reply: ReplyEmpty::new(),
                    });
                } else {
                    run_op!(Operation::Setlk {
                        ino,
                        fh: arg.fh,
                        owner: arg.owner,
                        lk: FileLock::new(&arg.lk),
                        sleep,
                        reply: ReplyEmpty::new(),
                    });
                }
            }
            RequestKind::Access { arg } => {
                run_op!(Operation::Access {
                    ino,
                    mask: arg.mask,
                    reply: ReplyEmpty::new(),
                });
            }
            RequestKind::Create { arg, name } => {
                run_op!(Operation::Create {
                    parent: ino,
                    name: &*name,
                    mode: arg.mode,
                    umask: Some(arg.umask),
                    open_flags: arg.flags,
                    reply: ReplyCreate::new(),
                });
            }
            RequestKind::Bmap { arg } => {
                run_op!(Operation::Bmap {
                    ino,
                    block: arg.block,
                    blocksize: arg.blocksize,
                    reply: ReplyBmap::new(),
                });
            }
            RequestKind::Fallocate { arg } => {
                run_op!(Operation::Fallocate {
                    ino,
                    fh: arg.fh,
                    offset: arg.offset,
                    length: arg.length,
                    mode: arg.mode,
                    reply: ReplyEmpty::new(),
                });
            }
            RequestKind::CopyFileRange { arg } => {
                run_op!(Operation::CopyFileRange {
                    ino_in: ino,
                    fh_in: arg.fh_in,
                    off_in: arg.off_in,
                    ino_out: arg.nodeid_out,
                    fh_out: arg.fh_out,
                    off_out: arg.off_out,
                    len: arg.len,
                    flags: arg.flags,
                    reply: ReplyWrite::new(),
                });
            }
            RequestKind::Poll { arg } => {
                run_op!(Operation::Poll {
                    ino,
                    fh: arg.fh,
                    events: arg.events,
                    kh: if arg.flags & polyfuse_sys::kernel::FUSE_POLL_SCHEDULE_NOTIFY != 0 {
                        Some(arg.kh)
                    } else {
                        None
                    },
                    reply: ReplyPoll::new(),
                });
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
            cx.reply_err(libc::ENOSYS).await?;
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
