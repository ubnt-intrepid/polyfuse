use crate::{
    reply::{reply_err, reply_payload, reply_unit, Payload},
    request::Arg,
};
use fuse_async_abi::InHeader;
use futures::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use std::io;

const RECV_BUF_SIZE: usize = crate::MAX_WRITE_SIZE + 4096;

#[derive(Debug)]
pub struct Buffer {
    recv_buf: Vec<u8>,
}

impl Buffer {
    pub fn new() -> Self {
        Self {
            recv_buf: Vec::with_capacity(RECV_BUF_SIZE),
        }
    }

    pub async fn receive<'a, I: ?Sized>(&'a mut self, io: &'a mut I) -> io::Result<bool>
    where
        I: AsyncRead + Unpin,
    {
        let old_len = self.recv_buf.len();
        unsafe {
            self.recv_buf.set_len(self.recv_buf.capacity());
        }

        loop {
            match io.read(&mut self.recv_buf[..]).await {
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

    pub fn parse(&self) -> io::Result<(&InHeader, Arg)> {
        crate::request::parse(&self.recv_buf[..])
    }

    pub async fn reply_err<'a, W>(&'a self, io: &'a mut W, unique: u64, err: i32) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        reply_err(io, unique, err).await
    }

    pub async fn reply_unit<'a, W>(&'a self, io: &'a mut W, unique: u64) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        reply_unit(io, unique).await
    }

    pub async fn reply_payload<'a, W, T>(
        &'a self,
        io: &'a mut W,
        unique: u64,
        payload: &'a T,
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
        T: Payload,
    {
        reply_payload(io, unique, payload).await
    }
}
