//! Lowlevel interface to handle FUSE requests.

use crate::{
    fs::{FileLock, Filesystem, Operation},
    reply::{Payload, ReplyData},
};
use futures::{
    channel::oneshot,
    future::{poll_fn, FusedFuture, FutureExt},
    io::{AsyncRead, AsyncWrite},
    lock::Mutex,
    select,
};
use polyfuse_sys::abi::{
    fuse_access_in, //
    fuse_bmap_in,
    fuse_create_in,
    fuse_flush_in,
    fuse_forget_in,
    fuse_fsync_in,
    fuse_getattr_in,
    fuse_getxattr_in,
    fuse_in_header,
    fuse_init_in,
    fuse_init_out,
    fuse_interrupt_in,
    fuse_link_in,
    fuse_lk_in,
    fuse_mkdir_in,
    fuse_mknod_in,
    fuse_opcode,
    fuse_open_in,
    fuse_out_header,
    fuse_read_in,
    fuse_release_in,
    fuse_rename_in,
    fuse_setattr_in,
    fuse_setxattr_in,
    fuse_write_in,
};
use smallvec::SmallVec;
use std::{
    collections::HashMap,
    convert::TryFrom,
    ffi::OsStr,
    fmt,
    io::{self, IoSlice},
    mem,
    os::unix::ffi::OsStrExt,
    pin::Pin,
    task::{self, Poll},
};

pub const MAX_WRITE_SIZE: u32 = 16 * 1024 * 1024;

/// An incoming FUSE request received from the kernel.
#[derive(Debug)]
pub struct Request<'a> {
    header: &'a fuse_in_header,
    kind: RequestKind<'a>,
    _p: (),
}

#[derive(Debug)]
enum RequestKind<'a> {
    Init {
        arg: &'a fuse_init_in,
    },
    Destroy,
    Lookup {
        name: &'a OsStr,
    },
    Forget {
        arg: &'a fuse_forget_in,
    },
    Getattr {
        arg: &'a fuse_getattr_in,
    },
    Setattr {
        arg: &'a fuse_setattr_in,
    },
    Readlink,
    Symlink {
        name: &'a OsStr,
        link: &'a OsStr,
    },
    Mknod {
        arg: &'a fuse_mknod_in,
        name: &'a OsStr,
    },
    Mkdir {
        arg: &'a fuse_mkdir_in,
        name: &'a OsStr,
    },
    Unlink {
        name: &'a OsStr,
    },
    Rmdir {
        name: &'a OsStr,
    },
    Rename {
        arg: &'a fuse_rename_in,
        name: &'a OsStr,
        newname: &'a OsStr,
    },
    Link {
        arg: &'a fuse_link_in,
        newname: &'a OsStr,
    },
    Open {
        arg: &'a fuse_open_in,
    },
    Read {
        arg: &'a fuse_read_in,
    },
    Write {
        arg: &'a fuse_write_in,
    },
    Release {
        arg: &'a fuse_release_in,
    },
    Statfs,
    Fsync {
        arg: &'a fuse_fsync_in,
    },
    Setxattr {
        arg: &'a fuse_setxattr_in,
        name: &'a OsStr,
        value: &'a [u8],
    },
    Getxattr {
        arg: &'a fuse_getxattr_in,
        name: &'a OsStr,
    },
    Listxattr {
        arg: &'a fuse_getxattr_in,
    },
    Removexattr {
        name: &'a OsStr,
    },
    Flush {
        arg: &'a fuse_flush_in,
    },
    Opendir {
        arg: &'a fuse_open_in,
    },
    Readdir {
        arg: &'a fuse_read_in,
    },
    Releasedir {
        arg: &'a fuse_release_in,
    },
    Fsyncdir {
        arg: &'a fuse_fsync_in,
    },
    Getlk {
        arg: &'a fuse_lk_in,
    },
    Setlk {
        arg: &'a fuse_lk_in,
        sleep: bool,
    },
    Access {
        arg: &'a fuse_access_in,
    },
    Create {
        arg: &'a fuse_create_in,
        name: &'a OsStr,
    },
    Interrupt {
        arg: &'a fuse_interrupt_in,
    },
    Bmap {
        arg: &'a fuse_bmap_in,
    },
    // Ioctl,
    // Poll,
    // NotifyReply,
    // BatchForget,
    // Fallocate,
    // Readdirplus,
    // Rename2,
    // Lseek,
    // CopyFileRange,
    Unknown,
}

