use crate::{
    bytes::{Bytes, Decoder, POD},
    io::SpliceRead,
    raw::{request::RequestBuf, FallbackBuf},
    types::RequestID,
};
use polyfuse_kernel::*;
use rustix::{io::Errno, param::page_size};
use std::{
    cmp, io, mem,
    sync::atomic::{AtomicU32, Ordering},
};
use zerocopy::{FromZeros as _, IntoBytes as _};

// The minimum supported ABI minor version by polyfuse.
const MINIMUM_SUPPORTED_MINOR_VERSION: u32 = 23;

const DEFAULT_MAX_PAGES_PER_REQ: usize = 32;
const MAX_MAX_PAGES: usize = 256;

const DEFAULT_MAX_WRITE: u32 = (FUSE_MIN_READ_BUFFER as usize
    - mem::size_of::<fuse_in_header>()
    - mem::size_of::<fuse_write_in>()) as u32;

// ==== KernelConfig ====

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ProtocolVersion {
    major: u32,
    minor: u32,
}

impl ProtocolVersion {
    pub const fn major(&self) -> u32 {
        self.major
    }

    pub const fn minor(&self) -> u32 {
        self.minor
    }
}

impl PartialOrd for ProtocolVersion {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ProtocolVersion {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        match Ord::cmp(&self.major, &other.major) {
            cmp::Ordering::Equal => Ord::cmp(&self.minor, &other.minor),
            cmp => cmp,
        }
    }
}

/// Parameters for setting up the connection with FUSE driver
/// and the kernel side behavior.
#[non_exhaustive]
pub struct KernelConfig {
    /// The protocol version.
    ///
    /// This field is automatically updated in `Session::init`.
    pub protocol_version: ProtocolVersion,

    /// Set the maximum readahead.
    pub max_readahead: u32,

    /// The maximum number of pending *background* requests.
    pub max_background: u16,

    /// The threshold number of pending background requests that the kernel marks the filesystem as *congested*.
    ///
    /// The value should be less or equal to `max_background`.
    ///
    /// If the value is not specified, it is automatically calculated using `max_background`.
    pub congestion_threshold: u16,

    /// The maximum length of bytes attached each `FUSE_WRITE` request.
    ///
    /// The specified value should satisfy the following inequalities:
    ///
    /// * `size_of::<fuse_in_header>() + size_of::<fuse_write_in>() + max_write <= FUSE_MIN_READ_BUFFER`
    /// * `max_write <= DEFAULT_MAX_PAGES_PER_REQ * pagesize()` (if `FUSE_MAX_PAGES` disabled)
    /// * `max_write <= MAX_MAX_PAGES * pagesize()` (if `FUSE_MAX_PAGES` enabled)
    ///
    /// If not satisfied, the value will be automatically adjusted into appropriate range during
    /// the initialization process.
    pub max_write: u32,

    /// The maximum number of pages attached each `FUSE_WRITE` request.
    ///
    /// This value will be automatically calculated during the initialization process.
    pub max_pages: u16,

    /// The timestamp resolution supported by the filesystem.
    ///
    /// The setting value has the nanosecond unit and should be a power of 10.
    ///
    /// The default value is 1.
    pub time_gran: u32,

