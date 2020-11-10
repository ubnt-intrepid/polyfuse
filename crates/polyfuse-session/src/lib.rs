//! Lowlevel interface to handle FUSE requests.

mod util;

use crate::util::{as_bytes, BuilderExt};
use bitflags::bitflags;
use futures::{
    future::Future,
    io::{AsyncRead, AsyncWrite},
    task::{self, Poll},
};
use lazy_static::lazy_static;
use polyfuse::{
    async_trait,
    op::{self, Operation},
    types::{FileLock, LockOwner},
    Filesystem,
};
use polyfuse_kernel::{
    self as kernel, //
    fuse_in_header,
    fuse_init_out,
    fuse_notify_code,
    fuse_notify_delete_out,
    fuse_notify_inval_entry_out,
    fuse_notify_inval_inode_out,
    fuse_notify_poll_wakeup_out,
    fuse_notify_retrieve_in,
    fuse_notify_retrieve_out,
    fuse_notify_store_out,
    fuse_opcode,
    fuse_out_header,
    fuse_write_in,
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
    convert::TryFrom,
    ffi::{OsStr, OsString},
    fmt,
    io::{self, IoSlice},
    mem,
    os::unix::ffi::OsStrExt,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
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
    static ref PAGE_SIZE: usize = unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize };
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
            tracing::debug!(congestion_threshold = threshold);
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

    #[doc(hidden)] // TODO: dox
    pub fn init_buf_size(&self) -> usize {
        BUFFER_HEADER_SIZE + *PAGE_SIZE * MAX_MAX_PAGES
    }

    /// Handle a `FUSE_INIT` request and create a new `Session`.
    #[allow(clippy::cognitive_complexity)]
    pub async fn try_init<I: ?Sized>(&self, io: &mut I) -> io::Result<Option<Session>>
    where
        I: AsyncRead + AsyncWrite + Unpin,
    {
        let request = io.read_request().await?;
        let header = &request.header;
        let op = Parser::new(header, &request.arg).parse()?;

        match op {
            OperationKind::Init { arg: init_in } => {
                let capable = CapabilityFlags::from_bits_truncate(init_in.flags);
                let readonly_flags = init_in.flags & !CapabilityFlags::all().bits();
                tracing::debug!("INIT request:");
                tracing::debug!("  proto = {}.{}:", init_in.major, init_in.minor);
                tracing::debug!("  flags = 0x{:08x} ({:?})", init_in.flags, capable);
                tracing::debug!("  max_readahead = 0x{:08X}", init_in.max_readahead);
                tracing::debug!("  max_pages = {}", init_in.flags & FUSE_MAX_PAGES != 0);
                tracing::debug!(
                    "  no_open_support = {}",
                    init_in.flags & FUSE_NO_OPEN_SUPPORT != 0
                );
                tracing::debug!(
                    "  no_opendir_support = {}",
                    init_in.flags & FUSE_NO_OPENDIR_SUPPORT != 0
                );

                let mut init_out = fuse_init_out::default();
                init_out.major = FUSE_KERNEL_VERSION;
                init_out.minor = FUSE_KERNEL_MINOR_VERSION;

                if init_in.major > 7 {
                    tracing::debug!("wait for a second INIT request with an older version.");
                    io.send_msg(header.unique, 0, unsafe { as_bytes(&init_out) })
                        .await?;
                    return Ok(None);
                }

                if init_in.major < 7 || init_in.minor < MINIMUM_SUPPORTED_MINOR_VERSION {
                    tracing::warn!(
                        "polyfuse supports only ABI 7.{} or later. {}.{} is not supported",
                        MINIMUM_SUPPORTED_MINOR_VERSION,
                        init_in.major,
                        init_in.minor
                    );
                    io.send_msg(header.unique, -libc::EPROTO, &()).await?;
                    return Ok(None);
                }

                init_out.minor = cmp::min(init_out.minor, init_in.minor);

                init_out.flags = (self.flags & capable).bits();
                init_out.flags |= kernel::FUSE_BIG_WRITES; // the flag was superseded by `max_write`.

                init_out.max_readahead = cmp::min(self.max_readahead, init_in.max_readahead);
                init_out.max_write = self.max_write;
                init_out.max_background = self.max_background;
                init_out.congestion_threshold = self.congestion_threshold;
                init_out.time_gran = self.time_gran;

                if init_in.flags & FUSE_MAX_PAGES != 0 {
                    init_out.flags |= FUSE_MAX_PAGES;
                    init_out.max_pages = cmp::min(
                        (init_out.max_write - 1) / (*PAGE_SIZE as u32) + 1,
                        u16::max_value() as u32,
                    ) as u16;
                }

                debug_assert_eq!(init_out.major, FUSE_KERNEL_VERSION);
                debug_assert!(init_out.minor >= MINIMUM_SUPPORTED_MINOR_VERSION);

                tracing::debug!("Reply to INIT:");
                tracing::debug!("  proto = {}.{}:", init_out.major, init_out.minor);
                tracing::debug!(
                    "  flags = 0x{:08x} ({:?})",
                    init_out.flags,
                    CapabilityFlags::from_bits_truncate(init_out.flags)
                );
                tracing::debug!("  max_readahead = 0x{:08X}", init_out.max_readahead);
                tracing::debug!("  max_write = 0x{:08X}", init_out.max_write);
                tracing::debug!("  max_background = 0x{:04X}", init_out.max_background);
                tracing::debug!(
                    "  congestion_threshold = 0x{:04X}",
                    init_out.congestion_threshold
                );
                tracing::debug!("  time_gran = {}", init_out.time_gran);
                io.send_msg(header.unique, 0, unsafe { as_bytes(&init_out) })
                    .await?;

                init_out.flags |= readonly_flags;

                let conn = ConnectionInfo(init_out);
                let bufsize = BUFFER_HEADER_SIZE + conn.max_write() as usize;

                Ok(Some(Session {
                    conn,
                    bufsize,
                    exited: AtomicBool::new(false),
                    notify_unique: AtomicU64::new(0),
                }))
            }
            _ => {
                tracing::warn!(
                    "ignoring an operation before init (opcode={:?})",
                    header.opcode
                );
                io.send_msg(header.unique, -libc::EIO, &()).await?;
                Ok(None)
            }
        }
    }
}

/// Information about the connection associated with a session.
pub struct ConnectionInfo(fuse_init_out);

impl fmt::Debug for ConnectionInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionInfo")
            .field("proto_major", &self.proto_major())
            .field("proto_minor", &self.proto_minor())
            .field("flags", &self.flags())
            .field("no_open_support", &self.no_open_support())
            .field("no_opendir_support", &self.no_opendir_support())
            .field("max_readahead", &self.max_readahead())
            .field("max_write", &self.max_write())
            .field("max_background", &self.max_background())
            .field("congestion_threshold", &self.congestion_threshold())
            .field("time_gran", &self.time_gran())
            .if_some(self.max_pages(), |f, pages| f.field("max_pages", &pages))
            .finish()
    }
}

impl ConnectionInfo {
    /// Returns the major version of the protocol.
    pub fn proto_major(&self) -> u32 {
        self.0.major
    }

