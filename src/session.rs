use crate::{
    bytes::{Bytes, Decoder, FillBytes, POD},
    op::Operation,
};
use polyfuse_kernel::*;
use std::{
    cmp,
    convert::{TryFrom, TryInto as _},
    ffi::OsStr,
    fmt,
    io::{self, prelude::*, IoSlice, IoSliceMut},
    mem::{self, MaybeUninit},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use zerocopy::IntoBytes as _;

// The minimum supported ABI minor version by polyfuse.
const MINIMUM_SUPPORTED_MINOR_VERSION: u32 = 23;

const DEFAULT_MAX_PAGES_PER_REQ: usize = 32;
const MAX_MAX_PAGES: usize = 256;

const DEFAULT_MAX_WRITE: u32 = (FUSE_MIN_READ_BUFFER as usize
    - mem::size_of::<fuse_in_header>()
    - mem::size_of::<fuse_write_in>()) as u32;

#[inline]
fn pagesize() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

// ==== KernelConfig ====

/// Parameters for setting up the connection with FUSE driver
/// and the kernel side behavior.
#[non_exhaustive]
pub struct KernelConfig {
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
        Self {
            max_readahead: u32::MAX,
            max_background: 0,
            congestion_threshold: 0,
            max_write: DEFAULT_MAX_WRITE,
            max_pages: 0, // read only
            time_gran: 1,
            flags: KernelFlags::default(),
        }
    }
}

// TODO: add FUSE_IOCTL_DIR
bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct KernelFlags: u32 {
        /// The filesystem supports asynchronous read requests.
        const ASYNC_READ = FUSE_ASYNC_READ;

        /// The filesystem supports the `O_TRUNC` open flag.
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
        /// on `create` operations.
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
    }
}

impl Default for KernelFlags {
    fn default() -> Self {
        Self::ASYNC_READ
            | Self::PARALLEL_DIROPS
            | Self::AUTO_INVAL_DATA
            | Self::HANDLE_KILLPRIV
            | Self::ASYNC_DIO
            | Self::ATOMIC_O_TRUNC
            | Self::NO_OPEN_SUPPORT
            | Self::NO_OPENDIR_SUPPORT
    }
}

// ==== Session ====

/// The object containing the contextrual information about a FUSE session.
pub struct Session {
    config: KernelConfig,
    major: u32,
    minor: u32,
    exited: AtomicBool,
    notify_unique: AtomicU64,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session").finish()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.exit();
    }
}

