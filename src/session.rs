//! Lowlevel interface to handle FUSE requests.

#![allow(clippy::needless_update)]

// FIXME: re-enable lints.
#![allow(clippy::cast_lossless, clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use crate::{
    fs::{FileLock, Filesystem, Forget, Operation},
    reply::{
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
    },
    request::{Request, RequestData, RequestKind},
};
use bitflags::bitflags;
use futures::{
    channel::oneshot,
    future::{Fuse, FusedFuture, Future, FutureExt},
    io::{AsyncRead, AsyncWrite},
    lock::Mutex,
};
use lazy_static::lazy_static;
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
    fuse_opcode,
    FUSE_KERNEL_MINOR_VERSION,
    FUSE_KERNEL_VERSION,
    FUSE_MAX_PAGES,
    FUSE_MIN_READ_BUFFER,
    FUSE_NO_OPENDIR_SUPPORT,
    FUSE_NO_OPEN_SUPPORT,
};
use smallvec::SmallVec;
use std::{
    cmp,
    collections::{HashMap, HashSet},
    convert::TryFrom,
    ffi::OsStr,
    fmt, io, mem,
    os::unix::ffi::OsStrExt,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    task::{self, Poll},
};

// The minimum supported ABI minor version by polyfuse.
const MINIMUM_SUPPORTED_MINOR_VERSION: u32 = 23;

const DEFAULT_MAX_WRITE: u32 = 16 * 1024 * 1024;
const MIN_MAX_WRITE: u32 = FUSE_MIN_READ_BUFFER - BUFFER_HEADER_SIZE as u32;

// copied from fuse_i.h
const MAX_MAX_PAGES: usize = 256;
//const DEFAULT_MAX_PAGES_PER_REQ: usize = 32;
const BUFFER_HEADER_SIZE: usize = 0x1000;

lazy_static! {
    static ref PAGE_SIZE: usize = { unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize } };
}