    /// Returns the minor version of the protocol.
    pub fn proto_minor(&self) -> u32 {
        self.0.minor
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

    #[doc(hidden)]
    pub fn max_background(&self) -> u16 {
        self.0.max_background
    }

    #[doc(hidden)]
    pub fn congestion_threshold(&self) -> u16 {
        self.0.congestion_threshold
    }

    #[doc(hidden)]
    pub fn time_gran(&self) -> u32 {
        self.0.time_gran
    }

    #[doc(hidden)]
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
        const ASYNC_READ = kernel::FUSE_ASYNC_READ;

        /// The filesystem supports the `O_TRUNC` open flag.
        ///
        /// Enabled by default.
        const ATOMIC_O_TRUNC = kernel::FUSE_ATOMIC_O_TRUNC;

        /// The kernel check the validity of attributes on every read.
        ///
        /// Enabled by default.
        const AUTO_INVAL_DATA = kernel::FUSE_AUTO_INVAL_DATA;

        /// The filesystem supports asynchronous direct I/O submission.
        ///
        /// Enabled by default.
        const ASYNC_DIO = kernel::FUSE_ASYNC_DIO;

        /// The kernel supports parallel directory operations.
        ///
        /// Enabled by default.
        const PARALLEL_DIROPS = kernel::FUSE_PARALLEL_DIROPS;

        /// The filesystem is responsible for unsetting setuid and setgid bits
        /// when a file is written, truncated, or its owner is changed.
        ///
        /// Enabled by default.
        const HANDLE_KILLPRIV = kernel::FUSE_HANDLE_KILLPRIV;

        /// The filesystem supports the POSIX-style file lock.
        const POSIX_LOCKS = kernel::FUSE_POSIX_LOCKS;

        /// The filesystem supports the `flock` handling.
        const FLOCK_LOCKS = kernel::FUSE_FLOCK_LOCKS;

        /// The filesystem supports lookups of `"."` and `".."`.
        const EXPORT_SUPPORT = kernel::FUSE_EXPORT_SUPPORT;

        /// The kernel should not apply the umask to the file mode on create
        /// operations.
        const DONT_MASK = kernel::FUSE_DONT_MASK;

        /// The writeback caching should be enabled.
        const WRITEBACK_CACHE = kernel::FUSE_WRITEBACK_CACHE;

        /// The filesystem supports POSIX access control lists.
        const POSIX_ACL = kernel::FUSE_POSIX_ACL;

        /// The filesystem supports `readdirplus` operations.
        const READDIRPLUS = kernel::FUSE_DO_READDIRPLUS;

        /// Indicates that the kernel uses the adaptive readdirplus.
        const READDIRPLUS_AUTO = kernel::FUSE_READDIRPLUS_AUTO;

        // TODO: splice read/write
        // const SPLICE_WRITE = kernel::FUSE_SPLICE_WRITE;
        // const SPLICE_MOVE = kernel::FUSE_SPLICE_MOVE;
        // const SPLICE_READ = kernel::FUSE_SPLICE_READ;

        // TODO: ioctl
        // const IOCTL_DIR = kernel::FUSE_IOCTL_DIR;
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

/// FUSE session driver.
#[derive(Debug)]
pub struct Session {
    conn: ConnectionInfo,
    bufsize: usize,
    exited: AtomicBool,
    notify_unique: AtomicU64,
}

impl Session {
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
        T: AsyncRead + AsyncWrite + Send + Unpin,
    {
        if self.exited() {
            tracing::warn!("The sesson has already been exited");
            return Ok(());
        }

        let request = io.read_request().await?;
        let header = &request.header;
        let op = Parser::new(header, &request.arg).parse()?;

        macro_rules! dispatch_op {
            ($( $Variant:ident => $name:ident, )*) => {
                match op {
                    OperationKind::Destroy => {
                        self.exit();
                        return Ok(());
                    }
                    OperationKind::Forget { arg } => {
                        fs.forget(Some(ForgetOne(&kernel::fuse_forget_one {
                            nodeid: header.nodeid,
                            nlookup: arg.nlookup,
                        }))).await;
                    },
                    OperationKind::BatchForget { forgets, .. } => {
                        fs.forget(forgets.iter().map(|forget| ForgetOne(forget))).await;
                    },

                    $(
                        OperationKind::$Variant(op) => {
                            fs.$name(SessionOperation {
                                io: &mut *io,
                                header,
                                op,
                            })
                            .await?;
                        }
                    )*

                    // TODO: interrupt, notify_reply

                    OperationKind::Init { .. } => {
                        tracing::warn!("ignore an INIT request after initializing the session");
                        io.send_msg(header.unique, -libc::EIO, &[]).await?;
                    }

                    _ => {
                        tracing::warn!("unknown opcode: {}", header.opcode);
                        io.send_msg(header.unique, -libc::EIO, &[]).await?;
                    },
                }
            };
        }

        dispatch_op! {
            Lookup => lookup,
            Getattr => getattr,
            Setattr => setattr,
            Readlink => readlink,
            Symlink => symlink,
            Mknod => mknod,
            Mkdir => mkdir,
            Unlink => unlink,
            Rmdir => rmdir,
            Rename => rename,
            Rename2 => rename,
            Link => link,
            Open => open,
            Read => read,
            Write => write,
            Release => release,
            Statfs => statfs,
            Fsync => fsync,
            Setxattr => setxattr,
            Getxattr => getxattr,
            Listxattr => listxattr,
            Removexattr => removexattr,
            Flush => flush,
            Opendir => opendir,
            Readdir => readdir,
            Releasedir => releasedir,
            Fsyncdir => fsyncdir,
            Getlk => getlk,
            Setlk => setlk,
            Flock => flock,
            Access => access,
            Create => create,
            Bmap => bmap,
            Fallocate => fallocate,
            CopyFileRange => copy_file_range,
            Poll => poll,
        }

        // match arg {
        //     // OperationKind::Operation(op) => match op {
        //     //     op @ Operation::Forget(..) => {
        //     //         cx.disable_writer();
        //     //         fs.call(&mut cx, op).await?;
        //     //     }
        //     //     op @ Operation::Interrupt(..) => {
        //     //         fs.call(&mut cx, op).await?;
        //     //     }
        //     //     op @ Operation::NotifyReply(..) => {
        //     //         cx.disable_writer();
        //     //         fs.call(&mut cx, op).await?;
        //     //     }
        //     //     op @ Operation::Statfs(..) => {
        //     //         fs.call(&mut cx, op).await?;
        //     //         if !cx.replied() {
        //     //             let mut st = StatFs::default();
        //     //             st.set_namelen(255);
        //     //             st.set_bsize(512);
        //     //             cx.reply(crate::reply::ReplyStatfs::new(st)).await?;
        //     //         }
        //     //     }
        //     //     op => {
        //     //         fs.call(&mut cx, op).await?;
        //     //         if !cx.replied() {
        //     //             cx.reply_err(libc::ENOSYS).await?;
        //     //         }
        //     //     }
        //     // },
        //     _ => todo!(),
        // }

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
        W: AsyncWrite + Unpin,
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
    W: AsyncWrite + Unpin,
{
    let code = unsafe { mem::transmute::<_, i32>(code) };
    writer.send_msg(0, code, data).await
}

struct ForgetOne<'a>(&'a kernel::fuse_forget_one);

impl op::Forget for ForgetOne<'_> {
    fn ino(&self) -> u64 {
        self.0.nodeid
    }

    fn nlookup(&self) -> u64 {
        self.0.nlookup
    }
}

struct SessionOperation<'ctx, I, Op> {
    io: I,
    header: &'ctx fuse_in_header,
    op: Op,
}

type OpResult<T> = std::result::Result<<T as Operation>::Ok, <T as Operation>::Error>;

#[async_trait]
impl<'ctx, I, Op> Operation for SessionOperation<'ctx, I, Op>
where
    I: AsyncWrite + Unpin + Send,
    Op: Send,
{
    type Ok = ();
    type Error = io::Error;

    fn unique(&self) -> u64 {
        self.header.unique
    }

    fn uid(&self) -> u32 {
        self.header.uid
    }

    fn gid(&self) -> u32 {
        self.header.gid
    }

    fn pid(&self) -> u32 {
        self.header.pid
    }

    async fn reply_err(mut self, error: i32) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, -error, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Lookup for SessionOperation<'ctx, I, Lookup<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        &*self.op.name
    }

    async fn reply<R>(mut self, reply: R) -> OpResult<Self>
    where
        R: op::ReplyEntry + Send + Sync,
    {
        let reply = ReplyEntry::new(&reply);
        self.io.send_msg(self.header.unique, 0, &reply).await
    }
}

#[async_trait]
impl<'ctx, I> op::Getattr for SessionOperation<'ctx, I, Getattr<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> Option<u64> {
        if self.op.arg.getattr_flags & kernel::FUSE_GETATTR_FH != 0 {
            Some(self.op.arg.fh)
        } else {
            None
        }
    }

    async fn reply<R>(mut self, reply: R) -> OpResult<Self>
    where
        R: op::ReplyAttr + Send + Sync,
    {
        let reply = ReplyAttr::new(&reply);
        self.io.send_msg(self.header.unique, 0, &reply).await
    }
}

impl<'ctx, I> SessionOperation<'ctx, I, Setattr<'ctx>> {
    #[inline(always)]
    fn get<R>(&self, flag: u32, f: impl FnOnce(&kernel::fuse_setattr_in) -> R) -> Option<R> {
        if self.op.arg.valid & flag != 0 {
            Some(f(&self.op.arg))
        } else {
            None
        }
    }
}

