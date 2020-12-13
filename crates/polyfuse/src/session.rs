//! Establish a FUSE session.

use crate::{
    bytes::write_bytes,
    conn::{Connection, MountOptions},
    decoder::Decoder,
    op::{DecodeError, Operation},
    reply::Reply,
};
use bitflags::bitflags;
use polyfuse_kernel::*;
use std::{
    convert::TryFrom,
    fmt,
    io::{self, prelude::*, IoSlice, IoSliceMut},
    mem,
    os::unix::prelude::*,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use zerocopy::AsBytes as _;

// The minimum supported ABI minor version by polyfuse.
const MINIMUM_SUPPORTED_MINOR_VERSION: u32 = 23;

const DEFAULT_MAX_WRITE: u32 = 16 * 1024 * 1024;
//const MIN_MAX_WRITE: u32 = FUSE_MIN_READ_BUFFER - BUFFER_HEADER_SIZE as u32;

// copied from fuse_i.h
const MAX_MAX_PAGES: usize = 256;
//const DEFAULT_MAX_PAGES_PER_REQ: usize = 32;
const BUFFER_HEADER_SIZE: usize = 0x1000;

#[inline]
fn pagesize() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

/// Information about the connection associated with a session.
pub struct ConnectionInfo {
    out: fuse_init_out,
    bufsize: usize,
}

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
            .field("bufsize", &self.bufsize)
            .finish()
    }
}

impl ConnectionInfo {
    /// Returns the major version of the protocol.
    pub fn proto_major(&self) -> u32 {
        self.out.major
    }

    /// Returns the minor version of the protocol.
    pub fn proto_minor(&self) -> u32 {
        self.out.minor
    }

    /// Return a set of capability flags sent to the kernel driver.
    pub fn flags(&self) -> CapabilityFlags {
        CapabilityFlags::from_bits_truncate(self.out.flags)
    }

    /// Return whether the kernel supports for zero-message opens.
    ///
    /// When the returned value is `true`, the kernel treat an `ENOSYS`
    /// error for a `FUSE_OPEN` request as successful and does not send
    /// subsequent `open` requests.  Otherwise, the filesystem should
    /// implement the handler for `open` requests appropriately.
    pub fn no_open_support(&self) -> bool {
        self.out.flags & FUSE_NO_OPEN_SUPPORT != 0
    }

    /// Return whether the kernel supports for zero-message opendirs.
    ///
    /// See the documentation of `no_open_support` for details.
    pub fn no_opendir_support(&self) -> bool {
        self.out.flags & FUSE_NO_OPENDIR_SUPPORT != 0
    }

    /// Returns the maximum readahead.
    pub fn max_readahead(&self) -> u32 {
        self.out.max_readahead
    }

    /// Returns the maximum size of the write buffer.
    pub fn max_write(&self) -> u32 {
        self.out.max_write
    }

    #[doc(hidden)]
    pub fn max_background(&self) -> u16 {
        self.out.max_background
    }

    #[doc(hidden)]
    pub fn congestion_threshold(&self) -> u16 {
        self.out.congestion_threshold
    }

    #[doc(hidden)]
    pub fn time_gran(&self) -> u32 {
        self.out.time_gran
    }

