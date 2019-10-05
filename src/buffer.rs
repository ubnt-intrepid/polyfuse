use crate::{
    abi::InHeader,
    request::{Arg, Parser},
};
use futures::io::{AsyncRead, AsyncReadExt};
use std::io;

const RECV_BUF_SIZE: usize = crate::MAX_WRITE_SIZE + 4096;

#[derive(Debug)]
pub enum Data<'a> {
    Bytes(&'a [u8]),

    #[doc(hidden)]
    __NonExhaustive(()),
}

/// A buffer to hold request data from the kernel.
#[derive(Debug)]
pub struct Buffer {
    recv_buf: Vec<u8>,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Buffer {
    /// Create a new `Buffer`.
    pub fn new() -> Self {
        Self {
            recv_buf: Vec::with_capacity(RECV_BUF_SIZE),
        }
    }

    /// Receive a request from the kernel.
    #[allow(clippy::needless_lifetimes)]
    pub async fn receive<'a, I: ?Sized>(
        &'a mut self,
        io: &mut I,
    ) -> io::Result<Option<(&'a InHeader, Arg<'a>, Option<Data<'a>>)>>
    where
        I: AsyncRead + Unpin,
    {
        let terminated = ReceiveSession::new(&mut self.recv_buf).run(io).await?;
        if terminated {
            log::debug!("connection to the kernel was closed");
            return Ok(None);
        }

        log::debug!("receive {} bytes", self.recv_buf.len());

        let (header, arg, offset) = Parser::new(&self.recv_buf[..]).parse()?;
        let data = match arg {
            Arg::Write(arg) => {
                let size = arg.size as usize;
                if offset + size < self.recv_buf.len() {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "receive_write"));
                }
                Some(Data::Bytes(&self.recv_buf[offset..offset + size]))
            }
            _ => None,
        };
        Ok(Some((header, arg, data)))
    }
}

struct ReceiveSession<'a> {
    recv_buf: &'a mut Vec<u8>,
    old_len: usize,
}

impl Drop for ReceiveSession<'_> {
    fn drop(&mut self) {
        unsafe {
            self.recv_buf.set_len(self.old_len);
        }
    }
}

impl<'a> ReceiveSession<'a> {
    fn new(recv_buf: &'a mut Vec<u8>) -> Self {
        let old_len = recv_buf.len();
        unsafe {
            recv_buf.set_len(recv_buf.capacity());
        }
        Self { recv_buf, old_len }
    }

    async fn run<I: ?Sized>(self, io: &mut I) -> io::Result<bool>
    where
        I: AsyncRead + Unpin,
    {
        loop {
            match io.read(&mut self.recv_buf[..]).await {
                Ok(count) => {
                    unsafe { self.recv_buf.set_len(count) };
                    std::mem::forget(self); // ignore drop
                    return Ok(false);
                }
                Err(err) => match err.raw_os_error() {
                    Some(libc::ENOENT) | Some(libc::EINTR) => continue,
                    Some(libc::ENODEV) => return Ok(true),
                    _ => return Err(err),
                },
            }
        }
    }
}