impl Session {
    /// Initialize a FUSE session by communicating with the kernel driver over
    /// the established channel.
    pub fn init<T>(mut conn: T, mut config: KernelConfig) -> io::Result<Self>
    where
        T: io::Read + io::Write,
    {
        let mut header_in = fuse_in_header::default();
        let mut arg_in =
            vec![0u8; FUSE_MIN_READ_BUFFER as usize - mem::size_of::<fuse_in_header>()];
        loop {
            let len = conn.read_vectored(&mut [
                io::IoSliceMut::new(header_in.as_mut_bytes()),
                io::IoSliceMut::new(&mut arg_in[..]),
            ])?;

            if len < mem::size_of::<fuse_in_header>() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "request message is too short",
                ));
            }

            if !matches!(
                fuse_opcode::try_from(header_in.opcode),
                Ok(fuse_opcode::FUSE_INIT)
            ) {
                // 原理上、FUSE_INIT の処理が完了するまで他のリクエストが pop されることはない
                // - ref: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/fs/fuse/fuse_i.h?h=v6.15.9#n693
                // カーネル側の実装に問題があると解釈し、そのリクエストを単に無視する
                tracing::error!(
                    "ignore any filesystem operations received before FUSE_INIT handling (unique={}, opcode={})",
                    header_in.unique,
                    header_in.opcode,
                );
                continue;
            }

            let init_in = Decoder::new(&arg_in[..len - mem::size_of::<fuse_in_header>()])
                .fetch::<fuse_init_in>() //
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::Other, "failed to decode fuse_init_in")
                })?;

            if init_in.major > 7 {
                // major version が大きい場合、カーネルにダウングレードを要求する
                tracing::debug!(
                    "The requested ABI version {}.{} is too large (expected version is {}.x)\n",
                    init_in.major,
                    init_in.minor,
                    FUSE_KERNEL_VERSION,
                );
                tracing::debug!("  -> Wait for a second INIT request with an older version.");
                let args = fuse_init_out {
                    major: FUSE_KERNEL_VERSION,
                    minor: FUSE_KERNEL_MINOR_VERSION,
                    ..Default::default()
                };
                write_bytes(&mut conn, Reply::new(header_in.unique, 0, args.as_bytes()))?;
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
                write_bytes(&mut conn, Reply::new(header_in.unique, libc::EPROTO, ()))?;
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

            let mut additional_flags = 0;
            if init_in.flags & FUSE_MAX_PAGES != 0 {
                additional_flags |= FUSE_MAX_PAGES;
                config.max_write = cmp::min(config.max_write, (MAX_MAX_PAGES * pagesize()) as u32);
                config.max_pages = cmp::min(
                    config.max_write.div_ceil(pagesize() as u32),
                    u16::max_value() as u32,
                ) as u16;
            } else {
                config.max_write = cmp::min(
                    config.max_write,
                    (DEFAULT_MAX_PAGES_PER_REQ * pagesize()) as u32,
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
                flags: config.flags.bits() | additional_flags | FUSE_BIG_WRITES, // the flag was superseded by `max_write`
                max_background: config.max_background,
                time_gran: config.time_gran,
                congestion_threshold: config.congestion_threshold,
                max_write: config.max_write,
                max_pages: config.max_pages,
                padding: 0,
                unused: [0; 8],
            };
            write_bytes(
                &mut conn,
                Reply::new(header_in.unique, 0, init_out.as_bytes()),
            )?;

            return Ok(Self {
                config,
                major,
                minor,
                exited: AtomicBool::new(false),
                notify_unique: AtomicU64::new(0),
            });
        }
    }

    #[inline]
    pub fn exited(&self) -> bool {
        // FIXME: choose appropriate atomic ordering.
        self.exited.load(Ordering::SeqCst)
    }

    #[inline]
    fn exit(&self) {
        // FIXME: choose appropriate atomic ordering.
        self.exited.store(true, Ordering::SeqCst)
    }

    /// Return the ABI version number of the FUSE wire protocol used in this session.
    #[inline]
    pub fn protocol_version(&self) -> (u32, u32) {
        (self.major, self.minor)
    }

    /// Return the configuration after negotiating with the kernel via the FUSE_INIT request.
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

    /// Receive an incoming FUSE request from the kernel.
    pub fn next_request<T>(&self, mut conn: T) -> io::Result<Option<Request>>
    where
        T: io::Read + io::Write,
    {
        let mut header = fuse_in_header::default();
        let mut arg = vec![0u8; self.request_buffer_size() - mem::size_of::<fuse_in_header>()];

        loop {
            match conn.read_vectored(&mut [
                io::IoSliceMut::new(header.as_mut_bytes()),
                io::IoSliceMut::new(&mut arg[..]),
            ]) {
                Ok(len) => {
                    if len < mem::size_of::<fuse_in_header>() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "dequeued request message is too short",
                        ));
                    }

                    match fuse_opcode::try_from(header.opcode) {
                        Ok(fuse_opcode::FUSE_INIT) => {
                            // FUSE_INIT リクエストは Session の初期化時に処理しているはずなので、ここで読み込まれることはないはず
                            tracing::error!("unexpected FUSE_INIT request received");
                            continue;
                        }

                        Ok(fuse_opcode::FUSE_DESTROY) => {
                            // TODO: FUSE_DESTROY 後にリクエストの読み込みを中断するかどうかを決める
                            tracing::debug!("FUSE_DESTROY received");
                            self.exit();
                            return Ok(None);
                        }

                        Ok(fuse_opcode::FUSE_IOCTL)
                        | Ok(fuse_opcode::FUSE_LSEEK)
                        | Ok(fuse_opcode::CUSE_INIT)
                        | Err(..) => {
                            tracing::warn!(
                                "unsupported opcode (unique={}, opcode={})",
                                header.unique,
                                header.opcode
                            );
                            write_bytes(&mut conn, Reply::new(header.unique, libc::ENOSYS, ()))?;
                            continue;
                        }

                        Ok(opcode) => {
                            // FIXME: impl fmt::Debug for fuse_opcode
                            tracing::debug!(
                                "Got a request (unique={}, opcode={})",
                                header.unique,
                                header.opcode
                            );
                            arg.resize(len - mem::size_of::<fuse_in_header>(), 0);
                            break Ok(Some(Request {
                                header,
                                opcode,
                                arg,
                            }));
                        }
                    }
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
    }
}

