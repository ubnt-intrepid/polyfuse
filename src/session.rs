//! Lowlevel interface to handle FUSE requests.

#![allow(clippy::needless_update)]

pub mod reply;

mod dirent;
mod fs;
mod request;

pub use dirent::{DirEntry, DirEntryType};
pub use fs::{FileAttr, FileLock, Filesystem, Forget, FsStatistics, Operation};
pub use request::Request;

use bytes::Bytes;
use futures::{
    channel::oneshot,
    future::{Fuse, FusedFuture, Future, FutureExt},
    io::{AsyncRead, AsyncWrite},
    lock::Mutex,
};
use polyfuse_sys::kernel::{
    fuse_forget_one, //
    fuse_in_header,
    fuse_init_out,
    fuse_notify_code,
    fuse_notify_delete_out,
    fuse_notify_inval_entry_out,
    fuse_notify_inval_inode_out,
    fuse_notify_poll_wakeup_out,
    fuse_notify_retrieve_out,
    fuse_notify_store_out,
};
use smallvec::SmallVec;
use std::{
    collections::{HashMap, HashSet},
    convert::TryFrom,
    ffi::OsStr,
    fmt, io, mem,
    os::unix::ffi::OsStrExt,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    task::{self, Poll},
};

use reply::{
    send_msg, //
    Payload,
    ReplyAttr,
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
};
use request::{receive_msg, RequestKind};

const MAX_WRITE_SIZE: u32 = 16 * 1024 * 1024;
const RECV_BUF_SIZE: usize = MAX_WRITE_SIZE as usize + 4096;

/// FUSE session driver.
#[derive(Debug)]
pub struct Session {
    proto_major: u32,
    proto_minor: u32,
    max_readahead: u32,
    bufsize: usize,
    exited: AtomicBool,
    interrupt_state: Mutex<InterruptState>,
    notify_unique: AtomicU64,
    notify_remains: Mutex<HashMap<u64, oneshot::Sender<(u64, Bytes)>>>,
}

#[derive(Debug)]
struct InterruptState {
    remains: HashMap<u64, oneshot::Sender<()>>,
    interrupted: HashSet<u64>,
}

