//! FUSE session driver.

use crate::{
    buf::{Buffer, MAX_WRITE_SIZE},
    fs::{Context, Filesystem, Operation},
    reply::{ReplyData, ReplyRaw},
};
use futures_io::{AsyncRead, AsyncWrite};
use futures_util::{
    ready,
    stream::{FuturesUnordered, StreamExt},
};
use polyfuse_abi::{
    parse::{Arg, Request},
    InitOut, LkFlags, LockType, ReleaseFlags, Unique,
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
    pub fn dispatch<I, T>(
        &mut self,
        buffer: &mut Buffer,
        channel: &I,
        fs: &mut T,
        background: &mut Background,
    ) -> io::Result<()>
    where
        I: AsyncWrite + Clone + Unpin + 'static,
        T: for<'s> Filesystem<&'s [u8]>,
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
        let unique = header.unique;
        let cx = Context {
            uid: header.uid,
            gid: header.gid,
            pid: header.pid,
        };
        let reply = ReplyRaw::new(unique, channel.clone());

        match arg {
            Arg::Init { .. } => {
                log::warn!("");
                background.spawn_task(unique, reply.reply_err(libc::EIO));
            }
            Arg::Destroy => {
                self.exit();
                background.spawn_task(unique, reply.reply(0, &[]));
            }
            Arg::Lookup { name } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Lookup {
                        parent: ino,
                        name,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Forget { arg } => {
                // no reply.
                background.spawn_task(
                    unique,
                    fs.call(
                        &cx,
                        Operation::Forget {
                            nlookups: &[(ino, arg.nlookup)],
                        },
                    ),
                );
            }
            Arg::Getattr { arg } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Getattr {
                        ino,
                        fh: arg.fh(),
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Setattr { arg } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Setattr {
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
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Readlink => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Readlink {
                        ino,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Symlink { name, link } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Symlink {
                        parent: ino,
                        name,
                        link,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Mknod { arg, name } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Mknod {
                        parent: ino,
                        name,
                        mode: arg.mode,
                        rdev: arg.rdev,
                        umask: Some(arg.umask),
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Mkdir { arg, name } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Mkdir {
                        parent: ino,
                        name,
                        mode: arg.mode,
                        umask: Some(arg.umask),
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Unlink { name } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Unlink {
                        parent: ino,
                        name,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Rmdir { name } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Rmdir {
                        parent: ino,
                        name,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Rename { arg, name, newname } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Rename {
                        parent: ino,
                        name,
                        newparent: arg.newdir,
                        newname,
                        flags: 0,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Link { arg, newname } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Link {
                        ino: arg.oldnodeid,
                        newparent: ino,
                        newname,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Open { arg } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Open {
                        ino,
                        flags: arg.flags,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Read { arg } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Read {
                        ino,
                        fh: arg.fh,
                        offset: arg.offset,
                        flags: arg.flags,
                        lock_owner: arg.lock_owner(),
                        reply: ReplyData::new(reply, arg.size),
                    },
                ),
            ),
            Arg::Write { arg } => match data {
                Some(data) => background.spawn_task(
                    unique,
                    fs.call(
                        &cx,
                        Operation::Write {
                            ino,
                            fh: arg.fh,
                            offset: arg.offset,
                            data,
                            size: arg.size,
                            flags: arg.flags,
                            lock_owner: arg.lock_owner(),
                            reply: reply.into(),
                        },
                    ),
                ),
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
                background.spawn_task(
                    unique,
                    fs.call(
                        &cx,
                        Operation::Release {
                            ino,
                            fh: arg.fh,
                            flags: arg.flags,
                            lock_owner,
                            flush,
                            flock_release,
                            reply: reply.into(),
                        },
                    ),
                )
            }
            Arg::Statfs => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Statfs {
                        ino,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Fsync { arg } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Fsync {
                        ino,
                        fh: arg.fh,
                        datasync: arg.datasync(),
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Setxattr { arg, name, value } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Setxattr {
                        ino,
                        name,
                        value,
                        flags: arg.flags,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Getxattr { arg, name } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Getxattr {
                        ino,
                        name,
                        size: arg.size,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Listxattr { arg } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Listxattr {
                        ino,
                        size: arg.size,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Removexattr { name } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Removexattr {
                        ino,
                        name,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Flush { arg } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Flush {
                        ino,
                        fh: arg.fh,
                        lock_owner: arg.lock_owner,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Opendir { arg } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Opendir {
                        ino,
                        flags: arg.flags,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Readdir { arg } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Readdir {
                        ino,
                        fh: arg.fh,
                        offset: arg.offset,
                        reply: ReplyData::new(reply, arg.size),
                    },
                ),
            ),
            Arg::Releasedir { arg } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Releasedir {
                        ino,
                        fh: arg.fh,
                        flags: arg.flags,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Fsyncdir { arg } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Fsyncdir {
                        ino,
                        fh: arg.fh,
                        datasync: arg.datasync(),
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Getlk { arg } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Getlk {
                        ino,
                        fh: arg.fh,
                        owner: arg.owner,
                        lk: &arg.lk,
                        reply: reply.into(),
                    },
                ),
            ),
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
                    background.spawn_task(
                        unique,
                        fs.call(
                            &cx,
                            Operation::Flock {
                                ino,
                                fh: arg.fh,
                                owner: arg.owner,
                                op,
                                reply: reply.into(),
                            },
                        ),
                    )
                } else {
                    background.spawn_task(
                        unique,
                        fs.call(
                            &cx,
                            Operation::Setlk {
                                ino,
                                fh: arg.fh,
                                owner: arg.owner,
                                lk: &arg.lk,
                                sleep,
                                reply: reply.into(),
                            },
                        ),
                    )
                }
            }
            Arg::Access { arg } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Access {
                        ino,
                        mask: arg.mask,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Create { arg, name } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Create {
                        parent: ino,
                        name,
                        mode: arg.mode,
                        umask: Some(arg.umask),
                        open_flags: arg.flags,
                        reply: reply.into(),
                    },
                ),
            ),
            Arg::Interrupt { arg } => {
                log::debug!("INTERRUPT (unique = {:?})", arg.unique);
                background.cancel_task(arg.unique);
            }
            Arg::Bmap { arg } => background.spawn_task(
                unique,
                fs.call(
                    &cx,
                    Operation::Bmap {
                        ino,
                        block: arg.block,
                        blocksize: arg.blocksize,
                        reply: reply.into(),
                    },
                ),
            ),

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
                background.spawn_task(unique, reply.reply_err(libc::ENOSYS));
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

/// A pool for tracking the execution of tasks spawned for each FUSE request.
#[derive(Debug)]
pub struct Background {
    tasks: FuturesUnordered<BackgroundTask>,
}

impl Default for Background {
    fn default() -> Self {
        Self::new()
    }
}

impl Background {
    pub fn new() -> Self {
        Self {
            tasks: FuturesUnordered::new(),
        }
    }

    pub fn num_remains(&self) -> usize {
        self.tasks.len()
    }

    pub fn spawn_task<T>(&mut self, unique: Unique, fut: T)
    where
        T: Future<Output = io::Result<()>> + 'static,
    {
        self.tasks.push(BackgroundTask {
            unique,
            fut: Some(Box::pin(fut)),
        });
    }

    pub fn cancel_task(&mut self, unique: Unique) {
        if let Some(task) = self.tasks.iter_mut().find(|task| task.unique() == unique) {
            task.cancel();
        }
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

pub struct BackgroundTask {
    fut: Option<Pin<Box<dyn Future<Output = io::Result<()>> + 'static>>>,
    unique: Unique,
}

impl fmt::Debug for BackgroundTask {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("BackgroundTask")
            .field("unique", &self.unique)
            .field("canceled", &self.fut.is_none())
            .finish()
    }
}

impl BackgroundTask {
    pub fn unique(&self) -> Unique {
        self.unique
    }

    pub fn cancel(&mut self) {
        drop(self.fut.take());
    }
}

impl Future for BackgroundTask {
    type Output = io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context) -> Poll<Self::Output> {
        let this = &mut *self;
        match &mut this.fut {
            Some(fut) => fut.as_mut().poll(cx),
            None => Poll::Ready(Ok(())),
        }
    }
}
