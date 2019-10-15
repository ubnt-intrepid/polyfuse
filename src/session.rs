//! FUSE session driver.

use crate::{
    abi::{
        parse::{Arg, Request},
        InitOut, LkFlags, LockType, ReleaseFlags,
    }, //
    buf::{Buffer, MAX_WRITE_SIZE},
    op::{AttrSet, Context, Operations},
    reply::ReplyRaw,
};
use futures::{
    io::{AsyncRead, AsyncWrite},
    ready,
    stream::{FuturesUnordered, StreamExt},
};
use std::{
    fmt,
    future::Future,
    io,
    pin::Pin,
    task::{self, Poll},
};

/// A FUSE filesystem driver.
///
/// This session driver does *not* receive the next request from the kernel,
/// until the processing result for the previous request has been sent.
#[derive(Debug)]
pub struct Session {
    proto_major: u32,
    proto_minor: u32,
    max_readahead: u32,
    exited: bool,
}

impl Session {
    /// Create a new session initializer.
    pub fn initializer() -> InitSession {
        InitSession::default()
    }

    /// Returns the major protocol version from the kernel.
    pub fn proto_major(&self) -> u32 {
        self.proto_major
    }

    /// Returns the minor protocol version from the kernel.
    pub fn proto_minor(&self) -> u32 {
        self.proto_minor
    }

    pub fn max_readahead(&self) -> u32 {
        self.max_readahead
    }