#[async_trait]
impl<'ctx, I> op::Setattr for SessionOperation<'ctx, I, Setattr<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> Option<u64> {
        self.get(kernel::FATTR_FH, |arg| arg.fh)
    }

    fn mode(&self) -> Option<u32> {
        self.get(kernel::FATTR_MODE, |arg| arg.mode)
    }

    fn uid(&self) -> Option<u32> {
        self.get(kernel::FATTR_UID, |arg| arg.uid)
    }

    fn gid(&self) -> Option<u32> {
        self.get(kernel::FATTR_GID, |arg| arg.gid)
    }

    fn size(&self) -> Option<u64> {
        self.get(kernel::FATTR_SIZE, |arg| arg.size)
    }

    fn atime_raw(&self) -> Option<(u64, u32, bool)> {
        self.get(kernel::FATTR_ATIME, |arg| {
            (
                arg.atime,
                arg.atimensec,
                arg.valid & kernel::FATTR_ATIME_NOW != 0,
            )
        })
    }

    fn mtime_raw(&self) -> Option<(u64, u32, bool)> {
        self.get(kernel::FATTR_MTIME, |arg| {
            (
                arg.mtime,
                arg.mtimensec,
                arg.valid & kernel::FATTR_MTIME_NOW != 0,
            )
        })
    }

    fn ctime_raw(&self) -> Option<(u64, u32)> {
        self.get(kernel::FATTR_CTIME, |arg| (arg.ctime, arg.ctimensec))
    }

    fn lock_owner(&self) -> Option<LockOwner> {
        self.get(kernel::FATTR_LOCKOWNER, |arg| {
            LockOwner::from_raw(arg.lock_owner)
        })
    }

    async fn reply<R>(mut self, reply: R) -> OpResult<Self>
    where
        R: op::ReplyAttr + Send + Sync,
    {
        let reply = ReplyAttr::new(&reply);
        self.io.send_msg(self.header.unique, 0, &reply).await
    }
}

#[async_trait]
impl<'ctx, I> op::Readlink for SessionOperation<'ctx, I, Readlink>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    async fn reply(mut self, reply: &OsStr) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, reply).await
    }
}

#[async_trait]
impl<'ctx, I> op::Symlink for SessionOperation<'ctx, I, Symlink<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        &*self.op.name
    }

    fn link(&self) -> &OsStr {
        &*self.op.link
    }

    async fn reply<R>(mut self, reply: R) -> OpResult<Self>
    where
        R: op::ReplyEntry + Send + Sync,
    {
        let reply = ReplyEntry::new(&reply);
        self.io.send_msg(self.header.unique, 0, &reply).await
    }
}

#[async_trait]
impl<'ctx, I> op::Mknod for SessionOperation<'ctx, I, Mknod<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        &*self.op.name
    }

    fn mode(&self) -> u32 {
        self.op.arg.mode
    }

    fn rdev(&self) -> u32 {
        self.op.arg.rdev
    }

    fn umask(&self) -> u32 {
        self.op.arg.umask
    }

    async fn reply<R>(mut self, reply: R) -> OpResult<Self>
    where
        R: op::ReplyEntry + Send + Sync,
    {
        let reply = ReplyEntry::new(&reply);
        self.io.send_msg(self.header.unique, 0, &reply).await
    }
}

#[async_trait]
impl<'ctx, I> op::Mkdir for SessionOperation<'ctx, I, Mkdir<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        &*self.op.name
    }

    fn mode(&self) -> u32 {
        self.op.arg.mode
    }

    fn umask(&self) -> u32 {
        self.op.arg.umask
    }

    async fn reply<R>(mut self, reply: R) -> OpResult<Self>
    where
        R: op::ReplyEntry + Send + Sync,
    {
        let reply = ReplyEntry::new(&reply);
        self.io.send_msg(self.header.unique, 0, &reply).await
    }
}

#[async_trait]
impl<'ctx, I> op::Unlink for SessionOperation<'ctx, I, Unlink<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        &*self.op.name
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Rmdir for SessionOperation<'ctx, I, Rmdir<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        &*self.op.name
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Rename for SessionOperation<'ctx, I, Rename<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        &*self.op.name
    }

    fn newparent(&self) -> u64 {
        self.op.arg.newdir
    }

    fn newname(&self) -> &OsStr {
        &*self.op.newname
    }

    fn flags(&self) -> u32 {
        0
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Rename for SessionOperation<'ctx, I, Rename2<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        &*self.op.name
    }

    fn newparent(&self) -> u64 {
        self.op.arg.newdir
    }

    fn newname(&self) -> &OsStr {
        &*self.op.newname
    }

    fn flags(&self) -> u32 {
        self.op.arg.flags
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Link for SessionOperation<'ctx, I, Link<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.op.arg.oldnodeid
    }

    fn newparent(&self) -> u64 {
        self.header.nodeid
    }

    fn newname(&self) -> &OsStr {
        &*self.op.newname
    }

    async fn reply<R>(mut self, reply: R) -> OpResult<Self>
    where
        R: op::ReplyEntry + Send + Sync,
    {
        let reply = ReplyEntry::new(&reply);
        self.io.send_msg(self.header.unique, 0, &reply).await
    }
}

#[async_trait]
impl<'ctx, I> op::Open for SessionOperation<'ctx, I, Open<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn flags(&self) -> u32 {
        self.op.arg.flags
    }

    async fn reply<R>(mut self, reply: R) -> OpResult<Self>
    where
        R: op::ReplyOpen + Send + Sync,
    {
        let reply = ReplyOpen::new(&reply);
        self.io.send_msg(self.header.unique, 0, &reply).await
    }
}

#[async_trait]
impl<'ctx, I> op::Read for SessionOperation<'ctx, I, Read<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.op.arg.fh
    }

    fn offset(&self) -> u64 {
        self.op.arg.offset
    }

    fn size(&self) -> u32 {
        self.op.arg.size
    }

    fn flags(&self) -> u32 {
        self.op.arg.flags
    }

    fn lock_owner(&self) -> Option<LockOwner> {
        if self.op.arg.read_flags & kernel::FUSE_READ_LOCKOWNER != 0 {
            Some(LockOwner::from_raw(self.op.arg.lock_owner))
        } else {
            None
        }
    }

    async fn reply<R>(mut self, data: &[u8]) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, data).await
    }
}

#[async_trait]
impl<'ctx, I> op::Write for SessionOperation<'ctx, I, Write<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.op.arg.fh
    }

    fn offset(&self) -> u64 {
        self.op.arg.offset
    }

    fn size(&self) -> u32 {
        self.op.arg.size
    }

    fn flags(&self) -> u32 {
        self.op.arg.flags
    }

    fn lock_owner(&self) -> Option<LockOwner> {
        if self.op.arg.write_flags & kernel::FUSE_WRITE_LOCKOWNER != 0 {
            Some(LockOwner::from_raw(self.op.arg.lock_owner))
        } else {
            None
        }
    }

    async fn reply(mut self, size: u32) -> OpResult<Self> {
        let reply = ReplyWrite(kernel::fuse_write_out {
            size,
            ..Default::default()
        });
        self.io.send_msg(self.header.unique, 0, &reply).await
    }
}

#[async_trait]
impl<'ctx, I> op::Release for SessionOperation<'ctx, I, Release<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.op.arg.fh
    }

    fn flags(&self) -> u32 {
        self.op.arg.flags
    }

    fn lock_owner(&self) -> LockOwner {
        // NOTE: fuse_release_in.lock_owner is available since ABI 7.8.
        LockOwner::from_raw(self.op.arg.lock_owner)
    }

    fn flush(&self) -> bool {
        self.op.arg.release_flags & kernel::FUSE_RELEASE_FLUSH != 0
    }

    fn flock_release(&self) -> bool {
        self.op.arg.release_flags & kernel::FUSE_RELEASE_FLOCK_UNLOCK != 0
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Statfs for SessionOperation<'ctx, I, Statfs>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    async fn reply<S>(self, stat: S) -> OpResult<Self>
    where
        S: polyfuse::types::FsStatistics + Send + Sync,
    {
        todo!()
    }
}

#[async_trait]
impl<'ctx, I> op::Fsync for SessionOperation<'ctx, I, Fsync<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.op.arg.fh
    }

    fn datasync(&self) -> bool {
        self.op.arg.fsync_flags & kernel::FUSE_FSYNC_FDATASYNC != 0
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Setxattr for SessionOperation<'ctx, I, Setxattr<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        &*self.op.name
    }

    fn value(&self) -> &[u8] {
        &*self.op.value
    }

    fn flags(&self) -> u32 {
        self.op.arg.flags
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Getxattr for SessionOperation<'ctx, I, Getxattr<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        &*self.op.name
    }

    fn size(&self) -> u32 {
        self.op.arg.size
    }

    async fn reply_size(self, size: u32) -> OpResult<Self> {
        todo!()
    }

    async fn reply(self, value: &[u8]) -> OpResult<Self> {
        todo!()
    }
}