/// FUSE session driver.
#[derive(Debug)]
pub struct Session {
    conn: ConnectionInfo,
    bufsize: usize,
    exited: AtomicBool,
    interrupt_state: Mutex<InterruptState>,
    notify_unique: AtomicU64,
    notify_remains: Mutex<HashMap<u64, oneshot::Sender<(u64, RequestData)>>>,
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
    #[allow(clippy::cognitive_complexity)]
    pub async fn start<I: ?Sized>(io: &mut I, initializer: SessionInitializer) -> io::Result<Self>
    where
        I: AsyncRead + AsyncWrite + Unpin,
    {
        loop {
            let mut req = Request::new(BUFFER_HEADER_SIZE + *PAGE_SIZE * MAX_MAX_PAGES);
            req.receive(io).await?;

            let (header, kind, _) = req.extract()?;

            match kind {
                RequestKind::Init { arg: init_in } => {
                    let capable = CapabilityFlags::from_bits_truncate(init_in.flags);
                    log::debug!("INIT request:");
                    log::debug!("  proto = {}.{}:", init_in.major, init_in.minor);
                    log::debug!("  flags = 0x{:08x} ({:?})", init_in.flags, capable);
                    log::debug!("  max_readahead = 0x{:08X}", init_in.max_readahead);
                    log::debug!("  max_pages = {}", init_in.flags & FUSE_MAX_PAGES != 0);
                    log::debug!(
                        "  no_open_support = {}",
                        init_in.flags & FUSE_NO_OPEN_SUPPORT != 0
                    );
                    log::debug!(
                        "  no_opendir_support = {}",
                        init_in.flags & FUSE_NO_OPENDIR_SUPPORT != 0
                    );

                    let mut init_out = fuse_init_out::default();
                    init_out.major = FUSE_KERNEL_VERSION;
                    init_out.minor = FUSE_KERNEL_MINOR_VERSION;

                    if init_in.major > 7 {
                        log::debug!("wait for a second INIT request with an older version.");
                        send_msg(&mut *io, header.unique, 0, &[init_out.as_bytes()]).await?;
                        continue;
                    }

                    if init_in.major < 7 || init_in.minor < MINIMUM_SUPPORTED_MINOR_VERSION {
                        log::warn!(
                            "polyfuse supports only ABI 7.{} or later. {}.{} is not supported",
                            MINIMUM_SUPPORTED_MINOR_VERSION,
                            init_in.major,
                            init_in.minor
                        );
                        send_msg(&mut *io, header.unique, -libc::EPROTO, &[]).await?;
                        continue;
                    }

                    init_out.minor = cmp::min(init_out.minor, init_in.minor);

                    init_out.flags = (initializer.flags & capable).bits();
                    init_out.flags |= polyfuse_sys::kernel::FUSE_BIG_WRITES; // the flag was superseded by `max_write`.

                    init_out.max_readahead =
                        cmp::min(initializer.max_readahead, init_in.max_readahead);
                    init_out.max_write = initializer.max_write;
                    init_out.max_background = initializer.max_background;
                    init_out.congestion_threshold = initializer.congestion_threshold;
                    init_out.time_gran = initializer.time_gran;

                    if init_in.flags & FUSE_MAX_PAGES != 0 {
                        init_out.flags |= FUSE_MAX_PAGES;
                        init_out.max_pages = cmp::min(
                            (init_out.max_write - 1) / (*PAGE_SIZE as u32) + 1,
                            u16::max_value() as u32,
                        ) as u16;
                    }

                    debug_assert_eq!(init_out.major, FUSE_KERNEL_VERSION);
                    debug_assert!(init_out.minor >= MINIMUM_SUPPORTED_MINOR_VERSION);

                    log::debug!("Reply to INIT:");
                    log::debug!("  proto = {}.{}:", init_out.major, init_out.minor);
                    log::debug!(
                        "  flags = 0x{:08x} ({:?})",
                        init_out.flags,
                        CapabilityFlags::from_bits_truncate(init_out.flags)
                    );
                    log::debug!("  max_readahead = 0x{:08X}", init_out.max_readahead);
                    log::debug!("  max_write = 0x{:08X}", init_out.max_write);
                    log::debug!("  max_background = 0x{:04X}", init_out.max_background);
                    log::debug!(
                        "  congestion_threshold = 0x{:04X}",
                        init_out.congestion_threshold
                    );
                    log::debug!("  time_gran = {}", init_out.time_gran);
                    send_msg(&mut *io, header.unique, 0, &[init_out.as_bytes()]).await?;

                    let conn = ConnectionInfo(init_out);
                    let bufsize = BUFFER_HEADER_SIZE + conn.max_write() as usize;

                    return Ok(Self {
                        conn,
                        bufsize,
                        exited: AtomicBool::new(false),
                        interrupt_state: Mutex::new(InterruptState {
                            remains: HashMap::new(),
                            interrupted: HashSet::new(),
                        }),
                        notify_unique: AtomicU64::new(0),
                        notify_remains: Mutex::new(HashMap::new()),
                    });
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
        }
    }

    fn exit(&self) {
        self.exited.store(true, Ordering::SeqCst);
    }

    fn exited(&self) -> bool {
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
    pub async fn receive<R: ?Sized>(&self, reader: &mut R, req: &mut Request) -> io::Result<()>
    where
        R: AsyncRead + Unpin,
    {
        loop {
            req.receive(reader).await?;

            match req.opcode() {
                Some(fuse_opcode::FUSE_INTERRUPT) | Some(fuse_opcode::FUSE_NOTIFY_REPLY) => (),
                _ => {
                    // check if the request is already interrupted by the kernel.
                    let mut state = self.interrupt_state.lock().await;
                    if state.interrupted.remove(&req.unique()) {
                        log::debug!("The request was interrupted (unique={})", req.unique());
                        continue;
                    }

                    return Ok(());
                }
            }

            let (header, kind, data) = req.extract()?;
            match kind {
                RequestKind::Interrupt { arg } => {
                    log::debug!("Receive INTERRUPT (unique = {:?})", arg.unique);
                    self.send_interrupt(arg.unique).await;
                }
                RequestKind::NotifyReply { arg } => {
                    let unique = header.unique;
                    log::debug!("Receive NOTIFY_REPLY (notify_unique = {:?})", unique);
                    self.send_notify_reply(unique, arg.offset, data).await;
                }
                _ => unreachable!(),
            }
        }
    }

    /// Process an incoming request using the specified filesystem operations.
    #[allow(clippy::cognitive_complexity)]
    pub async fn process<F: ?Sized, W: ?Sized>(
        &self,
        fs: &F,
        req: &mut Request,
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
            req.unique(),
            req.opcode(),
        );

        let (header, kind, data) = req.extract()?;
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
            RequestKind::Write { arg } => {
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

    async fn send_notify_reply(&self, unique: u64, offset: u64, data: RequestData) {
        if let Some(tx) = self.notify_remains.lock().await.remove(&unique) {
            let _ = tx.send((offset, data));
        }
    }
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
    rx: Fuse<oneshot::Receiver<(u64, RequestData)>>,
}

impl NotifyRetrieve {
    pub fn unique(&self) -> u64 {
        self.unique
    }
}

impl Future for NotifyRetrieve {
    type Output = (u64, RequestData);

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

/// Session initializer.
#[derive(Debug)]
pub struct SessionInitializer {
    max_readahead: u32,
    flags: CapabilityFlags,
    max_background: u16,
    congestion_threshold: u16,
    max_write: u32,
    time_gran: u32,
    max_pages: u16,
}

impl Default for SessionInitializer {
    fn default() -> Self {
        Self {
            max_readahead: u32::max_value(),
            flags: CapabilityFlags::default(),
            max_background: 0,
            congestion_threshold: 0,
            max_write: DEFAULT_MAX_WRITE,
            time_gran: 1,
            max_pages: 0,
        }
    }
}

impl SessionInitializer {
    /// Return a reference to the capability flags.
    pub fn flags(&mut self) -> &mut CapabilityFlags {
        &mut self.flags
    }

    /// Set the maximum readahead.
    pub fn max_readahead(&mut self, value: u32) -> &mut Self {
        self.max_readahead = value;
        self
    }

    /// Set the maximum size of the write buffer.
    ///
    /// # Panic
    /// It causes an assertion panic if the setting value is
    /// less than the absolute minimum.
    pub fn max_write(&mut self, value: u32) -> &mut Self {
        assert!(
            value >= MIN_MAX_WRITE,
            "max_write must be greater or equal to {}",
            MIN_MAX_WRITE,
        );
        self.max_write = value;
        self
    }

    /// Return the maximum number of pending *background* requests.
    pub fn max_background(&mut self, max_background: u16) -> &mut Self {
        self.max_background = max_background;
        self
    }

    /// Set the threshold number of pending background requests
    /// that the kernel marks the filesystem as *congested*.
    ///
    /// If the setting value is 0, the value is automatically
    /// calculated by using max_background.
    ///
    /// # Panics
    /// It cause a panic if the setting value is greater than `max_background`.
    pub fn congestion_threshold(&mut self, mut threshold: u16) -> &mut Self {
        assert!(
            threshold <= self.max_background,
            "The congestion_threshold must be less or equal to max_background"
        );
        if threshold == 0 {
            threshold = self.max_background * 3 / 4;
            log::debug!("congestion_threshold <- {}", threshold);
        }
        self.congestion_threshold = threshold;
        self
    }

    /// Set the timestamp resolution supported by the filesystem.
    ///
    /// The setting value has the nanosecond unit and should be a power of 10.
    ///
    /// The default value is 1.
    pub fn time_gran(&mut self, time_gran: u32) -> &mut Self {
        self.time_gran = time_gran;
        self
    }
}

/// Information about the connection associated with a session.
#[derive(Debug)]
pub struct ConnectionInfo(fuse_init_out);

impl ConnectionInfo {
    /// Returns the major version of the protocol.
    pub fn proto_major(&self) -> u32 {
        self.0.major
    }

    /// Returns the minor version of the protocol.
    pub fn proto_minor(&self) -> u32 {
        self.0.major
    }

    /// Return a set of capability flags sent to the kernel driver.
    pub fn flags(&self) -> CapabilityFlags {
        CapabilityFlags::from_bits_truncate(self.0.flags)
    }

    /// Return whether the kernel supports for zero-message opens.
    ///
    /// When the returned value is `true`, the kernel treat an `ENOSYS`
    /// error for a `FUSE_OPEN` request as successful and does not send
    /// subsequent `open` requests.  Otherwise, the filesystem should
    /// implement the handler for `open` requests appropriately.
    pub fn no_open_support(&self) -> bool {
        self.0.flags & FUSE_NO_OPEN_SUPPORT != 0
    }

    /// Return whether the kernel supports for zero-message opendirs.
    ///
    /// See the documentation of `no_open_support` for details.
    pub fn no_opendir_support(&self) -> bool {
        self.0.flags & FUSE_NO_OPENDIR_SUPPORT != 0
    }

    /// Returns the maximum readahead.
    pub fn max_readahead(&self) -> u32 {
        self.0.max_readahead
    }

    /// Returns the maximum size of the write buffer.
    pub fn max_write(&self) -> u32 {
        self.0.max_write
    }

    pub fn max_background(&self) -> u16 {
        self.0.max_background
    }

    pub fn congestion_threshold(&self) -> u16 {
        self.0.congestion_threshold
    }

    pub fn time_gran(&self) -> u32 {
        self.0.time_gran
    }

    pub fn max_pages(&self) -> Option<u16> {
        if self.0.flags & FUSE_MAX_PAGES != 0 {
            Some(self.0.max_pages)
        } else {
            None
        }
    }
}

bitflags! {
    /// Capability flags to control the behavior of the kernel driver.
    #[repr(transparent)]
    pub struct CapabilityFlags: u32 {
        /// The filesystem supports asynchronous read requests.
        ///
        /// Enabled by default.
        const ASYNC_READ = polyfuse_sys::kernel::FUSE_ASYNC_READ;

        /// The filesystem supports the `O_TRUNC` open flag.
        ///
        /// Enabled by default.
        const ATOMIC_O_TRUNC = polyfuse_sys::kernel::FUSE_ATOMIC_O_TRUNC;

        /// The kernel check the validity of attributes on every read.
        ///
        /// Enabled by default.
        const AUTO_INVAL_DATA = polyfuse_sys::kernel::FUSE_AUTO_INVAL_DATA;

        /// The filesystem supports asynchronous direct I/O submission.
        ///
        /// Enabled by default.
        const ASYNC_DIO = polyfuse_sys::kernel::FUSE_ASYNC_DIO;

        /// The kernel supports parallel directory operations.
        ///
        /// Enabled by default.
        const PARALLEL_DIROPS = polyfuse_sys::kernel::FUSE_PARALLEL_DIROPS;

        /// The filesystem is responsible for unsetting setuid and setgid bits
        /// when a file is written, truncated, or its owner is changed.
        ///
        /// Enabled by default.
        const HANDLE_KILLPRIV = polyfuse_sys::kernel::FUSE_HANDLE_KILLPRIV;

        /// The filesystem supports the POSIX-style file lock.
        const POSIX_LOCKS = polyfuse_sys::kernel::FUSE_POSIX_LOCKS;

        /// The filesystem supports the `flock` handling.
        const FLOCK_LOCKS = polyfuse_sys::kernel::FUSE_FLOCK_LOCKS;

        /// The filesystem supports lookups of `"."` and `".."`.
        const EXPORT_SUPPORT = polyfuse_sys::kernel::FUSE_EXPORT_SUPPORT;

        /// The kernel should not apply the umask to the file mode on create
        /// operations.
        const DONT_MASK = polyfuse_sys::kernel::FUSE_DONT_MASK;

        /// The filesystem supports the `FUSE_READDIRPLUS` operation.
        const READDIRPLUS = polyfuse_sys::kernel::FUSE_DO_READDIRPLUS;

        /// The filesystem supports adaptive readdirplus.
        ///
        /// This flag has no effect when `READDIRPLUS` is not set.
        const READDIRPLUS_AUTO = polyfuse_sys::kernel::FUSE_READDIRPLUS_AUTO;

        /// The writeback caching should be enabled.
        const WRITEBACK_CACHE = polyfuse_sys::kernel::FUSE_WRITEBACK_CACHE;

        /// The filesystem supports POSIX access control lists.
        const POSIX_ACL = polyfuse_sys::kernel::FUSE_POSIX_ACL;

        // TODO: splice read/write
        // const SPLICE_WRITE = polyfuse_sys::kernel::FUSE_SPLICE_WRITE;
        // const SPLICE_MOVE = polyfuse_sys::kernel::FUSE_SPLICE_MOVE;
        // const SPLICE_READ = polyfuse_sys::kernel::FUSE_SPLICE_READ;

        // TODO: ioctl
        // const IOCTL_DIR = polyfuse_sys::kernel::FUSE_IOCTL_DIR;
    }
}

impl Default for CapabilityFlags {
    fn default() -> Self {
        // TODO: IOCTL_DIR
        Self::ASYNC_READ
            | Self::PARALLEL_DIROPS
            | Self::AUTO_INVAL_DATA
            | Self::HANDLE_KILLPRIV
            | Self::ASYNC_DIO
            | Self::ATOMIC_O_TRUNC
    }
}
