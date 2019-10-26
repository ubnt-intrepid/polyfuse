use crate::{
    buf::{Buffer, MAX_WRITE_SIZE},
    fs::{Context, Filesystem, Operation},
    reply::{ReplyData, ReplyRaw},
};
use futures_channel::oneshot;
use futures_io::{AsyncRead, AsyncWrite};
use polyfuse_abi::{
    parse::{Arg, Request},
    InitOut, LkFlags, LockType, ReleaseFlags, Unique,
};
use std::{collections::HashMap, future::Future, io};

/// A FUSE filesystem driver.
#[derive(Debug)]
pub struct Session {
    proto_major: u32,
    proto_minor: u32,
    max_readahead: u32,
    exited: bool,
    remains: HashMap<Unique, oneshot::Sender<()>>,
}

impl Session {
    /// Create a new session initializer.
    pub fn initializer() -> InitSession {
        InitSession::default()
    }

    /// Dispatch an incoming request to the provided operations.
    #[allow(clippy::cognitive_complexity)]
    pub async fn dispatch<I, T, F>(
        &mut self,
        fs: &F,
        request: Request<'_>,
        data: Option<T>,
        channel: I,
    ) -> io::Result<()>
    where
        I: AsyncWrite + Unpin + 'static,
        F: Filesystem<T>,
    {
        if self.exited {
            log::warn!("The sesson has already been exited");
            return Ok(());
        }

        let Request { header, arg, .. } = request;

        let ino = header.nodeid;
        let unique = header.unique;
        let mut cx = Context {
            uid: header.uid,
            gid: header.gid,
            pid: header.pid,
            _anchor: std::marker::PhantomData,
        };
        let reply = ReplyRaw::new(unique, channel);

        match arg {
            Arg::Init { .. } => {
                log::warn!("");
                reply.reply_err(libc::EIO).await?;
            }
            Arg::Destroy => {
                self.exited = true;
                reply.reply(0, &[]).await?;
            }
            Arg::Lookup { name } => {
                fs.call(
                    &mut cx,
                    Operation::Lookup {
                        parent: ino,
                        name,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Forget { arg } => {
                // no reply.
                fs.call(
                    &mut cx,
                    Operation::Forget {
                        nlookups: &[(ino, arg.nlookup)],
                    },
                )
                .await?;
            }
            Arg::Getattr { arg } => {
                fs.call(
                    &mut cx,
                    Operation::Getattr {
                        ino,
                        fh: arg.fh(),
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Setattr { arg } => {
                fs.call(
                    &mut cx,
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
                )
                .await?;
            }
            Arg::Readlink => {
                fs.call(
                    &mut cx,
                    Operation::Readlink {
                        ino,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Symlink { name, link } => {
                fs.call(
                    &mut cx,
                    Operation::Symlink {
                        parent: ino,
                        name,
                        link,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Mknod { arg, name } => {
                fs.call(
                    &mut cx,
                    Operation::Mknod {
                        parent: ino,
                        name,
                        mode: arg.mode,
                        rdev: arg.rdev,
                        umask: Some(arg.umask),
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Mkdir { arg, name } => {
                fs.call(
                    &mut cx,
                    Operation::Mkdir {
                        parent: ino,
                        name,
                        mode: arg.mode,
                        umask: Some(arg.umask),
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Unlink { name } => {
                fs.call(
                    &mut cx,
                    Operation::Unlink {
                        parent: ino,
                        name,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Rmdir { name } => {
                fs.call(
                    &mut cx,
                    Operation::Rmdir {
                        parent: ino,
                        name,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Rename { arg, name, newname } => {
                fs.call(
                    &mut cx,
                    Operation::Rename {
                        parent: ino,
                        name,
                        newparent: arg.newdir,
                        newname,
                        flags: 0,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Link { arg, newname } => {
                fs.call(
                    &mut cx,
                    Operation::Link {
                        ino: arg.oldnodeid,
                        newparent: ino,
                        newname,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Open { arg } => {
                fs.call(
                    &mut cx,
                    Operation::Open {
                        ino,
                        flags: arg.flags,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Read { arg } => {
                fs.call(
                    &mut cx,
                    Operation::Read {
                        ino,
                        fh: arg.fh,
                        offset: arg.offset,
                        flags: arg.flags,
                        lock_owner: arg.lock_owner(),
                        reply: ReplyData::new(reply, arg.size),
                    },
                )
                .await?;
            }
            Arg::Write { arg } => match data {
                Some(data) => {
                    fs.call(
                        &mut cx,
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
                    )
                    .await?;
                }
                None => panic!("unexpected condition"),
            },
            Arg::Release { arg } => {
                let mut flush = false;
                let mut flock_release = false;
                let mut lock_owner = None;
                if self.proto_minor >= 8 {
                    flush = arg.release_flags.contains(ReleaseFlags::FLUSH);
                    lock_owner.get_or_insert_with(|| arg.lock_owner);
                }
                if arg.release_flags.contains(ReleaseFlags::FLOCK_UNLOCK) {
                    flock_release = true;
                    lock_owner.get_or_insert_with(|| arg.lock_owner);
                }
                fs.call(
                    &mut cx,
                    Operation::Release {
                        ino,
                        fh: arg.fh,
                        flags: arg.flags,
                        lock_owner,
                        flush,
                        flock_release,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Statfs => {
                fs.call(
                    &mut cx,
                    Operation::Statfs {
                        ino,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Fsync { arg } => {
                fs.call(
                    &mut cx,
                    Operation::Fsync {
                        ino,
                        fh: arg.fh,
                        datasync: arg.datasync(),
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Setxattr { arg, name, value } => {
                fs.call(
                    &mut cx,
                    Operation::Setxattr {
                        ino,
                        name,
                        value,
                        flags: arg.flags,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Getxattr { arg, name } => {
                fs.call(
                    &mut cx,
                    Operation::Getxattr {
                        ino,
                        name,
                        size: arg.size,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Listxattr { arg } => {
                fs.call(
                    &mut cx,
                    Operation::Listxattr {
                        ino,
                        size: arg.size,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Removexattr { name } => {
                fs.call(
                    &mut cx,
                    Operation::Removexattr {
                        ino,
                        name,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Flush { arg } => {
                fs.call(
                    &mut cx,
                    Operation::Flush {
                        ino,
                        fh: arg.fh,
                        lock_owner: arg.lock_owner,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Opendir { arg } => {
                fs.call(
                    &mut cx,
                    Operation::Opendir {
                        ino,
                        flags: arg.flags,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Readdir { arg } => {
                fs.call(
                    &mut cx,
                    Operation::Readdir {
                        ino,
                        fh: arg.fh,
                        offset: arg.offset,
                        reply: ReplyData::new(reply, arg.size),
                    },
                )
                .await?;
            }
            Arg::Releasedir { arg } => {
                fs.call(
                    &mut cx,
                    Operation::Releasedir {
                        ino,
                        fh: arg.fh,
                        flags: arg.flags,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Fsyncdir { arg } => {
                fs.call(
                    &mut cx,
                    Operation::Fsyncdir {
                        ino,
                        fh: arg.fh,
                        datasync: arg.datasync(),
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Getlk { arg } => {
                fs.call(
                    &mut cx,
                    Operation::Getlk {
                        ino,
                        fh: arg.fh,
                        owner: arg.owner,
                        lk: &arg.lk,
                        reply: reply.into(),
                    },
                )
                .await?;
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
                    fs.call(
                        &mut cx,
                        Operation::Flock {
                            ino,
                            fh: arg.fh,
                            owner: arg.owner,
                            op,
                            reply: reply.into(),
                        },
                    )
                    .await?;
                } else {
                    fs.call(
                        &mut cx,
                        Operation::Setlk {
                            ino,
                            fh: arg.fh,
                            owner: arg.owner,
                            lk: &arg.lk,
                            sleep,
                            reply: reply.into(),
                        },
                    )
                    .await?;
                }
            }
            Arg::Access { arg } => {
                fs.call(
                    &mut cx,
                    Operation::Access {
                        ino,
                        mask: arg.mask,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Create { arg, name } => {
                fs.call(
                    &mut cx,
                    Operation::Create {
                        parent: ino,
                        name,
                        mode: arg.mode,
                        umask: Some(arg.umask),
                        open_flags: arg.flags,
                        reply: reply.into(),
                    },
                )
                .await?;
            }
            Arg::Interrupt { arg } => {
                log::debug!("INTERRUPT (unique = {:?})", arg.unique);
                if let Some(tx) = self.remains.remove(&unique) {
                    let _ = tx.send(());
                }
            }
            Arg::Bmap { arg } => {
                fs.call(
                    &mut cx,
                    Operation::Bmap {
                        ino,
                        block: arg.block,
                        blocksize: arg.blocksize,
                        reply: reply.into(),
                    },
                )
                .await?;
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
            Arg::Unknown => {
                log::warn!("unsupported opcode: {:?}", header.opcode);
                reply.reply_err(libc::ENOSYS).await?;
            }
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub async fn register(&mut self, unique: Unique) -> impl Future<Output = ()> {
        let (tx, rx) = oneshot::channel();
        self.remains.insert(unique, tx);
        async move {
            let _ = rx.await;
        }
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
    pub async fn start<'a, I>(self, io: &'a mut I) -> io::Result<Session>
    where
        I: AsyncRead + AsyncWrite + Unpin,
    {
        let mut buf = Buffer::default();

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
                remains: HashMap::new(),
            });
        }
    }
}
