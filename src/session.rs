use crate::{
    bytes::{Decoder, ToBytes},
    msg::{send_msg, MessageKind},
    request::RequestBuf,
    types::RequestID,
};
use polyfuse_kernel::*;
use rustix::{io::Errno, param::page_size};
use std::{
    cmp, io, mem,
    sync::atomic::{AtomicBool, Ordering},
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

/// Parameters for setting up the connection with FUSE driver
/// and the kernel side behavior.
#[derive(Debug)]
#[non_exhaustive]
pub struct KernelConfig {
    /// The major number of protocol version.
    ///
    /// This field is automatically updated during initialize process.
    pub major: u32,

    /// The minor number of protocol version.
    ///
    /// This field is automatically updated during initialize process.
    pub minor: u32,

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
            major: FUSE_KERNEL_VERSION,
            minor: FUSE_KERNEL_MINOR_VERSION,
            max_readahead: u32::MAX,
            max_background: 0,
            congestion_threshold: 0,
            max_write: DEFAULT_MAX_WRITE,
            max_pages: 0, // read only
            time_gran: 1,
            flags: KernelFlags::new(),
        }
    }

    fn negotiate(&mut self, init_in: &fuse_init_in) -> Result<(), NegotiationError> {
        if init_in.major > 7 {
            return Err(NegotiationError::TooLargeProtocolVersion);
        }

        if init_in.major < 7 || init_in.minor < MINIMUM_SUPPORTED_MINOR_VERSION {
            return Err(NegotiationError::TooSmallProtocolVersion);
        }

        debug_assert_eq!(init_in.major, 7, "The kernel version");
        debug_assert!(
            init_in.minor >= MINIMUM_SUPPORTED_MINOR_VERSION,
            "The minor kernel version"
        );

        self.major = FUSE_KERNEL_VERSION;
        self.minor = cmp::min(init_in.minor, FUSE_KERNEL_MINOR_VERSION);

        let capable = KernelFlags::from_bits_truncate(init_in.flags);
        self.flags |= KernelFlags::READ_ONLY;
        self.flags &= capable;

        self.max_readahead = cmp::min(self.max_readahead, init_in.max_readahead);

        if self.congestion_threshold == 0 {
            self.congestion_threshold = self.max_background * 3 / 4;
        }
        self.congestion_threshold = cmp::min(self.congestion_threshold, self.max_background);

        self.max_write = cmp::max(
            self.max_write,
            FUSE_MIN_READ_BUFFER
                - (mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_write_in>()) as u32,
        );

        if self.flags.contains(KernelFlags::MAX_PAGES) {
            self.max_write = cmp::min(self.max_write, (MAX_MAX_PAGES * page_size()) as u32);
            self.max_pages = cmp::min(
                self.max_write.div_ceil(page_size() as u32),
                u16::max_value() as u32,
            ) as u16;
        } else {
            self.max_write = cmp::min(
                self.max_write,
                (DEFAULT_MAX_PAGES_PER_REQ * page_size()) as u32,
            );
            self.max_pages = 0;
        }

        Ok(())
    }

    const fn to_arg(&self) -> fuse_init_out {
        fuse_init_out {
            major: self.major,
            minor: self.minor,
            max_readahead: self.max_readahead,
            flags: self.flags.bits(),
            max_background: self.max_background,
            time_gran: self.time_gran,
            congestion_threshold: self.congestion_threshold,
            max_write: self.max_write,
            max_pages: self.max_pages,
            padding: 0,
            unused: [0; 8],
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum NegotiationError {
    #[error("The requested protocol is too large")]
    TooLargeProtocolVersion,

    #[error("The requested protocol is too small")]
    TooSmallProtocolVersion,
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
    config: KernelConfig,
    exited: AtomicBool,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.exit();
    }
}

impl Session {
    /// Initialize a FUSE session by communicating with the kernel driver over
    /// the established channel.
    pub fn init<T, B>(mut conn: T, mut buf: B, mut config: KernelConfig) -> io::Result<Self>
    where
        T: io::Write,
        B: RequestBuf<T>,
    {
        loop {
            let (header, arg, _remains) = buf.receive(&mut conn)?;

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

            let init_in = Decoder::new(arg)
                .fetch::<fuse_init_in>() //
                .map_err(|_| Errno::INVAL)?;

            tracing::debug!("INIT request:");
            tracing::debug!("  version = {}.{}:", init_in.major, init_in.minor);
            tracing::debug!(
                "  capable flags = {:?}",
                KernelFlags::from_bits_truncate(init_in.flags)
            );
            tracing::debug!("  max_readahead = 0x{:08X}", init_in.max_readahead);

            match config.negotiate(init_in) {
                Ok(()) => (),
                Err(NegotiationError::TooLargeProtocolVersion) => {
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
                    send_msg(
                        &mut conn,
                        MessageKind::Reply {
                            unique: header.unique(),
                            error: None,
                        },
                        out.as_bytes(),
                    )?;
                    continue;
                }
                Err(NegotiationError::TooSmallProtocolVersion) => {
                    // バージョンが小さすぎる場合は、プロコトルエラーを報告する。
                    // minor version の下限を指定しているのは polyfuse 独自のものであり、古いバージョンへの対応を回避するための策。
                    tracing::error!(
                        "The requested ABI version {}.{} is too small (expected version is {}.{} or higher)",
                        init_in.major,
                        init_in.minor,
                        FUSE_KERNEL_VERSION,
                        FUSE_KERNEL_MINOR_VERSION,
                    );
                    send_msg(
                        &mut conn,
                        MessageKind::Reply {
                            unique: header.unique(),
                            error: Some(Errno::PROTO),
                        },
                        (),
                    )?;
                    continue;
                }
            }

            tracing::debug!("Reply to INIT:");
            tracing::debug!("  proto = {}.{}:", config.major, config.minor);
            tracing::debug!("  flags = {:?}", config.flags);
            tracing::debug!("  max_readahead = 0x{:08X}", config.max_readahead);
            tracing::debug!("  max_write = 0x{:08X}", config.max_write);
            tracing::debug!("  max_background = 0x{:04X}", config.max_background);
            tracing::debug!(
                "  congestion_threshold = 0x{:04X}",
                config.congestion_threshold
            );
            tracing::debug!("  time_gran = {}", config.time_gran);

            let init_out = config.to_arg();
            send_msg(
                &mut conn,
                MessageKind::Reply {
                    unique: header.unique(),
                    error: None,
                },
                init_out.as_bytes(),
            )?;

            return Ok(Self {
                config,
                exited: AtomicBool::new(false),
            });
        }
    }

    #[inline]
    pub fn config(&self) -> &KernelConfig {
        &self.config
    }

    #[inline]
    pub fn request_buffer_size(&self) -> usize {
        mem::size_of::<fuse_in_header>()
            + mem::size_of::<fuse_write_in>()
            + self.config.max_write as usize
    }

    #[inline]
    pub fn exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    #[inline]
    pub fn exit(&self) {
        self.exited.store(true, Ordering::Release)
    }

    /// Receive an incoming FUSE request from the kernel.
    pub fn recv_request<T, B>(&self, mut conn: T, buf: &mut B) -> io::Result<bool>
    where
        T: io::Write,
        B: RequestBuf<T>,
    {
        while !self.exited() {
            let header = match buf.receive(&mut conn) {
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

                Ok((header, ..)) => header,
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
                    return Ok(true);
                }
            }
        }

        Ok(false)
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
        B: ToBytes,
    {
        send_msg(conn, MessageKind::Reply { unique, error }, arg.to_bytes())
            .or_else(|err| self.handle_reply_error(err))
    }

    /// Send a notification message to the kernel.
    pub fn send_notify<T, B>(&self, conn: T, code: fuse_notify_code, arg: B) -> io::Result<()>
    where
        T: io::Write,
        B: ToBytes,
    {
        send_msg(conn, MessageKind::Notify { code }, arg.to_bytes())
            .or_else(|err| self.handle_reply_error(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiate_default() {
        let init_in = fuse_init_in {
            major: FUSE_KERNEL_VERSION,
            minor: FUSE_KERNEL_MINOR_VERSION.saturating_add(20),
            max_readahead: 40,
            flags: KernelFlags::all().bits() | FUSE_MAX_PAGES,
        };

        let mut config = KernelConfig::default();
        config.negotiate(&init_in).unwrap();

        let expected_max_pages = DEFAULT_MAX_WRITE.div_ceil(page_size() as u32) as u16;

        assert_eq!(config.major, FUSE_KERNEL_VERSION);
        assert_eq!(config.minor, FUSE_KERNEL_MINOR_VERSION);
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
    }

    #[test]
    fn negotiate_mismatched_protocol_version() {
        let init_in = fuse_init_in {
            major: FUSE_KERNEL_VERSION + 1,
            minor: 9999,
            max_readahead: 0,
            flags: KernelFlags::all().bits() | FUSE_MAX_PAGES,
        };
        let mut config = KernelConfig::default();
        assert!(matches!(
            config.negotiate(&init_in),
            Err(NegotiationError::TooLargeProtocolVersion)
        ));

        let init_in = fuse_init_in {
            major: FUSE_KERNEL_VERSION - 1,
            minor: 9999,
            max_readahead: 0,
            flags: KernelFlags::all().bits() | FUSE_MAX_PAGES,
        };
        let mut config = KernelConfig::default();
        assert!(matches!(
            config.negotiate(&init_in),
            Err(NegotiationError::TooSmallProtocolVersion)
        ));

        let init_in = fuse_init_in {
            major: FUSE_KERNEL_VERSION,
            minor: MINIMUM_SUPPORTED_MINOR_VERSION.saturating_sub(1),
            max_readahead: 0,
            flags: KernelFlags::all().bits() | FUSE_MAX_PAGES,
        };
        let mut config = KernelConfig::default();
        assert!(matches!(
            config.negotiate(&init_in),
            Err(NegotiationError::TooSmallProtocolVersion)
        ));
    }

    #[test]
    fn test_disabled_max_pages() {
        let init_in = fuse_init_in {
            major: FUSE_KERNEL_VERSION,
            minor: FUSE_KERNEL_MINOR_VERSION,
            max_readahead: 40,
            flags: (KernelFlags::all().bits() | FUSE_MAX_PAGES) & !FUSE_MAX_PAGES,
        };

        let mut config = KernelConfig {
            max_pages: 42,
            ..Default::default()
        };
        config.negotiate(&init_in).unwrap();

        assert_eq!(config.major, FUSE_KERNEL_VERSION);
        assert_eq!(config.minor, FUSE_KERNEL_MINOR_VERSION);
        assert_eq!(config.max_readahead, 40);
        assert_eq!(config.max_background, 0);
        assert_eq!(config.congestion_threshold, 0);
        assert_eq!(config.max_write, DEFAULT_MAX_WRITE);
        assert_eq!(config.max_pages, 0);
        assert_eq!(config.time_gran, 1);
        assert_eq!(
            config.flags,
            (KernelFlags::default() | KernelFlags::READ_ONLY) & !KernelFlags::MAX_PAGES
        );
    }

    #[test]
    fn config_to_args() {
        let config = KernelConfig::default();
        let arg = config.to_arg();

        assert_eq!(arg.major, config.major);
        assert_eq!(arg.minor, config.minor);
        assert_eq!(arg.max_readahead, config.max_readahead);
        assert_eq!(arg.max_background, config.max_background);
        assert_eq!(arg.congestion_threshold, config.congestion_threshold);
        assert_eq!(arg.max_write, config.max_write);
        assert_eq!(arg.max_pages, config.max_pages);
        assert_eq!(arg.time_gran, config.time_gran);
        assert_eq!(arg.flags, config.flags.bits());
    }
}
