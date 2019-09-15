use crate::{
    abi::{fuse_attr_out, fuse_init_out, fuse_open_out}, //
    channel::Channel,
    op::Operations,
    request::{Op, Request},
};
use std::{
    io::{self},
    mem,
    path::PathBuf,
};
use tokio_io::AsyncReadExt;

const MAX_WRITE_SIZE: usize = 16 * 1024 * 1024;
const RECV_BUFFER_SIZE: usize = MAX_WRITE_SIZE + 4096;

#[derive(Debug)]
pub struct Filesystem {
    ch: Channel,
    recv_buf: Vec<u8>,
    exited: bool,
}

impl Filesystem {
    pub fn new(mountpoint: impl Into<PathBuf>) -> io::Result<Self> {
        Ok(Self {
            ch: Channel::new(mountpoint)?,
            recv_buf: Vec::with_capacity(RECV_BUFFER_SIZE),
            exited: false,
        })
    }

    pub async fn receive(&mut self) -> io::Result<bool> {
        // TODO: splice

        if self.exited {
            return Ok(true);
        }

        let old_len = self.recv_buf.len();
        unsafe {
            self.recv_buf.set_len(self.recv_buf.capacity());
        }

        loop {
            match self.ch.read(&mut self.recv_buf[..]).await {
                Ok(count) => {
                    unsafe { self.recv_buf.set_len(count) };
                    return Ok(false);
                }
                Err(err) => match err.raw_os_error() {
                    Some(libc::ENOENT) | Some(libc::EINTR) => continue,
                    Some(libc::ENODEV) => {
                        unsafe {
                            self.recv_buf.set_len(old_len);
                        }
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
        if self.exited {
            return Ok(());
        }

        let (in_header, op) = crate::request::parse(&self.recv_buf[..])?;
        let req = Request { in_header };
        let op = match op {
            Some(op) => {
                log::debug!("opcode = {:?}", in_header.opcode());
                op
            }
            None => {
                log::debug!("unsupported opcode: {:?}", in_header.opcode());
                crate::reply::reply_err(&mut self.ch, in_header, libc::ENOSYS).await?;
                return Ok(());
            }
        };

        match op {
            Op::Init(op) => {
                ops.init(&req, &op).await;
                let mut init_out: fuse_init_out = unsafe { mem::zeroed() };
                init_out.major = op.major();
                init_out.minor = op.minor();
                init_out.max_readahead = op.max_readahead();
                init_out.flags = op.flags();
                crate::reply::reply_payload(
                    &mut self.ch, //
                    in_header,
                    0,
                    &init_out,
                )
                .await?;
            }
            Op::Destroy => {
                ops.destroy().await;
                self.exited = true;
                crate::reply::reply_none(&mut self.ch, in_header).await?;
            }
            Op::Getattr(op) => match ops.getattr(&req, &op).await {
                Ok((attr, valid, valid_nsec)) => {
                    let mut attr_out: fuse_attr_out = unsafe { mem::zeroed() };
                    attr_out.attr_valid = valid;
                    attr_out.attr_valid_nsec = valid_nsec;
                    attr_out.attr = attr.0;
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
            },
            Op::Open(op) => match ops.open(&req, op).await {
                Ok((fh, open_flags)) => {
                    let mut open_out: fuse_open_out = unsafe { mem::zeroed() };
                    open_out.fh = fh;
                    open_out.open_flags = open_flags;
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
            },
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