impl Request<'_> {
    pub fn unique(&self) -> u64 {
        self.header.unique
    }

    pub fn opcode(&self) -> Option<fuse_opcode> {
        fuse_opcode::try_from(self.header.opcode).ok()
    }
}

trait FromBytes<'a> {
    const SIZE: usize;

    unsafe fn from_bytes(bytes: &'a [u8]) -> &'a Self;
}

macro_rules! impl_from_bytes {
    ($($t:ty,)*) => {$(
        impl<'a> FromBytes<'a> for $t {
            const SIZE: usize = mem::size_of::<Self>();

            unsafe fn from_bytes(bytes: &'a [u8]) -> &'a Self {
                debug_assert_eq!(bytes.len(), Self::SIZE);
                &*(bytes.as_ptr() as *const Self)
            }
        }
    )*};
}

impl_from_bytes! {
    fuse_in_header,
    fuse_init_in,
    fuse_forget_in,
    fuse_getattr_in,
    fuse_setattr_in,
    fuse_mknod_in,
    fuse_mkdir_in,
    fuse_rename_in,
    fuse_link_in,
    fuse_open_in,
    fuse_read_in,
    fuse_write_in,
    fuse_release_in,
    fuse_fsync_in,
    fuse_setxattr_in,
    fuse_getxattr_in,
    fuse_flush_in,
    fuse_lk_in,
    fuse_access_in,
    fuse_create_in,
    fuse_interrupt_in,
    fuse_bmap_in,
}

#[derive(Debug)]
struct Parser<'a> {
    buf: &'a [u8],
    offset: usize,
}