#[async_trait]
impl<'ctx, I> op::Listxattr for SessionOperation<'ctx, I, Listxattr<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn size(&self) -> u32 {
        self.op.arg.size
    }

    async fn reply_size(self, size: u32) -> OpResult<Self> {
        todo!()
    }

    async fn reply(self, value: &[u8]) -> OpResult<Self> {
        todo!()
    }
}

#[async_trait]
impl<'ctx, I> op::Removexattr for SessionOperation<'ctx, I, Removexattr<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        &*self.op.name
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Flush for SessionOperation<'ctx, I, Flush<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.op.arg.fh
    }

    fn lock_owner(&self) -> LockOwner {
        LockOwner::from_raw(self.op.arg.lock_owner)
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Opendir for SessionOperation<'ctx, I, Opendir<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn flags(&self) -> u32 {
        self.op.arg.flags
    }

    async fn reply<R>(self, reply: R) -> OpResult<Self>
    where
        R: op::ReplyOpen + Send + Sync,
    {
        todo!()
    }
}

#[async_trait]
impl<'ctx, I> op::Readdir for SessionOperation<'ctx, I, Readdir<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.op.arg.fh
    }

    fn offset(&self) -> u64 {
        self.op.arg.offset
    }

    fn size(&self) -> u32 {
        self.op.arg.size
    }

    fn is_plus(&self) -> bool {
        self.op.is_plus
    }

    async fn reply<D>(self, dirs: D) -> OpResult<Self>
    where
        D: IntoIterator + Send,
        D::IntoIter: Send,
        D::Item: polyfuse::types::DirEntry + Send + Sync,
    {
        todo!()
    }
}

#[async_trait]
impl<'ctx, I> op::Releasedir for SessionOperation<'ctx, I, Releasedir<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.op.arg.fh
    }

    fn flags(&self) -> u32 {
        self.op.arg.flags
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Fsyncdir for SessionOperation<'ctx, I, Fsyncdir<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.op.arg.fh
    }

    fn datasync(&self) -> bool {
        self.op.arg.fsync_flags & kernel::FUSE_FSYNC_FDATASYNC != 0
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Getlk for SessionOperation<'ctx, I, Getlk<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.op.arg.fh
    }

    fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.op.arg.owner)
    }

    fn lk(&self) -> &(dyn FileLock + Send + Sync) {
        todo!()
    }

    async fn reply<L>(self, lk: L) -> OpResult<Self>
    where
        L: polyfuse::types::FileLock + Send + Sync,
    {
        todo!()
    }
}

#[async_trait]
impl<'ctx, I> op::Setlk for SessionOperation<'ctx, I, Setlk<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.op.arg.fh
    }

    fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.op.arg.owner)
    }

    fn lk(&self) -> &(dyn FileLock + Send + Sync) {
        todo!()
    }

    fn sleep(&self) -> bool {
        self.op.sleep
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Flock for SessionOperation<'ctx, I, Flock<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.op.arg.fh
    }

    fn owner_id(&self) -> LockOwner {
        LockOwner::from_raw(self.op.arg.owner)
    }

    fn op(&self) -> Option<u32> {
        const F_RDLCK: u32 = libc::F_RDLCK as u32;
        const F_WRLCK: u32 = libc::F_WRLCK as u32;
        const F_UNLCK: u32 = libc::F_UNLCK as u32;

        let mut op = match self.op.arg.lk.typ {
            F_RDLCK => libc::LOCK_SH as u32,
            F_WRLCK => libc::LOCK_EX as u32,
            F_UNLCK => libc::LOCK_UN as u32,
            _ => return None,
        };
        if !self.op.sleep {
            op |= libc::LOCK_NB as u32;
        }

        Some(op)
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Access for SessionOperation<'ctx, I, Access<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn mask(&self) -> u32 {
        self.op.arg.mask
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::Create for SessionOperation<'ctx, I, Create<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.op.name
    }

    fn mode(&self) -> u32 {
        self.op.arg.mode
    }

    fn umask(&self) -> u32 {
        self.op.arg.umask
    }

    fn open_flags(&self) -> u32 {
        self.op.arg.flags
    }

    async fn reply<E, O>(self, entry: E, open: O) -> OpResult<Self>
    where
        E: op::ReplyEntry + Send + Sync,
        O: op::ReplyOpen + Send + Sync,
    {
        todo!()
    }
}

#[async_trait]
impl<'ctx, I> op::Bmap for SessionOperation<'ctx, I, Bmap<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn block(&self) -> u64 {
        self.op.arg.block
    }

    fn blocksize(&self) -> u32 {
        self.op.arg.blocksize
    }

    async fn reply(self, block: u64) -> OpResult<Self> {
        todo!()
    }
}

#[async_trait]
impl<'ctx, I> op::Fallocate for SessionOperation<'ctx, I, Fallocate<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.op.arg.fh
    }

    fn offset(&self) -> u64 {
        self.op.arg.offset
    }

    fn length(&self) -> u64 {
        self.op.arg.length
    }

    fn mode(&self) -> u32 {
        self.op.arg.mode
    }

    async fn reply(mut self) -> OpResult<Self> {
        self.io.send_msg(self.header.unique, 0, &[]).await
    }
}

#[async_trait]
impl<'ctx, I> op::CopyFileRange for SessionOperation<'ctx, I, CopyFileRange<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino_in(&self) -> u64 {
        self.header.nodeid
    }

    fn fh_in(&self) -> u64 {
        self.op.arg.fh_in
    }

    fn offset_in(&self) -> u64 {
        self.op.arg.off_in
    }

    fn ino_out(&self) -> u64 {
        self.op.arg.nodeid_out
    }

    fn fh_out(&self) -> u64 {
        self.op.arg.fh_out
    }

    fn offset_out(&self) -> u64 {
        self.op.arg.off_out
    }

    fn length(&self) -> u64 {
        self.op.arg.len
    }

    fn flags(&self) -> u64 {
        self.op.arg.flags
    }

    async fn reply(self, size: u32) -> OpResult<Self> {
        todo!()
    }
}

#[async_trait]
impl<'ctx, I> op::Poll for SessionOperation<'ctx, I, PollOp<'ctx>>
where
    I: AsyncWrite + Unpin + Send,
{
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.op.arg.fh
    }

    fn events(&self) -> u32 {
        self.op.arg.events
    }

    fn kh(&self) -> Option<u64> {
        if self.op.arg.flags & kernel::FUSE_POLL_SCHEDULE_NOTIFY != 0 {
            Some(self.op.arg.kh)
        } else {
            None
        }
    }

    async fn reply(self, revents: u32) -> OpResult<Self> {
        todo!()
    }
}

#[cfg(test)]
mod tests_session {
    use super::*;
    use futures::{
        executor::block_on,
        io::{AsyncRead, AsyncWrite},
        task::{self, Poll},
    };
    use kernel::{fuse_in_header, fuse_init_in};
    use pin_project_lite::pin_project;
    use std::{
        io::{IoSlice, IoSliceMut},
        mem,
        pin::Pin,
    };

    pin_project! {
        struct Unite<R, W> {
            #[pin]
            reader: R,
            #[pin]
            writer: W,
        }
    }

    impl<R, W> AsyncRead for Unite<R, W>
    where
        R: AsyncRead,
    {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            dst: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            self.project().reader.poll_read(cx, dst)
        }

        fn poll_read_vectored(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            dst: &mut [IoSliceMut<'_>],
        ) -> Poll<io::Result<usize>> {
            self.project().reader.poll_read_vectored(cx, dst)
        }
    }

    impl<R, W> AsyncWrite for Unite<R, W>
    where
        W: AsyncWrite,
    {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            src: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.project().writer.poll_write(cx, src)
        }

        fn poll_write_vectored(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            src: &[IoSlice<'_>],
        ) -> Poll<io::Result<usize>> {
            self.project().writer.poll_write_vectored(cx, src)
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
            self.project().writer.poll_flush(cx)
        }

        fn poll_close(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
            self.project().writer.poll_close(cx)
        }
    }

