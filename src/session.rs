//! Lowlevel interface to handle FUSE requests.

pub mod reply;

mod buf;
mod fs;
mod request;

pub use buf::Buffer;
pub use fs::{FileAttr, FileLock, Filesystem, Forget, FsStatistics, Operation};
pub use request::Request;

use futures::{
    channel::oneshot,
    future::{Fuse, FusedFuture, Future, FutureExt},
    io::{AsyncRead, AsyncWrite},
    lock::Mutex,
};
use polyfuse_sys::abi::{fuse_forget_one, fuse_in_header, fuse_init_out};
use std::{
    collections::{HashMap, HashSet},
    fmt, io,
    pin::Pin,
    task::{self, Poll},
};

use reply::{send_msg, Payload, ReplyData};
use request::RequestKind;

pub const MAX_WRITE_SIZE: u32 = 16 * 1024 * 1024;

/// FUSE session driver.
#[derive(Debug)]
pub struct Session {
    proto_major: u32,
    proto_minor: u32,
    max_readahead: u32,
    state: Mutex<SessionState>,
}

#[derive(Debug)]
struct SessionState {
    exited: bool,
    remains: HashMap<u64, oneshot::Sender<()>>,
    interrupted: HashSet<u64>,
}

impl Session {
    /// Start a new FUSE session.
    ///
    /// This function receives an INIT request from the kernel and replies
    /// after initializing the connection parameters.
    pub async fn start<I>(io: &mut I, initializer: SessionInitializer) -> io::Result<Self>
    where
        I: AsyncRead + AsyncWrite + Unpin,
    {
        drop(initializer);

        let mut buf = Buffer::default();

        loop {
            let terminated = buf.receive(io).await?;
            if terminated {
                log::warn!("the connection is closed");
                return Err(io::Error::from_raw_os_error(libc::ENODEV));
            }

            let (Request { header, kind, .. }, _data) = buf.decode()?;

            let (proto_major, proto_minor, max_readahead);
            match kind {
                RequestKind::Init { arg } => {
                    let mut init_out = fuse_init_out::default();

                    if arg.major > 7 {
                        log::debug!("wait for a second INIT request with a 7.X version.");
                        send_msg(&mut *io, header.unique, 0, &[init_out.as_bytes()]).await?;
                        continue;
                    }

                    if arg.major < 7 || (arg.major == 7 && arg.minor < 6) {
                        log::warn!("unsupported protocol version: {}.{}", arg.major, arg.minor);
                        send_msg(&mut *io, header.unique, -libc::EPROTO, &[]).await?;
                        return Err(io::Error::from_raw_os_error(libc::EPROTO));
                    }

                    // remember the kernel parameters.
                    proto_major = arg.major;
                    proto_minor = arg.minor;
                    max_readahead = arg.max_readahead;

                    // TODO: max_background, congestion_threshold, time_gran, max_pages
                    init_out.max_readahead = arg.max_readahead;
                    init_out.max_write = MAX_WRITE_SIZE;

                    send_msg(&mut *io, header.unique, 0, &[init_out.as_bytes()]).await?;
                }
                _ => {
                    log::warn!(
                        "ignoring an operation before init (opcode={:?})",
                        header.opcode
                    );
                    send_msg(&mut *io, header.unique, -libc::EIO, &[]).await?;
                    continue;
                }
            }

            return Ok(Session {
                proto_major,
                proto_minor,
                max_readahead,
                state: Mutex::new(SessionState {
                    exited: false,
                    remains: HashMap::new(),
                    interrupted: HashSet::new(),
                }),
            });
        }
    }