    /// The flags.
    pub flags: KernelFlags,
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl KernelConfig {
    pub const fn new() -> Self {
        Self {
            protocol_version: ProtocolVersion {
                major: FUSE_KERNEL_VERSION,
                minor: FUSE_KERNEL_MINOR_VERSION,
            },
            max_readahead: u32::MAX,
            max_background: 0,
            congestion_threshold: 0,
            max_write: DEFAULT_MAX_WRITE,
            max_pages: 0, // read only
            time_gran: 1,
            flags: KernelFlags::new(),
        }
    }

    pub fn request_buffer_size(&self) -> usize {
        mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_write_in>() + self.max_write as usize
    }
}

// TODO: add FUSE_IOCTL_DIR
bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct KernelFlags: u32 {
        /// Indicates whether the kernel do readahead asynchronously or not.
        const ASYNC_READ = FUSE_ASYNC_READ;

        /// Indicates whether the kernel does not filtered the `O_TRUNC` open_flag
        /// out or not.
        const ATOMIC_O_TRUNC = FUSE_ATOMIC_O_TRUNC;

        /// The kernel check the validity of attributes on every read.
        const AUTO_INVAL_DATA = FUSE_AUTO_INVAL_DATA;

        /// The filesystem supports asynchronous direct I/O submission.
        const ASYNC_DIO = FUSE_ASYNC_DIO;

        /// The kernel supports parallel directory operations.
        const PARALLEL_DIROPS = FUSE_PARALLEL_DIROPS;

        /// The filesystem is responsible for unsetting setuid and setgid bits
        /// when a file is written, truncated, or its owner is changed.
        const HANDLE_KILLPRIV = FUSE_HANDLE_KILLPRIV;

        /// The filesystem supports the POSIX-style file lock.
        const POSIX_LOCKS = FUSE_POSIX_LOCKS;

        /// The filesystem supports the `flock` handling.
        const FLOCK_LOCKS = FUSE_FLOCK_LOCKS;

        /// The filesystem supports lookups of `"."` and `".."`.
        const EXPORT_SUPPORT = FUSE_EXPORT_SUPPORT;

        /// The kernel should not apply the umask to the file mode
        /// on `mknod`, `mkdir` and `create` operations.
        const DONT_MASK = FUSE_DONT_MASK;

        /// The kernel should enable writeback caching.
        const WRITEBACK_CACHE = FUSE_WRITEBACK_CACHE;

        /// The filesystem supports POSIX access control lists.
        const POSIX_ACL = FUSE_POSIX_ACL;

        /// The filesystem supports `readdirplus` operations.
        const DO_READDIRPLUS = FUSE_DO_READDIRPLUS;

        /// The kernel uses the adaptive readdirplus.
        ///
        /// This option is meaningful only if `readdirplus` is enabled.
        const READDIRPLUS_AUTO = FUSE_READDIRPLUS_AUTO;

        /// Specify whether the kernel supports for zero-message opens.
        ///
        /// When the value is `true`, the kernel treat an `ENOSYS` error
        /// for a `FUSE_OPEN` request as successful and does not send
        /// subsequent `open` requests.  Otherwise, the filesystem should
        /// implement the handler for `open` requests appropriately.
        const NO_OPEN_SUPPORT = FUSE_NO_OPEN_SUPPORT;

        /// Specify whether the kernel supports for zero-message opendirs.
        ///
        /// See the documentation of `no_open_support` for details.
        const NO_OPENDIR_SUPPORT = FUSE_NO_OPENDIR_SUPPORT;

        /// Indicates whether the content of `FUSE_READLINK` replies are cached or not.
        const CACHE_SYMLINKS = FUSE_CACHE_SYMLINKS;

        /// Indicates whether the kernel invalidate the page caches only on explicit
        /// requests.
        const EXPLICIT_INVAL_DATA = FUSE_EXPLICIT_INVAL_DATA;

        /// The kernel supports `splice(2)` read on the device.
        ///
        /// This flag is read-only.
        const SPLICE_READ = FUSE_SPLICE_READ;

        /// The kernel supports `splice(2)` write on the device.
        ///
        /// This flag is read-only.
        const SPLICE_WRITE = FUSE_SPLICE_WRITE;

        /// The kernel supports `splice(2)` move on the device.
        ///
        /// This flag is read-only.
        const SPLICE_MOVE = FUSE_SPLICE_MOVE;

        /// The filesystem can handle `FUSE_WRITE` requests with the size larger
        /// than 4KB.
        ///
        /// This flag is read-only.
        const BIG_WRITES = FUSE_BIG_WRITES;

        const MAX_PAGES = FUSE_MAX_PAGES;

        /// Reading from device will return `ECONNABORTED` rather than `ENODEV`
        /// if the connection is aborted via `fusectl`.
        ///
        /// This flag is read-only.
        const ABORT_ERROR = FUSE_ABORT_ERROR;
    }
}

impl Default for KernelFlags {
    fn default() -> Self {
        Self::new()
    }
}

impl KernelFlags {
    pub const fn new() -> Self {
        Self::empty()
            .union(Self::ATOMIC_O_TRUNC)
            .union(Self::AUTO_INVAL_DATA)
            .union(Self::NO_OPEN_SUPPORT)
            .union(Self::NO_OPENDIR_SUPPORT)
    }

