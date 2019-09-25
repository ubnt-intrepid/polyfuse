use crate::{
    channel::Channel, //
    op::Operations,
    reply::InitOut,
    request::Op,
};
use std::{
    io::{self},
    path::PathBuf,
};
use tokio_io::AsyncReadExt;

const MAX_WRITE_SIZE: usize = 16 * 1024 * 1024;
const RECV_BUFFER_SIZE: usize = MAX_WRITE_SIZE + 4096;

#[derive(Debug)]
pub struct Filesystem {
    ch: Channel,
    recv_buf: Vec<u8>,
    got_init: bool,
    got_destroy: bool,
    proto_major: u32,
    proto_minor: u32,
}

impl Filesystem {
    pub fn new(mountpoint: impl Into<PathBuf>) -> io::Result<Self> {
        Ok(Self {
            ch: Channel::new(mountpoint)?,
            recv_buf: Vec::with_capacity(RECV_BUFFER_SIZE),
            got_init: false,
            got_destroy: false,
            proto_major: 0,
            proto_minor: 0,
        })
    }

    pub async fn receive(&mut self) -> io::Result<bool> {
        // TODO: splice

        if self.got_destroy {
            return Ok(true);
        }

        let old_len = self.recv_buf.len();
        unsafe {
            self.recv_buf.set_len(self.recv_buf.capacity());
        }

        loop {
            match self.ch.read(&mut self.recv_buf[..]).await {
                Ok(count) => {
                    log::debug!("read {} bytes", count);
                    unsafe { self.recv_buf.set_len(count) };
                    return Ok(false);
                }
                Err(err) => match err.raw_os_error() {
                    Some(libc::ENOENT) | Some(libc::EINTR) => continue,
                    Some(libc::ENODEV) => {
                        log::debug!("connection to the kernel was closed");
                        unsafe {
                            self.recv_buf.set_len(old_len);
                        }
                        self.got_destroy = true;
                        return Ok(true);
                    }
                    _ => {
                        unsafe {
                            self.recv_buf.set_len(old_len);
                        }
                        return Err(err);
                    }
                },
            }
        }
    }

    pub async fn process<'a, T: Operations>(&'a mut self, ops: &'a mut T) -> io::Result<()> {
        if self.got_destroy {
            return Ok(());
        }

        let (header, op) = crate::request::parse(&self.recv_buf[..])?;
        log::debug!("Got a request: header={:?}, op={:?}", header, op);

        macro_rules! reply_payload {
            ($e:expr) => {
                match ($e).await {
                    Ok(out) => {
                        crate::reply::reply_payload(&mut self.ch, header.unique(), 0, &out).await?
                    }
                    Err(err) => {
                        crate::reply::reply_err(&mut self.ch, header.unique(), err).await?;
                    }
                }
            };
        }

        macro_rules! reply_unit {
            ($e:expr) => {
                match ($e).await {
                    Ok(()) => crate::reply::reply_unit(&mut self.ch, header.unique()).await?,
                    Err(err) => {
                        crate::reply::reply_err(&mut self.ch, header.unique(), err).await?;
                    }
                }
            };
        }

        match op {
            Op::Init(op) => {
                let mut init_out = InitOut::default();

                if op.major() > 7 {
                    log::debug!("wait for a second INIT request with a 7.X version.");
                    crate::reply::reply_payload(&mut self.ch, header.unique(), 0, &init_out)
                        .await?;
                    return Ok(());
                }

                if op.major() < 7 || (op.major() == 7 && op.minor() < 6) {
                    log::warn!(
                        "unsupported protocol version: {}.{}",
                        op.major(),
                        op.minor()
                    );
                    crate::reply::reply_err(&mut self.ch, header.unique(), libc::EPROTO).await?;
                    return Ok(());
                }

                // remember the protocol version
                self.proto_major = op.major();
                self.proto_minor = op.minor();

                // TODO: max_background, congestion_threshold, time_gran, max_pages
                init_out.set_max_readahead(op.max_readahead());
                init_out.set_max_write(MAX_WRITE_SIZE as u32);

                self.got_init = true;
                if let Err(err) = ops.init(header, &op, &mut init_out).await {
                    crate::reply::reply_err(&mut self.ch, header.unique(), err).await?;
                    return Ok(());
                };
                crate::reply::reply_payload(
                    &mut self.ch, //
                    header.unique(),
                    0,
                    &init_out,
                )
                .await?;
            }
            _ if !self.got_init => {
                log::warn!(
                    "ignoring an operation before init (opcode={:?})",
                    header.opcode()
                );
                crate::reply::reply_err(&mut self.ch, header.unique(), libc::EIO).await?;
                return Ok(());
            }

            Op::Destroy => {
                ops.destroy().await;
                self.got_destroy = true;
                crate::reply::reply_unit(&mut self.ch, header.unique()).await?;
            }
            _ if self.got_destroy => {
                log::warn!(
                    "ignoring an operation before init (opcode={:?})",
                    header.opcode()
                );
                crate::reply::reply_err(&mut self.ch, header.unique(), libc::EIO).await?;
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
            Op::Read(op) => match ops.read(header, op).await {
                Ok(data) => {
                    crate::reply::reply_payload(&mut self.ch, header.unique(), 0, &*data).await?
                }
                Err(err) => crate::reply::reply_err(&mut self.ch, header.unique(), err).await?,
            },

            Op::Flush(op) => reply_unit!(ops.flush(header, op)),
            Op::Release(op) => reply_unit!(ops.release(header, op)),

            Op::Setxattr { op, name, value } => reply_unit!(ops.setxattr(header, op, name, value)),
            Op::Getxattr { op, name } => reply_payload!(ops.getxattr(header, op, name)),
            Op::Listxattr { op } => reply_payload!(ops.listxattr(header, op)),
            Op::Removexattr { name } => reply_unit!(ops.removexattr(header, name)),

            Op::Unknown { opcode, .. } => {
                log::warn!("unsupported opcode: {:?}", opcode);
                crate::reply::reply_err(&mut self.ch, header.unique(), libc::ENOSYS).await?;
                return Ok(());
            }
        }

        Ok(())
    }
}