impl<'a> Parser<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, offset: 0 }
    }

    fn fetch_bytes(&mut self, count: usize) -> io::Result<&'a [u8]> {
        if self.buf.len() < self.offset + count {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "fetch"));
        }

        let data = &self.buf[self.offset..self.offset + count];
        self.offset += count;

        Ok(data)
    }

    fn fetch_str(&mut self) -> io::Result<&'a OsStr> {
        let len = self.buf[self.offset..]
            .iter()
            .position(|&b| b == b'\0')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "fetch_str: missing \\0"))?;
        self.fetch_bytes(len).map(OsStr::from_bytes)
    }

    fn fetch<T: FromBytes<'a>>(&mut self) -> io::Result<&'a T> {
        self.fetch_bytes(T::SIZE)
            .map(|data| unsafe { T::from_bytes(data) })
    }

    fn offset(&self) -> usize {
        self.offset
    }

    fn parse(&mut self) -> io::Result<(&'a fuse_in_header, RequestKind<'a>, usize)> {
        let header = self.parse_header()?;
        let arg = self.parse_arg(header)?;
        Ok((header, arg, self.offset()))
    }

    #[allow(clippy::cast_ptr_alignment)]
    fn parse_header(&mut self) -> io::Result<&'a fuse_in_header> {
        let header = self.fetch::<fuse_in_header>()?;

        if self.buf.len() < header.len as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "received data is too short",
            ));
        }

        Ok(header)
    }

    fn parse_arg(&mut self, header: &'a fuse_in_header) -> io::Result<RequestKind<'a>> {
        match fuse_opcode::try_from(header.opcode).ok() {
            Some(fuse_opcode::FUSE_INIT) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Init { arg })
            }
            Some(fuse_opcode::FUSE_DESTROY) => Ok(RequestKind::Destroy),
            Some(fuse_opcode::FUSE_LOOKUP) => {
                let name = self.fetch_str()?;
                Ok(RequestKind::Lookup { name })
            }
            Some(fuse_opcode::FUSE_FORGET) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Forget { arg })
            }
            Some(fuse_opcode::FUSE_GETATTR) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Getattr { arg })
            }
            Some(fuse_opcode::FUSE_SETATTR) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Setattr { arg })
            }
            Some(fuse_opcode::FUSE_READLINK) => Ok(RequestKind::Readlink),
            Some(fuse_opcode::FUSE_SYMLINK) => {
                let name = self.fetch_str()?;
                let link = self.fetch_str()?;
                Ok(RequestKind::Symlink { name, link })
            }
            Some(fuse_opcode::FUSE_MKNOD) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(RequestKind::Mknod { arg, name })
            }
            Some(fuse_opcode::FUSE_MKDIR) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(RequestKind::Mkdir { arg, name })
            }
            Some(fuse_opcode::FUSE_UNLINK) => {
                let name = self.fetch_str()?;
                Ok(RequestKind::Unlink { name })
            }
            Some(fuse_opcode::FUSE_RMDIR) => {
                let name = self.fetch_str()?;
                Ok(RequestKind::Rmdir { name })
            }
            Some(fuse_opcode::FUSE_RENAME) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                let newname = self.fetch_str()?;
                Ok(RequestKind::Rename { arg, name, newname })
            }
            Some(fuse_opcode::FUSE_LINK) => {
                let arg = self.fetch()?;
                let newname = self.fetch_str()?;
                Ok(RequestKind::Link { arg, newname })
            }
            Some(fuse_opcode::FUSE_OPEN) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Open { arg })
            }
            Some(fuse_opcode::FUSE_READ) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Read { arg })
            }
            Some(fuse_opcode::FUSE_WRITE) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Write { arg })
            }
            Some(fuse_opcode::FUSE_RELEASE) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Release { arg })
            }
            Some(fuse_opcode::FUSE_STATFS) => Ok(RequestKind::Statfs),
            Some(fuse_opcode::FUSE_FSYNC) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Fsync { arg })
            }
            Some(fuse_opcode::FUSE_SETXATTR) => {
                let arg: &fuse_setxattr_in = self.fetch()?;
                let name = self.fetch_str()?;
                let value = self.fetch_bytes(arg.size as usize)?;
                Ok(RequestKind::Setxattr { arg, name, value })
            }
            Some(fuse_opcode::FUSE_GETXATTR) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(RequestKind::Getxattr { arg, name })
            }
            Some(fuse_opcode::FUSE_LISTXATTR) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Listxattr { arg })
            }
            Some(fuse_opcode::FUSE_REMOVEXATTR) => {
                let name = self.fetch_str()?;
                Ok(RequestKind::Removexattr { name })
            }
            Some(fuse_opcode::FUSE_FLUSH) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Flush { arg })
            }
            Some(fuse_opcode::FUSE_OPENDIR) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Opendir { arg })
            }
            Some(fuse_opcode::FUSE_READDIR) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Readdir { arg })
            }
            Some(fuse_opcode::FUSE_RELEASEDIR) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Releasedir { arg })
            }
            Some(fuse_opcode::FUSE_FSYNCDIR) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Fsyncdir { arg })
            }
            Some(fuse_opcode::FUSE_GETLK) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Getlk { arg })
            }
            Some(fuse_opcode::FUSE_SETLK) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Setlk { arg, sleep: false })
            }
            Some(fuse_opcode::FUSE_SETLKW) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Setlk { arg, sleep: true })
            }
            Some(fuse_opcode::FUSE_ACCESS) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Access { arg })
            }
            Some(fuse_opcode::FUSE_CREATE) => {
                let arg = self.fetch()?;
                let name = self.fetch_str()?;
                Ok(RequestKind::Create { arg, name })
            }
            Some(fuse_opcode::FUSE_INTERRUPT) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Interrupt { arg })
            }
            Some(fuse_opcode::FUSE_BMAP) => {
                let arg = self.fetch()?;
                Ok(RequestKind::Bmap { arg })
            }
            _ => Ok(RequestKind::Unknown),
        }
    }
}

/// A buffer to hold request data from the kernel.
#[derive(Debug)]
pub struct Buffer {
    recv_buf: Vec<u8>,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new(Self::DEFAULT_BUF_SIZE)
    }
}

impl Buffer {
    pub const DEFAULT_BUF_SIZE: usize = MAX_WRITE_SIZE as usize + 4096;

    /// Create a new `Buffer`.
    pub fn new(bufsize: usize) -> Self {
        Self {
            recv_buf: Vec::with_capacity(bufsize),
        }
    }

