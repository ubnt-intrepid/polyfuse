//! FUSE session driver.

use crate::{
    abi::{
        parse::{Arg, Request},
        InitOut,
    }, //
    buf::{Buffer, MAX_WRITE_SIZE},
    op::Operations,
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
    got_destroy: bool,
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

    pub fn got_destroy(&self) -> bool {
        self.got_destroy
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
        let (Request { header, arg, .. }, data) = buffer.extract()?;
        log::debug!(
            "Got a request: unique={}, opcode={:?}, arg={:?}, data={:?}",
            header.unique,
            header.opcode(),
            arg,
            data
        );

        let reply = ReplyRaw::new(header.unique, channel.clone());
        if self.got_destroy {
            log::warn!(
                "ignoring an operation after destroy (opcode={:?})",
                header.opcode
            );
            background.spawn_task(reply.reply_err(libc::EIO));
            return Ok(());
        }

        match arg {
            Arg::Init { .. } => {
                log::warn!("");
                background.spawn_task(reply.reply_err(libc::EIO));
            }
            Arg::Destroy => {
                self.got_destroy = true;
                background.spawn_task(reply.reply(0, &[]));
            }
            Arg::Lookup { name } => background.spawn_task(ops.lookup(header, name, reply.into())),
            Arg::Forget { arg } => {
                ops.forget(header, arg);
                // no reply.
            }
            Arg::Getattr { arg } => background.spawn_task(ops.getattr(header, &arg, reply.into())),
            Arg::Setattr { arg } => background.spawn_task(ops.setattr(header, &arg, reply.into())),
            Arg::Readlink => background.spawn_task(ops.readlink(header, reply.into())),
            Arg::Symlink { name, link } => {
                background.spawn_task(ops.symlink(header, name, link, reply.into()))
            }
            Arg::Mknod { arg, name } => {
                background.spawn_task(ops.mknod(header, arg, name, reply.into()))
            }
            Arg::Mkdir { arg, name } => {
                background.spawn_task(ops.mkdir(header, arg, name, reply.into()))
            }
            Arg::Unlink { name } => background.spawn_task(ops.unlink(header, name, reply.into())),
            Arg::Rmdir { name } => background.spawn_task(ops.unlink(header, name, reply.into())),
            Arg::Rename { arg, name, newname } => {
                background.spawn_task(ops.rename(header, arg, name, newname, reply.into()))
            }
            Arg::Link { arg, newname } => {
                background.spawn_task(ops.link(header, arg, newname, reply.into()))
            }
            Arg::Open { arg } => background.spawn_task(ops.open(header, arg, reply.into())),
            Arg::Read { arg } => background.spawn_task(ops.read(header, arg, reply.into())),
            Arg::Write { arg } => match data {
                Some(data) => background.spawn_task(ops.write(header, arg, data, reply.into())),
                None => panic!("unexpected condition"),
            },
            Arg::Release { arg } => background.spawn_task(ops.release(header, arg, reply.into())),
            Arg::Statfs => background.spawn_task(ops.statfs(header, reply.into())),
            Arg::Fsync { arg } => background.spawn_task(ops.fsync(header, arg, reply.into())),
            Arg::Setxattr { arg, name, value } => {
                background.spawn_task(ops.setxattr(header, arg, name, value, reply.into()))
            }
            Arg::Getxattr { arg, name } => {
                background.spawn_task(ops.getxattr(header, arg, name, reply.into()))
            }
            Arg::Listxattr { arg } => {
                background.spawn_task(ops.listxattr(header, arg, reply.into()))
            }
            Arg::Removexattr { name } => {
                background.spawn_task(ops.removexattr(header, name, reply.into()))
            }
            Arg::Flush { arg } => background.spawn_task(ops.flush(header, arg, reply.into())),
            Arg::Opendir { arg } => background.spawn_task(ops.opendir(header, arg, reply.into())),
            Arg::Readdir { arg } => background.spawn_task(ops.readdir(header, arg, reply.into())),
            Arg::Releasedir { arg } => {
                background.spawn_task(ops.releasedir(header, arg, reply.into()))
            }
            Arg::Fsyncdir { arg } => background.spawn_task(ops.fsyncdir(header, arg, reply.into())),
            Arg::Getlk { arg } => background.spawn_task(ops.getlk(header, arg, reply.into())),
            Arg::Setlk { arg } => {
                background.spawn_task(ops.setlk(header, arg, false, reply.into()))
            }
            Arg::Setlkw { arg } => {
                background.spawn_task(ops.setlk(header, arg, true, reply.into()))
            }
            Arg::Access { arg } => background.spawn_task(ops.access(header, arg, reply.into())),
            Arg::Create { arg } => background.spawn_task(ops.create(header, arg, reply.into())),
            Arg::Bmap { arg } => background.spawn_task(ops.bmap(header, arg, reply.into())),

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
                got_destroy: false,
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