    #[test]
    fn init_default() {
        #[allow(clippy::cast_possible_truncation)]
        let in_header = fuse_in_header {
            len: (mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_init_in>()) as u32,
            opcode: kernel::FUSE_INIT,
            unique: 2,
            nodeid: 0,
            uid: 100,
            gid: 100,
            pid: 12,
            padding: 0,
        };
        let init_in = fuse_init_in {
            major: 7,
            minor: 23,
            max_readahead: 40,
            flags: CapabilityFlags::all().bits()
                | FUSE_MAX_PAGES
                | FUSE_NO_OPEN_SUPPORT
                | FUSE_NO_OPENDIR_SUPPORT,
        };

        let mut input = vec![];
        input.extend_from_slice(unsafe { crate::util::as_bytes(&in_header) });
        input.extend_from_slice(unsafe { crate::util::as_bytes(&init_in) });

        let mut reader = &input[..];
        let mut writer = Vec::<u8>::new();
        let mut io = Unite {
            reader: &mut reader,
            writer: &mut writer,
        };

        let init_session = SessionInitializer::default();
        let session = block_on(init_session.try_init(&mut io))
            .expect("initialization failed") // Result<Option<Session>>
            .expect("empty session"); // Option<Session>

        let conn = session.connection_info();
        assert_eq!(conn.proto_major(), 7);
        assert_eq!(conn.proto_minor(), 23);
        assert_eq!(conn.max_readahead(), 40);
        assert_eq!(conn.max_background(), 0);
        assert_eq!(conn.congestion_threshold(), 0);
        assert_eq!(conn.max_write(), DEFAULT_MAX_WRITE);
        assert_eq!(
            conn.max_pages(),
            Some((DEFAULT_MAX_WRITE / (*PAGE_SIZE as u32)) as u16)
        );
        assert_eq!(conn.time_gran(), 1);
        assert!(conn.no_open_support());
        assert!(conn.no_opendir_support());
    }
}

// ==== parse ====

enum OperationKind<'a> {
    Init {
        arg: &'a kernel::fuse_init_in,
    },
    Destroy,
    Forget {
        arg: &'a kernel::fuse_forget_in,
    },
    BatchForget {
        forgets: &'a [kernel::fuse_forget_one],
    },
    Interrupt(Interrupt<'a>),
    NotifyReply(NotifyReply<'a>),
    Lookup(Lookup<'a>),
    Getattr(Getattr<'a>),
    Setattr(Setattr<'a>),
    Readlink(Readlink),
    Symlink(Symlink<'a>),
    Mknod(Mknod<'a>),
    Mkdir(Mkdir<'a>),
    Unlink(Unlink<'a>),
    Rmdir(Rmdir<'a>),
    Rename(Rename<'a>),
    Rename2(Rename2<'a>),
    Link(Link<'a>),
    Open(Open<'a>),
    Read(Read<'a>),
    Write(Write<'a>),
    Release(Release<'a>),
    Statfs(Statfs),
    Fsync(Fsync<'a>),
    Setxattr(Setxattr<'a>),
    Getxattr(Getxattr<'a>),
    Listxattr(Listxattr<'a>),
    Removexattr(Removexattr<'a>),
    Flush(Flush<'a>),
    Opendir(Opendir<'a>),
    Readdir(Readdir<'a>),
    Releasedir(Releasedir<'a>),
    Fsyncdir(Fsyncdir<'a>),
    Getlk(Getlk<'a>),
    Setlk(Setlk<'a>),
    Flock(Flock<'a>),
    Access(Access<'a>),
    Create(Create<'a>),
    Bmap(Bmap<'a>),
    Fallocate(Fallocate<'a>),
    CopyFileRange(CopyFileRange<'a>),
    Poll(PollOp<'a>),

    Unknown,
}

struct Interrupt<'a> {
    arg: &'a kernel::fuse_interrupt_in,
}

struct NotifyReply<'a> {
    arg: &'a kernel::fuse_notify_retrieve_in,
}

struct Lookup<'a> {
    name: &'a OsStr,
}

struct Getattr<'a> {
    arg: &'a kernel::fuse_getattr_in,
}

struct Setattr<'a> {
    arg: &'a kernel::fuse_setattr_in,
}

struct Readlink;

struct Symlink<'a> {
    name: &'a OsStr,
    link: &'a OsStr,
}

struct Mknod<'a> {
    arg: &'a kernel::fuse_mknod_in,
    name: &'a OsStr,
}

struct Mkdir<'a> {
    arg: &'a kernel::fuse_mkdir_in,
    name: &'a OsStr,
}

struct Unlink<'a> {
    name: &'a OsStr,
}

struct Rmdir<'a> {
    name: &'a OsStr,
}

struct Rename<'a> {
    arg: &'a kernel::fuse_rename_in,
    name: &'a OsStr,
    newname: &'a OsStr,
}

struct Rename2<'a> {
    arg: &'a kernel::fuse_rename2_in,
    name: &'a OsStr,
    newname: &'a OsStr,
}

struct Link<'a> {
    arg: &'a kernel::fuse_link_in,
    newname: &'a OsStr,
}

struct Open<'a> {
    arg: &'a kernel::fuse_open_in,
}

struct Read<'a> {
    arg: &'a kernel::fuse_read_in,
}

struct Write<'a> {
    arg: &'a kernel::fuse_write_in,
}

struct Release<'a> {
    arg: &'a kernel::fuse_release_in,
}

struct Statfs;

struct Fsync<'a> {
    arg: &'a kernel::fuse_fsync_in,
}

struct Setxattr<'a> {
    arg: &'a kernel::fuse_setxattr_in,
    name: &'a OsStr,
    value: &'a [u8],
}

struct Getxattr<'a> {
    arg: &'a kernel::fuse_getxattr_in,
    name: &'a OsStr,
}

struct Listxattr<'a> {
    arg: &'a kernel::fuse_getxattr_in,
}

struct Removexattr<'a> {
    name: &'a OsStr,
}

struct Flush<'a> {
    arg: &'a kernel::fuse_flush_in,
}

struct Opendir<'a> {
    arg: &'a kernel::fuse_open_in,
}

struct Readdir<'a> {
    arg: &'a kernel::fuse_read_in,
    is_plus: bool,
}

struct Releasedir<'a> {
    arg: &'a kernel::fuse_release_in,
}

struct Fsyncdir<'a> {
    arg: &'a kernel::fuse_fsync_in,
}

struct Getlk<'a> {
    arg: &'a kernel::fuse_lk_in,
}

struct Setlk<'a> {
    arg: &'a kernel::fuse_lk_in,
    sleep: bool,
}

struct Flock<'a> {
    arg: &'a kernel::fuse_lk_in,
    sleep: bool,
}

struct Access<'a> {
    arg: &'a kernel::fuse_access_in,
}

struct Create<'a> {
    arg: &'a kernel::fuse_create_in,
    name: &'a OsStr,
}

struct Bmap<'a> {
    arg: &'a kernel::fuse_bmap_in,
}

struct Fallocate<'a> {
    arg: &'a kernel::fuse_fallocate_in,
}

struct CopyFileRange<'a> {
    arg: &'a kernel::fuse_copy_file_range_in,
}

struct PollOp<'a> {
    arg: &'a kernel::fuse_poll_in,
}

// ==== parser ====

