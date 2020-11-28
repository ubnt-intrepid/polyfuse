//! Establish a FUSE session.

use crate::request::Request;
use crate::{util::Decoder, write};
use bitflags::bitflags;
use futures::io::{AsyncRead, AsyncReadExt as _};
use polyfuse_kernel::{self as kernel, fuse_opcode};
use std::{
    convert::TryFrom,
    fmt, io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
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

pub struct Config {
    max_readahead: u32,
    flags: CapabilityFlags,
    max_background: u16,
    congestion_threshold: u16,
    max_write: u32,
    time_gran: u32,
    #[allow(dead_code)]
    max_pages: u16,
}

impl Default for Config {
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

impl Config {
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
    // ///
    // /// # Panic
    // /// It causes an assertion panic if the setting value is
    // /// less than the absolute minimum.
    pub fn max_write(&mut self, value: u32) -> &mut Self {
        // assert!(
        //     value >= MIN_MAX_WRITE,
        //     "max_write must be greater or equal to {}",
        //     MIN_MAX_WRITE,
        // );
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
}

/// The object containing the contextrual information about a FUSE session.
#[derive(Debug)]
pub struct Session {
    conn: ConnectionInfo,
    bufsize: usize,
    exited: AtomicBool,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.exit();
    }
}

impl Session {
    #[inline]
    pub(crate) fn exited(&self) -> bool {
        // FIXME: choose appropriate atomic ordering.
        self.exited.load(Ordering::SeqCst)
    }

    #[inline]
    pub(crate) fn exit(&self) {
        // FIXME: choose appropriate atomic ordering.
        self.exited.store(true, Ordering::SeqCst)
    }

    /// Returns the information about the FUSE connection.
    #[inline]
    pub fn connection_info(&self) -> &ConnectionInfo {
        &self.conn
    }

    /// Start a FUSE daemon mount on the specified path.
    pub async fn start<T>(conn: T, config: Config) -> io::Result<Arc<Self>>
    where
        T: AsyncRead + io::Write + Unpin,
    {
        init(conn, config).await.map(Arc::new)
    }

    /// Receive an incoming FUSE request from the kernel.
    pub async fn next_request<T>(self: &Arc<Self>, conn: T) -> io::Result<Option<Request>>
    where
        T: AsyncRead + Unpin,
    {
        let mut conn = conn;

        let mut buf = vec![0u8; self.bufsize];

        loop {
            match conn.read(&mut buf[..]).await {
                Ok(len) => {
                    unsafe {
                        buf.set_len(len);
                    }
                    break;
                }

                Err(err) => match err.raw_os_error() {
                    Some(libc::ENODEV) => {
                        tracing::debug!("ENODEV");
                        return Ok(None);
                    }
                    Some(libc::ENOENT) => {
                        tracing::debug!("ENOENT");
                        continue;
                    }
                    _ => return Err(err),
                },
            }
        }

        Ok(Some(Request {
            buf,
            session: self.clone(),
        }))
    }
}

async fn init<T>(mut conn: T, config: Config) -> io::Result<Session>
where
    T: AsyncRead + io::Write + Unpin,
{
    let init_buf_size = BUFFER_HEADER_SIZE + pagesize() * MAX_MAX_PAGES;
    let mut buf = vec![0u8; init_buf_size];

    for _ in 0..10 {
        conn.read(&mut buf[..]).await?;
        match try_init(&config, &buf[..], &mut conn).await? {
            Some(session) => return Ok(session),
            None => continue,
        }
    }

    Err(io::Error::new(
        io::ErrorKind::ConnectionRefused,
        "session initialization is aborted",
    ))
}

#[allow(clippy::cognitive_complexity)]
async fn try_init<W>(config: &Config, buf: &[u8], writer: W) -> io::Result<Option<Session>>
where
    W: io::Write,
{
    let mut decoder = Decoder::new(buf);
    let header = decoder
        .fetch::<kernel::fuse_in_header>() //
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "failed to decode fuse_in_header"))?;

    match fuse_opcode::try_from(header.opcode) {
        Ok(fuse_opcode::FUSE_INIT) => {
            let init_in = decoder
                .fetch::<kernel::fuse_init_in>() //
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::Other, "failed to decode fuse_init_in")
                })?;

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

            init_out.flags = (config.flags & capable).bits();
            init_out.flags |= kernel::FUSE_BIG_WRITES; // the flag was superseded by `max_write`.

            init_out.max_readahead = std::cmp::min(config.max_readahead, init_in.max_readahead);
            init_out.max_write = config.max_write;
            init_out.max_background = config.max_background;
            init_out.congestion_threshold = config.congestion_threshold;
            init_out.time_gran = config.time_gran;

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