    const READ_ONLY: Self = Self::empty()
        .union(Self::SPLICE_READ)
        .union(Self::SPLICE_WRITE)
        .union(Self::SPLICE_MOVE)
        .union(Self::BIG_WRITES)
        .union(Self::MAX_PAGES)
        .union(Self::ABORT_ERROR);
}

// ==== Session ====

/// The object containing the contextrual information about a FUSE session.
#[derive(Debug)]
pub struct Session {
    state: AtomicU32,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.exit();
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    const UNINIT: u32 = 0;
    const RUNNING: u32 = 1;
    const EXITED: u32 = 2;

    /// Create new instance of `Session`.
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(Self::UNINIT),
        }
    }

    /// Initialize a FUSE session by communicating with the kernel driver over
    /// the established channel.
    pub fn init<T>(&mut self, mut conn: T, config: &mut KernelConfig) -> io::Result<()>
    where
        T: io::Read + io::Write,
    {
        if *self.state.get_mut() != Self::UNINIT {
            tracing::warn!("The session has already been initialized.");
            return Ok(());
        }

        let mut buf = FallbackBuf::new(FUSE_MIN_READ_BUFFER as usize);
        loop {
            buf.reset()?;
            let header = buf.receive_fallback(&mut conn)?;
            if !matches!(header.opcode(), Ok(fuse_opcode::FUSE_INIT)) {
                // 原理上、FUSE_INIT の処理が完了するまで他のリクエストが pop されることはない
                // - ref: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/fs/fuse/fuse_i.h?h=v6.15.9#n693
                // カーネル側の実装に問題があると解釈し、そのリクエストを単に無視する
                tracing::error!(
                    "ignore any filesystem operations received before FUSE_INIT handling (unique={}, opcode={:?})",
                    header.unique(),
                    header.opcode(),
                );
                continue;
            }

            let (header, arg, _remains) = buf.parts();
            let init_in = Decoder::new(arg)
                .fetch::<fuse_init_in>() //
                .map_err(|_| Errno::INVAL)?;

            if init_in.major > 7 {
                // major version が大きい場合、カーネルにダウングレードを要求する
                tracing::debug!(
                    "The requested ABI version {}.{} is too large (expected version is {}.x)\n",
                    init_in.major,
                    init_in.minor,
                    FUSE_KERNEL_VERSION,
                );
                tracing::debug!("  -> Wait for a second INIT request with an older version.");
                let mut out = fuse_init_out::new_zeroed();
                out.major = FUSE_KERNEL_VERSION;
                out.minor = FUSE_KERNEL_MINOR_VERSION;
                self.send_reply(&mut conn, header.unique(), None, out.as_bytes())?;
                continue;
            }

            if init_in.major < 7 || init_in.minor < MINIMUM_SUPPORTED_MINOR_VERSION {
                // バージョンが小さすぎる場合は、プロコトルエラーを報告する。
                // minor version の下限を指定しているのは polyfuse 独自のものであり、古いバージョンへの対応を回避するための策。
                tracing::error!(
                    "The requested ABI version {}.{} is too small (expected version is {}.{} or higher)",
                    init_in.major,
                    init_in.minor,
                    FUSE_KERNEL_VERSION,
                    FUSE_KERNEL_MINOR_VERSION,
                );
                self.send_reply(&mut conn, header.unique(), Some(Errno::PROTO), ())?;
                continue;
            }

            debug_assert_eq!(init_in.major, 7, "The kernel version");
            debug_assert!(
                init_in.minor >= MINIMUM_SUPPORTED_MINOR_VERSION,
                "The minor kernel version"
            );

            let major = FUSE_KERNEL_VERSION;
            let minor = cmp::min(init_in.minor, FUSE_KERNEL_MINOR_VERSION);

            let capable = KernelFlags::from_bits_truncate(init_in.flags);
            config.flags |= KernelFlags::READ_ONLY;
            config.flags &= capable;

            config.max_readahead = cmp::min(config.max_readahead, init_in.max_readahead);

            if config.congestion_threshold == 0 {
                config.congestion_threshold = config.max_background * 3 / 4;
            }
            config.congestion_threshold =
                cmp::min(config.congestion_threshold, config.max_background);

            config.max_write = cmp::max(
                config.max_write,
                FUSE_MIN_READ_BUFFER
                    - (mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_write_in>()) as u32,
            );

            if config.flags.contains(KernelFlags::MAX_PAGES) {
                config.max_write = cmp::min(config.max_write, (MAX_MAX_PAGES * page_size()) as u32);
                config.max_pages = cmp::min(
                    config.max_write.div_ceil(page_size() as u32),
                    u16::max_value() as u32,
                ) as u16;
            } else {
                config.max_write = cmp::min(
                    config.max_write,
                    (DEFAULT_MAX_PAGES_PER_REQ * page_size()) as u32,
                );
                config.max_pages = 0;
            }

            tracing::debug!("INIT request:");
            tracing::debug!("  proto = {}.{}:", init_in.major, init_in.minor);
            tracing::debug!("  flags = 0x{:08x} ({:?})", init_in.flags, capable);
            tracing::debug!("  max_readahead = 0x{:08X}", init_in.max_readahead);
            tracing::debug!(
                "  max_pages_enabled = {}",
                init_in.flags & FUSE_MAX_PAGES != 0
            );

            tracing::debug!("Reply to INIT:");
            tracing::debug!("  proto = {}.{}:", major, minor);
            tracing::debug!("  flags = {:?}", config.flags);
            tracing::debug!("  max_readahead = 0x{:08X}", config.max_readahead);
            tracing::debug!("  max_write = 0x{:08X}", config.max_write);
            tracing::debug!("  max_background = 0x{:04X}", config.max_background);
            tracing::debug!(
                "  congestion_threshold = 0x{:04X}",
                config.congestion_threshold
            );
            tracing::debug!("  time_gran = {}", config.time_gran);

            let init_out = fuse_init_out {
                major,
                minor,
                max_readahead: config.max_readahead,
                flags: config.flags.bits(),
                max_background: config.max_background,
                time_gran: config.time_gran,
                congestion_threshold: config.congestion_threshold,
                max_write: config.max_write,
                max_pages: config.max_pages,
                padding: 0,
                unused: [0; 8],
            };
            self.send_reply(&mut conn, header.unique(), None, init_out.as_bytes())?;

            *self.state.get_mut() = Self::RUNNING;

            return Ok(());
        }
    }

    #[inline]
    fn exit(&self) {
        // FIXME: choose appropriate atomic ordering.
        self.state.store(Self::EXITED, Ordering::SeqCst)
    }

    /// Receive an incoming FUSE request from the kernel.
    pub fn recv_request<T, B>(&self, mut conn: T, buf: &mut B) -> io::Result<bool>
    where
        T: SpliceRead + io::Write,
        B: RequestBuf,
    {
        loop {
            buf.reset()?;

            let header = match buf.try_receive(&mut conn) {
                // ref: https://github.com/libfuse/libfuse/blob/fuse-3.10.5/lib/fuse_lowlevel.c#L2865
                Err(err) => match Errno::from_io_error(&err) {
                    Some(Errno::NODEV) => {
                        tracing::debug!("The connection has already been disconnected");
                        self.exit();
                        return Ok(false);
                    }
                    Some(Errno::INTR) => {
                        tracing::debug!("The read operation is interrupted");
                        continue;
                    }
                    #[allow(unreachable_patterns)]
                    Some(Errno::AGAIN) | Some(Errno::WOULDBLOCK) => {
                        return Err(err);
                    }
                    _ => {
                        tracing::error!("failed to receive the FUSE request: {}", err);
                        return Err(err);
                    }
                },

                Ok(header) => header,
            };

            match header.opcode() {
                Ok(fuse_opcode::FUSE_INIT) => {
                    // FUSE_INIT リクエストは Session の初期化時に処理しているはずなので、ここで読み込まれることはないはず
                    tracing::error!("unexpected FUSE_INIT request received");
                    continue;
                }

                Ok(fuse_opcode::FUSE_DESTROY) => {
                    // TODO: FUSE_DESTROY 後にリクエストの読み込みを中断するかどうかを決める
                    tracing::debug!("FUSE_DESTROY received");
                    self.exit();
                    return Ok(false);
                }

                Ok(opcode @ fuse_opcode::FUSE_IOCTL) | Ok(opcode @ fuse_opcode::CUSE_INIT) => {
                    tracing::warn!(
                        "unsupported opcode (unique={}, opcode={:?})",
                        header.unique(),
                        opcode
                    );
                    self.send_reply(&mut conn, header.unique(), Some(Errno::NOSYS), ())?;
                    continue;
                }

                Err(opcode) => {
                    tracing::warn!(
                        "The opcode `{}' is not recognized by the current version of polyfuse.",
                        opcode
                    );
                    self.send_reply(&mut conn, header.unique(), Some(Errno::NOSYS), ())?;
                    continue;
                }

                Ok(opcode) => {
                    tracing::debug!(
                        "Got a request (unique={}, opcode={:?})",
                        header.unique(),
                        opcode
                    );
                    break Ok(true);
                }
            }
        }
    }

    fn handle_reply_error(&self, err: io::Error) -> io::Result<()> {
        match Errno::from_io_error(&err) {
            Some(Errno::NODEV) => {
                // 切断済みであれば無視
                self.exit();
                Ok(())
            }
            Some(Errno::NOENT) => Ok(()),
            _ => Err(err),
        }
    }

    /// Send a reply message of completed request to the kernel.
    ///
    /// Note that the instance of `conn` must be the same as the one used
    /// when the corresponding request was received via `read_request`.
    /// If anything else (including cloning with `FUSE_IOC_CLONE`) is specified,
    /// the corresponding kernel processing will be isolated, and the process
    /// that issued the associated syscall may enter a deadlock state.
    pub fn send_reply<T, B>(
        &self,
        conn: T,
        unique: RequestID,
        error: Option<Errno>,
        arg: B,
    ) -> io::Result<()>
    where
        T: io::Write,
        B: Bytes,
    {
        let len = (mem::size_of::<fuse_out_header>() + arg.size())
            .try_into()
            .expect("Argument size is too large");

        crate::bytes::write_bytes(
            conn,
            (
                POD(fuse_out_header {
                    len,
                    error: -error.map_or(0, |err| err.raw_os_error()),
                    unique: unique.into_raw(),
                }),
                arg,
            ),
        )
        .or_else(|err| self.handle_reply_error(err))
    }

    /// Send a notification message to the kernel.
    pub fn send_notify<T, B>(&self, conn: T, code: fuse_notify_code, arg: B) -> io::Result<()>
    where
        T: io::Write,
        B: Bytes,
    {
        let len = (mem::size_of::<fuse_out_header>() + arg.size())
            .try_into()
            .expect("Argument size is too large");

        crate::bytes::write_bytes(
            conn,
            (
                POD(fuse_out_header {
                    len,
                    error: code as i32,
                    unique: 0, // unique=0 indicates that the message is a notification.
                }),
                arg,
            ),
        )
        .or_else(|err| self.handle_reply_error(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem;
    use zerocopy::transmute;

    #[test]
    fn proto_version_smoketest() {
        let ver1 = ProtocolVersion {
            major: 7,
            minor: 31,
        };
        assert_eq!(ver1.major(), 7);
        assert_eq!(ver1.minor(), 31);
        assert_eq!(ver1, ver1);

        let ver2 = ProtocolVersion {
            major: 6,
            minor: 99,
        };
        assert!(ver1 > ver2);

        let ver3 = ProtocolVersion {
            major: 7,
            minor: 99,
        };
        assert!(ver1 < ver3);

        let ver4 = ProtocolVersion { major: 8, minor: 0 };
        assert!(ver1 < ver4);
    }

    #[test]
    fn kernel_config_smoketest() {
        let config = KernelConfig::default();
        assert_eq!(
            config.request_buffer_size(),
            config.max_write as usize
                + mem::size_of::<fuse_in_header>()
                + mem::size_of::<fuse_write_in>()
        );
    }

    struct Unite<R, W> {
        reader: R,
        writer: W,
    }

    impl<R, W> io::Read for Unite<R, W>
    where
        R: io::Read,
    {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.reader.read(buf)
        }

        fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
            self.reader.read_vectored(bufs)
        }
    }

    impl<R, W> io::Write for Unite<R, W>
    where
        W: io::Write,
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
    fn session_init() {
        let input_len = mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_init_in>();
        let in_header = fuse_in_header {
            len: input_len as u32,
            opcode: transmute!(fuse_opcode::FUSE_INIT),
            unique: 2,
            nodeid: 0,
            uid: 100,
            gid: 100,
            pid: 12,
            padding: 0,
        };
        let init_in = fuse_init_in {
            major: FUSE_KERNEL_VERSION,
            minor: FUSE_KERNEL_MINOR_VERSION,
            max_readahead: 40,
            flags: KernelFlags::all().bits() | FUSE_MAX_PAGES,
        };

        let mut input = Vec::with_capacity(input_len);
        input.extend_from_slice(in_header.as_bytes());
        input.extend_from_slice(init_in.as_bytes());
        assert_eq!(input.len(), input_len);

        let mut output = Vec::<u8>::new();

        let mut session = Session::new();
        let mut config = KernelConfig::default();
        session
            .init(
                Unite {
                    reader: &input[..],
                    writer: &mut output,
                },
                &mut config,
            )
            .expect("initialization failed");

        let expected_max_pages = DEFAULT_MAX_WRITE.div_ceil(page_size() as u32) as u16;

        assert_eq!(
            config.protocol_version,
            ProtocolVersion {
                major: FUSE_KERNEL_VERSION,
                minor: FUSE_KERNEL_MINOR_VERSION
            }
        );
        assert_eq!(config.max_readahead, 40);
        assert_eq!(config.max_background, 0);
        assert_eq!(config.congestion_threshold, 0);
        assert_eq!(config.max_write, DEFAULT_MAX_WRITE);
        assert_eq!(config.max_pages, expected_max_pages);
        assert_eq!(config.time_gran, 1);
        assert_eq!(
            config.flags,
            KernelFlags::default() | KernelFlags::READ_ONLY
        );

        let output_len = mem::size_of::<fuse_out_header>() + mem::size_of::<fuse_init_out>();
        let out_header = fuse_out_header {
            len: output_len as u32,
            error: 0,
            unique: 2,
        };
        let init_out = fuse_init_out {
            major: FUSE_KERNEL_VERSION,
            minor: FUSE_KERNEL_MINOR_VERSION,
            max_readahead: 40,
            flags: (KernelFlags::default() | KernelFlags::READ_ONLY).bits(),
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

    #[inline]
    fn bytes(bytes: &[u8]) -> &[u8] {
        bytes
    }
    macro_rules! b {
        ($($b:expr),*$(,)?) => ( *bytes(&[$($b),*]) );
    }

    #[test]
    fn send_reply_empty() {
        let session = Session::new();
        let mut buf = vec![0u8; 0];
        session
            .send_reply(&mut buf, RequestID::from_raw(42), Some(Errno::INTR), ())
            .unwrap();
        assert_eq!(buf[0..4], b![0x10, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(buf[4..8], b![0xfc, 0xff, 0xff, 0xff], "header.error");
        assert_eq!(
            buf[8..16],
            b![0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
    }

    #[test]
    fn send_reply_single_data() {
        let session = Session::default();
        let mut buf = vec![0u8; 0];
        session
            .send_reply(&mut buf, RequestID::from_raw(42), None, "hello")
            .unwrap();
        assert_eq!(buf[0..4], b![0x15, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(buf[4..8], b![0x00, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            buf[8..16],
            b![0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
        assert_eq!(buf[16..], b![0x68, 0x65, 0x6c, 0x6c, 0x6f], "payload");
    }

    #[test]
    fn send_msg_chunked_data() {
        let session = Session::default();
        let payload: &[&[u8]] = &[
            "hello, ".as_ref(), //
            "this ".as_ref(),
            "is a ".as_ref(),
            "message.".as_ref(),
        ];
        let mut buf = vec![0u8; 0];
        session
            .send_reply(&mut buf, RequestID::from_raw(26), None, payload)
            .unwrap();
        assert_eq!(buf[0..4], b![0x29, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(buf[4..8], b![0x00, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            buf[8..16],
            b![0x1a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
        assert_eq!(buf[16..], *b"hello, this is a message.", "payload");
    }
}
