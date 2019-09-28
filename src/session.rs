use crate::{
    buffer::Buffer,
    error::Error, //
    op::Operations,
    request::Arg,
    MAX_WRITE_SIZE,
};
use fuse_async_abi::InitOut;
use futures::{
    future::{FusedFuture, FutureExt},
    io::{AsyncRead, AsyncWrite},
    select,
};
use std::{io, pin::Pin};

#[derive(Debug)]
pub struct Session {
    proto_major: Option<u32>,
    proto_minor: Option<u32>,
    got_init: bool,
    got_destroy: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// Create a new `Session`.
    pub fn new() -> Self {
        Self {
            proto_major: None,
            proto_minor: None,
            got_init: false,
            got_destroy: false,
        }
    }

    /// Returns the major protocol version negotiated with the kernel.
    pub fn proto_major(&self) -> Option<u32> {
        self.proto_major
    }

    /// Returns the minor protocol version negotiated with the kernel.
    pub fn proto_minor(&self) -> Option<u32> {
        self.proto_minor
    }

    #[cfg(feature = "tokio")]
    pub async fn run<'a, I, T>(
        &'a mut self,
        io: &'a mut I,
        buf: &'a mut Buffer,
        ops: &'a mut T,
    ) -> io::Result<()>
    where
        I: AsyncRead + AsyncWrite + Unpin,
        T: Operations + Send,
    {
        self.run_until(io, buf, ops, crate::tokio::default_shutdown_signal()?)
            .await?;
        Ok(())
    }

    /// Run a session with the kernel until the provided shutdown signal is received.
    pub async fn run_until<'a, I, T, S>(
        &'a mut self,
        io: &'a mut I,
        buf: &'a mut Buffer,
        ops: &'a mut T,
        sig: S,
    ) -> io::Result<Option<S::Output>>
    where
        I: AsyncRead + AsyncWrite + Unpin,
        T: Operations + Send,
        S: FusedFuture + Unpin,
    {
        let mut sig = sig;
        loop {
            let mut turn = self.turn(io, buf, ops);
            let mut turn = unsafe { Pin::new_unchecked(&mut turn) }.fuse();

            select! {
                res = turn => {
                    let destroyed = res?;
                    if destroyed {
                        log::debug!("The session is closed successfully");
                        return Ok(None);
                    }
                },
                res = sig => {
                    log::debug!("Got shutdown signal");
                    return Ok(Some(res))
                },
            };
        }
    }

    /// Receives one request from the channel, and returns its processing result.
    #[allow(clippy::cognitive_complexity)]
    pub async fn turn<'a, I, T>(
        &'a mut self,
        io: &'a mut I,
        buf: &'a mut Buffer,
        ops: &'a mut T,
    ) -> io::Result<bool>
    where
        I: AsyncRead + AsyncWrite + Unpin,
        T: Operations + Send,
    {
        if self.got_destroy {
            return Ok(true);
        }

        if buf.receive(io).await? {
            self.got_destroy = true;
            return Ok(true);
        }

        let (header, arg) = buf.parse()?;
        let unique = header.unique;
        let opcode = header.opcode;
        log::debug!("Got a request: header={:?}, arg={:?}", header, arg);

        macro_rules! reply_payload {
            ($e:expr) => {
                match ($e).await {
                    Ok(out) => buf.reply_payload(io, unique, &out).await?,
                    Err(Error(err)) => buf.reply_err(io, unique, err).await?,
                }
            };
        }

        macro_rules! reply_unit {
            ($e:expr) => {
                match ($e).await {
                    Ok(()) => buf.reply_unit(io, unique).await?,
                    Err(Error(err)) => buf.reply_err(io, unique, err).await?,
                }
            };
        }

        match arg {
            Arg::Init(arg) => {
                let mut init_out = InitOut::default();

                if arg.major > 7 {
                    log::debug!("wait for a second INIT request with a 7.X version.");
                    buf.reply_payload(io, unique, &init_out).await?;
                    return Ok(false);
                }

                if arg.major < 7 || (arg.major == 7 && arg.minor < 6) {
                    log::warn!("unsupported protocol version: {}.{}", arg.major, arg.minor);
                    buf.reply_err(io, unique, libc::EPROTO).await?;
                    return Err(io::Error::from_raw_os_error(libc::EPROTO));
                }

                // remember the protocol version
                self.proto_major = Some(arg.major);
                self.proto_minor = Some(arg.minor);

                // TODO: max_background, congestion_threshold, time_gran, max_pages
                init_out.max_readahead = arg.max_readahead;
                init_out.max_write = MAX_WRITE_SIZE as u32;

                self.got_init = true;
                if let Err(Error(err)) = ops.init(header, &arg, &mut init_out).await {
                    buf.reply_err(io, unique, err).await?;
                    return Ok(false);
                };
                buf.reply_payload(io, unique, &init_out).await?;
            }
            _ if !self.got_init => {
                log::warn!("ignoring an operation before init (opcode={:?})", opcode,);
                buf.reply_err(io, unique, libc::EIO).await?;
                return Ok(false);
            }
            Arg::Destroy => {
                ops.destroy().await;
                self.got_destroy = true;
                buf.reply_unit(io, unique).await?;
            }
            _ if self.got_destroy => {
                log::warn!(
                    "ignoring an operation before init (opcode={:?})",
                    header.opcode
                );
                buf.reply_err(io, unique, libc::EIO).await?;
                return Ok(false);
            }
            Arg::Lookup { name } => reply_payload!(ops.lookup(header, name)),
            Arg::Forget(arg) => {
                ops.forget(header, arg).await;
                buf.reply_none(io, unique).await?;
            }
            Arg::Getattr(arg) => reply_payload!(ops.getattr(header, &arg)),
            Arg::Setattr(arg) => reply_payload!(ops.setattr(header, &arg)),
            Arg::Readlink => reply_payload!(ops.readlink(header)),
            Arg::Symlink { name, link } => reply_payload!(ops.symlink(header, name, link)),
            Arg::Mknod { arg, name } => reply_payload!(ops.mknod(header, arg, name)),
            Arg::Mkdir { arg, name } => reply_payload!(ops.mkdir(header, arg, name)),
            Arg::Unlink { name } => reply_unit!(ops.unlink(header, name)),
            Arg::Rmdir { name } => reply_unit!(ops.unlink(header, name)),
            Arg::Rename { arg, name, newname } => {
                reply_unit!(ops.rename(header, arg, name, newname))
            }
            Arg::Link { arg, newname } => reply_payload!(ops.link(header, arg, newname)),
            Arg::Open(arg) => reply_payload!(ops.open(header, arg)),
            Arg::Read(arg) => reply_payload!(ops.read(header, arg)),
            Arg::Write { arg, data } => reply_payload!(ops.write(header, arg, data)),
            Arg::Release(arg) => reply_unit!(ops.release(header, arg)),
            Arg::Statfs => reply_payload!(ops.statfs(header)),
            Arg::Fsync(arg) => reply_unit!(ops.fsync(header, arg)),
            Arg::Setxattr { arg, name, value } => {
                reply_unit!(ops.setxattr(header, arg, name, value))
            }
            Arg::Getxattr { arg, name } => reply_payload!(ops.getxattr(header, arg, name)),
            Arg::Listxattr { arg } => reply_payload!(ops.listxattr(header, arg)),
            Arg::Removexattr { name } => reply_unit!(ops.removexattr(header, name)),
            Arg::Flush(arg) => reply_unit!(ops.flush(header, arg)),
            Arg::Opendir(arg) => reply_payload!(ops.opendir(header, arg)),
            Arg::Readdir(arg) => reply_payload!(ops.readdir(header, arg)),
            Arg::Releasedir(arg) => reply_unit!(ops.releasedir(header, arg)),
            Arg::Fsyncdir(arg) => reply_unit!(ops.fsyncdir(header, arg)),
            Arg::Getlk(arg) => reply_payload!(ops.getlk(header, arg)),
            Arg::Setlk(arg) => reply_unit!(ops.setlk(header, arg, false)),
            Arg::Setlkw(arg) => reply_unit!(ops.setlk(header, arg, true)),
            Arg::Access(arg) => reply_unit!(ops.access(header, arg)),
            Arg::Create(arg) => reply_payload!(ops.create(header, arg)),
            Arg::Bmap(arg) => reply_payload!(ops.bmap(header, arg)),

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
                log::warn!("unsupported opcode: {:?}", opcode);
                buf.reply_err(io, unique, libc::ENOSYS).await?;
            }
        }

        Ok(false)
    }
}