#[cfg(test)]
mod tests {
    use super::*;

    use futures::{
        executor::block_on,
        io::AsyncRead,
        task::{self, Poll},
    };
    use pin_project_lite::pin_project;
    use polyfuse_kernel::{
        fuse_in_header, fuse_init_in, fuse_init_out, fuse_opcode, fuse_out_header,
    };
    use std::{
        io::{self, Write},
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
            buf: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            self.project().reader.poll_read(cx, buf)
        }

        fn poll_read_vectored(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            bufs: &mut [io::IoSliceMut<'_>],
        ) -> Poll<io::Result<usize>> {
            self.project().reader.poll_read_vectored(cx, bufs)
        }
    }

    impl<R, W> Write for Unite<R, W>
    where
        W: Write,
    {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writer.write(buf)
        }

        fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
            self.writer.write_vectored(bufs)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.writer.flush()
        }
    }

    #[test]
    fn init_default() {
        let input_len = mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_init_in>();
        let in_header = fuse_in_header {
            len: input_len as u32,
            opcode: fuse_opcode::FUSE_INIT as u32,
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
                | kernel::FUSE_MAX_PAGES
                | kernel::FUSE_NO_OPEN_SUPPORT
                | kernel::FUSE_NO_OPENDIR_SUPPORT,
        };

        let mut input = Vec::with_capacity(input_len);
        input.extend_from_slice(unsafe { crate::util::as_bytes(&in_header) });
        input.extend_from_slice(unsafe { crate::util::as_bytes(&init_in) });
        assert_eq!(input.len(), input_len);

        let mut output = Vec::<u8>::new();

        let session = block_on(Session::start(
            Unite {
                reader: &input[..],
                writer: &mut output,
            },
            Config::default(),
        ))
        .expect("initialization failed");

        let expected_max_pages = (DEFAULT_MAX_WRITE / (pagesize() as u32)) as u16;

        let output_len = mem::size_of::<fuse_out_header>() + mem::size_of::<fuse_init_out>();
        let out_header = fuse_out_header {
            len: output_len as u32,
            error: 0,
            unique: 2,
        };
        let init_out = fuse_init_out {
            major: 7,
            minor: 23,
            max_readahead: 40,
            flags: CapabilityFlags::default().bits()
                | kernel::FUSE_MAX_PAGES
                | kernel::FUSE_BIG_WRITES,
            max_background: 0,
            congestion_threshold: 0,
            max_write: DEFAULT_MAX_WRITE,
            time_gran: 1,
            max_pages: expected_max_pages,
            padding: 0,
            unused: [0; 8],
        };

        let mut expected = Vec::with_capacity(output_len);
        expected.extend_from_slice(unsafe { crate::util::as_bytes(&out_header) });
        expected.extend_from_slice(unsafe { crate::util::as_bytes(&init_out) });
        assert_eq!(output.len(), output_len);

        assert_eq!(expected[0..4], output[0..4], "out_header.len");
        assert_eq!(expected[4..8], output[4..8], "out_header.error");
        assert_eq!(expected[8..16], output[8..16], "out_header.unique");

        let expected = &expected[mem::size_of::<fuse_out_header>()..];
        let output = &output[mem::size_of::<fuse_out_header>()..];
        assert_eq!(expected[0..4], output[0..4], "init_out.major");
        assert_eq!(expected[4..8], output[4..8], "init_out.minor");
        assert_eq!(expected[8..12], output[8..12], "init_out.max_readahead");
        assert_eq!(expected[12..16], output[12..16], "init_out.flags");
        assert_eq!(expected[16..18], output[16..18], "init_out.max_background");
        assert_eq!(
            expected[18..20],
            output[18..20],
            "init_out.congestion_threshold"
        );
        assert_eq!(expected[20..24], output[20..24], "init_out.max_write");
        assert_eq!(expected[24..28], output[24..28], "init_out.time_gran");
        assert_eq!(expected[28..30], output[28..30], "init_out.max_pages");
        assert!(
            output[30..30 + 2 + 4 * 8].iter().all(|&b| b == 0x00),
            "init_out.paddings"
        );

        let conn = &session.conn;
        assert_eq!(conn.proto_major(), 7);
        assert_eq!(conn.proto_minor(), 23);
        assert_eq!(conn.max_readahead(), 40);
        assert_eq!(conn.max_background(), 0);
        assert_eq!(conn.congestion_threshold(), 0);
        assert_eq!(conn.max_write(), DEFAULT_MAX_WRITE);
        assert_eq!(conn.max_pages(), Some(expected_max_pages));
        assert_eq!(conn.time_gran(), 1);
        assert!(conn.no_open_support());
        assert!(conn.no_opendir_support());
    }
}
