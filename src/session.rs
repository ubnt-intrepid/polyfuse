use crate::{
    error::Error, //
    op::Operations,
    request::Op,
    MAX_WRITE_SIZE,
};
use fuse_async_abi::InitOut;
use futures::{
    future::{FusedFuture, FutureExt},
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    select,
    stream::StreamExt,
};
use std::{io, pin::Pin};

#[derive(Debug)]
pub struct Session {
    proto_major: Option<u32>,
    proto_minor: Option<u32>,
    got_init: bool,
    got_destroy: bool,
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

    /// Run a session with the kernel.
    pub async fn run<'a, I, T>(
        &'a mut self,
        io: &'a mut I,
        buf: &'a mut Vec<u8>,
        op: &'a mut T,
    ) -> io::Result<()>
    where
        I: AsyncRead + AsyncWrite + Unpin,
        T: Operations,
    {
        self.run_until(io, buf, op, default_shutdown_signal()?)
            .await?;
        Ok(())
    }

    /// Run a session with the kernel until the provided shutdown signal is received.
    pub async fn run_until<'a, I, T, S>(
        &'a mut self,
        io: &'a mut I,
        buf: &'a mut Vec<u8>,
        op: &'a mut T,
        sig: S,
    ) -> io::Result<Option<S::Output>>
    where
        I: AsyncRead + AsyncWrite + Unpin,
        T: Operations,
        S: FusedFuture + Unpin,
    {
        let mut sig = sig;
        loop {
            let mut turn = self.turn(io, buf, op);
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
    pub async fn turn<'a, I, T>(
        &'a mut self,
        io: &'a mut I,
        buf: &'a mut Vec<u8>,
        op: &'a mut T,
    ) -> io::Result<bool>
    where
        I: AsyncRead + AsyncWrite + Unpin,
        T: Operations,
    {
        if self.receive(io, buf).await? {
            return Ok(true);
        }
        self.process(io, buf, op).await?;
        Ok(false)
    }

    async fn receive<'a, I>(&'a mut self, io: &'a mut I, buf: &'a mut Vec<u8>) -> io::Result<bool>
    where
        I: AsyncRead + AsyncWrite + Unpin,
    {
        if self.got_destroy {
            return Ok(true);
        }

        let old_len = buf.len();
        unsafe {
            buf.set_len(buf.capacity());
        }

        loop {
            match io.read(&mut buf[..]).await {
                Ok(count) => {
                    log::debug!("read {} bytes", count);
                    unsafe { buf.set_len(count) };
                    return Ok(false);
                }
                Err(err) => match err.raw_os_error() {
                    Some(libc::ENOENT) | Some(libc::EINTR) => continue,
                    Some(libc::ENODEV) => {
                        log::debug!("connection to the kernel was closed");
                        unsafe {
                            buf.set_len(old_len);
                        }
                        self.got_destroy = true;
                        return Ok(true);
                    }
                    _ => {
                        unsafe {
                            buf.set_len(old_len);
                        }
                        return Err(err);
                    }
                },
            }
        }
    }

    async fn process<'a, I, T>(
        &'a mut self,
        io: &'a mut I,
        buf: &'a mut Vec<u8>,
        ops: &'a mut T,
    ) -> io::Result<()>
    where
        I: AsyncRead + AsyncWrite + Unpin,
        T: Operations,
    {
        if self.got_destroy {
            return Ok(());
        }

        let (header, op) = crate::request::parse(&buf[..])?;
        log::debug!("Got a request: header={:?}, op={:?}", header, op);

        macro_rules! reply_payload {
            ($e:expr) => {
                match ($e).await {
                    Ok(out) => crate::reply::reply_payload(io, header.unique(), 0, &out).await?,
                    Err(Error(err)) => {
                        crate::reply::reply_err(io, header.unique(), err).await?;
                    }
                }
            };
        }

        macro_rules! reply_unit {
            ($e:expr) => {
                match ($e).await {
                    Ok(()) => crate::reply::reply_unit(io, header.unique()).await?,
                    Err(Error(err)) => {
                        crate::reply::reply_err(io, header.unique(), err).await?;
                    }
                }
            };
        }

        match op {
            Op::Init(op) => {
                let mut init_out = InitOut::default();

                if op.major() > 7 {
                    log::debug!("wait for a second INIT request with a 7.X version.");
                    crate::reply::reply_payload(io, header.unique(), 0, &init_out).await?;
                    return Ok(());
                }

                if op.major() < 7 || (op.major() == 7 && op.minor() < 6) {
                    log::warn!(
                        "unsupported protocol version: {}.{}",
                        op.major(),
                        op.minor()
                    );
                    crate::reply::reply_err(io, header.unique(), libc::EPROTO).await?;
                    return Ok(());
                }

                // remember the protocol version
                self.proto_major = Some(op.major());
                self.proto_minor = Some(op.minor());

                // TODO: max_background, congestion_threshold, time_gran, max_pages
                init_out.set_max_readahead(op.max_readahead());
                init_out.set_max_write(MAX_WRITE_SIZE as u32);

                self.got_init = true;
                if let Err(Error(err)) = ops.init(header, &op, &mut init_out).await {
                    crate::reply::reply_err(io, header.unique(), err).await?;
                    return Ok(());
                };
                crate::reply::reply_payload(io, header.unique(), 0, &init_out).await?;
            }
            _ if !self.got_init => {
                log::warn!(
                    "ignoring an operation before init (opcode={:?})",
                    header.opcode()
                );
                crate::reply::reply_err(io, header.unique(), libc::EIO).await?;
                return Ok(());
            }
            Op::Destroy => {
                ops.destroy().await;
                self.got_destroy = true;
                crate::reply::reply_unit(io, header.unique()).await?;
            }
            _ if self.got_destroy => {
                log::warn!(
                    "ignoring an operation before init (opcode={:?})",
                    header.opcode()
                );
                crate::reply::reply_err(io, header.unique(), libc::EIO).await?;
                return Ok(());
            }
            Op::Lookup { name } => reply_payload!(ops.lookup(header, name)),
            Op::Forget(op) => {
                ops.forget(header, op).await;
                // no reply
            }
            Op::Getattr(op) => reply_payload!(ops.getattr(header, &op)),
            Op::Setattr(op) => reply_payload!(ops.setattr(header, &op)),
            Op::Readlink => reply_payload!(ops.readlink(header)),
            Op::Symlink { name, link } => reply_payload!(ops.symlink(header, name, link)),
            Op::Mknod { op, name } => reply_payload!(ops.mknod(header, op, name)),
            Op::Mkdir { op, name } => reply_payload!(ops.mkdir(header, op, name)),
            Op::Unlink { name } => reply_unit!(ops.unlink(header, name)),
            Op::Rmdir { name } => reply_unit!(ops.unlink(header, name)),
            Op::Rename { op, name, newname } => reply_unit!(ops.rename(header, op, name, newname)),
            Op::Link { op, newname } => reply_payload!(ops.link(header, op, newname)),
            Op::Open(op) => reply_payload!(ops.open(header, op)),
            Op::Read(op) => reply_payload!(ops.read(header, op)),
            Op::Write { op, data } => reply_payload!(ops.write(header, op, data)),
            Op::Release(op) => reply_unit!(ops.release(header, op)),
            Op::Statfs => reply_payload!(ops.statfs(header)),
            Op::Fsync(op) => reply_unit!(ops.fsync(header, op)),
            Op::Setxattr { op, name, value } => reply_unit!(ops.setxattr(header, op, name, value)),
            Op::Getxattr { op, name } => reply_payload!(ops.getxattr(header, op, name)),
            Op::Listxattr { op } => reply_payload!(ops.listxattr(header, op)),
            Op::Removexattr { name } => reply_unit!(ops.removexattr(header, name)),
            Op::Flush(op) => reply_unit!(ops.flush(header, op)),
            Op::Opendir(op) => reply_payload!(ops.opendir(header, op)),
            Op::Readdir(op) => reply_payload!(ops.readdir(header, op)),
            Op::Releasedir(op) => reply_unit!(ops.releasedir(header, op)),
            Op::Fsyncdir(op) => reply_unit!(ops.fsyncdir(header, op)),
            Op::Getlk(op) => reply_payload!(ops.getlk(header, op)),
            Op::Setlk(op) => reply_unit!(ops.setlk(header, op, false)),
            Op::Setlkw(op) => reply_unit!(ops.setlk(header, op, true)),
            Op::Access(op) => reply_unit!(ops.access(header, op)),
            Op::Create(op) => reply_payload!(ops.create(header, op)),
            Op::Bmap(op) => reply_payload!(ops.bmap(header, op)),

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
            Op::Unknown { opcode, .. } => {
                log::warn!("unsupported opcode: {:?}", opcode);
                crate::reply::reply_err(io, header.unique(), libc::ENOSYS).await?;
                return Ok(());
            }
        }

        Ok(())
    }
}

fn default_shutdown_signal() -> io::Result<impl FusedFuture<Output = ()> + Unpin> {
    let ctrl_c = tokio_net::signal::ctrl_c()?;
    Ok(ctrl_c.into_future().map(|_| ()))
}