// ==== Request ====

/// Context about an incoming FUSE request.
pub struct Request {
    header: fuse_in_header,
    opcode: fuse_opcode,
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
    pub fn operation(&self) -> Result<Operation<'_, Data<'_>>, crate::op::Error> {
        let arg: &[u8] = &self.arg[..];

        let (arg, data) = match fuse_opcode::try_from(self.header.opcode).ok() {
            Some(fuse_opcode::FUSE_WRITE) | Some(fuse_opcode::FUSE_NOTIFY_REPLY) => {
                arg.split_at(mem::size_of::<fuse_write_in>())
            }
            _ => (arg, &[] as &[_]),
        };

        Operation::decode(&self.header, self.opcode, arg, Data { data })
    }
}

impl Session {
    pub fn reply<T, B>(&self, conn: T, req: &Request, arg: B) -> io::Result<()>
    where
        T: io::Write,
        B: Bytes,
    {
        write_bytes(conn, Reply::new(req.unique(), 0, arg))
    }

    pub fn reply_error<T>(&self, conn: T, req: &Request, code: i32) -> io::Result<()>
    where
        T: io::Write,
    {
        write_bytes(conn, Reply::new(req.unique(), code, ()))
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

impl Session {
    /// Notify the cache invalidation about an inode to the kernel.
    pub fn notify_inval_inode<T>(&self, conn: T, ino: u64, off: i64, len: i64) -> io::Result<()>
    where
        T: io::Write,
    {
        write_bytes(
            conn,
            Notify::new(
                fuse_notify_code::FUSE_NOTIFY_INVAL_INODE,
                POD(fuse_notify_inval_inode_out { ino, off, len }),
            ),
        )
    }

    /// Notify the invalidation about a directory entry to the kernel.
    pub fn notify_inval_entry<T>(&self, conn: T, parent: u64, name: &OsStr) -> io::Result<()>
    where
        T: io::Write,
    {
        let namelen = name.len().try_into().expect("provided name is too long");

        write_bytes(
            conn,
            Notify::new(
                fuse_notify_code::FUSE_NOTIFY_INVAL_ENTRY,
                (
                    POD(fuse_notify_inval_entry_out {
                        parent,
                        namelen,
                        padding: 0,
                    }),
                    (name, b"\0".as_slice()),
                ),
            ),
        )
    }

    /// Notify the invalidation about a directory entry to the kernel.
    ///
    /// The role of this notification is similar to `notify_inval_entry`.
    /// Additionally, when the provided `child` inode matches the inode
    /// in the dentry cache, the inotify will inform the deletion to
    /// watchers if exists.
    pub fn notify_delete<T>(&self, conn: T, parent: u64, child: u64, name: &OsStr) -> io::Result<()>
    where
        T: io::Write,
    {
        let namelen = name.len().try_into().expect("provided name is too long");

        write_bytes(
            conn,
            Notify::new(
                fuse_notify_code::FUSE_NOTIFY_DELETE,
                (
                    POD(fuse_notify_delete_out {
                        parent,
                        child,
                        namelen,
                        padding: 0,
                    }),
                    (name, b"\0".as_ref()),
                ),
            ),
        )
    }

    /// Push the data in an inode for updating the kernel cache.
    pub fn notify_store<T, B>(&self, conn: T, ino: u64, offset: u64, data: B) -> io::Result<()>
    where
        T: io::Write,
        B: Bytes,
    {
        let size = data.size().try_into().expect("provided data is too large");

        write_bytes(
            conn,
            Notify::new(
                fuse_notify_code::FUSE_NOTIFY_STORE,
                (
                    POD(fuse_notify_store_out {
                        nodeid: ino,
                        offset,
                        size,
                        padding: 0,
                    }),
                    data,
                ),
            ),
        )
    }

    /// Retrieve data in an inode from the kernel cache.
    pub fn notify_retrieve<T>(&self, conn: T, ino: u64, offset: u64, size: u32) -> io::Result<u64>
    where
        T: io::Write,
    {
        // FIXME: choose appropriate memory ordering.
        let notify_unique = self.notify_unique.fetch_add(1, Ordering::SeqCst);

        write_bytes(
            conn,
            Notify::new(
                fuse_notify_code::FUSE_NOTIFY_RETRIEVE,
                POD(fuse_notify_retrieve_out {
                    nodeid: ino,
                    offset,
                    size,
                    notify_unique,
                    padding: 0,
                }),
            ),
        )?;

        Ok(notify_unique)
    }

    /// Send I/O readiness to the kernel.
    pub fn notify_poll_wakeup<T>(&self, conn: T, kh: u64) -> io::Result<()>
    where
        T: io::Write,
    {
        write_bytes(
            conn,
            Notify::new(
                fuse_notify_code::FUSE_NOTIFY_POLL,
                POD(fuse_notify_poll_wakeup_out { kh }),
            ),
        )
    }
}

// ==== utils ====

struct Reply<T> {
    header: fuse_out_header,
    arg: T,
}
impl<T> Reply<T>
where
    T: Bytes,
{
    #[inline]
    fn new(unique: u64, error: i32, arg: T) -> Self {
        let len = (mem::size_of::<fuse_out_header>() + arg.size())
            .try_into()
            .expect("Argument size is too large");
        Self {
            header: fuse_out_header {
                len,
                error: -error,
                unique,
            },
            arg,
        }
    }
}
impl<T> Bytes for Reply<T>
where
    T: Bytes,
{
    #[inline]
    fn size(&self) -> usize {
        self.header.len as usize
    }

    #[inline]
    fn count(&self) -> usize {
        self.arg.count() + 1
    }

    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        dst.put(self.header.as_bytes());
        self.arg.fill_bytes(dst);
    }
}

struct Notify<T> {
    header: fuse_out_header,
    arg: T,
}
impl<T> Notify<T>
where
    T: Bytes,
{
    #[inline]
    fn new(code: fuse_notify_code, arg: T) -> Self {
        let len = (mem::size_of::<fuse_out_header>() + arg.size())
            .try_into()
            .expect("Argument size is too large");
        Self {
            header: fuse_out_header {
                len,
                error: code as i32,
                unique: 0,
            },
            arg,
        }
    }
}
impl<T> Bytes for Notify<T>
where
    T: Bytes,
{
    #[inline]
    fn size(&self) -> usize {
        self.header.len as usize
    }

    #[inline]
    fn count(&self) -> usize {
        self.arg.count() + 1
    }

    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        dst.put(self.header.as_bytes());
        self.arg.fill_bytes(dst);
    }
}

