use crate::{
    bytes::Bytes,
    conn::Connection,
    mount::{Fusermount, MountOptions},
    notify::{self, Notify},
    op::{self, Forget, Operation},
    request::{RemainingData, RequestBuffer},
    session::{KernelConfig, KernelFlags, Session},
    types::{GID, PID, UID},
};
use libc::{ENOENT, ENOSYS};
use std::{
    future::Future,
    io,
    num::NonZeroUsize,
    panic,
    path::PathBuf,
    sync::{Arc, Weak},
};
use tokio::{
    io::unix::AsyncFd,
    task::{AbortHandle, JoinSet},
};

use self::reply::{
    ReplyAttr, ReplyBmap, ReplyCreate, ReplyData, ReplyDir, ReplyEntry, ReplyLock, ReplyLseek,
    ReplyOpen, ReplyPoll, ReplyStatfs, ReplyUnit, ReplyWrite, ReplyXattr,
};

const NUM_WORKERS_PER_THREAD: usize = 4;

macro_rules! define_ops {
    ($( $name:ident: { $Arg:ident, $Reply:ident } ),*$(,)*) => {$(
        #[allow(unused_variables)]
        fn $name(
            self: &Arc<Self>,
            req: Request<'_>,
            op: op::$Arg<'_>,
            reply: reply::$Reply<'_>,
        ) -> impl Future<Output = reply::Result> + Send {
            async {
                Err(reply::Error::unimplemented())
            }
        }
    )*};
}

pub trait Filesystem {
    define_ops! {
        lookup: { Lookup, ReplyEntry },
        getattr: { Getattr, ReplyAttr },
        setattr: { Setattr, ReplyAttr },
        readlink: { Readlink, ReplyData },
        symlink: { Symlink, ReplyEntry },
        mknod: { Mknod, ReplyEntry },
        mkdir: { Mkdir, ReplyEntry },
        unlink: { Unlink, ReplyUnit },
        rmdir: { Rmdir, ReplyUnit },
        rename: { Rename, ReplyUnit },
        link: { Link, ReplyEntry },
        open: { Open, ReplyOpen },
        read: { Read, ReplyData },
        release: { Release, ReplyUnit },
        statfs: { Statfs, ReplyStatfs },
        fsync: { Fsync, ReplyUnit },
        setxattr: { Setxattr, ReplyUnit },
        getxattr: { Getxattr, ReplyXattr },
        listxattr: { Listxattr, ReplyXattr },
        removexattr: { Removexattr, ReplyUnit },
        flush: { Flush, ReplyUnit },
        opendir: { Opendir, ReplyOpen },
        readdir: { Readdir, ReplyDir },
        releasedir: { Releasedir, ReplyUnit },
        fsyncdir: { Fsyncdir, ReplyUnit },
        getlk: { Getlk, ReplyLock },
        setlk: { Setlk, ReplyUnit },
        flock: { Flock, ReplyUnit },
        access: { Access, ReplyUnit },
        create: { Create, ReplyCreate },
        bmap: { Bmap, ReplyBmap },
        fallocate: { Fallocate, ReplyUnit },
        copy_file_range: { CopyFileRange, ReplyWrite },
        poll: { Poll, ReplyPoll },
        lseek: { Lseek, ReplyLseek },
    }

    #[allow(unused_variables)]
    fn write(
        self: &Arc<Self>,
        req: Request<'_>,
        op: op::Write<'_>,
        data: Data<'_>,
        reply: reply::ReplyWrite<'_>,
    ) -> impl Future<Output = reply::Result> + Send {
        async { Err(reply::Error::unimplemented()) }
    }

    #[allow(unused_variables)]
    fn forget(self: &Arc<Self>, forgets: &[Forget]) -> impl Future<Output = ()> + Send {
        async {}
    }

    #[allow(unused_variables)]
    fn notify_reply(
        self: &Arc<Self>,
        op: op::NotifyReply<'_>,
        data: Data<'_>,
    ) -> impl Future<Output = io::Result<()>> + Send {
        async { Ok(()) }
    }
}