impl Session {
    /// Start a new FUSE session.
    ///
    /// This function receives an INIT request from the kernel and replies
    /// after initializing the connection parameters.
    pub async fn start<I: ?Sized>(io: &mut I, initializer: SessionInitializer) -> io::Result<Self>
    where
        I: AsyncRead + AsyncWrite + Unpin,
    {
        drop(initializer);

        loop {
            let Request { header, kind, .. } = receive_msg(io, RECV_BUF_SIZE)
                .await? //
                .ok_or_else(|| {
                    log::warn!("the connection is closed");
                    io::Error::from_raw_os_error(libc::ENODEV)
                })?;

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
                bufsize: RECV_BUF_SIZE,
                exited: AtomicBool::new(false),
                interrupt_state: Mutex::new(InterruptState {
                    remains: HashMap::new(),
                    interrupted: HashSet::new(),
                }),
                notify_unique: AtomicU64::new(0),
                notify_remains: Mutex::new(HashMap::new()),
            });
        }
    }

    fn exit(&self) {
        self.exited.store(true, Ordering::SeqCst);
    }

    fn exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    /// Receive an request from the kernel.
    pub async fn receive<R: ?Sized>(&self, reader: &mut R) -> io::Result<Option<Request>>
    where
        R: AsyncRead + Unpin,
    {
        loop {
            let req = match receive_msg(reader, self.bufsize).await? {
                Some(req) => req,
                None => return Ok(None),
            };
            match req.kind {
                RequestKind::Interrupt { arg } => {
                    log::debug!("Receive INTERRUPT (unique = {:?})", arg.unique);
                    self.send_interrupt(arg.unique).await;
                    continue;
                }
                RequestKind::NotifyReply { arg, data } => {
                    let unique = req.header.unique;
                    log::debug!("Receive NOTIFY_REPLY (notify_unique = {:?})", unique);
                    self.send_notify_reply(unique, arg.offset, data).await;
                    continue;
                }
                _ => {
                    // check if the request is already interrupted by the kernel.
                    let mut state = self.interrupt_state.lock().await;
                    if state.interrupted.remove(&req.header.unique) {
                        log::debug!("The request was interrupted (unique={})", req.header.unique);
                        continue;
                    }
                    return Ok(Some(req));
                }
            }
        }
    }

    /// Process an incoming request using the specified filesystem operations.
    #[allow(clippy::cognitive_complexity)]
    pub async fn process<F: ?Sized, W: ?Sized>(
        &self,
        fs: &F,
        req: &Request,
        writer: &mut W,
    ) -> io::Result<()>
    where
        F: Filesystem,
        W: AsyncWrite + Send + Unpin,
    {
        let mut writer = writer;

        if self.exited() {
            log::warn!("The sesson has already been exited");
            return Ok(());
        }

        log::debug!(
            "Handle a request: unique={}, opcode={:?}",
            req.header.unique,
            req.opcode(),
        );
        let Request { header, kind, .. } = req;
        let ino = header.nodeid;

        let mut cx = Context {
            header: &header,
            writer: Some(&mut writer),
            session: &*self,
        };

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
                        forgets: make_forgets(&*forgets),
                    },
                )
                .await?;
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
                debug_assert_eq!(data.len(), arg.size as usize);
                run_op!(Operation::Write {
                    ino,
                    fh: arg.fh,
                    offset: arg.offset,
                    data: &*data,
                    flags: arg.flags,
                    lock_owner: arg.lock_owner(),
                    reply: ReplyWrite::new(),
                });
            }
            RequestKind::Release { arg } => {
                let mut flush = false;
                let mut flock_release = false;
                let mut lock_owner = None;
                if self.proto_minor >= 8 {
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
                    plus: *plus,
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
                        sleep: *sleep,
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
                log::warn!("ignore an INIT request after initializing the session");
                cx.reply_err(libc::EIO).await?;
            }

            RequestKind::Interrupt { .. } | RequestKind::NotifyReply { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unexpected request kind",
                ));
            }

            RequestKind::Unknown => {
                log::warn!("unsupported opcode: {:?}", header.opcode);
                cx.reply_err(libc::ENOSYS).await?;
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
        W: AsyncWrite + Unpin,
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
            &[out.as_bytes()],
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
        W: AsyncWrite + Unpin,
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
    pub async fn notify_delete<W: ?Sized>(
        &self,
        writer: &mut W,
        parent: u64,
        child: u64,
        name: impl AsRef<OsStr>,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
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
            &[out.as_bytes(), name.as_bytes()],
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
        W: AsyncWrite + Unpin,
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
        let data: SmallVec<[_; 4]> = Some(out.as_bytes())
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
    ) -> io::Result<NotifyRetrieve>
    where
        W: AsyncWrite + Unpin,
    {
        if self.exited() {
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

        Ok(NotifyRetrieve {
            unique: notify_unique,
            rx: rx.fuse(),
        })
    }

    /// Send I/O readiness to the kernel.
    pub async fn notify_poll_wakeup<W: ?Sized>(&self, writer: &mut W, kh: u64) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
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
            &[out.as_bytes()],
        )
        .await
    }

    async fn enable_interrupt(&self, unique: u64) -> Interrupt {
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
            log::debug!("Sent interrupt signal to unique={}", unique);
        }
    }

    async fn send_notify_reply(&self, unique: u64, offset: u64, data: Bytes) {
        if let Some(tx) = self.notify_remains.lock().await.remove(&unique) {
            let _ = tx.send((offset, data));
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

#[derive(Debug)]
pub struct NotifyRetrieve {
    unique: u64,
    rx: Fuse<oneshot::Receiver<(u64, Bytes)>>,
}

impl NotifyRetrieve {
    pub fn unique(&self) -> u64 {
        self.unique
    }
}

impl Future for NotifyRetrieve {
    type Output = (u64, Bytes);

    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context) -> Poll<Self::Output> {
        self.rx.poll_unpin(cx).map(|res| res.expect("canceled"))
    }
}

impl FusedFuture for NotifyRetrieve {
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
