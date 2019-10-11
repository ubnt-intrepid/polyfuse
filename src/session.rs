//! FUSE session driver.

use crate::{
    abi::InitOut, //
    op::Operations,
    reply::ReplyRaw,
    request::{Arg, Buffer, Request, MAX_WRITE_SIZE},
};
use futures::io::{AsyncRead, AsyncWrite};
use std::io;

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

    /// Process an incoming request and return its reply to the kernel.
    #[allow(clippy::cognitive_complexity)]
    pub async fn process<T>(
        &mut self,
        request: Request<'_>,
        data: Option<T>,
        reply: ReplyRaw<'_>,
        ops: &mut impl Operations<T>,
    ) -> io::Result<bool> {
        let Request { header, arg, .. } = request;
        match arg {
            Arg::Init { .. } => reply.reply_err(libc::EIO).await?,
            Arg::Destroy => {
                self.got_destroy = true;
                reply.reply(0, &[]).await?;
            }
            _ if self.got_destroy => {
                log::warn!(
                    "ignoring an operation after destroy (opcode={:?})",
                    header.opcode
                );
                reply.reply_err(libc::EIO).await?;
                return Ok(false);
            }
            Arg::Lookup { name } => ops.lookup(header, name, reply.into()).await?,
            Arg::Forget { arg } => {
                ops.forget(header, arg);
            }
            Arg::Getattr { arg } => ops.getattr(header, &arg, reply.into()).await?,
            Arg::Setattr { arg } => ops.setattr(header, &arg, reply.into()).await?,
            Arg::Readlink => ops.readlink(header, reply.into()).await?,
            Arg::Symlink { name, link } => ops.symlink(header, name, link, reply.into()).await?,
            Arg::Mknod { arg, name } => ops.mknod(header, arg, name, reply.into()).await?,
            Arg::Mkdir { arg, name } => ops.mkdir(header, arg, name, reply.into()).await?,
            Arg::Unlink { name } => ops.unlink(header, name, reply.into()).await?,
            Arg::Rmdir { name } => ops.unlink(header, name, reply.into()).await?,
            Arg::Rename { arg, name, newname } => {
                ops.rename(header, arg, name, newname, reply.into()).await?
            }
            Arg::Link { arg, newname } => ops.link(header, arg, newname, reply.into()).await?,
            Arg::Open { arg } => ops.open(header, arg, reply.into()).await?,
            Arg::Read { arg } => ops.read(header, arg, reply.into()).await?,
            Arg::Write { arg } => match data {
                Some(data) => ops.write(header, arg, data, reply.into()).await?,
                None => panic!("unexpected condition"),
            },
            Arg::Release { arg } => ops.release(header, arg, reply.into()).await?,
            Arg::Statfs => ops.statfs(header, reply.into()).await?,
            Arg::Fsync { arg } => ops.fsync(header, arg, reply.into()).await?,
            Arg::Setxattr { arg, name, value } => {
                ops.setxattr(header, arg, name, value, reply.into()).await?
            }
            Arg::Getxattr { arg, name } => ops.getxattr(header, arg, name, reply.into()).await?,
            Arg::Listxattr { arg } => ops.listxattr(header, arg, reply.into()).await?,
            Arg::Removexattr { name } => ops.removexattr(header, name, reply.into()).await?,
            Arg::Flush { arg } => ops.flush(header, arg, reply.into()).await?,
            Arg::Opendir { arg } => ops.opendir(header, arg, reply.into()).await?,
            Arg::Readdir { arg } => ops.readdir(header, arg, reply.into()).await?,
            Arg::Releasedir { arg } => ops.releasedir(header, arg, reply.into()).await?,
            Arg::Fsyncdir { arg } => ops.fsyncdir(header, arg, reply.into()).await?,
            Arg::Getlk { arg } => ops.getlk(header, arg, reply.into()).await?,
            Arg::Setlk { arg } => ops.setlk(header, arg, false, reply.into()).await?,
            Arg::Setlkw { arg } => ops.setlk(header, arg, true, reply.into()).await?,
            Arg::Access { arg } => ops.access(header, arg, reply.into()).await?,
            Arg::Create { arg } => ops.create(header, arg, reply.into()).await?,
            Arg::Bmap { arg } => ops.bmap(header, arg, reply.into()).await?,

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
                reply.reply_err(libc::ENOSYS).await?;
            }
        }

        Ok(false)
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