    /// Acquires an incoming request from the kernel.
    ///
    /// The received data is stored in the internal buffer, and could be
    /// retrieved using `decode`.
    pub fn poll_receive<I: ?Sized>(
        &mut self,
        cx: &mut task::Context,
        io: &mut I,
    ) -> Poll<io::Result<bool>>
    where
        I: AsyncRead + Unpin,
    {
        let old_len = self.recv_buf.len();
        unsafe {
            let capacity = self.recv_buf.capacity();
            self.recv_buf.set_len(capacity);
        }

        loop {
            match Pin::new(&mut *io).poll_read(cx, &mut self.recv_buf[..]) {
                Poll::Pending => {
                    unsafe {
                        self.recv_buf.set_len(old_len);
                    }
                    return Poll::Pending;
                }
                Poll::Ready(Ok(count)) => {
                    unsafe {
                        self.recv_buf.set_len(count);
                    }
                    return Poll::Ready(Ok(false));
                }
                Poll::Ready(Err(err)) => match err.raw_os_error() {
                    Some(libc::ENOENT) | Some(libc::EINTR) => {
                        log::debug!("continue reading from the kernel");
                        continue;
                    }
                    Some(libc::ENODEV) => {
                        log::debug!("the connection was closed by the kernel");
                        return Poll::Ready(Ok(true));
                    }
                    _ => return Poll::Ready(Err(err)),
                },
            }
        }
    }

    /// Receive a request from the kernel asynchronously.
    ///
    /// This method is a helper to call `poll_receive` in async functions.
    pub async fn receive<I: ?Sized>(&mut self, io: &mut I) -> io::Result<bool>
    where
        I: AsyncRead + Unpin,
    {
        poll_fn(move |cx| self.poll_receive(cx, io)).await
    }

    /// Extract the last incoming request.
    pub fn decode(&mut self) -> io::Result<(Request<'_>, Option<&[u8]>)> {
        let (header, kind, offset) = Parser::new(&self.recv_buf[..]).parse()?;
        let data = match kind {
            RequestKind::Write { arg: write_in } => {
                let size = write_in.size as usize;
                if offset + size < self.recv_buf.len() {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "receive_write"));
                }
                Some(&self.recv_buf[offset..offset + size])
            }
            _ => None,
        };
        Ok((
            Request {
                header,
                kind,
                _p: (),
            },
            data,
        ))
    }
}

/// FUSE session driver.
#[derive(Debug)]
pub struct Session {
    proto_major: u32,
    proto_minor: u32,
    max_readahead: u32,
    state: Mutex<SessionState>,
}

#[derive(Debug)]
struct SessionState {
    exited: bool,
    remains: HashMap<u64, oneshot::Sender<()>>,
}

impl Session {
    /// Start a new FUSE session.
    ///
    /// This function receives an INIT request from the kernel and replies
    /// after initializing the connection parameters.
    pub async fn start<I>(io: &mut I, initializer: SessionInitializer) -> io::Result<Self>
    where
        I: AsyncRead + AsyncWrite + Unpin,
    {
        drop(initializer);

        let mut buf = Buffer::default();

        loop {
            let terminated = buf.receive(io).await?;
            if terminated {
                log::warn!("the connection is closed");
                return Err(io::Error::from_raw_os_error(libc::ENODEV));
            }

            let (Request { header, kind, .. }, _data) = buf.decode()?;

            let (proto_major, proto_minor, max_readahead);
            match kind {
                RequestKind::Init { arg } => {
                    let mut init_out = fuse_init_out::default();

                    if arg.major > 7 {
                        log::debug!("wait for a second INIT request with a 7.X version.");
                        send_reply(&mut *io, header.unique, 0, &[init_out.as_bytes()]).await?;
                        continue;
                    }

                    if arg.major < 7 || (arg.major == 7 && arg.minor < 6) {
                        log::warn!("unsupported protocol version: {}.{}", arg.major, arg.minor);
                        send_reply(&mut *io, header.unique, libc::EPROTO, &[]).await?;
                        return Err(io::Error::from_raw_os_error(libc::EPROTO));
                    }

                    // remember the kernel parameters.
                    proto_major = arg.major;
                    proto_minor = arg.minor;
                    max_readahead = arg.max_readahead;

                    // TODO: max_background, congestion_threshold, time_gran, max_pages
                    init_out.max_readahead = arg.max_readahead;
                    init_out.max_write = MAX_WRITE_SIZE;

                    send_reply(&mut *io, header.unique, 0, &[init_out.as_bytes()]).await?;
                }
                _ => {
                    log::warn!(
                        "ignoring an operation before init (opcode={:?})",
                        header.opcode
                    );
                    send_reply(&mut *io, header.unique, libc::EIO, &[]).await?;
                    continue;
                }
            }

            return Ok(Session {
                proto_major,
                proto_minor,
                max_readahead,
                state: Mutex::new(SessionState {
                    exited: false,
                    remains: HashMap::new(),
                }),
            });
        }
    }