struct Parser<'a> {
    header: &'a fuse_in_header,
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Parser<'a> {
    fn new(header: &'a fuse_in_header, bytes: &'a [u8]) -> Self {
        Self {
            header,
            bytes,
            offset: 0,
        }
    }

    fn fetch_bytes(&mut self, count: usize) -> io::Result<&'a [u8]> {
        if self.bytes.len() < self.offset + count {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "fetch"));
        }
        let bytes = &self.bytes[self.offset..self.offset + count];
        self.offset += count;
        Ok(bytes)
    }

    fn fetch_array<T>(&mut self, count: usize) -> io::Result<&'a [T]> {
        self.fetch_bytes(mem::size_of::<T>() * count)
            .map(|bytes| unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const T, count) })
    }

    fn fetch_str(&mut self) -> io::Result<&'a OsStr> {
        let len = self.bytes[self.offset..]
            .iter()
            .position(|&b| b == b'\0')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "fetch_str: missing \\0"))?;
        self.fetch_bytes(len).map(|s| {
            self.offset = std::cmp::min(self.bytes.len(), self.offset + 1);
            OsStr::from_bytes(s)
        })
    }

    fn fetch<T>(&mut self) -> io::Result<&'a T> {
        self.fetch_bytes(mem::size_of::<T>())
            .map(|data| unsafe { &*(data.as_ptr() as *const T) })
    }

    fn parse(&mut self) -> io::Result<OperationKind<'a>> {
        let header = self.header;
        match fuse_opcode::try_from(header.opcode).ok() {
            Some(fuse_opcode::FUSE_INIT) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Init { arg })
            }
            Some(fuse_opcode::FUSE_DESTROY) => Ok(OperationKind::Destroy),
            Some(fuse_opcode::FUSE_FORGET) => {
                let arg = self.fetch::<kernel::fuse_forget_in>()?;
                Ok(OperationKind::Forget { arg })
            }
            Some(fuse_opcode::FUSE_BATCH_FORGET) => {
                let arg = self.fetch::<kernel::fuse_batch_forget_in>()?;
                let forgets = self.fetch_array(arg.count as usize)?;
                Ok(OperationKind::BatchForget { forgets })
            }
            Some(fuse_opcode::FUSE_INTERRUPT) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Interrupt(Interrupt { arg }))
            }
            Some(fuse_opcode::FUSE_NOTIFY_REPLY) => {
                let arg = self.fetch()?;
                Ok(OperationKind::NotifyReply(NotifyReply { arg }))
            }

            Some(fuse_opcode::FUSE_LOOKUP) => {
                let name = self.fetch_str()?;
                Ok(OperationKind::Lookup(Lookup { name }))
            }
            Some(fuse_opcode::FUSE_GETATTR) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Getattr(Getattr { arg }))
            }
            Some(fuse_opcode::FUSE_SETATTR) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Setattr(Setattr { arg }))
            }
            Some(fuse_opcode::FUSE_READLINK) => Ok(OperationKind::Readlink(Readlink)),
            Some(fuse_opcode::FUSE_SYMLINK) => {
                let name = self.fetch_str()?;
                let link = self.fetch_str()?;
                Ok(OperationKind::Symlink(Symlink { name, link }))
            }
            Some(fuse_opcode::FUSE_MKNOD) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(OperationKind::Mknod(Mknod { arg, name }))
            }
            Some(fuse_opcode::FUSE_MKDIR) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(OperationKind::Mkdir(Mkdir { arg, name }))
            }
            Some(fuse_opcode::FUSE_UNLINK) => {
                let name = self.fetch_str()?;
                Ok(OperationKind::Unlink(Unlink { name }))
            }
            Some(fuse_opcode::FUSE_RMDIR) => {
                let name = self.fetch_str()?;
                Ok(OperationKind::Rmdir(Rmdir { name }))
            }

            Some(fuse_opcode::FUSE_RENAME) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                let newname = self.fetch_str()?;
                Ok(OperationKind::Rename(Rename { arg, name, newname }))
            }
            Some(fuse_opcode::FUSE_LINK) => {
                let arg = self.fetch()?;
                let newname = self.fetch_str()?;
                Ok(OperationKind::Link(Link { arg, newname }))
            }
            Some(fuse_opcode::FUSE_OPEN) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Open(Open { arg }))
            }
            Some(fuse_opcode::FUSE_READ) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Read(Read { arg }))
            }
            Some(fuse_opcode::FUSE_WRITE) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Write(Write { arg }))
            }
            Some(fuse_opcode::FUSE_RELEASE) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Release(Release { arg }))
            }
            Some(fuse_opcode::FUSE_STATFS) => Ok(OperationKind::Statfs(Statfs)),
            Some(fuse_opcode::FUSE_FSYNC) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Fsync(Fsync { arg }))
            }
            Some(fuse_opcode::FUSE_SETXATTR) => {
                let arg = self.fetch::<kernel::fuse_setxattr_in>()?;
                let name = self.fetch_str()?;
                let value = self.fetch_bytes(arg.size as usize)?;
                Ok(OperationKind::Setxattr(Setxattr { arg, name, value }))
            }
            Some(fuse_opcode::FUSE_GETXATTR) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(OperationKind::Getxattr(Getxattr { arg, name }))
            }
            Some(fuse_opcode::FUSE_LISTXATTR) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Listxattr(Listxattr { arg }))
            }
            Some(fuse_opcode::FUSE_REMOVEXATTR) => {
                let name = self.fetch_str()?;
                Ok(OperationKind::Removexattr(Removexattr { name }))
            }
            Some(fuse_opcode::FUSE_FLUSH) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Flush(Flush { arg }))
            }
            Some(fuse_opcode::FUSE_OPENDIR) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Opendir(Opendir { arg }))
            }
            Some(fuse_opcode::FUSE_READDIR) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Readdir(Readdir {
                    arg,
                    is_plus: false,
                }))
            }
            Some(fuse_opcode::FUSE_RELEASEDIR) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Releasedir(Releasedir { arg }))
            }
            Some(fuse_opcode::FUSE_FSYNCDIR) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Fsyncdir(Fsyncdir { arg }))
            }
            Some(fuse_opcode::FUSE_GETLK) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Getlk(Getlk { arg }))
            }
            Some(fuse_opcode::FUSE_SETLK) => {
                let arg = self.fetch()?;
                Ok(new_lock_op(arg, false))
            }
            Some(fuse_opcode::FUSE_SETLKW) => {
                let arg = self.fetch()?;
                Ok(new_lock_op(arg, true))
            }
            Some(fuse_opcode::FUSE_ACCESS) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Access(Access { arg }))
            }
            Some(fuse_opcode::FUSE_CREATE) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(OperationKind::Create(Create { arg, name }))
            }
            Some(fuse_opcode::FUSE_BMAP) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Bmap(Bmap { arg }))
            }
            Some(fuse_opcode::FUSE_FALLOCATE) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Fallocate(Fallocate { arg }))
            }
            Some(fuse_opcode::FUSE_READDIRPLUS) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Readdir(Readdir { arg, is_plus: true }))
            }
            Some(fuse_opcode::FUSE_RENAME2) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                let newname = self.fetch_str()?;
                Ok(OperationKind::Rename2(Rename2 { arg, name, newname }))
            }
            Some(fuse_opcode::FUSE_COPY_FILE_RANGE) => {
                let arg = self.fetch()?;
                Ok(OperationKind::CopyFileRange(CopyFileRange { arg }))
            }
            Some(fuse_opcode::FUSE_POLL) => {
                let arg = self.fetch()?;
                Ok(OperationKind::Poll(PollOp { arg }))
            }
            _ => Ok(OperationKind::Unknown),
        }
    }
}

fn new_lock_op<'a>(arg: &'a kernel::fuse_lk_in, sleep: bool) -> OperationKind<'a> {
    if arg.lk_flags & kernel::FUSE_LK_FLOCK != 0 {
        OperationKind::Flock(Flock { arg, sleep })
    } else {
        OperationKind::Setlk(Setlk { arg, sleep })
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests_parse {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn parse_lookup() {
        let name = CString::new("foo").unwrap();
        let parent = 1;

        let header = fuse_in_header {
            len: (mem::size_of::<fuse_in_header>() + name.as_bytes_with_nul().len()) as u32,
            opcode: kernel::FUSE_LOOKUP,
            unique: 2,
            nodeid: parent,
            uid: 1,
            gid: 1,
            pid: 42,
            padding: 0,
        };
        let mut payload = vec![];
        payload.extend_from_slice(name.as_bytes_with_nul());

        let mut parser = Parser::new(&header, &payload[..]);
        let op = parser.parse().unwrap();
        match op {
            OperationKind::Lookup(op) => {
                assert_eq!(op.name.as_bytes(), name.as_bytes());
            }
            _ => panic!("incorret operation is returned"),
        }
    }

    #[test]
    fn parse_symlink() {
        let name = CString::new("foo").unwrap();
        let link = CString::new("bar").unwrap();
        let parent = 1;

        let header = fuse_in_header {
            len: (mem::size_of::<fuse_in_header>()
                + name.as_bytes_with_nul().len()
                + link.as_bytes_with_nul().len()) as u32,
            opcode: kernel::FUSE_SYMLINK,
            unique: 2,
            nodeid: parent,
            uid: 1,
            gid: 1,
            pid: 42,
            padding: 0,
        };
        let mut payload = vec![];
        payload.extend_from_slice(name.as_bytes_with_nul());
        payload.extend_from_slice(link.as_bytes_with_nul());

        let mut parser = Parser::new(&header, &payload[..]);
        let op = parser.parse().unwrap();
        match op {
            OperationKind::Symlink(op) => {
                assert_eq!(op.name.as_bytes(), name.as_bytes());
                assert_eq!(op.link.as_bytes(), link.as_bytes());
            }
            _ => panic!("incorret operation is returned"),
        }
    }
}

// ==== I/O ====

trait ReaderExt: AsyncRead {
    fn read_request(&mut self) -> ReadRequest<'_, Self>
    where
        Self: Unpin,
    {
        ReadRequest {
            reader: self,
            header: None,
            arg: None,
            state: ReadRequestState::Init,
        }
    }
}

impl<R: AsyncRead + ?Sized> ReaderExt for R {}

#[allow(missing_debug_implementations)]
struct ReadRequest<'r, R: ?Sized> {
    reader: &'r mut R,
    header: Option<fuse_in_header>,
    arg: Option<Vec<u8>>,
    state: ReadRequestState,
}