#[inline]
fn write_bytes<W, T>(mut writer: W, bytes: T) -> io::Result<()>
where
    W: io::Write,
    T: Bytes,
{
    let size = bytes.size();
    let count = bytes.count();

    let written;

    macro_rules! small_write {
        ($n:expr) => {{
            let mut vec: [MaybeUninit<IoSlice<'_>>; $n] =
                unsafe { MaybeUninit::uninit().assume_init() };
            bytes.fill_bytes(&mut FillWriteBytes {
                vec: &mut vec[..],
                offset: 0,
            });
            let vec = unsafe { slice_assume_init_ref(&vec[..]) };

            written = writer.write_vectored(vec)?;
        }};
    }

    match count {
        // Skip writing.
        0 => return Ok(()),

        // Avoid heap allocation if count is small.
        1 => small_write!(1),
        2 => small_write!(2),
        3 => small_write!(3),
        4 => small_write!(4),

        count => {
            let mut vec: Vec<IoSlice<'_>> = Vec::with_capacity(count);
            unsafe {
                let dst = std::slice::from_raw_parts_mut(
                    vec.as_mut_ptr().cast(), //
                    count,
                );
                bytes.fill_bytes(&mut FillWriteBytes {
                    vec: dst,
                    offset: 0,
                });
                vec.set_len(count);
            }

            written = writer.write_vectored(&vec)?;
        }
    }

    if written < size {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "written data is too short",
        ));
    }

    Ok(())
}