    pub fn exit(&mut self) {
        self.exited = true;
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    /// Dispatch an incoming request to the provided operations.
    #[allow(clippy::cognitive_complexity)]
    pub fn dispatch<'a, I, T>(
        &mut self,
        buffer: &mut Buffer,
        channel: I,
        ops: &mut T,
        background: &mut Background<'a>,
    ) -> io::Result<()>
    where
        I: AsyncWrite + Clone + Unpin + 'a,
        T: for<'s> Operations<&'s [u8]>,
    {
        if self.exited() {
            log::warn!("The sesson has already been exited");
            return Ok(());
        }

        let (Request { header, arg, .. }, data) = buffer.extract()?;
        log::debug!(
            "Got a request: unique={}, opcode={:?}, arg={:?}, data={:?}",
            header.unique,
            header.opcode(),
            arg,
            data
        );

        let ino = header.nodeid;
        let cx = Context {
            uid: header.uid,
            gid: header.gid,
            pid: header.pid,
            _p: (),
        };
        let reply = ReplyRaw::new(header.unique, channel.clone());

        match arg {
            Arg::Init { .. } => {
                log::warn!("");
                background.spawn_task(reply.reply_err(libc::EIO));
            }
            Arg::Destroy => {
                self.exit();
                background.spawn_task(reply.reply(0, &[]));
            }
            Arg::Lookup { name } => background.spawn_task(ops.lookup(&cx, ino, name, reply.into())),
            Arg::Forget { arg } => {
                // no reply.
                background.spawn_task(ops.forget(&cx, &[(ino, arg.nlookup)]));
            }
            Arg::Getattr { arg } => {
                background.spawn_task(ops.getattr(&cx, ino, arg.fh(), reply.into()))
            }
            Arg::Setattr { arg } => {
                let attrs = AttrSet {
                    mode: arg.mode(),
                    uid: arg.uid(),
                    gid: arg.gid(),
                    size: arg.size(),
                    atime: arg.atime(),
                    mtime: arg.mtime(),
                    ctime: arg.ctime(),
                    ..Default::default()
                };
                background.spawn_task(ops.setattr(&cx, ino, arg.fh(), attrs, reply.into()))
            }
            Arg::Readlink => background.spawn_task(ops.readlink(&cx, ino, reply.into())),
            Arg::Symlink { name, link } => {
                background.spawn_task(ops.symlink(&cx, ino, name, link, reply.into()))
            }
            Arg::Mknod { arg, name } => background.spawn_task(ops.mknod(
                &cx,
                ino,
                name,
                arg.mode,
                arg.rdev,
                Some(arg.umask),
                reply.into(),
            )),
            Arg::Mkdir { arg, name } => background.spawn_task(ops.mkdir(
                &cx,
                ino,
                name,
                arg.mode,
                Some(arg.umask),
                reply.into(),
            )),
            Arg::Unlink { name } => background.spawn_task(ops.unlink(&cx, ino, name, reply.into())),
            Arg::Rmdir { name } => background.spawn_task(ops.rmdir(&cx, ino, name, reply.into())),
            Arg::Rename { arg, name, newname } => background.spawn_task(ops.rename(
                &cx,
                ino,
                name,
                arg.newdir,
                newname,
                0,
                reply.into(),
            )),
            Arg::Link { arg, newname } => {
                background.spawn_task(ops.link(&cx, arg.oldnodeid, ino, newname, reply.into()))
            }
            Arg::Open { arg } => background.spawn_task(ops.open(&cx, ino, arg.flags, reply.into())),
            Arg::Read { arg } => background.spawn_task(ops.read(
                &cx,
                ino,
                arg.fh,
                arg.offset,
                arg.size,
                arg.flags,
                arg.lock_owner(),
                reply.into(),
            )),
            Arg::Write { arg } => match data {
                Some(data) => background.spawn_task(ops.write(
                    &cx,
                    ino,
                    arg.fh,
                    arg.offset,
                    data,
                    arg.size,
                    arg.flags,
                    arg.lock_owner(),
                    reply.into(),
                )),
                None => panic!("unexpected condition"),
            },
            Arg::Release { arg } => {
                let mut flush = false;
                let mut flock_release = false;
                let mut lock_owner = None;
                if self.proto_minor() >= 8 {
                    flush = arg.release_flags.contains(ReleaseFlags::FLUSH);
                    lock_owner.get_or_insert_with(|| arg.lock_owner);
                }
                if arg.release_flags.contains(ReleaseFlags::FLOCK_UNLOCK) {
                    flock_release = true;
                    lock_owner.get_or_insert_with(|| arg.lock_owner);
                }
                background.spawn_task(ops.release(
                    &cx,
                    ino,
                    arg.fh,
                    arg.flags,
                    lock_owner,
                    flush,
                    flock_release,
                    reply.into(),
                ))
            }
            Arg::Statfs => background.spawn_task(ops.statfs(&cx, ino, reply.into())),
            Arg::Fsync { arg } => {
                background.spawn_task(ops.fsync(&cx, ino, arg.fh, arg.datasync(), reply.into()))
            }
            Arg::Setxattr { arg, name, value } => {
                background.spawn_task(ops.setxattr(&cx, ino, name, value, arg.flags, reply.into()))
            }
            Arg::Getxattr { arg, name } => {
                background.spawn_task(ops.getxattr(&cx, ino, name, arg.size, reply.into()))
            }
            Arg::Listxattr { arg } => {
                background.spawn_task(ops.listxattr(&cx, ino, arg.size, reply.into()))
            }
            Arg::Removexattr { name } => {
                background.spawn_task(ops.removexattr(&cx, ino, name, reply.into()))
            }
            Arg::Flush { arg } => {
                background.spawn_task(ops.flush(&cx, ino, arg.fh, arg.lock_owner, reply.into()))
            }
            Arg::Opendir { arg } => {
                background.spawn_task(ops.opendir(&cx, ino, arg.flags, reply.into()))
            }
            Arg::Readdir { arg } => background.spawn_task(ops.readdir(
                &cx,
                ino,
                arg.fh,
                arg.offset,
                arg.size,
                reply.into(),
            )),
            Arg::Releasedir { arg } => {
                background.spawn_task(ops.releasedir(&cx, ino, arg.fh, arg.flags, reply.into()))
            }
            Arg::Fsyncdir { arg } => {
                background.spawn_task(ops.fsyncdir(&cx, ino, arg.fh, arg.datasync(), reply.into()))
            }
            Arg::Getlk { arg } => {
                background.spawn_task(ops.getlk(&cx, ino, arg.fh, arg.owner, &arg.lk, reply.into()))
            }
            Arg::Setlk { arg, sleep } => {
                if arg.lk_flags.contains(LkFlags::FLOCK) {
                    #[allow(clippy::cast_possible_wrap)]
                    let mut op = match arg.lk.typ {
                        LockType::Read => libc::LOCK_SH as u32,
                        LockType::Write => libc::LOCK_EX as u32,
                        LockType::Unlock => libc::LOCK_UN as u32,
                    };
                    if !sleep {
                        op |= libc::LOCK_NB as u32;
                    }
                    background.spawn_task(ops.flock(&cx, ino, arg.fh, arg.owner, op, reply.into()))
                } else {
                    background.spawn_task(ops.setlk(
                        &cx,
                        ino,
                        arg.fh,
                        arg.owner,
                        &arg.lk,
                        sleep,
                        reply.into(),
                    ))
                }
            }
            Arg::Access { arg } => {
                background.spawn_task(ops.access(&cx, ino, arg.mask, reply.into()))
            }
            Arg::Create { arg, name } => background.spawn_task(ops.create(
                &cx,
                ino,
                name,
                arg.mode,
                Some(arg.umask),
                arg.flags,
                reply.into(),
            )),
            Arg::Bmap { arg } => {
                background.spawn_task(ops.bmap(&cx, ino, arg.block, arg.blocksize, reply.into()))
            }

            // Interrupt,
            // Ioctl,
            // Poll,
            // NotifyReply,
            // BatchForget,
            // Fallocate,
            // Readdirplus,
            // Rename2,
            // Lseek,
            // CopyFileRange,
            Arg::Unknown => {
                log::warn!("unsupported opcode: {:?}", header.opcode);
                background.spawn_task(reply.reply_err(libc::ENOSYS));
            }
        }

        Ok(())
    }
}