#[derive(Copy, Clone)]
enum ReadRequestState {
    Init,
    ReadingHeader,
    ReadingArg,
    Done,
}

impl<R: ?Sized> Future for ReadRequest<'_, R>
where
    R: AsyncRead + Unpin,
{
    type Output = io::Result<Request>;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let me = self.get_mut();
        loop {
            match me.state {
                ReadRequestState::Init => {
                    me.header
                        .get_or_insert_with(|| unsafe { mem::MaybeUninit::zeroed().assume_init() });
                    me.state = ReadRequestState::ReadingHeader;
                    continue;
                }
                ReadRequestState::ReadingHeader => {
                    let header = me.header.as_mut().expect("header is empty");
                    let count = futures::ready!(Pin::new(&mut me.reader)
                        .poll_read(cx, unsafe { crate::util::as_bytes_mut(header) }))?;
                    if count < mem::size_of::<fuse_in_header>() {
                        return Poll::Ready(Err(io::Error::from_raw_os_error(libc::EINVAL)));
                    }
                    me.state = ReadRequestState::ReadingArg;
                    let arg_len = match fuse_opcode::try_from(header.opcode).ok() {
                        Some(fuse_opcode::FUSE_WRITE) => mem::size_of::<fuse_write_in>(),
                        Some(fuse_opcode::FUSE_NOTIFY_REPLY) => {
                            mem::size_of::<fuse_notify_retrieve_in>()
                        } // = size_of::<fuse_write_in>()
                        _ => header.len as usize - mem::size_of::<fuse_in_header>(),
                    };
                    me.arg.get_or_insert_with(|| Vec::with_capacity(arg_len));
                    continue;
                }
                ReadRequestState::ReadingArg => {
                    {
                        struct Guard<'a>(&'a mut Vec<u8>);
                        impl Drop for Guard<'_> {
                            fn drop(&mut self) {
                                unsafe {
                                    self.0.set_len(0);
                                }
                            }
                        }

                        let arg = Guard(me.arg.as_mut().expect("arg is empty"));
                        unsafe {
                            arg.0.set_len(arg.0.capacity());
                        }

                        let count = futures::ready!(
                            Pin::new(&mut me.reader) //
                                .poll_read(cx, &mut arg.0[..])
                        )?;
                        if count < arg.0.len() {
                            return Poll::Ready(Err(io::Error::from_raw_os_error(libc::EINVAL)));
                        }

                        unsafe {
                            arg.0.set_len(count);
                        }
                        mem::forget(arg);
                    }

                    me.state = ReadRequestState::Done;
                    let header = me.header.take().unwrap();
                    let arg = me.arg.take().unwrap();

                    return Poll::Ready(Ok(Request { header, arg }));
                }
                ReadRequestState::Done => unreachable!(),
            }
        }
    }
}

struct Request {
    header: fuse_in_header,
    arg: Vec<u8>,
}

trait WriterExt: AsyncWrite {
    fn send_msg<'w, T: ?Sized>(
        &'w mut self,
        unique: u64,
        error: i32,
        data: &'w T,
    ) -> SendMsg<'w, Self>
    where
        Self: Unpin,
        T: Reply,
    {
        struct VecCollector<'a, 'v> {
            vec: &'v mut Vec<&'a [u8]>,
            total_len: usize,
        }

        impl<'a> Collector<'a> for VecCollector<'a, '_> {
            fn append(&mut self, bytes: &'a [u8]) {
                self.vec.push(bytes);
                self.total_len += bytes.len();
            }
        }

        let mut vec = Vec::new();
        let mut collector = VecCollector {
            vec: &mut vec,
            total_len: 0,
        };
        data.collect_bytes(&mut collector);

        let len = u32::try_from(mem::size_of::<fuse_out_header>() + collector.total_len).unwrap();
        let header = fuse_out_header { unique, error, len };

        drop(collector);

        SendMsg {
            writer: self,
            header,
            vec,
        }
    }
}

impl<W: AsyncWrite + ?Sized> WriterExt for W {}

struct SendMsg<'w, W: ?Sized> {
    writer: &'w mut W,
    header: fuse_out_header,
    vec: Vec<&'w [u8]>,
}