    /// Process an incoming request using the specified filesystem operations.
    #[allow(clippy::cognitive_complexity)]
    pub async fn process<F, T, W>(
        &self,
        fs: &F,
        req: Request<'_>,
        data: Option<T>,
        writer: &mut W,
    ) -> io::Result<()>
    where
        F: Filesystem<T>,
        T: Send,
        W: AsyncWrite + Send + Unpin,
    {
        if self.state.lock().await.exited {
            log::warn!("The sesson has already been exited");
            return Ok(());
        }

        let Request { header, kind, .. } = req;
        let ino = header.nodeid;

        let mut cx = Context {
            header,
            writer: &mut *writer,
            session: &*self,
        };

        macro_rules! run_op {
            ($op:expr) => {
                let mut intr = {
                    let mut state = self.state.lock().await;
                    let (tx, rx) = oneshot::channel();
                    state.remains.insert(header.unique, tx);
                    rx.fuse()
                };
                let mut task = fs.call(&mut cx, $op).fuse();
                select! {
                    _ = intr => {
                        log::debug!("interrupted (unique = {})", header.unique);
                    },
                    res = task => res?,
                }
                drop(task);
                if intr.is_terminated() {
                    cx.reply_err(libc::EINTR).await?;
                }
            };
        }

        match kind {
            RequestKind::Init { .. } => {
                log::warn!("");
                cx.reply_err(libc::EIO).await?;
            }
            RequestKind::Destroy => {
                self.state.lock().await.exited = true;
                cx.send_reply(0, &[]).await?;
            }
            RequestKind::Interrupt { arg } => {
                log::debug!("INTERRUPT (unique = {:?})", arg.unique);
                let mut state = self.state.lock().await;
                if let Some(tx) = state.remains.remove(&arg.unique) {
                    let _ = tx.send(());
                    log::debug!("Sent interrupt signal to unique={}", arg.unique);
                }
            }
            RequestKind::Lookup { name } => {
                run_op!(Operation::Lookup {
                    parent: ino,
                    name,
                    reply: Default::default(),
                });
            }
            RequestKind::Forget { arg } => {
                // no reply.
                fs.call(
                    &mut cx,
                    Operation::Forget {
                        nlookups: &[(ino, arg.nlookup)],
                    },
                )
                .await?;
            }
            RequestKind::Getattr { arg } => {
                run_op!(Operation::Getattr {
                    ino,
                    fh: arg.fh(),
                    reply: Default::default(),
                });
            }
            RequestKind::Setattr { arg } => {
                run_op!(Operation::Setattr {
                    ino,
                    fh: arg.fh(),
                    mode: arg.mode(),
                    uid: arg.uid(),
                    gid: arg.gid(),
                    size: arg.size(),
                    atime: arg.atime(),
                    mtime: arg.mtime(),
                    ctime: arg.ctime(),
                    lock_owner: arg.lock_owner(),
                    reply: Default::default(),
                });
            }
            RequestKind::Readlink => {
                run_op!(Operation::Readlink {
                    ino,
                    reply: Default::default(),
                });
            }
            RequestKind::Symlink { name, link } => {
                run_op!(Operation::Symlink {
                    parent: ino,
                    name,
                    link,
                    reply: Default::default(),
                });
            }
            RequestKind::Mknod { arg, name } => {
                run_op!(Operation::Mknod {
                    parent: ino,
                    name,
                    mode: arg.mode,
                    rdev: arg.rdev,
                    umask: Some(arg.umask),
                    reply: Default::default(),
                });
            }
            RequestKind::Mkdir { arg, name } => {
                run_op!(Operation::Mkdir {
                    parent: ino,
                    name,
                    mode: arg.mode,
                    umask: Some(arg.umask),
                    reply: Default::default(),
                });
            }
            RequestKind::Unlink { name } => {
                run_op!(Operation::Unlink {
                    parent: ino,
                    name,
                    reply: Default::default(),
                });
            }
            RequestKind::Rmdir { name } => {
                run_op!(Operation::Rmdir {
                    parent: ino,
                    name,
                    reply: Default::default(),
                });
            }
            RequestKind::Rename { arg, name, newname } => {
                run_op!(Operation::Rename {
                    parent: ino,
                    name,
                    newparent: arg.newdir,
                    newname,
                    flags: 0,
                    reply: Default::default(),
                });
            }
            RequestKind::Link { arg, newname } => {
                run_op!(Operation::Link {
                    ino: arg.oldnodeid,
                    newparent: ino,
                    newname,
                    reply: Default::default(),
                });
            }
            RequestKind::Open { arg } => {
                run_op!(Operation::Open {
                    ino,
                    flags: arg.flags,
                    reply: Default::default(),
                });
            }
            RequestKind::Read { arg } => {
                run_op!(Operation::Read {
                    ino,
                    fh: arg.fh,
                    offset: arg.offset,
                    flags: arg.flags,
                    lock_owner: arg.lock_owner(),
                    reply: ReplyData::new(arg.size),
                });
            }
            RequestKind::Write { arg } => match data {
                Some(data) => {
                    run_op!(Operation::Write {
                        ino,
                        fh: arg.fh,
                        offset: arg.offset,
                        data,
                        size: arg.size,
                        flags: arg.flags,
                        lock_owner: arg.lock_owner(),
                        reply: Default::default(),
                    });
                }
                None => panic!("unexpected condition"),
            },
            RequestKind::Release { arg } => {
                let mut flush = false;
                let mut flock_release = false;
                let mut lock_owner = None;
                if self.proto_minor >= 8 {
                    flush = arg.release_flags & polyfuse_sys::abi::FUSE_RELEASE_FLUSH != 0;
                    lock_owner.get_or_insert_with(|| arg.lock_owner);
                }
                if arg.release_flags & polyfuse_sys::abi::FUSE_RELEASE_FLOCK_UNLOCK != 0 {
                    flock_release = true;
                    lock_owner.get_or_insert_with(|| arg.lock_owner);
                }
                run_op!(Operation::Release {
                    ino,
                    fh: arg.fh,
                    flags: arg.flags,
                    lock_owner,
                    flush,
                    flock_release,
                    reply: Default::default(),
                });
            }
            RequestKind::Statfs => {
                run_op!(Operation::Statfs {
                    ino,
                    reply: Default::default(),
                });
            }
            RequestKind::Fsync { arg } => {
                run_op!(Operation::Fsync {
                    ino,
                    fh: arg.fh,
                    datasync: arg.datasync(),
                    reply: Default::default(),
                });
            }
            RequestKind::Setxattr { arg, name, value } => {
                run_op!(Operation::Setxattr {
                    ino,
                    name,
                    value,
                    flags: arg.flags,
                    reply: Default::default(),
                });
            }
            RequestKind::Getxattr { arg, name } => {
                run_op!(Operation::Getxattr {
                    ino,
                    name,
                    size: arg.size,
                    reply: Default::default(),
                });
            }
            RequestKind::Listxattr { arg } => {
                run_op!(Operation::Listxattr {
                    ino,
                    size: arg.size,
                    reply: Default::default(),
                });
            }
            RequestKind::Removexattr { name } => {
                run_op!(Operation::Removexattr {
                    ino,
                    name,
                    reply: Default::default(),
                });
            }
            RequestKind::Flush { arg } => {
                run_op!(Operation::Flush {
                    ino,
                    fh: arg.fh,
                    lock_owner: arg.lock_owner,
                    reply: Default::default(),
                });
            }
            RequestKind::Opendir { arg } => {
                run_op!(Operation::Opendir {
                    ino,
                    flags: arg.flags,
                    reply: Default::default(),
                });
            }
            RequestKind::Readdir { arg } => {
                run_op!(Operation::Readdir {
                    ino,
                    fh: arg.fh,
                    offset: arg.offset,
                    reply: ReplyData::new(arg.size),
                });
            }
            RequestKind::Releasedir { arg } => {
                run_op!(Operation::Releasedir {
                    ino,
                    fh: arg.fh,
                    flags: arg.flags,
                    reply: Default::default(),
                });
            }
            RequestKind::Fsyncdir { arg } => {
                run_op!(Operation::Fsyncdir {
                    ino,
                    fh: arg.fh,
                    datasync: arg.datasync(),
                    reply: Default::default(),
                });
            }
            RequestKind::Getlk { arg } => {
                run_op!(Operation::Getlk {
                    ino,
                    fh: arg.fh,
                    owner: arg.owner,
                    lk: FileLock::new(&arg.lk),
                    reply: Default::default(),
                });
            }
            RequestKind::Setlk { arg, sleep } => {
                if arg.lk_flags & polyfuse_sys::abi::FUSE_LK_FLOCK != 0 {
                    const F_RDLCK: u32 = libc::F_RDLCK as u32;
                    const F_WRLCK: u32 = libc::F_WRLCK as u32;
                    const F_UNLCK: u32 = libc::F_UNLCK as u32;
                    #[allow(clippy::cast_possible_wrap)]
                    let mut op = match arg.lk.typ {
                        F_RDLCK => libc::LOCK_SH as u32,
                        F_WRLCK => libc::LOCK_EX as u32,
                        F_UNLCK => libc::LOCK_UN as u32,
                        _ => return cx.reply_err(libc::EIO).await,
                    };
                    if !sleep {
                        op |= libc::LOCK_NB as u32;
                    }
                    run_op!(Operation::Flock {
                        ino,
                        fh: arg.fh,
                        owner: arg.owner,
                        op,
                        reply: Default::default(),
                    });
                } else {
                    run_op!(Operation::Setlk {
                        ino,
                        fh: arg.fh,
                        owner: arg.owner,
                        lk: FileLock::new(&arg.lk),
                        sleep,
                        reply: Default::default(),
                    });
                }
            }
            RequestKind::Access { arg } => {
                run_op!(Operation::Access {
                    ino,
                    mask: arg.mask,
                    reply: Default::default(),
                });
            }
            RequestKind::Create { arg, name } => {
                run_op!(Operation::Create {
                    parent: ino,
                    name,
                    mode: arg.mode,
                    umask: Some(arg.umask),
                    open_flags: arg.flags,
                    reply: Default::default(),
                });
            }
            RequestKind::Bmap { arg } => {
                run_op!(Operation::Bmap {
                    ino,
                    block: arg.block,
                    blocksize: arg.blocksize,
                    reply: Default::default(),
                });
            }

            // Ioctl,
            // Poll,
            // NotifyReply,
            // BatchForget,
            // Fallocate,
            // Readdirplus,
            // Rename2,
            // Lseek,
            // CopyFileRange,
            RequestKind::Unknown => {
                log::warn!("unsupported opcode: {:?}", header.opcode);
                cx.reply_err(libc::ENOSYS).await?;
            }
        }

        Ok(())
    }
}