    #[doc(hidden)]
    pub fn max_pages(&self) -> Option<u16> {
        if self.out.flags & FUSE_MAX_PAGES != 0 {
            Some(self.out.max_pages)
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
        const ASYNC_READ = FUSE_ASYNC_READ;

        /// The filesystem supports the `O_TRUNC` open flag.
        ///
        /// Enabled by default.
        const ATOMIC_O_TRUNC = FUSE_ATOMIC_O_TRUNC;

        /// The kernel check the validity of attributes on every read.
        ///
        /// Enabled by default.
        const AUTO_INVAL_DATA = FUSE_AUTO_INVAL_DATA;

        /// The filesystem supports asynchronous direct I/O submission.
        ///
        /// Enabled by default.
        const ASYNC_DIO = FUSE_ASYNC_DIO;

        /// The kernel supports parallel directory operations.
        ///
        /// Enabled by default.
        const PARALLEL_DIROPS = FUSE_PARALLEL_DIROPS;

        /// The filesystem is responsible for unsetting setuid and setgid bits
        /// when a file is written, truncated, or its owner is changed.
        ///
        /// Enabled by default.
        const HANDLE_KILLPRIV = FUSE_HANDLE_KILLPRIV;

        /// The filesystem supports the POSIX-style file lock.
        const POSIX_LOCKS = FUSE_POSIX_LOCKS;

        /// The filesystem supports the `flock` handling.
        const FLOCK_LOCKS = FUSE_FLOCK_LOCKS;

        /// The filesystem supports lookups of `"."` and `".."`.
        const EXPORT_SUPPORT = FUSE_EXPORT_SUPPORT;

        /// The kernel should not apply the umask to the file mode on create
        /// operations.
        const DONT_MASK = FUSE_DONT_MASK;

        /// The writeback caching should be enabled.
        const WRITEBACK_CACHE = FUSE_WRITEBACK_CACHE;

        /// The filesystem supports POSIX access control lists.
        const POSIX_ACL = FUSE_POSIX_ACL;

        /// The filesystem supports `readdirplus` operations.
        const READDIRPLUS = FUSE_DO_READDIRPLUS;

        /// Indicates that the kernel uses the adaptive readdirplus.
        const READDIRPLUS_AUTO = FUSE_READDIRPLUS_AUTO;

        // TODO: splice read/write
        // const SPLICE_WRITE = FUSE_SPLICE_WRITE;
        // const SPLICE_MOVE = FUSE_SPLICE_MOVE;
        // const SPLICE_READ = FUSE_SPLICE_READ;

        // TODO: ioctl
        // const IOCTL_DIR = FUSE_IOCTL_DIR;
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
    inner: Arc<SessionInner>,
}

#[derive(Debug)]
struct SessionInner {
    conn: Connection,
    conn_info: ConnectionInfo,
    exited: AtomicBool,
}

impl SessionInner {
    #[inline]
    fn exited(&self) -> bool {
        // FIXME: choose appropriate atomic ordering.
        self.exited.load(Ordering::SeqCst)
    }

    #[inline]
    fn exit(&self) {
        // FIXME: choose appropriate atomic ordering.
        self.exited.store(true, Ordering::SeqCst)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.inner.exit();
    }
}

impl AsRawFd for Session {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.conn.as_raw_fd()
    }
}

impl Session {
    /// Start a FUSE daemon mount on the specified path.
    pub fn mount(mountpoint: PathBuf, mountopts: MountOptions, config: Config) -> io::Result<Self> {
        let conn = Connection::open(mountpoint, mountopts)?;
        let conn_info = init_session(&conn, &conn, config)?;

        Ok(Self {
            inner: Arc::new(SessionInner {
                conn,
                conn_info,
                exited: AtomicBool::new(false),
            }),
        })
    }

    /// Returns the information about the FUSE connection.
    #[inline]
    pub fn connection_info(&self) -> &ConnectionInfo {
        &self.inner.conn_info
    }

    /// Receive an incoming FUSE request from the kernel.
    pub fn next_request(&self) -> io::Result<Option<Request>> {
        let mut conn = &self.inner.conn;

        // FIXME: Align the allocated region in `arg` with the FUSE argument types.
        let mut header = fuse_in_header::default();
        let mut arg = vec![0u8; self.inner.conn_info.bufsize - mem::size_of::<fuse_in_header>()];

        loop {
            match conn.read_vectored(&mut [
                io::IoSliceMut::new(header.as_bytes_mut()),
                io::IoSliceMut::new(&mut arg[..]),
            ]) {
                Ok(len) => {
                    if len < mem::size_of::<fuse_in_header>() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "dequeued request message is too short",
                        ));
                    }
                    unsafe {
                        arg.set_len(len - mem::size_of::<fuse_in_header>());
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
            session: self.inner.clone(),
            header,
            arg,
        }))
    }
}

/// Context about an incoming FUSE request.
pub struct Request {
    session: Arc<SessionInner>,
    header: fuse_in_header,
    arg: Vec<u8>,
}

impl Request {
    /// Return the unique ID of the request.
    #[inline]
    pub fn unique(&self) -> u64 {
        self.header.unique
    }

    /// Return the user ID of the calling process.
    #[inline]
    pub fn uid(&self) -> u32 {
        self.header.uid
    }

    /// Return the group ID of the calling process.
    #[inline]
    pub fn gid(&self) -> u32 {
        self.header.gid
    }

    /// Return the process ID of the calling process.
    #[inline]
    pub fn pid(&self) -> u32 {
        self.header.pid
    }

    /// Decode the argument of this request.
    pub fn operation(&self) -> Result<Operation<'_, Data<'_>>, DecodeError> {
        if self.session.exited() {
            return Ok(Operation::unknown());
        }

        let (arg, data) = match fuse_opcode::try_from(self.header.opcode).ok() {
            Some(fuse_opcode::FUSE_WRITE) | Some(fuse_opcode::FUSE_NOTIFY_REPLY) => {
                self.arg.split_at(mem::size_of::<fuse_write_in>())
            }
            _ => (&self.arg[..], &[] as &[_]),
        };

        Operation::decode(&self.header, arg, Data { data })
    }
}

impl io::Write for Request {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self).write(buf)
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        (&*self).write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self).flush()
    }
}

impl io::Write for &Request {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.session.exited() {
            return Ok(0);
        }
        (&self.session.conn).write(buf)
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        if self.session.exited() {
            return Ok(0);
        }
        (&self.session.conn).write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The remaining part of request message.
pub struct Data<'op> {
    data: &'op [u8],
}

impl fmt::Debug for Data<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Data").finish()
    }
}

impl<'op> io::Read for Data<'op> {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        io::Read::read(&mut self.data, buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        io::Read::read_vectored(&mut self.data, bufs)
    }
}

