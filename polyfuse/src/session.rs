//! Lowlevel interface to handle FUSE requests.

#![allow(clippy::needless_update)]
// FIXME: re-enable lints.
#![allow(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use crate::{
    common::StatFs,
    context::Context,
    fs::Filesystem,
    io::{Reader, ReaderExt, Writer, WriterExt},
    kernel::{
        fuse_init_out, //
        FUSE_KERNEL_MINOR_VERSION,
        FUSE_KERNEL_VERSION,
        FUSE_MAX_PAGES,
        FUSE_MIN_READ_BUFFER,
        FUSE_NO_OPENDIR_SUPPORT,
        FUSE_NO_OPEN_SUPPORT,
    },
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
    util::{as_bytes, BuilderExt},
};
use bitflags::bitflags;
use lazy_static::lazy_static;
use smallvec::SmallVec;
use std::{
    cmp,
    convert::TryFrom,
    ffi::OsStr,
    fmt, io, mem,
    os::unix::ffi::OsStrExt,
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
        I: Reader + Writer + Unpin,
    {
        let request = io.read_request().await?;

        let header = request.header();
        let arg = request.arg()?;

        match arg {
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
                init_out.flags |= crate::kernel::FUSE_BIG_WRITES; // the flag was superseded by `max_write`.

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
        const ASYNC_READ = crate::kernel::FUSE_ASYNC_READ;

        /// The filesystem supports the `O_TRUNC` open flag.
        ///
        /// Enabled by default.
        const ATOMIC_O_TRUNC = crate::kernel::FUSE_ATOMIC_O_TRUNC;

        /// The kernel check the validity of attributes on every read.
        ///
        /// Enabled by default.
        const AUTO_INVAL_DATA = crate::kernel::FUSE_AUTO_INVAL_DATA;

        /// The filesystem supports asynchronous direct I/O submission.
        ///
        /// Enabled by default.
        const ASYNC_DIO = crate::kernel::FUSE_ASYNC_DIO;

        /// The kernel supports parallel directory operations.
        ///
        /// Enabled by default.
        const PARALLEL_DIROPS = crate::kernel::FUSE_PARALLEL_DIROPS;

        /// The filesystem is responsible for unsetting setuid and setgid bits
        /// when a file is written, truncated, or its owner is changed.
        ///
        /// Enabled by default.
        const HANDLE_KILLPRIV = crate::kernel::FUSE_HANDLE_KILLPRIV;

        /// The filesystem supports the POSIX-style file lock.
        const POSIX_LOCKS = crate::kernel::FUSE_POSIX_LOCKS;

        /// The filesystem supports the `flock` handling.
        const FLOCK_LOCKS = crate::kernel::FUSE_FLOCK_LOCKS;

        /// The filesystem supports lookups of `"."` and `".."`.
        const EXPORT_SUPPORT = crate::kernel::FUSE_EXPORT_SUPPORT;

        /// The kernel should not apply the umask to the file mode on create
        /// operations.
        const DONT_MASK = crate::kernel::FUSE_DONT_MASK;

        /// The writeback caching should be enabled.
        const WRITEBACK_CACHE = crate::kernel::FUSE_WRITEBACK_CACHE;

        /// The filesystem supports POSIX access control lists.
        const POSIX_ACL = crate::kernel::FUSE_POSIX_ACL;

        /// The filesystem supports `readdirplus` operations.
        const READDIRPLUS = crate::kernel::FUSE_DO_READDIRPLUS;

        /// Indicates that the kernel uses the adaptive readdirplus.
        const READDIRPLUS_AUTO = crate::kernel::FUSE_READDIRPLUS_AUTO;

        // TODO: splice read/write
        // const SPLICE_WRITE = crate::kernel::FUSE_SPLICE_WRITE;
        // const SPLICE_MOVE = crate::kernel::FUSE_SPLICE_MOVE;
        // const SPLICE_READ = crate::kernel::FUSE_SPLICE_READ;

        // TODO: ioctl
        // const IOCTL_DIR = crate::kernel::FUSE_IOCTL_DIR;
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
        T: Reader + Writer + Send + Unpin,
    {
        if self.exited() {
            tracing::warn!("The sesson has already been exited");
            return Ok(());
        }

        let request = io.read_request().await?;
        let header = request.header();
        let arg = request.arg()?;

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
                        cx.reply(crate::reply::ReplyStatfs::new(st)).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{fuse_in_header, fuse_init_in};
    use futures::executor::block_on;
    use std::mem;

    #[test]
    fn init_default() {
        #[allow(clippy::cast_possible_truncation)]
        let in_header = fuse_in_header {
            len: (mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_init_in>()) as u32,
            opcode: crate::kernel::FUSE_INIT,
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
        let mut io = crate::io::unite(&mut reader, &mut writer);

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
