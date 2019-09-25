use crate::{
    channel::Channel,
    op::Operations,
    reply::InitOut,
    request::{Op, Request},
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

        let (in_header, op) = crate::request::parse(&self.recv_buf[..])?;
        let req = Request { in_header };
        let op = match op {
            Some(op) => op,
            None => {
                log::warn!("unsupported opcode: {:?}", in_header.opcode());
                crate::reply::reply_err(&mut self.ch, in_header, libc::ENOSYS).await?;
                return Ok(());
            }
        };

        log::debug!("Got an operation: {:?}", op);
        match op {
            Op::Init(op) => {
                let mut init_out = InitOut::default();

                if op.major() > 7 {
                    log::debug!("wait for a second INIT request with a 7.X version.");
                    crate::reply::reply_payload(&mut self.ch, in_header, 0, &init_out).await?;
                    return Ok(());
                }

                if op.major() < 7 || (op.major() == 7 && op.minor() < 6) {
                    log::warn!(
                        "unsupported protocol version: {}.{}",
                        op.major(),
                        op.minor()
                    );
                    crate::reply::reply_err(&mut self.ch, in_header, libc::EPROTO).await?;
                    return Ok(());
                }

                // remember the protocol version
                self.proto_major = op.major();
                self.proto_minor = op.minor();

                // TODO: max_background, congestion_threshold, time_gran, max_pages
                init_out.set_max_readahead(op.max_readahead());
                init_out.set_max_write(MAX_WRITE_SIZE as u32);

                self.got_init = true;
                if let Err(err) = ops.init(&req, &op, &mut init_out).await {
                    crate::reply::reply_err(&mut self.ch, in_header, err).await?;
                    return Ok(());
                };
                crate::reply::reply_payload(
                    &mut self.ch, //
                    in_header,
                    0,
                    &init_out,
                )
                .await?;
            }
            _ if !self.got_init => {
                log::warn!(
                    "ignoring an operation before init (opcode={:?})",
                    in_header.opcode()
                );
                crate::reply::reply_err(&mut self.ch, in_header, libc::EIO).await?;
                return Ok(());
            }

            Op::Destroy => {
                ops.destroy().await;
                self.got_destroy = true;
                crate::reply::reply_none(&mut self.ch, in_header).await?;
            }
            _ if self.got_destroy => {
                log::warn!(
                    "ignoring an operation before init (opcode={:?})",
                    in_header.opcode()
                );
                crate::reply::reply_err(&mut self.ch, in_header, libc::EIO).await?;
                return Ok(());
            }

            Op::Lookup { name } => {
                match ops.lookup(&req, name).await {
                    Ok(entry_out) => {
                        crate::reply::reply_payload(
                            &mut self.ch, //
                            in_header,
                            0,
                            &entry_out,
                        )
                        .await?
                    }
                    Err(err) => {
                        crate::reply::reply_err(
                            &mut self.ch, //
                            in_header,
                            err,
                        )
                        .await?;
                    }
                }
            }

            Op::Forget(op) => {
                ops.forget(&req, op.nlookup()).await;
                // no reply
            }

            Op::Getattr(op) => {
                match ops.getattr(&req, &op).await {
                    Ok(attr_out) => {
                        crate::reply::reply_payload(
                            &mut self.ch, //
                            in_header,
                            0,
                            &attr_out,
                        )
                        .await?
                    }
                    Err(err) => {
                        crate::reply::reply_err(
                            &mut self.ch, //
                            in_header,
                            err,
                        )
                        .await?
                    }
                }
            }

            Op::Open(op) => {
                match ops.open(&req, op).await {
                    Ok(open_out) => {
                        crate::reply::reply_payload(
                            &mut self.ch, //
                            in_header,
                            0,
                            &open_out,
                        )
                        .await?
                    }
                    Err(err) => {
                        crate::reply::reply_err(
                            &mut self.ch, //
                            in_header,
                            err,
                        )
                        .await?
                    }
                }
            }
            Op::Read(op) => match ops.read(&req, op).await {
                Ok(data) => crate::reply::reply_payload(&mut self.ch, in_header, 0, &*data).await?,
                Err(err) => crate::reply::reply_err(&mut self.ch, in_header, err).await?,
            },
            Op::Flush(op) => match ops.flush(&req, op).await {
                Ok(()) => crate::reply::reply_err(&mut self.ch, in_header, 0).await?,
                Err(err) => crate::reply::reply_err(&mut self.ch, in_header, err).await?,
            },
            Op::Release(op) => match ops.release(&req, op).await {
                Ok(()) => crate::reply::reply_err(&mut self.ch, in_header, 0).await?,
                Err(err) => crate::reply::reply_err(&mut self.ch, in_header, err).await?,
            },
        }

        Ok(())
    }
}