struct Global {
    session: Session,
    conn: Connection,
}

impl Global {
    fn notifier(self: &Arc<Self>) -> Notifier {
        Notifier {
            global: Arc::downgrade(self),
        }
    }

    fn send_notify<T>(&self, notify: T) -> io::Result<()>
    where
        T: notify::Notify,
    {
        self.session.send_notify(&self.conn, notify)
    }
}

pub struct Notifier {
    global: Weak<Global>,
}

impl Notifier {
    pub fn send(&self, notify: impl Notify) -> io::Result<()> {
        if let Some(global) = self.global.upgrade() {
            global.send_notify(notify)?;
        }
        Ok(())
    }
}

pub struct Spawner<'a> {
    join_set: &'a mut JoinSet<io::Result<()>>,
}

impl Spawner<'_> {
    pub fn spawn<F>(&mut self, future: F) -> AbortHandle
    where
        F: Future<Output = io::Result<()>> + Send + 'static,
    {
        self.join_set.spawn(future)
    }

    pub fn spawn_blocking<F>(&mut self, f: F) -> AbortHandle
    where
        F: FnOnce() -> io::Result<()> + Send + 'static,
    {
        self.join_set.spawn_blocking(f)
    }
}

/// The context for a single FUSE request used by the filesystem.
pub struct Request<'req> {
    global: &'req Arc<Global>,
    buf: &'req RequestBuffer,
    join_set: &'req mut JoinSet<io::Result<()>>,
}

impl Request<'_> {
    pub fn uid(&self) -> UID {
        self.buf.uid()
    }

    pub fn gid(&self) -> GID {
        self.buf.gid()
    }

    pub fn pid(&self) -> PID {
        self.buf.pid()
    }

    pub fn notifier(&self) -> Notifier {
        self.global.notifier()
    }

    pub fn spawner(&mut self) -> Spawner<'_> {
        Spawner {
            join_set: self.join_set,
        }
    }
}

pub struct Data<'req> {
    inner: RemainingData<'req>,
}

impl io::Read for Data<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        self.inner.read_vectored(bufs)
    }
}

struct ReplySender<'req> {
    session: &'req Session,
    conn: &'req Connection,
    buf: &'req RequestBuffer,
}

impl ReplySender<'_> {
    fn send<B>(self, arg: B) -> reply::Result
    where
        B: Bytes,
    {
        self.session
            .send_reply(self.conn, self.buf.unique(), 0, arg)
            .map_err(reply::Error::Fatal)?;
        Ok(reply::Replied::new())
    }
}

pub mod reply {
    use super::ReplySender;
    use crate::{
        bytes::Bytes,
        types::{FileAttr, FileID, FileLock, FileType, NodeID, PollEvents, Statfs},
    };
    use bitflags::bitflags;
    use libc::{EIO, ENOSYS};
    use polyfuse_kernel::*;
    use std::{ffi::OsStr, io, mem, os::unix::prelude::*, time::Duration};
    use zerocopy::{FromZeros as _, IntoBytes as _};

    pub type Result<T = Replied, E = Error> = std::result::Result<T, E>;

    fn fill_fuse_attr(slot: &mut fuse_attr, attr: &FileAttr) {
        slot.ino = attr.ino.into_raw();
        slot.size = attr.size;
        slot.blocks = attr.blocks;
        slot.atime = attr.atime.as_secs();
        slot.mtime = attr.mtime.as_secs();
        slot.ctime = attr.ctime.as_secs();
        slot.atimensec = attr.atime.subsec_nanos();
        slot.mtimensec = attr.mtime.subsec_nanos();
        slot.ctimensec = attr.ctime.subsec_nanos();
        slot.mode = attr.mode.into_raw();
        slot.nlink = attr.nlink;
        slot.uid = attr.uid.into_raw();
        slot.gid = attr.gid.into_raw();
        slot.rdev = attr.rdev.into_kernel_dev();
        slot.blksize = attr.blksize;
    }

