use super::request::{Parser, Request, RequestKind};
use futures::{future::poll_fn, io::AsyncRead};
use std::{
    io,
    pin::Pin,
    task::{self, Poll},
};

/// A buffer to hold request data from the kernel.
#[derive(Debug)]
pub struct Buffer {
    recv_buf: Vec<u8>,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new(Self::DEFAULT_BUF_SIZE)
    }
}

impl Buffer {
    pub const DEFAULT_BUF_SIZE: usize = super::MAX_WRITE_SIZE as usize + 4096;

    /// Create a new `Buffer`.
    pub fn new(bufsize: usize) -> Self {
        Self {
            recv_buf: Vec::with_capacity(bufsize),
        }
    }

    /// Acquires an incoming request from the kernel.
    ///
    /// The received data is stored in the internal buffer, and could be
    /// retrieved using `decode`.
    pub fn poll_receive<I: ?Sized>(
        &mut self,
        cx: &mut task::Context,
        io: &mut I,
    ) -> Poll<io::Result<bool>>
    where
        I: AsyncRead + Unpin,
    {
        let old_len = self.recv_buf.len();
        unsafe {
            let capacity = self.recv_buf.capacity();
            self.recv_buf.set_len(capacity);
        }

        loop {
            match Pin::new(&mut *io).poll_read(cx, &mut self.recv_buf[..]) {
                Poll::Pending => {
                    unsafe {
                        self.recv_buf.set_len(old_len);
                    }
                    return Poll::Pending;
                }
                Poll::Ready(Ok(count)) => {
                    unsafe {
                        self.recv_buf.set_len(count);
                    }
                    return Poll::Ready(Ok(false));
                }
                Poll::Ready(Err(err)) => match err.raw_os_error() {
                    Some(libc::ENOENT) | Some(libc::EINTR) => {
                        log::debug!("continue reading from the kernel");
                        continue;
                    }
                    Some(libc::ENODEV) => {
                        log::debug!("the connection was closed by the kernel");
                        return Poll::Ready(Ok(true));
                    }
                    _ => return Poll::Ready(Err(err)),
                },
            }
        }
    }

    /// Receive a request from the kernel asynchronously.
    ///
    /// This method is a helper to call `poll_receive` in async functions.
    pub async fn receive<I: ?Sized>(&mut self, io: &mut I) -> io::Result<bool>
    where
        I: AsyncRead + Unpin,
    {
        poll_fn(move |cx| self.poll_receive(cx, io)).await
    }

    /// Extract the last incoming request.
    pub fn decode(&mut self) -> io::Result<(Request<'_>, Option<&[u8]>)> {
        let (header, kind, offset) = Parser::new(&self.recv_buf[..]).parse()?;
        let data = match kind {
            RequestKind::Write { arg: write_in } => {
                let size = write_in.size as usize;
                if offset + size < self.recv_buf.len() {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "decode: write"));
                }
                Some(&self.recv_buf[offset..offset + size])
            }
            RequestKind::NotifyReply { arg: retrieve_in } => {
                let size = retrieve_in.size as usize;
                if offset + size < self.recv_buf.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "decode: retrieve_in",
                    ));
                }
                Some(&self.recv_buf[offset..offset + size])
            }
            _ => None,
        };
        Ok((Request::new(header, kind), data))
    }
}