/// Session initializer.
#[derive(Debug, Default)]
pub struct SessionInitializer {
    _p: (),
}

/// Contextural information about an incoming request.
pub struct Context<'a> {
    header: &'a fuse_in_header,
    writer: &'a mut (dyn AsyncWrite + Send + Unpin),
    #[allow(dead_code)]
    session: &'a Session,
}

impl fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Context").finish()
    }
}

impl<'a> Context<'a> {
    /// Return the user ID of the calling process.
    pub fn uid(&self) -> u32 {
        self.header.uid
    }

    /// Return the group ID of the calling process.
    pub fn gid(&self) -> u32 {
        self.header.gid
    }

    /// Return the process ID of the calling process.
    pub fn pid(&self) -> u32 {
        self.header.pid
    }

    /// Reply to the kernel with an error code.
    pub async fn reply_err(&mut self, error: i32) -> io::Result<()> {
        self.send_reply(error, &[]).await
    }

    /// Reply to the kernel with the specified data.
    #[inline]
    pub(crate) async fn send_reply(&mut self, error: i32, data: &[&[u8]]) -> io::Result<()> {
        send_reply(&mut self.writer, self.header.unique, error, data).await?;
        log::debug!(
            "Reply to unique={}: error={}, data={:?}",
            self.header.unique,
            error,
            data
        );
        Ok(())
    }
}

async fn send_reply(
    writer: &mut (impl AsyncWrite + Unpin),
    unique: u64,
    error: i32,
    data: &[&[u8]],
) -> io::Result<()> {
    let data_len: usize = data.iter().map(|t| t.len()).sum();

    let out_header = fuse_out_header {
        unique,
        error: -error,
        len: u32::try_from(std::mem::size_of::<fuse_out_header>() + data_len).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("the total length of data is too long: {}", e),
            )
        })?,
    };

    // Unfortunately, IoSlice<'_> does not implement Send and
    // the data vector must be created in `poll` function.
    poll_fn(move |cx| {
        let vec: SmallVec<[_; 4]> = Some(IoSlice::new(out_header.as_bytes()))
            .into_iter()
            .chain(data.iter().map(|t| IoSlice::new(&*t)))
            .collect();
        Pin::new(&mut *writer).poll_write_vectored(cx, &*vec)
    })
    .await?;

    Ok(())
}