    /// The ZST to ensure that the filesystem responds to a FUSE request exactly once.
    #[derive(Debug)]
    pub struct Replied {
        _private: (),
    }
    impl Replied {
        pub(super) const fn new() -> Self {
            Self { _private: () }
        }
    }

    #[derive(Debug, thiserror::Error)]
    pub enum Error {
        #[error("operation failed with error code {}", _0)]
        Code(i32),

        #[error("fatal error: {}", _0)]
        Fatal(#[source] io::Error),
    }

    impl From<i32> for Error {
        fn from(code: i32) -> Self {
            Self::Code(code)
        }
    }

    impl From<io::Error> for Error {
        fn from(err: io::Error) -> Self {
            Self::Code(err.raw_os_error().unwrap_or(EIO))
        }
    }

    impl Error {
        pub const fn unimplemented() -> Self {
            Self::Code(ENOSYS)
        }
    }

    pub struct ReplyUnit<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyUnit<'req> {
        pub(super) const fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send(self) -> Result {
            self.sender.send(())
        }
    }

    pub struct ReplyEntry<'req> {
        sender: ReplySender<'req>,
        out: fuse_entry_out,
    }

    impl<'req> ReplyEntry<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self {
                sender,
                out: fuse_entry_out::new_zeroed(),
            }
        }

        /// Set the inode number of this entry.
        ///
        /// If the value is not set, it means that the entry is *negative*.
        /// Returning a negative entry is also possible with the `ENOENT` error,
        /// but the *zeroed* entries also have the ability to specify the lifetime
        /// of the entry cache by using the `ttl_entry` parameter.
        #[inline]
        pub fn ino(&mut self, ino: NodeID) {
            self.out.nodeid = ino.into_raw();
        }

        /// Fill attribute values about this entry.
        #[inline]
        pub fn attr(&mut self, attr: &FileAttr) {
            fill_fuse_attr(&mut self.out.attr, attr);
        }

        /// Set the generation of this entry.
        ///
        /// This parameter is used to distinguish the inode from the past one
        /// when the filesystem reuse inode numbers.  That is, the operations
        /// must ensure that the pair of entry's inode number and generation
        /// are unique for the lifetime of the filesystem.
        pub fn generation(&mut self, generation: u64) {
            self.out.generation = generation;
        }

        /// Set the validity timeout for inode attributes.
        ///
        /// The operations should set this value to very large
        /// when the changes of inode attributes are caused
        /// only by FUSE requests.
        pub fn ttl_attr(&mut self, ttl: Duration) {
            self.out.attr_valid = ttl.as_secs();
            self.out.attr_valid_nsec = ttl.subsec_nanos();
        }

        /// Set the validity timeout for the name.
        ///
        /// The operations should set this value to very large
        /// when the changes/deletions of directory entries are
        /// caused only by FUSE requests.
        pub fn ttl_entry(&mut self, ttl: Duration) {
            self.out.entry_valid = ttl.as_secs();
            self.out.entry_valid_nsec = ttl.subsec_nanos();
        }

        pub fn send(self) -> Result {
            self.sender.send(self.out.as_bytes())
        }
    }

    pub struct ReplyAttr<'req> {
        sender: ReplySender<'req>,
        out: fuse_attr_out,
    }

    impl<'req> ReplyAttr<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self {
                sender,
                out: fuse_attr_out::new_zeroed(),
            }
        }

        /// Return the object to fill attribute values.
        #[inline]
        pub fn attr(&mut self, attr: &FileAttr) {
            fill_fuse_attr(&mut self.out.attr, attr);
        }

        /// Set the validity timeout for this attribute.
        pub fn ttl(&mut self, ttl: Duration) {
            self.out.attr_valid = ttl.as_secs();
            self.out.attr_valid_nsec = ttl.subsec_nanos();
        }

        pub fn send(self) -> Result {
            self.sender.send(self.out.as_bytes())
        }
    }

    pub struct ReplyData<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyData<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send<B>(self, bytes: B) -> Result
        where
            B: Bytes,
        {
            self.sender.send(bytes)
        }
    }

    pub struct ReplyWrite<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyWrite<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send(self, size: u32) -> Result {
            self.sender
                .send(fuse_write_out { size, padding: 0 }.as_bytes())
        }
    }

    pub struct ReplyDir<'req> {
        sender: ReplySender<'req>,
        buf: Vec<u8>,
    }

    impl<'req> ReplyDir<'req> {
        pub(super) fn new(sender: ReplySender<'req>, capacity: usize) -> Self {
            Self {
                sender,
                buf: Vec::with_capacity(capacity),
            }
        }

        pub fn push_entry(
            &mut self,
            name: &OsStr,
            ino: NodeID,
            typ: Option<FileType>,
            offset: u64,
        ) -> bool {
            let name = name.as_bytes();
            let remaining = self.buf.capacity() - self.buf.len();

            let entry_size = mem::size_of::<fuse_dirent>() + name.len();
            let aligned_entry_size = aligned(entry_size);

            if remaining < aligned_entry_size {
                return true;
            }

            let typ = match typ {
                Some(FileType::BlockDevice) => libc::DT_BLK,
                Some(FileType::CharacterDevice) => libc::DT_CHR,
                Some(FileType::Directory) => libc::DT_DIR,
                Some(FileType::Fifo) => libc::DT_FIFO,
                Some(FileType::SymbolicLink) => libc::DT_LNK,
                Some(FileType::Regular) => libc::DT_REG,
                Some(FileType::Socket) => libc::DT_SOCK,
                None => libc::DT_UNKNOWN,
            };

            let dirent = fuse_dirent {
                ino: ino.into_raw(),
                off: offset,
                namelen: name.len().try_into().expect("name length is too long"),
                typ: typ as u32,
                name: [],
            };
            let lenbefore = self.buf.len();
            self.buf.extend_from_slice(dirent.as_bytes());
            self.buf.extend_from_slice(name);
            self.buf.resize(lenbefore + aligned_entry_size, 0);

            false
        }

        pub fn send(self) -> Result {
            self.sender.send(&self.buf[..])
        }
    }

    #[inline]
    const fn aligned(len: usize) -> usize {
        (len + mem::size_of::<u64>() - 1) & !(mem::size_of::<u64>() - 1)
    }

    pub struct ReplyOpen<'req> {
        sender: ReplySender<'req>,
        out: fuse_open_out,
    }

    impl<'req> ReplyOpen<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self {
                sender,
                out: fuse_open_out::new_zeroed(),
            }
        }

        /// Set the handle of opened file.
        pub fn fh(&mut self, fh: FileID) {
            self.out.fh = fh.into_raw();
        }

        /// Specify the flags for the opened file.
        pub fn flags(&mut self, flags: OpenOutFlags) {
            self.out.open_flags = flags.bits();
        }

        pub fn send(self) -> Result {
            self.sender.send(self.out.as_bytes())
        }
    }

    pub struct ReplyCreate<'req> {
        sender: ReplySender<'req>,
        entry_out: fuse_entry_out,
        open_out: fuse_open_out,
    }

    impl<'req> ReplyCreate<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self {
                sender,
                entry_out: fuse_entry_out::new_zeroed(),
                open_out: fuse_open_out::new_zeroed(),
            }
        }

        /// Set the inode number of this entry.
        ///
        /// See [`ReplyEntry::ino`] for details.
        #[inline]
        pub fn ino(&mut self, ino: NodeID) {
            self.entry_out.nodeid = ino.into_raw();
        }

        /// Fill attribute values about this entry.
        #[inline]
        pub fn attr(&mut self, attr: &FileAttr) {
            fill_fuse_attr(&mut self.entry_out.attr, attr);
        }

        /// Set the generation of this entry.
        ///
        /// See [`ReplyEntry::generation`] for details.
        pub fn generation(&mut self, generation: u64) {
            self.entry_out.generation = generation;
        }

        /// Set the validity timeout for inode attributes.
        ///
        /// See [`ReplyEntry::ttl_attr`] for details.
        pub fn ttl_attr(&mut self, ttl: Duration) {
            self.entry_out.attr_valid = ttl.as_secs();
            self.entry_out.attr_valid_nsec = ttl.subsec_nanos();
        }

        /// Set the validity timeout for the name.
        ///
        /// See [`ReplyEntry::ttl_entry`] for details.
        pub fn ttl_entry(&mut self, ttl: Duration) {
            self.entry_out.entry_valid = ttl.as_secs();
            self.entry_out.entry_valid_nsec = ttl.subsec_nanos();
        }

        /// Set the handle of opened file.
        pub fn fh(&mut self, fh: FileID) {
            self.open_out.fh = fh.into_raw();
        }

        /// Specify the flags for the opened file.
        pub fn flags(&mut self, flags: OpenOutFlags) {
            self.open_out.open_flags = flags.bits();
        }

        pub fn send(self) -> Result {
            self.sender
                .send((self.entry_out.as_bytes(), self.open_out.as_bytes()))
        }
    }

    bitflags! {
        #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
        #[repr(transparent)]
        pub struct OpenOutFlags: u32 {
            /// Indicates that the direct I/O is used on this file.
            const DIRECT_IO = FOPEN_DIRECT_IO;

            /// Indicates that the currently cached file data in the kernel
            /// need not be invalidated.
            const KEEP_CACHE = FOPEN_KEEP_CACHE;

            /// Indicates that the opened file is not seekable.
            const NONSEEKABLE = FOPEN_NONSEEKABLE;

            /// Enable caching of entries returned by `readdir`.
            ///
            /// This flag is meaningful only for `opendir` operations.
            const CACHE_DIR = FOPEN_CACHE_DIR;
        }
    }

    pub struct ReplyStatfs<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyStatfs<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send(self, st: &Statfs) -> Result {
            self.sender.send(
                fuse_statfs_out {
                    st: fuse_kstatfs {
                        bsize: st.bsize,
                        frsize: st.frsize,
                        blocks: st.blocks,
                        bfree: st.bfree,
                        bavail: st.bavail,
                        files: st.files,
                        ffree: st.ffree,
                        namelen: st.namelen,
                        padding: 0,
                        spare: [0; 6],
                    },
                }
                .as_bytes(),
            )
        }
    }

    pub struct ReplyXattr<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyXattr<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send_size(self, size: u32) -> Result {
            self.sender
                .send(fuse_getxattr_out { size, padding: 0 }.as_bytes())
        }

        pub fn send_value<B>(self, data: B) -> Result
        where
            B: Bytes,
        {
            self.sender.send(data)
        }
    }

    pub struct ReplyLock<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyLock<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send(self, lk: &FileLock) -> Result {
            self.sender.send(
                fuse_lk_out {
                    lk: fuse_file_lock {
                        typ: lk.typ,
                        start: lk.start,
                        end: lk.end,
                        pid: lk.pid.into_raw(),
                    },
                }
                .as_bytes(),
            )
        }
    }

    pub struct ReplyBmap<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyBmap<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send(self, block: u64) -> Result {
            self.sender.send(fuse_bmap_out { block }.as_bytes())
        }
    }

    pub struct ReplyPoll<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyPoll<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send(self, revents: PollEvents) -> Result {
            self.sender.send(
                fuse_poll_out {
                    revents: revents.bits(),
                    padding: 0,
                }
                .as_bytes(),
            )
        }
    }

    pub struct ReplyLseek<'req> {
        sender: ReplySender<'req>,
    }

    impl<'req> ReplyLseek<'req> {
        pub(super) fn new(sender: ReplySender<'req>) -> Self {
            Self { sender }
        }

        pub fn send(self, offset: u64) -> Result {
            self.sender.send(fuse_lseek_out { offset }.as_bytes())
        }
    }
}

