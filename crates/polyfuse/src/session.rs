//! Establish a FUSE session.

use crate::{
    request::parse::{Arg, Request},
    write,
};
use bitflags::bitflags;
use futures::io::{AsyncRead, AsyncReadExt as _};
use polyfuse_kernel as kernel;
use std::{
    fmt, io,
    sync::atomic::{AtomicBool, Ordering},
};

// The minimum supported ABI minor version by polyfuse.
const MINIMUM_SUPPORTED_MINOR_VERSION: u32 = 23;

const DEFAULT_MAX_WRITE: u32 = 16 * 1024 * 1024;
//const MIN_MAX_WRITE: u32 = kernel::FUSE_MIN_READ_BUFFER - BUFFER_HEADER_SIZE as u32;

// copied from fuse_i.h
const MAX_MAX_PAGES: usize = 256;
//const DEFAULT_MAX_PAGES_PER_REQ: usize = 32;
const BUFFER_HEADER_SIZE: usize = 0x1000;

#[inline]
fn pagesize() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

/// Information about the connection associated with a session.
pub struct ConnectionInfo(kernel::fuse_init_out);

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
            .field("max_pages", &self.max_pages())
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
        self.0.flags & kernel::FUSE_NO_OPEN_SUPPORT != 0
    }

    /// Return whether the kernel supports for zero-message opendirs.
    ///
    /// See the documentation of `no_open_support` for details.
    pub fn no_opendir_support(&self) -> bool {
        self.0.flags & kernel::FUSE_NO_OPENDIR_SUPPORT != 0
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
        if self.0.flags & kernel::FUSE_MAX_PAGES != 0 {
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

pub struct SessionInitializer {
    pub(crate) max_readahead: u32,
    pub(crate) flags: CapabilityFlags,
    pub(crate) max_background: u16,
    pub(crate) congestion_threshold: u16,
    pub(crate) max_write: u32,
    pub(crate) time_gran: u32,
    #[allow(dead_code)]
    pub(crate) max_pages: u16,
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
    pub async fn init<'w, R, W: ?Sized>(&self, reader: R, writer: &'w W) -> io::Result<Session>
    where
        R: AsyncRead,
        &'w W: io::Write,
    {
        let init_buf_size = BUFFER_HEADER_SIZE + pagesize() * MAX_MAX_PAGES;
        let mut buf = vec![0u8; init_buf_size];

        futures::pin_mut!(reader);

        loop {
            reader.read(&mut buf[..]).await?;
            match self.try_init(&buf[..], writer).await? {
                Some(session) => return Ok(session),
                None => continue,
            }
        }
    }

    #[allow(clippy::cognitive_complexity)]
    async fn try_init<W>(&self, buf: &[u8], writer: W) -> io::Result<Option<Session>>
    where
        W: io::Write,
    {
        let Request { header, arg, .. } = Request::parse(buf)?;
        match arg {
            Arg::Init { arg: init_in } => {
                let capable = CapabilityFlags::from_bits_truncate(init_in.flags);
                let readonly_flags = init_in.flags & !CapabilityFlags::all().bits();
                tracing::debug!("INIT request:");
                tracing::debug!("  proto = {}.{}:", init_in.major, init_in.minor);
                tracing::debug!("  flags = 0x{:08x} ({:?})", init_in.flags, capable);
                tracing::debug!("  max_readahead = 0x{:08X}", init_in.max_readahead);
                tracing::debug!(
                    "  max_pages = {}",
                    init_in.flags & kernel::FUSE_MAX_PAGES != 0
                );
                tracing::debug!(
                    "  no_open_support = {}",
                    init_in.flags & kernel::FUSE_NO_OPEN_SUPPORT != 0
                );
                tracing::debug!(
                    "  no_opendir_support = {}",
                    init_in.flags & kernel::FUSE_NO_OPENDIR_SUPPORT != 0
                );

                let mut init_out = kernel::fuse_init_out::default();
                init_out.major = kernel::FUSE_KERNEL_VERSION;
                init_out.minor = kernel::FUSE_KERNEL_MINOR_VERSION;

                if init_in.major > 7 {
                    tracing::debug!("wait for a second INIT request with an older version.");
                    write::send_reply(writer, header.unique, unsafe {
                        crate::util::as_bytes(&init_out)
                    })?;
                    return Ok(None);
                }

                if init_in.major < 7 || init_in.minor < MINIMUM_SUPPORTED_MINOR_VERSION {
                    tracing::warn!(
                        "polyfuse supports only ABI 7.{} or later. {}.{} is not supported",
                        MINIMUM_SUPPORTED_MINOR_VERSION,
                        init_in.major,
                        init_in.minor
                    );
                    write::send_error(writer, header.unique, libc::EPROTO)?;
                    return Ok(None);
                }

                init_out.minor = std::cmp::min(init_out.minor, init_in.minor);

                init_out.flags = (self.flags & capable).bits();
                init_out.flags |= kernel::FUSE_BIG_WRITES; // the flag was superseded by `max_write`.

                init_out.max_readahead = std::cmp::min(self.max_readahead, init_in.max_readahead);
                init_out.max_write = self.max_write;
                init_out.max_background = self.max_background;
                init_out.congestion_threshold = self.congestion_threshold;
                init_out.time_gran = self.time_gran;

                if init_in.flags & kernel::FUSE_MAX_PAGES != 0 {
                    init_out.flags |= kernel::FUSE_MAX_PAGES;
                    init_out.max_pages = std::cmp::min(
                        (init_out.max_write - 1) / (pagesize() as u32) + 1,
                        u16::max_value() as u32,
                    ) as u16;
                }

                debug_assert_eq!(init_out.major, kernel::FUSE_KERNEL_VERSION);
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
                write::send_reply(writer, header.unique, unsafe {
                    crate::util::as_bytes(&init_out)
                })?;

                init_out.flags |= readonly_flags;

                let conn = ConnectionInfo(init_out);
                let bufsize = BUFFER_HEADER_SIZE + conn.max_write() as usize;

                Ok(Some(Session {
                    conn,
                    bufsize,
                    exited: AtomicBool::new(false),
                }))
            }

            _ => {
                tracing::warn!(
                    "ignoring an operation before init (opcode={:?})",
                    header.opcode
                );
                write::send_error(writer, header.unique, libc::EIO)?;
                Ok(None)
            }
        }
    }
}

pub struct Session {
    #[allow(dead_code)]
    conn: ConnectionInfo,
    bufsize: usize,
    exited: AtomicBool,
}

impl Session {
    #[inline]
    pub fn buffer_size(&self) -> usize {
        self.bufsize
    }

    #[inline]
    pub fn exited(&self) -> bool {
        // FIXME: choose appropriate atomic ordering.
        self.exited.load(Ordering::SeqCst)
    }

    #[inline]
    pub fn exit(&self) {
        // FIXME: choose appropriate atomic ordering.
        self.exited.store(true, Ordering::SeqCst)
    }
}