impl<'op> BufRead for Data<'op> {
    #[inline]
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        io::BufRead::fill_buf(&mut self.data)
    }

    #[inline]
    fn consume(&mut self, amt: usize) {
        io::BufRead::consume(&mut self.data, amt)
    }
}

fn init_session<R, W>(mut reader: R, mut writer: W, config: Config) -> io::Result<ConnectionInfo>
where
    R: io::Read,
    W: io::Write,
{
    // FIXME: align the allocated buffer in `buf` with FUSE argument types.
    let mut header = fuse_in_header::default();
    let mut arg = vec![0u8; pagesize() * MAX_MAX_PAGES];

    for _ in 0..10 {
        let len = reader.read_vectored(&mut [
            io::IoSliceMut::new(header.as_bytes_mut()),
            io::IoSliceMut::new(&mut arg[..]),
        ])?;
        if len < mem::size_of::<fuse_in_header>() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request message is too short",
            ));
        }

        let mut decoder = Decoder::new(&arg[..]);

        match fuse_opcode::try_from(header.opcode) {
            Ok(fuse_opcode::FUSE_INIT) => {
                let init_in = decoder
                    .fetch::<fuse_init_in>() //
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::Other, "failed to decode fuse_init_in")
                    })?;

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
                    write_bytes(
                        &mut writer,
                        Reply::new(header.unique, 0, init_out.as_bytes()),
                    )?;
                    continue;
                }

                if init_in.major < 7 || init_in.minor < MINIMUM_SUPPORTED_MINOR_VERSION {
                    tracing::warn!(
                        "polyfuse supports only ABI 7.{} or later. {}.{} is not supported",
                        MINIMUM_SUPPORTED_MINOR_VERSION,
                        init_in.major,
                        init_in.minor
                    );
                    write_bytes(&mut writer, Reply::new(header.unique, libc::EPROTO, ()))?;
                    continue;
                }

                init_out.minor = std::cmp::min(init_out.minor, init_in.minor);

                init_out.flags = (config.flags & capable).bits();
                init_out.flags |= FUSE_BIG_WRITES; // the flag was superseded by `max_write`.

                init_out.max_readahead = std::cmp::min(config.max_readahead, init_in.max_readahead);
                init_out.max_write = config.max_write;
                init_out.max_background = config.max_background;
                init_out.congestion_threshold = config.congestion_threshold;
                init_out.time_gran = config.time_gran;

                if init_in.flags & FUSE_MAX_PAGES != 0 {
                    init_out.flags |= FUSE_MAX_PAGES;
                    init_out.max_pages = std::cmp::min(
                        (init_out.max_write - 1) / (pagesize() as u32) + 1,
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
                write_bytes(writer, Reply::new(header.unique, 0, init_out.as_bytes()))?;

                init_out.flags |= readonly_flags;

                let bufsize = BUFFER_HEADER_SIZE + init_out.max_write as usize;

                return Ok(ConnectionInfo {
                    out: init_out,
                    bufsize,
                });
            }

            _ => {
                tracing::warn!(
                    "ignoring an operation before init (opcode={:?})",
                    header.opcode
                );
                write_bytes(&mut writer, Reply::new(header.unique, libc::EIO, ()))?;
                continue;
            }
        }
    }

    Err(io::Error::new(
        io::ErrorKind::ConnectionRefused,
        "session initialization is aborted",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem;

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
                | FUSE_MAX_PAGES
                | FUSE_NO_OPEN_SUPPORT
                | FUSE_NO_OPENDIR_SUPPORT,
        };

        let mut input = Vec::with_capacity(input_len);
        input.extend_from_slice(in_header.as_bytes());
        input.extend_from_slice(init_in.as_bytes());
        assert_eq!(input.len(), input_len);

        let mut output = Vec::<u8>::new();

        let conn_info = init_session(&input[..], &mut output, Config::default()) //
            .expect("initialization failed");

        let expected_max_pages = (DEFAULT_MAX_WRITE / (pagesize() as u32)) as u16;
        assert_eq!(conn_info.proto_major(), 7);
        assert_eq!(conn_info.proto_minor(), 23);
        assert_eq!(conn_info.max_readahead(), 40);
        assert_eq!(conn_info.max_background(), 0);
        assert_eq!(conn_info.congestion_threshold(), 0);
        assert_eq!(conn_info.max_write(), DEFAULT_MAX_WRITE);
        assert_eq!(conn_info.max_pages(), Some(expected_max_pages));
        assert_eq!(conn_info.time_gran(), 1);
        assert!(conn_info.no_open_support());
        assert!(conn_info.no_opendir_support());

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
            flags: CapabilityFlags::default().bits() | FUSE_MAX_PAGES | FUSE_BIG_WRITES,
            max_background: 0,
            congestion_threshold: 0,
            max_write: DEFAULT_MAX_WRITE,
            time_gran: 1,
            max_pages: expected_max_pages,
            padding: 0,
            unused: [0; 8],
        };

        let mut expected = Vec::with_capacity(output_len);
        expected.extend_from_slice(out_header.as_bytes());
        expected.extend_from_slice(init_out.as_bytes());
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
    }
}