pub struct Daemon {
    global: Arc<Global>,
    config: KernelConfig,
    fusermount: Fusermount,
    join_set: JoinSet<io::Result<()>>,
}

impl Daemon {
    pub async fn mount(
        mountpoint: PathBuf,
        mountopts: MountOptions,
        mut config: KernelConfig,
    ) -> io::Result<Self> {
        let (conn, fusermount) = crate::mount::mount(mountpoint, mountopts)?;
        let conn = Connection::from(conn);

        let mut session = Session::new();
        session.init(&conn, &mut config)?;

        Ok(Self {
            global: Arc::new(Global { session, conn }),
            config,
            fusermount,
            join_set: JoinSet::new(),
        })
    }

    pub fn config(&self) -> &KernelConfig {
        &self.config
    }

    pub fn notifier(&self) -> Notifier {
        self.global.notifier()
    }

    pub fn spawner(&mut self) -> Spawner<'_> {
        Spawner {
            join_set: &mut self.join_set,
        }
    }

    fn new_worker(&self, _i: usize) -> io::Result<Worker> {
        let conn = self.global.conn.try_ioc_clone()?;
        Ok(Worker {
            global: self.global.clone(),
            conn: AsyncFd::new(conn)?,
            join_set: JoinSet::new(),
        })
    }

    fn new_request_buffer(&self) -> io::Result<RequestBuffer> {
        if self.config.flags.contains(KernelFlags::SPLICE_READ) {
            RequestBuffer::new_splice(self.config.request_buffer_size())
        } else {
            RequestBuffer::new_fallback(self.config.request_buffer_size())
        }
    }

    pub async fn run<T>(mut self, fs: Arc<T>, num_workers: Option<NonZeroUsize>) -> io::Result<()>
    where
        T: Filesystem + Send + Sync + 'static,
    {
        let num_workers = num_workers
            .map(|n| n.get())
            .unwrap_or_else(|| num_cpus::get() * NUM_WORKERS_PER_THREAD);

        for i in 0..num_workers {
            let worker = self.new_worker(i)?;
            let fs = fs.clone();
            let buf = self.new_request_buffer()?;
            self.join_set
                .spawn(async move { worker.run(buf, fs).await });
        }

        while let Some(result) = self.join_set.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    tracing::error!("A worker thread is exited with error: {}", err);
                }
                Err(err) if err.is_panic() => panic::resume_unwind(err.into_panic()),
                Err(_err) => (),
            }
        }

        self.fusermount.unmount()?;

        Ok(())
    }
}