struct FillWriteBytes<'a, 'vec> {
    vec: &'vec mut [MaybeUninit<IoSlice<'a>>],
    offset: usize,
}

impl<'a, 'vec> FillBytes<'a> for FillWriteBytes<'a, 'vec> {
    fn put(&mut self, chunk: &'a [u8]) {
        self.vec[self.offset] = MaybeUninit::new(IoSlice::new(chunk));
        self.offset += 1;
    }
}

// FIXME: replace with stabilized MaybeUninit::slice_assume_init_ref.
#[inline(always)]
unsafe fn slice_assume_init_ref<T>(slice: &[MaybeUninit<T>]) -> &[T] {
    #[allow(unused_unsafe)]
    unsafe {
        &*(slice as *const [MaybeUninit<T>] as *const [T])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem;

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

        fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
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

        fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
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
            opcode: fuse_opcode::FUSE_INIT as u32,
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

        let session = Session::init(
            Unite {
                reader: &input[..],
                writer: &mut output,
            },
            KernelConfig::default(),
        )
        .expect("initialization failed");

        let expected_max_pages = DEFAULT_MAX_WRITE.div_ceil(pagesize() as u32) as u16;

        assert_eq!(session.major, FUSE_KERNEL_VERSION);
        assert_eq!(session.minor, FUSE_KERNEL_MINOR_VERSION);
        assert_eq!(session.config.max_readahead, 40);
        assert_eq!(session.config.max_background, 0);
        assert_eq!(session.config.congestion_threshold, 0);
        assert_eq!(session.config.max_write, DEFAULT_MAX_WRITE);
        assert_eq!(session.config.max_pages, expected_max_pages);
        assert_eq!(session.config.time_gran, 1);
        assert_eq!(session.config.flags, KernelFlags::default());

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
            flags: KernelFlags::default().bits() | FUSE_MAX_PAGES | FUSE_BIG_WRITES,
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
    fn send_msg_empty() {
        let mut buf = vec![0u8; 0];
        write_bytes(&mut buf, Reply::new(42, -4, &[])).unwrap();
        assert_eq!(buf[0..4], b![0x10, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(buf[4..8], b![0x04, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            buf[8..16],
            b![0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
    }

    #[test]
    fn send_msg_single_data() {
        let mut buf = vec![0u8; 0];
        write_bytes(&mut buf, Reply::new(42, 0, "hello")).unwrap();
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
        let payload: &[&[u8]] = &[
            "hello, ".as_ref(), //
            "this ".as_ref(),
            "is a ".as_ref(),
            "message.".as_ref(),
        ];
        let mut buf = vec![0u8; 0];
        write_bytes(&mut buf, Reply::new(26, 0, payload)).unwrap();
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
