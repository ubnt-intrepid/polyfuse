use crate::{
    bytes::Bytes,
    conn::Connection,
    mount::{Mount, MountOptions},
    msg::{send_msg, MessageKind},
    op::{DecodeError, Operation},
    reply::ReplyArg,
    request::{FallbackBuf, RequestHeader, ToRequestParts, TryReceive},
    types::RequestID,
};
use polyfuse_kernel::*;
use rustix::{io::Errno, param::page_size};
use std::{
    borrow::Cow,
    cmp, io, mem,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use zerocopy::{FromZeros as _, IntoBytes as _, TryFromBytes as _};

const FILESYSTEM_MAX_STACK_DEPTH: u32 = 2;

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

    /// The timestamp resolution supported by the filesystem.
    ///
    /// The setting value has the nanosecond unit and should be a power of 10.
    ///
    /// The default value is 1.
    pub time_gran: u32,

    /// The maximum number of pages that can be sent from the filesystem on each
    /// reply/notification.
    pub max_pages: u16,

    /// The maximum stack depth for passthrough backing files.
    pub max_stack_depth: u32,

    // TODO: add documentation
    pub request_timeout: u16,

    /// The flags.
    pub flags: KernelFlags,
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl KernelConfig {
    const MIN_MAX_WRITE: u32 = FUSE_MIN_READ_BUFFER
        - (mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_write_in>()) as u32;
    const DEFAULT_MAX_PAGES_PER_REQ: u16 = 32;
    const MAX_MAX_PAGES: u16 = 256;

    pub const fn new() -> Self {
        Self {
            major: FUSE_KERNEL_VERSION,
            minor: FUSE_KERNEL_MINOR_VERSION,
            max_readahead: u32::MAX,
            max_background: 0,
            congestion_threshold: 0,
            max_write: Self::MIN_MAX_WRITE,
            max_pages: Self::DEFAULT_MAX_PAGES_PER_REQ,
            time_gran: 1,
            max_stack_depth: 0,
            request_timeout: 0,
            flags: KernelFlags::new(),
        }
    }

    fn negotiate(&mut self, init_in: InitIn<'_>) -> Result<(), NegotiationError> {
        let (major, minor) = init_in.protocol_version();
        if major > 7 {
            return Err(NegotiationError::TooLargeProtocolVersion);
        }
        if major < 7 {
            return Err(NegotiationError::TooSmallProtocolVersion);
        }
        self.major = major;
        self.minor = cmp::min(self.minor, minor);

        if matches!(init_in, InitIn::Compat3(..)) {
            // do nothing.
            return Ok(());
        }

        // update flags.
        // * カーネル側の挙動を変更しないフラグ (SPLICE_READ など) は強制的にオンにする
        // * カーネル側でサポートしていないフラグはクリアする
        self.flags = (self.flags | KernelFlags::READ_ONLY) & init_in.flags();

        // max_readahead: since ABI 7.6
        // * 最小値は 4096 (それ未満の値を指定した場合、fc->max_readahead への代入時に強制的に上寄せされる)
        self.max_readahead = cmp::min(self.max_readahead, init_in.max_readahead());
        self.max_readahead = cmp::max(self.max_readahead, 4096);

        // max_write: since ABI 7.6
        // * 7.5 は対応するカーネルのバージョンが見つからなかったので欠番扱いにする
        self.max_write = cmp::max(self.max_write, Self::MIN_MAX_WRITE);
        self.max_write = cmp::min(
            self.max_write,
            (Self::MAX_MAX_PAGES as usize * page_size()) as u32,
        );

        // max_background/congestion_threshold: since ABI 7.13
        // * max_background = 0 は許容される
        // * congestion_threshold = 0 の場合は max_background * 3/4 に上書きする
        // * max_background よりも大きい場合は下寄せする
        if self.minor >= 13 {
            if self.congestion_threshold == 0 {
                self.congestion_threshold = self.max_background * 3 / 4;
            }
            self.congestion_threshold = cmp::min(self.congestion_threshold, self.max_background);
        } else {
            self.max_background = 0;
            self.congestion_threshold = 0;
        }

        // time_gran: since ABI 7.13
        // * 最も近い 10 のべき乗の値へと下寄せされる
        // * 0 の場合は 1 にする
        // * 最大値は 1_000_000_000
        if self.minor >= 13 {
            self.time_gran = 10u32.pow(cmp::max(self.time_gran, 1).ilog10());
            self.time_gran = cmp::min(self.time_gran, 1_000_000_000);
        } else {
            self.time_gran = 0;
        }

        // max_pages: since ABI 7.28
        // * FS 側からの書き込み (特に READ / NOTIFY_RETRIEVE) 時の最大ページ数を指定する
        if self.minor >= 28 && self.flags.contains(KernelFlags::MAX_PAGES) {
            self.max_pages = cmp::max(self.max_pages, 1);
            self.max_pages = cmp::min(self.max_pages, Self::MAX_MAX_PAGES);
        } else {
            self.max_pages = Self::DEFAULT_MAX_PAGES_PER_REQ;
        }

        if self.minor >= 40
            && self.flags.contains(KernelFlags::PASSTHROUGH)
            && !self.flags.contains(KernelFlags::WRITEBACK_CACHE)
        {
            self.max_stack_depth = cmp::min(self.max_stack_depth, FILESYSTEM_MAX_STACK_DEPTH);
        } else {
            self.flags.remove(KernelFlags::PASSTHROUGH);
            self.max_stack_depth = 0;
        }

        // TODO: request_timeout(7.43~)
        self.request_timeout = 0;

        Ok(())
    }

    const fn to_out(&self) -> InitOut {
        if self.minor <= 3 {
            InitOut::Compat3(fuse_init_in_out_compat_3 {
                major: self.major,
                minor: self.minor,
            })
        } else if self.minor <= 22 {
            InitOut::Compat22(fuse_init_out_compat_22 {
                major: self.major,
                minor: self.minor,
                max_readahead: self.max_readahead,
                flags: (self.flags.bits() & (u32::MAX as u64)) as _,
                max_background: self.max_background,
                congestion_threshold: self.congestion_threshold,
                max_write: self.max_write,
            })
        } else {
            InitOut::Current(fuse_init_out {
                major: self.major,
                minor: self.minor,
                max_readahead: self.max_readahead,
                flags: (self.flags.bits() & (u32::MAX as u64)) as _,
                max_background: self.max_background,
                congestion_threshold: self.congestion_threshold,
                max_write: self.max_write,
                time_gran: self.time_gran,
                max_pages: self.max_pages,
                map_alignment: 0,
                flags2: if self.flags.contains(KernelFlags::INIT_EXT) {
                    ((self.flags.bits() >> 32) & (u32::MAX as u64)) as _
                } else {
                    0
                },
                max_stack_depth: self.max_stack_depth,
                request_timeout: self.request_timeout,
                unused: [0; 11],
            })
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum InitIn<'a> {
    Compat3(&'a fuse_init_in_out_compat_3),
    Compat35(&'a fuse_init_in_compat_35),
    Current(&'a fuse_init_in),
}

impl<'a> InitIn<'a> {
    fn from_bytes(bytes: &'a [u8]) -> Option<Self> {
        let (arg, _) = fuse_init_in_out_compat_3::try_ref_from_prefix(bytes).ok()?;
        if arg.minor <= 3 {
            Some(Self::Compat3(arg))
        } else if arg.minor <= 35 {
            let (arg, _) = fuse_init_in_compat_35::try_ref_from_prefix(bytes).ok()?;
            Some(Self::Compat35(arg))
        } else {
            // fuse_init_in 側は FUSE_INIT_EXT の有無に関わらず sizeof(fuse_init_in) 分だけ送信される
            let (arg, _) = fuse_init_in::try_ref_from_prefix(bytes).ok()?;
            Some(Self::Current(arg))
        }
    }

    pub const fn protocol_version(&self) -> (u32, u32) {
        match *self {
            Self::Compat3(arg) => (arg.major, arg.minor),
            Self::Compat35(arg) => (arg.major, arg.minor),
            Self::Current(arg) => (arg.major, arg.minor),
        }
    }

    pub const fn flags(&self) -> KernelFlags {
        match *self {
            Self::Compat3(..) => KernelFlags::empty(),
            Self::Compat35(arg) => KernelFlags::from_bits_truncate(arg.flags as u64),
            Self::Current(arg) => {
                KernelFlags::from_bits_truncate((arg.flags as u64) | ((arg.flags2 as u64) << 32))
            }
        }
    }

    pub const fn max_readahead(&self) -> u32 {
        match *self {
            Self::Compat35(arg) => arg.max_readahead,
            Self::Current(arg) => arg.max_readahead,
            _ => 0,
        }
    }
}

enum InitOut {
    Compat3(fuse_init_in_out_compat_3),
    Compat22(fuse_init_out_compat_22),
    Current(fuse_init_out),
}

impl InitOut {
    fn to_bytes(&self) -> &[u8] {
        match self {
            Self::Compat3(out) => out.as_bytes(),
            Self::Compat22(out) => out.as_bytes(),
            Self::Current(out) => out.as_bytes(),
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
    /// The flags to obtain the supported feature, or control the behavior of the FUSE kernel driver.
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct KernelFlags: u64 {
        /// Indicates whether the kernel do readahead asynchronously or not.
        const ASYNC_READ = FUSE_ASYNC_READ as _;

        /// Indicates whether the kernel does not filtered the `O_TRUNC` open_flag
        /// out or not.
        ///
        /// This flag is enabled by default.
        const ATOMIC_O_TRUNC = FUSE_ATOMIC_O_TRUNC as _;

        /// The kernel check the validity of attributes on every read.
        ///
        /// This flag is enabled by default.
        const AUTO_INVAL_DATA = FUSE_AUTO_INVAL_DATA as _;

        /// The filesystem supports asynchronous direct I/O submission.
        const ASYNC_DIO = FUSE_ASYNC_DIO as _;

        /// The kernel supports parallel directory operations.
        const PARALLEL_DIROPS = FUSE_PARALLEL_DIROPS as _;

        /// The filesystem is responsible for unsetting suid/sgid bits
        /// when a file is modified (written, truncated, or its owner is changed).
        const HANDLE_KILLPRIV = FUSE_HANDLE_KILLPRIV as _;

        /// The filesystem supports the POSIX-style file lock.
        const POSIX_LOCKS = FUSE_POSIX_LOCKS as _;

        /// The filesystem supports the BSD-style file lock.
        const FLOCK_LOCKS = FUSE_FLOCK_LOCKS as _;

        /// The filesystem supports NFS exporting.
        const EXPORT_SUPPORT = FUSE_EXPORT_SUPPORT as _;

        /// The kernel does not apply the umask to the file mode on the
        /// creation operations (`mknod`, `mkdir` and `create`).
        ///
        /// If this flag is enabled, the filesystem should properly handle
        /// the argument `umask` in the request parameters.
        const DONT_MASK = FUSE_DONT_MASK as _;

        /// The kernel uses writeback caching policy.
        ///
        /// By default, the kernel uses the write-through policy.
        const WRITEBACK_CACHE = FUSE_WRITEBACK_CACHE as _;

        /// The filesystem supports POSIX access control lists.
        ///
        /// If this flag is enabled, the kernel enables the mount options
        /// `default_permissions` implicitly.
        const POSIX_ACL = FUSE_POSIX_ACL as _;

        /// The filesystem supports the `FUSE_READDIRPLUS` opcode.
        const DO_READDIRPLUS = FUSE_DO_READDIRPLUS as _;

        /// The kernel uses the adaptive readdirplus.
        ///
        /// This option is meaningful only if [`Self::DO_READDIRPLUS`] is enabled.
        const READDIRPLUS_AUTO = FUSE_READDIRPLUS_AUTO as _;

        /// Specify whether the kernel supports for zero-message opens.
        ///
        /// When the value is `true`, the kernel treat an `ENOSYS` error
        /// for a `FUSE_OPEN` request as successful and does not send
        /// subsequent `open` requests.  Otherwise, the filesystem should
        /// implement the handler for `open` requests appropriately.
        ///
        /// This flag is enabled by default.
        const NO_OPEN_SUPPORT = FUSE_NO_OPEN_SUPPORT as _;

        /// Specify whether the kernel supports for zero-message opendirs.
        ///
        /// See also [`Self::NO_OPEN_SUPPORT`].
        ///
        /// This flag is enabled by default.
        const NO_OPENDIR_SUPPORT = FUSE_NO_OPENDIR_SUPPORT as _;

        /// Indicates whether the content of `FUSE_READLINK` replies are cached or not.
        const CACHE_SYMLINKS = FUSE_CACHE_SYMLINKS as _;

        /// Indicates whether the kernel invalidate the page caches
        /// only on explicit notifications by the filesystem.
        const EXPLICIT_INVAL_DATA = FUSE_EXPLICIT_INVAL_DATA as _;

        /// The kernel supports `splice(2)` read on the device.
        ///
        /// This flag is read-only.
        const SPLICE_READ = FUSE_SPLICE_READ as _;

        /// The kernel supports `splice(2)` write on the device.
        ///
        /// This flag is read-only.
        const SPLICE_WRITE = FUSE_SPLICE_WRITE as _;

        /// The kernel supports `splice(2)` move on the device.
        ///
        /// This flag is read-only.
        const SPLICE_MOVE = FUSE_SPLICE_MOVE as _;

        /// The filesystem can handle `FUSE_WRITE` requests with the size larger
        /// than 4KB.
        ///
        /// This flag is read-only.
        const BIG_WRITES = FUSE_BIG_WRITES as _;

        /// Reading from device will return `ECONNABORTED` rather than `ENODEV`
        /// if the connection is aborted via `fusectl`.
        ///
        /// This flag is read-only.
        const ABORT_ERROR = FUSE_ABORT_ERROR as _;

        /// Specify whether the parameter [`KernelConfig::max_pages`] is available or not.
        ///
        /// This flag is read-only.
        const MAX_PAGES = FUSE_MAX_PAGES as _;

        // TODO: add doc
        const HANDLE_KILLPRIV_V2 = FUSE_HANDLE_KILLPRIV_V2 as _;

        /// The kernel supports the extended version of `fuse_setxattr_in`.
        ///
        /// This flag is read-only.
        const SETXATTR_EXT = FUSE_SETXATTR_EXT as _;

        /// The kernel supports the extended version of `fuse_init_out`.
        ///
        /// This flag is read-only.
        const INIT_EXT = FUSE_INIT_EXT as _;

        // TODO: add doc
        const SECURITY_CTX = FUSE_SECURITY_CTX;

        // TODO: add doc
        const CREATE_SUPP_GROUP = FUSE_CREATE_SUPP_GROUP;

        /// This flag is read-only.
        const HAS_EXPIRE_ONLY = FUSE_HAS_EXPIRE_ONLY;

        // TODO: add doc
        const DIRECT_IO_ALLOW_MMAP = FUSE_DIRECT_IO_ALLOW_MMAP;

        // TODO: add doc
        const PASSTHROUGH = FUSE_PASSTHROUGH;

        // TODO: add doc
        const NO_EXPORT_SUPPORT = FUSE_NO_EXPORT_SUPPORT;

        /// This flag is read-only.
        const HAS_RESEND = FUSE_HAS_RESEND;

        // TODO: add doc
        const ALLOW_IDMAP = FUSE_ALLOW_IDMAP;

        /// The filesystem uses FUSE over io_uring.
        const OVER_IO_URING = FUSE_OVER_IO_URING;

        // TODO: add doc
        const REQUEST_TIMEOUT = FUSE_REQUEST_TIMEOUT;
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

    // The read-only flags.
    const READ_ONLY: Self = Self::empty()
        .union(Self::SPLICE_READ)
        .union(Self::SPLICE_WRITE)
        .union(Self::SPLICE_MOVE)
        .union(Self::BIG_WRITES)
        .union(Self::MAX_PAGES)
        .union(Self::ABORT_ERROR)
        .union(Self::SETXATTR_EXT)
        .union(Self::INIT_EXT)
        .union(Self::HAS_EXPIRE_ONLY)
        .union(Self::HAS_RESEND);
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
        B: TryReceive<T>,
    {
        if self.exited() {
            return Ok(false);
        }

        let header = match buf.try_receive(&mut conn) {
            Err(err) => match Errno::from_io_error(&err) {
                Some(Errno::NODEV) => {
                    self.exit();
                    return Ok(false);
                }
                _ => return Err(err),
            },
            Ok(header) => header,
        };

        match header.opcode() {
            Ok(fuse_opcode::FUSE_INIT) => {
                // FUSE_INIT リクエストは Session の初期化時に処理しているはずなので、ここで読み込まれることはないはず
                tracing::error!("unexpected FUSE_INIT request received");
                return Err(Errno::PROTO.into());
            }

            Ok(fuse_opcode::FUSE_DESTROY) => {
                // TODO: FUSE_DESTROY 後にリクエストの読み込みを中断するかどうかを決める
                tracing::debug!("FUSE_DESTROY received");
                self.exit();
                return Ok(false);
            }
            _ => (),
        }

        Ok(true)
    }

    pub fn decode<'op, B>(&self, buf: &'op mut B) -> Result<RequestParts<'op, B>, DecodeError>
    where
        B: ToRequestParts,
    {
        let (header, arg, remains) = buf.to_request_parts();
        let op = match Operation::decode(&self.config, header, arg, remains) {
            Ok(op) => Some(op),
            Err(DecodeError::UnsupportedOpcode) => None,
            Err(err) => return Err(err),
        };
        Ok((header, op))
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
        B: ReplyArg,
    {
        struct SessionReplySender<'a, T> {
            session: &'a Session,
            conn: T,
            unique: RequestID,
            error: Option<Errno>,
        }
        impl<T> crate::reply::ReplySender for SessionReplySender<'_, T>
        where
            T: io::Write,
        {
            type Error = io::Error;

            fn config(&self) -> &KernelConfig {
                self.session.config()
            }

            fn send_bytes<B: Bytes>(self, bytes: B) -> Result<(), Self::Error> {
                send_msg(
                    self.conn,
                    MessageKind::Reply {
                        unique: self.unique,
                        error: self.error,
                    },
                    bytes,
                )
                .or_else(|err| self.session.handle_reply_error(err))
            }
        }
        arg.reply(SessionReplySender {
            session: self,
            conn,
            unique,
            error,
        })
    }

    /// Send a notification message to the kernel.
    pub fn send_notify<T, B>(&self, conn: T, code: fuse_notify_code, arg: B) -> io::Result<()>
    where
        T: io::Write,
        B: Bytes,
    {
        send_msg(conn, MessageKind::Notify { code }, arg)
            .or_else(|err| self.handle_reply_error(err))
    }
}

pub type RequestParts<'op, B> = (
    &'op RequestHeader,
    Option<Operation<'op, <B as ToRequestParts>::RemainingData<'op>>>,
);

pub fn connect(
    mountpoint: Cow<'static, Path>,
    mountopts: MountOptions,
    mut config: KernelConfig,
) -> io::Result<(Session, Connection, Mount)> {
    let (conn, mount) = crate::mount::mount(&mountpoint, &mountopts)?;

    let mut buf = FallbackBuf::new(FUSE_MIN_READ_BUFFER as usize);
    loop {
        buf.try_receive(&mut &conn)?;
        let (header, arg, _remains) = buf.to_request_parts();

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

        let init_in = InitIn::from_bytes(arg).ok_or(Errno::INVAL)?;

        match config.negotiate(init_in) {
            Ok(()) => (),
            Err(NegotiationError::TooLargeProtocolVersion) => {
                let (major, minor) = init_in.protocol_version();
                // major version が大きい場合、カーネルにダウングレードを要求する
                tracing::debug!(
                    "The requested ABI version {}.{} is too large (expected version is {}.x)\n",
                    major,
                    minor,
                    FUSE_KERNEL_VERSION,
                );
                tracing::debug!("  -> Wait for a second INIT request with an older version.");
                let mut out = fuse_init_out::new_zeroed();
                out.major = FUSE_KERNEL_VERSION;
                out.minor = FUSE_KERNEL_MINOR_VERSION;
                send_msg(
                    &conn,
                    MessageKind::Reply {
                        unique: header.unique(),
                        error: None,
                    },
                    out.as_bytes(),
                )?;
                continue;
            }
            Err(NegotiationError::TooSmallProtocolVersion) => {
                let (major, minor) = init_in.protocol_version();
                // バージョンが小さすぎる場合は、プロコトルエラーを報告する。
                tracing::error!(
                    "The requested ABI version {}.{} is too small (expected version is {}.x)",
                    major,
                    minor,
                    FUSE_KERNEL_VERSION,
                );
                send_msg(
                    &conn,
                    MessageKind::Reply {
                        unique: header.unique(),
                        error: Some(Errno::PROTO),
                    },
                    (),
                )?;
                continue;
            }
        }

        let out = config.to_out();
        send_msg(
            &conn,
            MessageKind::Reply {
                unique: header.unique(),
                error: None,
            },
            out.to_bytes(),
        )?;

        let session = Session {
            config,
            exited: AtomicBool::new(false),
        };
        return Ok((session, conn, mount));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::{Buf as _, BufMut as _};

    #[test]
    fn negotiate_default() {
        let flags = KernelFlags::all();
        let init_in = fuse_init_in {
            major: FUSE_KERNEL_VERSION,
            minor: FUSE_KERNEL_MINOR_VERSION.saturating_add(20),
            max_readahead: 8000,
            flags: flags.bits() as _,
            flags2: (flags.bits() >> 32) as _,
            unused: [0; 11],
        };
        let init_in = InitIn::Current(&init_in);

        let mut config = KernelConfig::default();
        config.negotiate(init_in).unwrap();

        assert_eq!(config.major, FUSE_KERNEL_VERSION);
        assert_eq!(config.minor, FUSE_KERNEL_MINOR_VERSION);
        assert_eq!(config.max_readahead, 8000);
        assert_eq!(config.max_background, 0);
        assert_eq!(config.congestion_threshold, 0);
        assert_eq!(config.max_write, KernelConfig::MIN_MAX_WRITE);
        assert_eq!(config.max_pages, KernelConfig::DEFAULT_MAX_PAGES_PER_REQ);
        assert_eq!(config.time_gran, 1);
        assert_eq!(
            config.flags,
            KernelFlags::default() | KernelFlags::READ_ONLY
        );
    }

    #[test]
    fn negotiate_mismatched_protocol_version() {
        let init_in = fuse_init_in_compat_35 {
            major: FUSE_KERNEL_VERSION + 1,
            minor: 9999,
            max_readahead: 0,
            flags: KernelFlags::all().bits() as _,
        };
        let init_in = InitIn::Compat35(&init_in);

        let mut config = KernelConfig::default();
        assert!(matches!(
            config.negotiate(init_in),
            Err(NegotiationError::TooLargeProtocolVersion)
        ));

        let init_in = fuse_init_in_out_compat_3 {
            major: FUSE_KERNEL_VERSION - 1,
            minor: 9999,
        };
        let init_in = InitIn::Compat3(&init_in);

        let mut config = KernelConfig::default();
        assert!(matches!(
            config.negotiate(init_in),
            Err(NegotiationError::TooSmallProtocolVersion)
        ));
    }

    #[test]
    fn test_disabled_max_pages() {
        let flags = KernelFlags::all() & !KernelFlags::MAX_PAGES;
        let init_in = fuse_init_in {
            major: FUSE_KERNEL_VERSION,
            minor: FUSE_KERNEL_MINOR_VERSION,
            max_readahead: 99,
            flags: flags.bits() as _,
            flags2: (flags.bits() >> 32) as _,
            unused: [0; 11],
        };
        let init_in = InitIn::Current(&init_in);

        let mut config = KernelConfig {
            max_pages: 42,
            ..KernelConfig::new()
        };
        config.negotiate(init_in).unwrap();

        assert_eq!(config.major, FUSE_KERNEL_VERSION);
        assert_eq!(config.minor, FUSE_KERNEL_MINOR_VERSION);
        assert_eq!(config.max_readahead, 4096);
        assert_eq!(config.max_background, 0);
        assert_eq!(config.congestion_threshold, 0);
        assert_eq!(config.max_write, KernelConfig::MIN_MAX_WRITE);
        assert_eq!(config.max_pages, KernelConfig::DEFAULT_MAX_PAGES_PER_REQ);
        assert_eq!(config.time_gran, 1);
        assert_eq!(
            config.flags,
            (KernelFlags::new() | KernelFlags::READ_ONLY) & !KernelFlags::MAX_PAGES
        );
    }

    #[test]
    fn init_in_from_bytes() {
        let mut input = vec![];
        input.put_u32_ne(7);
        input.put_u32_ne(2);
        let parsed = InitIn::from_bytes(&input[..]).unwrap();
        assert!(matches!(parsed, InitIn::Compat3(..)));
        assert_eq!(parsed.protocol_version(), (7, 2));
        assert_eq!(parsed.max_readahead(), 0);
        assert_eq!(parsed.flags(), KernelFlags::empty());

        let mut input = vec![];
        input.put_u32_ne(7);
        input.put_u32_ne(32);
        assert!(InitIn::from_bytes(&input[..]).is_none());
        input.put_u32_ne(9876);
        input.put_u32_ne(FUSE_POSIX_ACL);
        let parsed = InitIn::from_bytes(&input[..]).unwrap();
        assert!(matches!(parsed, InitIn::Compat35(..)));
        assert_eq!(parsed.protocol_version(), (7, 32));
        assert_eq!(parsed.max_readahead(), 9876);
        assert_eq!(parsed.flags(), KernelFlags::POSIX_ACL);

        let mut input = vec![];
        input.put_u32_ne(7);
        input.put_u32_ne(42);
        input.put_u32_ne(8888);
        input.put_u32_ne(FUSE_SPLICE_READ | FUSE_BIG_WRITES);
        assert!(InitIn::from_bytes(&input[..]).is_none());
        input.put_u32_ne((FUSE_ALLOW_IDMAP >> 32) as u32);
        input.put_slice(&[0; 11 * mem::size_of::<u32>()]);
        let parsed = InitIn::from_bytes(&input[..]).unwrap();
        assert!(matches!(parsed, InitIn::Current(..)));
        assert_eq!(parsed.protocol_version(), (7, 42));
        assert_eq!(parsed.max_readahead(), 8888);
        assert_eq!(
            parsed.flags(),
            KernelFlags::SPLICE_READ | KernelFlags::BIG_WRITES | KernelFlags::ALLOW_IDMAP
        );
    }

    #[test]
    fn config_to_out() {
        let config = KernelConfig::default();
        let out = config.to_out();
        assert!(matches!(out, InitOut::Current(..)));

        let mut bytes = out.to_bytes();
        assert_eq!(bytes.len(), mem::size_of::<fuse_init_out>());
        assert_eq!(bytes.get_u32_ne(), config.major);
        assert_eq!(bytes.get_u32_ne(), config.minor);
        assert_eq!(bytes.get_u32_ne(), config.max_readahead);
        assert_eq!(bytes.get_u32_ne(), config.flags.bits() as u32);
        assert_eq!(bytes.get_u16_ne(), config.max_background);
        assert_eq!(bytes.get_u16_ne(), config.congestion_threshold);
        assert_eq!(bytes.get_u32_ne(), config.max_write);
        assert_eq!(bytes.get_u32_ne(), config.time_gran);
        assert_eq!(bytes.get_u16_ne(), config.max_pages);
        assert_eq!(bytes.get_u16_ne(), 0);
        assert_eq!(bytes.get_u32_ne(), (config.flags.bits() >> 32) as u32);
        assert_eq!(bytes.get_u32_ne(), config.max_stack_depth);
        assert_eq!(bytes.get_u16_ne(), config.request_timeout);
        assert_eq!(bytes, [0u8; 22]);
    }
}