struct Worker {
    global: Arc<Global>,
    conn: AsyncFd<Connection>,
    join_set: JoinSet<io::Result<()>>,
}

impl Worker {
    async fn run<T>(mut self, mut buf: RequestBuffer, fs: Arc<T>) -> io::Result<()>
    where
        T: Filesystem,
    {
        while self.read_request(&mut buf).await? {
            self.handle_request(&buf, &fs).await?;
        }
        self.join_set.shutdown().await;
        Ok(())
    }

    async fn read_request(&self, buf: &mut RequestBuffer) -> io::Result<bool> {
        loop {
            let mut guard = self.conn.readable().await?;
            match guard.try_io(|conn| self.global.session.recv_request(conn.get_ref(), buf)) {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }

    async fn handle_request<T>(&mut self, buf: &RequestBuffer, fs: &Arc<T>) -> io::Result<()>
    where
        T: Filesystem,
    {
        let span = tracing::debug_span!("handle_request", unique = ?buf.unique());
        let _enter = span.enter();

        let (op, data) = buf
            .operation()
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        tracing::debug!(?op);

        let req = Request {
            global: &self.global,
            buf,
            join_set: &mut self.join_set,
        };

        let sender = ReplySender {
            session: &self.global.session,
            conn: self.conn.get_ref(),
            buf,
        };

        let result = match op {
            Operation::Lookup(op) => fs.lookup(req, op, ReplyEntry::new(sender)).await,
            Operation::Getattr(op) => fs.getattr(req, op, ReplyAttr::new(sender)).await,
            Operation::Setattr(op) => fs.setattr(req, op, ReplyAttr::new(sender)).await,
            Operation::Readlink(op) => fs.readlink(req, op, ReplyData::new(sender)).await,
            Operation::Symlink(op) => fs.symlink(req, op, ReplyEntry::new(sender)).await,
            Operation::Mknod(op) => fs.mknod(req, op, ReplyEntry::new(sender)).await,
            Operation::Mkdir(op) => fs.mkdir(req, op, ReplyEntry::new(sender)).await,
            Operation::Unlink(op) => fs.unlink(req, op, ReplyUnit::new(sender)).await,
            Operation::Rmdir(op) => fs.rmdir(req, op, ReplyUnit::new(sender)).await,
            Operation::Rename(op) => fs.rename(req, op, ReplyUnit::new(sender)).await,
            Operation::Link(op) => fs.link(req, op, ReplyEntry::new(sender)).await,
            Operation::Open(op) => fs.open(req, op, ReplyOpen::new(sender)).await,
            Operation::Read(op) => fs.read(req, op, ReplyData::new(sender)).await,
            Operation::Release(op) => fs.release(req, op, ReplyUnit::new(sender)).await,
            Operation::Statfs(op) => fs.statfs(req, op, ReplyStatfs::new(sender)).await,
            Operation::Fsync(op) => fs.fsync(req, op, ReplyUnit::new(sender)).await,
            Operation::Setxattr(op) => fs.setxattr(req, op, ReplyUnit::new(sender)).await,
            Operation::Getxattr(op) => fs.getxattr(req, op, ReplyXattr::new(sender)).await,
            Operation::Listxattr(op) => fs.listxattr(req, op, ReplyXattr::new(sender)).await,
            Operation::Removexattr(op) => fs.removexattr(req, op, ReplyUnit::new(sender)).await,
            Operation::Flush(op) => fs.flush(req, op, ReplyUnit::new(sender)).await,
            Operation::Opendir(op) => fs.opendir(req, op, ReplyOpen::new(sender)).await,
            Operation::Readdir(op) => {
                let capacity = op.size() as usize;
                fs.readdir(req, op, ReplyDir::new(sender, capacity)).await
            }
            Operation::Releasedir(op) => fs.releasedir(req, op, ReplyUnit::new(sender)).await,
            Operation::Fsyncdir(op) => fs.fsyncdir(req, op, ReplyUnit::new(sender)).await,
            Operation::Getlk(op) => fs.getlk(req, op, ReplyLock::new(sender)).await,
            Operation::Setlk(op) => fs.setlk(req, op, ReplyUnit::new(sender)).await,
            Operation::Flock(op) => fs.flock(req, op, ReplyUnit::new(sender)).await,
            Operation::Access(op) => fs.access(req, op, ReplyUnit::new(sender)).await,
            Operation::Create(op) => fs.create(req, op, ReplyCreate::new(sender)).await,
            Operation::Bmap(op) => fs.bmap(req, op, ReplyBmap::new(sender)).await,
            Operation::Fallocate(op) => fs.fallocate(req, op, ReplyUnit::new(sender)).await,
            Operation::CopyFileRange(op) => {
                fs.copy_file_range(req, op, ReplyWrite::new(sender)).await
            }
            Operation::Poll(op) => fs.poll(req, op, ReplyPoll::new(sender)).await,
            Operation::Lseek(op) => fs.lseek(req, op, ReplyLseek::new(sender)).await,
            Operation::Write(op) => {
                fs.write(req, op, Data { inner: data }, ReplyWrite::new(sender))
                    .await
            }
            Operation::NotifyReply(op) => {
                fs.notify_reply(op, Data { inner: data }).await?;
                return Ok(());
            }
            Operation::Forget(forgets) => {
                fs.forget(forgets.as_ref()).await;
                return Ok(());
            }
            Operation::Interrupt(op) => {
                tracing::warn!("interrupted(unique={})", op.unique());
                // TODO: handle interrupt requests.
                Err(ENOSYS.into())
            }
        };

        match result {
            Ok(..) => {}
            Err(reply::Error::Fatal(err)) => match err.raw_os_error() {
                Some(ENOENT) => {
                    // missing in processing queue
                }
                _ => return Err(err),
            },
            Err(reply::Error::Code(errno)) => {
                self.global
                    .session
                    .send_reply(self.conn.get_ref(), buf.unique(), errno, ())?
            }
        }

        Ok(())
    }
}