    /// Process an incoming request using the specified filesystem operations.
    #[allow(clippy::cognitive_complexity)]
    pub async fn process<F, T, W>(
        &self,
        fs: &F,
        req: Request<'_>,
        data: Option<T>,
        writer: &mut W,
    ) -> io::Result<()>
    where
        F: Filesystem<T>,
        T: Send,
        W: AsyncWrite + Send + Unpin,
    {
        let Request { header, kind, .. } = req;
        let ino = header.nodeid;

        {
            let mut state = self.state.lock().await;

            if state.exited {
                log::warn!("The sesson has already been exited");
                return Ok(());
            }

            if state.interrupted.remove(&header.unique) {
                log::debug!("The request was interrupted (unique={})", header.unique);
                return Ok(());
            }
        }

        let mut cx = Context {
            header,
            writer: Some(&mut *writer),
            session: &*self,
        };

        macro_rules! run_op {
            ($op:expr) => {
                fs.call(&mut cx, $op).await?;
            };
        }

        match kind {
            RequestKind::Init { .. } => {
                log::warn!("");
                cx.reply_err(libc::EIO).await?;
            }
            RequestKind::Destroy => {
                self.state.lock().await.exited = true;
                cx.reply(&[]).await?;
            }
            RequestKind::Interrupt { arg } => {
                self.interrupt(arg.unique).await;
            }
            RequestKind::Lookup { name } => {
                run_op!(Operation::Lookup {
                    parent: ino,
                    name,
                    reply: Default::default(),
                });
            }
            RequestKind::Forget { arg } => {
                // no reply.
                fs.call(
                    &mut cx,
                    Operation::Forget {
                        forgets: &[Forget::new(ino, arg.nlookup)],
                    },
                )
                .await?;
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
                fs.call(
                    &mut cx,
                    Operation::Forget {
                        forgets: make_forgets(forgets),
                    },
                )
                .await?;
            }
            RequestKind::Getattr { arg } => {
                run_op!(Operation::Getattr {
                    ino,
                    fh: arg.fh(),
                    reply: Default::default(),
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
                    reply: Default::default(),
                });
            }
            RequestKind::Readlink => {
                run_op!(Operation::Readlink {
                    ino,
                    reply: Default::default(),
                });
            }
            RequestKind::Symlink { name, link } => {
                run_op!(Operation::Symlink {
                    parent: ino,
                    name,
                    link,
                    reply: Default::default(),
                });
            }
            RequestKind::Mknod { arg, name } => {
                run_op!(Operation::Mknod {
                    parent: ino,
                    name,
                    mode: arg.mode,
                    rdev: arg.rdev,
                    umask: Some(arg.umask),
                    reply: Default::default(),
                });
            }
            RequestKind::Mkdir { arg, name } => {
                run_op!(Operation::Mkdir {
                    parent: ino,
                    name,
                    mode: arg.mode,
                    umask: Some(arg.umask),
                    reply: Default::default(),
                });
            }
            RequestKind::Unlink { name } => {
                run_op!(Operation::Unlink {
                    parent: ino,
                    name,
                    reply: Default::default(),
                });
            }
            RequestKind::Rmdir { name } => {
                run_op!(Operation::Rmdir {
                    parent: ino,
                    name,
                    reply: Default::default(),
                });
            }
            RequestKind::Rename { arg, name, newname } => {
                run_op!(Operation::Rename {
                    parent: ino,
                    name,
                    newparent: arg.newdir,
                    newname,
                    flags: 0,
                    reply: Default::default(),
                });
            }
            RequestKind::Rename2 { arg, name, newname } => {
                run_op!(Operation::Rename {
                    parent: ino,
                    name,
                    newparent: arg.newdir,
                    newname,
                    flags: arg.flags,
                    reply: Default::default(),
                });
            }
            RequestKind::Link { arg, newname } => {
                run_op!(Operation::Link {
                    ino: arg.oldnodeid,
                    newparent: ino,
                    newname,
                    reply: Default::default(),
                });
            }
            RequestKind::Open { arg } => {
                run_op!(Operation::Open {
                    ino,
                    flags: arg.flags,
                    reply: Default::default(),
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
            RequestKind::Write { arg } => match data {
                Some(data) => {
                    run_op!(Operation::Write {
                        ino,
                        fh: arg.fh,
                        offset: arg.offset,
                        data,
                        size: arg.size,
                        flags: arg.flags,
                        lock_owner: arg.lock_owner(),
                        reply: Default::default(),
                    });
                }
                None => panic!("unexpected condition"),
            },
            RequestKind::Release { arg } => {
                let mut flush = false;
                let mut flock_release = false;
                let mut lock_owner = None;
                if self.proto_minor >= 8 {
                    flush = arg.release_flags & polyfuse_sys::abi::FUSE_RELEASE_FLUSH != 0;
                    lock_owner.get_or_insert_with(|| arg.lock_owner);
                }
                if arg.release_flags & polyfuse_sys::abi::FUSE_RELEASE_FLOCK_UNLOCK != 0 {
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
                    reply: Default::default(),
                });
            }
            RequestKind::Statfs => {
                run_op!(Operation::Statfs {
                    ino,
                    reply: Default::default(),
                });
            }
            RequestKind::Fsync { arg } => {
                run_op!(Operation::Fsync {
                    ino,
                    fh: arg.fh,
                    datasync: arg.datasync(),
                    reply: Default::default(),
                });
            }
            RequestKind::Setxattr { arg, name, value } => {
                run_op!(Operation::Setxattr {
                    ino,
                    name,
                    value,
                    flags: arg.flags,
                    reply: Default::default(),
                });
            }
            RequestKind::Getxattr { arg, name } => {
                run_op!(Operation::Getxattr {
                    ino,
                    name,
                    size: arg.size,
                    reply: Default::default(),
                });
            }
            RequestKind::Listxattr { arg } => {
                run_op!(Operation::Listxattr {
                    ino,
                    size: arg.size,
                    reply: Default::default(),
                });
            }
            RequestKind::Removexattr { name } => {
                run_op!(Operation::Removexattr {
                    ino,
                    name,
                    reply: Default::default(),
                });
            }
            RequestKind::Flush { arg } => {
                run_op!(Operation::Flush {
                    ino,
                    fh: arg.fh,
                    lock_owner: arg.lock_owner,
                    reply: Default::default(),
                });
            }
            RequestKind::Opendir { arg } => {
                run_op!(Operation::Opendir {
                    ino,
                    flags: arg.flags,
                    reply: Default::default(),
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
                    reply: Default::default(),
                });
            }
            RequestKind::Fsyncdir { arg } => {
                run_op!(Operation::Fsyncdir {
                    ino,
                    fh: arg.fh,
                    datasync: arg.datasync(),
                    reply: Default::default(),
                });
            }
            RequestKind::Getlk { arg } => {
                run_op!(Operation::Getlk {
                    ino,
                    fh: arg.fh,
                    owner: arg.owner,
                    lk: FileLock::new(&arg.lk),
                    reply: Default::default(),
                });
            }
            RequestKind::Setlk { arg, sleep } => {
                if arg.lk_flags & polyfuse_sys::abi::FUSE_LK_FLOCK != 0 {
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
                        reply: Default::default(),
                    });
                } else {
                    run_op!(Operation::Setlk {
                        ino,
                        fh: arg.fh,
                        owner: arg.owner,
                        lk: FileLock::new(&arg.lk),
                        sleep,
                        reply: Default::default(),
                    });
                }
            }
            RequestKind::Access { arg } => {
                run_op!(Operation::Access {
                    ino,
                    mask: arg.mask,
                    reply: Default::default(),
                });
            }
            RequestKind::Create { arg, name } => {
                run_op!(Operation::Create {
                    parent: ino,
                    name,
                    mode: arg.mode,
                    umask: Some(arg.umask),
                    open_flags: arg.flags,
                    reply: Default::default(),
                });
            }
            RequestKind::Bmap { arg } => {
                run_op!(Operation::Bmap {
                    ino,
                    block: arg.block,
                    blocksize: arg.blocksize,
                    reply: Default::default(),
                });
            }
            RequestKind::Fallocate { arg } => {
                run_op!(Operation::Fallocate {
                    ino,
                    fh: arg.fh,
                    offset: arg.offset,
                    length: arg.length,
                    mode: arg.mode,
                    reply: Default::default(),
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
                    reply: Default::default(),
                });
            }

            RequestKind::Unknown => {
                log::warn!("unsupported opcode: {:?}", header.opcode);
                cx.reply_err(libc::ENOSYS).await?;
            }
        }

        Ok(())
    }