impl<W: ?Sized> Future for SendMsg<'_, W>
where
    W: AsyncWrite + Unpin,
{
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let me = self.get_mut();

        // Unfortunately, IoSlice<'_> does not implement Send and
        // the data vector must be created in `poll` function.
        let vec: SmallVec<[_; 4]> =
            Some(IoSlice::new(unsafe { crate::util::as_bytes(&me.header) }))
                .into_iter()
                .chain(me.vec.iter().map(|t| IoSlice::new(&*t)))
                .collect();

        let count = futures::ready!(Pin::new(&mut *me.writer).poll_write_vectored(cx, &*vec))?;
        if count < me.header.len as usize {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::Other,
                "written data is too short",
            )));
        }

        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests_io {
    use super::*;
    use futures::{
        executor::block_on,
        task::{self, Poll},
    };
    use kernel::fuse_init_in;
    use pin_project_lite::pin_project;
    use std::{
        io::{self, IoSlice},
        ops::Index,
        pin::Pin,
    };

    #[inline]
    fn bytes(bytes: &[u8]) -> &[u8] {
        bytes
    }
    macro_rules! b {
        ($($b:expr),*$(,)?) => ( *bytes(&[$($b),*]) );
    }

    #[test]
    fn read_request_simple() {
        #[allow(clippy::cast_possible_truncation)]
        let in_header = fuse_in_header {
            len: (mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_init_in>()) as u32,
            opcode: kernel::FUSE_INIT,
            unique: 2,
            nodeid: 0,
            uid: 100,
            gid: 100,
            pid: 12,
            padding: 0,
        };
        let init_in = fuse_init_in {
            major: 7,
            minor: 23,
            max_readahead: 40,
            flags: kernel::FUSE_AUTO_INVAL_DATA | kernel::FUSE_DO_READDIRPLUS,
        };

        let mut input = vec![];
        input.extend_from_slice(unsafe { crate::util::as_bytes(&in_header) });
        input.extend_from_slice(unsafe { crate::util::as_bytes(&init_in) });

        let mut reader = &input[..];

        let request = block_on(reader.read_request()).expect("parser failed");
        assert_eq!(request.header.len, in_header.len);
        assert_eq!(request.header.opcode, in_header.opcode);
        assert_eq!(request.header.unique, in_header.unique);
        assert_eq!(request.header.nodeid, in_header.nodeid);
        assert_eq!(request.header.uid, in_header.uid);
        assert_eq!(request.header.gid, in_header.gid);
        assert_eq!(request.header.pid, in_header.pid);

        let op = Parser::new(&request.header, &*request.arg)
            .parse()
            .expect("failed to parse argument");
        match op {
            OperationKind::Init { arg, .. } => {
                assert_eq!(arg.major, init_in.major);
                assert_eq!(arg.minor, init_in.minor);
                assert_eq!(arg.max_readahead, init_in.max_readahead);
                assert_eq!(arg.flags, init_in.flags);
            }
            _ => panic!("operation mismachd"),
        }
    }

    #[test]
    fn read_request_too_short() {
        #[allow(clippy::cast_possible_truncation)]
        let in_header = fuse_in_header {
            len: (mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_init_in>()) as u32,
            opcode: kernel::FUSE_INIT,
            unique: 2,
            nodeid: 0,
            uid: 100,
            gid: 100,
            pid: 12,
            padding: 0,
        };

        let mut input = vec![];
        input.extend_from_slice(unsafe { &crate::util::as_bytes(&in_header)[0..10] });

        let mut reader = &input[..];

        assert!(
            block_on(reader.read_request()).is_err(),
            "parser should fail"
        );
    }

    pin_project! {
        #[derive(Default)]
        struct DummyWriter {
            #[pin]
            vec: Vec<u8>,
        }
    }

    impl<I> Index<I> for DummyWriter
    where
        Vec<u8>: Index<I>,
    {
        type Output = <Vec<u8> as Index<I>>::Output;

        fn index(&self, index: I) -> &Self::Output {
            self.vec.index(index)
        }
    }

    impl AsyncWrite for DummyWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            src: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.project().vec.poll_write(cx, src)
        }

        fn poll_write_vectored(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            src: &[IoSlice<'_>],
        ) -> Poll<io::Result<usize>> {
            self.project().vec.poll_write_vectored(cx, src)
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
            self.project().vec.poll_flush(cx)
        }

        fn poll_close(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
            self.project().vec.poll_close(cx)
        }
    }

    #[test]
    fn send_msg_empty() {
        let mut writer = DummyWriter::default();
        block_on(writer.send_msg(42, 4, &[])).unwrap();
        assert_eq!(writer[0..4], b![0x10, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(writer[4..8], b![0x04, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            writer[8..16],
            b![0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
    }

    #[test]
    fn send_msg_single_data() {
        let mut writer = DummyWriter::default();
        block_on(writer.send_msg(42, 0, "hello")).unwrap();
        assert_eq!(writer[0..4], b![0x15, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(writer[4..8], b![0x00, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            writer[8..16],
            b![0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
        assert_eq!(writer[16..], b![0x68, 0x65, 0x6c, 0x6c, 0x6f], "payload");
    }

    #[test]
    fn send_msg_chunked_data() {
        let payload: &[&[u8]] = &[
            "hello, ".as_ref(), //
            "this ".as_ref(),
            "is a ".as_ref(),
            "message.".as_ref(),
        ];
        let mut writer = DummyWriter::default();
        block_on(writer.send_msg(26, 0, payload)).unwrap();
        assert_eq!(writer[0..4], b![0x29, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(writer[4..8], b![0x00, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            writer[8..16],
            b![0x1a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
        assert_eq!(writer[16..], *b"hello, this is a message.", "payload");
    }
}

// ==== reply ====

/// A trait that represents the data structure contained in the reply sent to the kernel.
///
/// The trait is roughly a generalization of `AsRef<[u8]>`, representing *scattered* bytes
/// that are not necessarily in contiguous memory space.
pub trait Reply {
    /// Collect the *scattered* bytes in the `collector`.
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>;
}

// ==== pointer types ====

macro_rules! impl_reply_body_for_pointers {
    () => {
        #[inline]
        fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
        where
            T: Collector<'a>,
        {
            (**self).collect_bytes(collector)
        }
    };
}

impl<R: ?Sized> Reply for &R
where
    R: Reply,
{
    impl_reply_body_for_pointers!();
}

impl<R: ?Sized> Reply for &mut R
where
    R: Reply,
{
    impl_reply_body_for_pointers!();
}

impl<R: ?Sized> Reply for Box<R>
where
    R: Reply,
{
    impl_reply_body_for_pointers!();
}

impl<R: ?Sized> Reply for std::rc::Rc<R>
where
    R: Reply,
{
    impl_reply_body_for_pointers!();
}

impl<R: ?Sized> Reply for std::sync::Arc<R>
where
    R: Reply,
{
    impl_reply_body_for_pointers!();
}

// ==== empty bytes ====

impl Reply for () {
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, _: &mut T)
    where
        T: Collector<'a>,
    {
    }
}

impl Reply for [u8; 0] {
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, _: &mut T)
    where
        T: Collector<'a>,
    {
    }
}

// ==== compound types ====

macro_rules! impl_reply_for_tuple {
    ($($T:ident),+ $(,)?) => {
        impl<$($T),+> Reply for ($($T,)+)
        where
            $( $T: Reply, )+
        {
            #[allow(nonstandard_style)]
            #[inline]
            fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
            where
                T: Collector<'a>,
            {
                let ($($T,)+) = self;
                $(
                    $T.collect_bytes(collector);
                )+
            }
        }
    }
}

impl_reply_for_tuple!(T1);
impl_reply_for_tuple!(T1, T2);
impl_reply_for_tuple!(T1, T2, T3);
impl_reply_for_tuple!(T1, T2, T3, T4);
impl_reply_for_tuple!(T1, T2, T3, T4, T5);

impl<R> Reply for [R]
where
    R: Reply,
{
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        for t in self {
            t.collect_bytes(collector);
        }
    }
}

impl<R> Reply for Vec<R>
where
    R: Reply,
{
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        for t in self {
            t.collect_bytes(collector);
        }
    }
}

// ==== Option<T> ====

impl<T> Reply for Option<T>
where
    T: Reply,
{
    #[inline]
    fn collect_bytes<'a, C: ?Sized>(&'a self, collector: &mut C)
    where
        C: Collector<'a>,
    {
        if let Some(ref reply) = self {
            reply.collect_bytes(collector);
        }
    }
}

// ==== continuous bytes ====

mod impl_scattered_bytes_for_cont {
    use super::*;

    #[inline(always)]
    fn as_bytes(t: &(impl AsRef<[u8]> + ?Sized)) -> &[u8] {
        t.as_ref()
    }

    macro_rules! impl_reply {
        ($($t:ty),*$(,)?) => {$(
            impl Reply for $t {
                #[inline]
                fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
                where
                    T: Collector<'a>,
                {
                    let this = as_bytes(self);
                    if !this.is_empty() {
                        collector.append(this);
                    }
                }
            }
        )*};
    }

    impl_reply! {
        [u8],
        str,
        String,
        Vec<u8>,
        std::borrow::Cow<'_, [u8]>,
    }
}

impl Reply for OsStr {
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        self.as_bytes().collect_bytes(collector)
    }
}

impl Reply for OsString {
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        (**self).collect_bytes(collector)
    }
}

impl Reply for std::path::Path {
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        self.as_os_str().collect_bytes(collector)
    }
}

impl Reply for std::path::PathBuf {
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        (**self).collect_bytes(collector)
    }
}

/// Container for collecting the scattered bytes.
pub trait Collector<'a> {
    /// Append a chunk of bytes into itself.
    fn append(&mut self, buf: &'a [u8]);
}

macro_rules! impl_reply {
    ($t:ty) => {
        impl Reply for $t {
            #[inline]
            fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
            where
                T: Collector<'a>,
            {
                collector.append(unsafe { crate::util::as_bytes(self) })
            }
        }
    };
}

/// Reply with the inode attributes.
#[must_use]
pub struct ReplyAttr(kernel::fuse_attr_out);

impl_reply!(ReplyAttr);

impl ReplyAttr {
    fn new(attr: &dyn op::ReplyAttr) -> Self {
        todo!()
    }
}

/// Reply with entry params.
#[must_use]
pub struct ReplyEntry(kernel::fuse_entry_out);

impl_reply!(ReplyEntry);

impl ReplyEntry {
    fn new(entry: &dyn op::ReplyEntry) -> Self {
        todo!()
    }
}

/// Reply with an opened file.
#[must_use]
pub struct ReplyOpen(kernel::fuse_open_out);

impl_reply!(ReplyOpen);

impl ReplyOpen {
    fn new(open: &dyn op::ReplyOpen) -> Self {
        todo!()
    }
}

/// Reply with the information about written data.
#[must_use]
pub struct ReplyWrite(kernel::fuse_write_out);

impl_reply!(ReplyWrite);

/// Reply to a request about extended attributes.
#[must_use]
pub struct ReplyXattr(kernel::fuse_getxattr_out);

impl_reply!(ReplyXattr);

/// Reply with the filesystem staticstics.
#[must_use]
pub struct ReplyStatfs(kernel::fuse_statfs_out);

impl_reply!(ReplyStatfs);

/// Reply with a file lock.
#[must_use]
pub struct ReplyLk(kernel::fuse_lk_out);

impl_reply!(ReplyLk);

/// Reply with the mapped block index.
#[must_use]
pub struct ReplyBmap(kernel::fuse_bmap_out);

impl_reply!(ReplyBmap);

/// Reply with the poll result.
#[must_use]
pub struct ReplyPoll(kernel::fuse_poll_out);

impl_reply!(ReplyPoll);