/// Session initializer.
#[derive(Debug, Default)]
pub struct InitSession {
    _p: (),
}

impl InitSession {
    /// Start a new FUSE session.
    ///
    /// This function receives an INIT request from the kernel and replies
    /// after initializing the connection parameters.
    pub async fn start<'a, I>(self, io: &'a mut I, buf: &'a mut Buffer) -> io::Result<Session>
    where
        I: AsyncRead + AsyncWrite + Unpin,
    {
        loop {
            let terminated = buf.receive(io).await?;
            if terminated {
                log::warn!("the connection is closed");
                return Err(io::Error::from_raw_os_error(libc::ENODEV));
            }

            let (Request { header, arg, .. }, _data) = buf.extract()?;
            let reply = ReplyRaw::new(header.unique, &mut *io);

            let (proto_major, proto_minor, max_readahead);
            match arg {
                Arg::Init { arg } => {
                    let mut init_out = InitOut::default();

                    if arg.major > 7 {
                        log::debug!("wait for a second INIT request with a 7.X version.");
                        reply.reply(0, &[init_out.as_ref()]).await?;
                        continue;
                    }

                    if arg.major < 7 || (arg.major == 7 && arg.minor < 6) {
                        log::warn!("unsupported protocol version: {}.{}", arg.major, arg.minor);
                        reply.reply_err(libc::EPROTO).await?;
                        return Err(io::Error::from_raw_os_error(libc::EPROTO));
                    }

                    // remember the kernel parameters.
                    proto_major = arg.major;
                    proto_minor = arg.minor;
                    max_readahead = arg.max_readahead;

                    // TODO: max_background, congestion_threshold, time_gran, max_pages
                    init_out.max_readahead = arg.max_readahead;
                    init_out.max_write = MAX_WRITE_SIZE;

                    reply.reply(0, &[init_out.as_ref()]).await?;
                }
                _ => {
                    log::warn!(
                        "ignoring an operation before init (opcode={:?})",
                        header.opcode
                    );
                    reply.reply_err(libc::EIO).await?;
                    continue;
                }
            }

            return Ok(Session {
                proto_major,
                proto_minor,
                max_readahead,
                exited: false,
            });
        }
    }
}

pub struct Background<'a> {
    tasks: FuturesUnordered<Pin<Box<dyn Future<Output = io::Result<()>> + 'a>>>,
}

impl fmt::Debug for Background<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Background").finish()
    }
}

impl Default for Background<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Background<'a> {
    pub fn new() -> Self {
        Self {
            tasks: FuturesUnordered::new(),
        }
    }

    pub fn num_remains(&self) -> usize {
        self.tasks.len()
    }

    pub fn spawn_task<T>(&mut self, task: T)
    where
        T: Future<Output = io::Result<()>> + 'a,
    {
        self.tasks.push(Box::pin(task));
    }

    pub fn poll_tasks(&mut self, cx: &mut task::Context) -> Poll<io::Result<()>> {
        while let Some(res) = ready!(self.tasks.poll_next_unpin(cx)) {
            if let Err(e) = res {
                return Poll::Ready(Err(e));
            }
        }
        Poll::Ready(Ok(()))
    }
}