    async fn enable_interrupt(&self, unique: u64) -> Interrupt {
        let mut state = self.state.lock().await;
        let (tx, rx) = oneshot::channel();
        state.remains.insert(unique, tx);
        Interrupt(rx.fuse())
    }

    async fn interrupt(&self, unique: u64) {
        log::debug!("INTERRUPT (unique = {:?})", unique);
        let mut state = self.state.lock().await;
        if let Some(tx) = state.remains.remove(&unique) {
            state.interrupted.insert(unique);
            let _ = tx.send(());
            log::debug!("Sent interrupt signal to unique={}", unique);
        }
    }
}

/// Session initializer.
#[derive(Debug, Default)]
pub struct SessionInitializer {
    _p: (),
}

/// Contextural information about an incoming request.
pub struct Context<'a> {
    header: &'a fuse_in_header,
    writer: Option<&'a mut (dyn AsyncWrite + Send + Unpin)>,
    session: &'a Session,
}

impl fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Context").finish()
    }
}

impl<'a> Context<'a> {
    /// Return the user ID of the calling process.
    pub fn uid(&self) -> u32 {
        self.header.uid
    }

    /// Return the group ID of the calling process.
    pub fn gid(&self) -> u32 {
        self.header.gid
    }

    /// Return the process ID of the calling process.
    pub fn pid(&self) -> u32 {
        self.header.pid
    }

    #[inline]
    pub(crate) async fn reply(&mut self, data: &[u8]) -> io::Result<()> {
        self.reply_vectored(&[data]).await
    }

    #[inline]
    pub(crate) async fn reply_vectored(&mut self, data: &[&[u8]]) -> io::Result<()> {
        if let Some(ref mut writer) = self.writer {
            send_msg(writer, self.header.unique, 0, data).await?;
        }
        Ok(())
    }

    /// Reply to the kernel with an error code.
    pub async fn reply_err(&mut self, error: i32) -> io::Result<()> {
        if let Some(ref mut writer) = self.writer {
            send_msg(writer, self.header.unique, -error, &[]).await?;
        }
        Ok(())
    }

    /// Register the request with the sesssion and get a signal
    /// that will be notified when the request is canceld by the kernel.
    pub async fn on_interrupt(&mut self) -> Interrupt {
        self.session.enable_interrupt(self.header.unique).await
    }
}

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
