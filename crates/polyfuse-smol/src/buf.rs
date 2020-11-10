use crate::channel::Channel;
use futures::{
    io::{AsyncRead, AsyncReadExt},
    task::{self, Poll},
};
use std::{
    io::{self, IoSliceMut, Read},
    mem, ops,
    pin::Pin,
};

struct Guard<'a> {
    vec: &'a mut Vec<u8>,
    len: usize,
}

impl<'a> Guard<'a> {
    fn new(vec: &'a mut Vec<u8>) -> Self {
        let len = vec.len();
        Self { vec, len }
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        unsafe {
            self.vec.set_len(self.len);
        }
    }
}

impl ops::Deref for Guard<'_> {
    type Target = Vec<u8>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &*self.vec
    }
}

impl ops::DerefMut for Guard<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.vec
    }
}

#[derive(Debug)]
pub struct RequestBuffer(io::Cursor<Vec<u8>>);

impl RequestBuffer {
    pub fn new(bufsize: usize) -> Self {
        Self(io::Cursor::new(vec![0; bufsize]))
    }

    pub async fn receive_from(&mut self, channel: &mut Channel) -> io::Result<bool> {
        let mut buf = Guard::new(self.0.get_mut());
        unsafe {
            let capacity = buf.capacity();
            buf.set_len(capacity);
        }

        loop {
            match channel.read(&mut buf[..]).await {
                Ok(len) => {
                    unsafe {
                        buf.set_len(len);
                    }
                    mem::forget(buf);
                    self.0.set_position(0);
                    return Ok(false);
                }
                Err(err) => match err.raw_os_error() {
                    Some(libc::ENODEV) => {
                        tracing::debug!("caught ENODEV. closing the connection");
                        return Ok(true);
                    }
                    Some(libc::ENOENT) => {
                        // ENOENT means that the operation was interrupted, and it is safe to restart.
                        tracing::debug!("caught ENOENT. ignore it and continue reading");
                        continue;
                    }
                    _ => return Err(err),
                },
            }
        }
    }
}

impl AsyncRead for RequestBuffer {
    #[inline]
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut task::Context<'_>,
        dst: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(self.get_mut().0.read(dst))
    }

    #[inline]
    fn poll_read_vectored(
        self: Pin<&mut Self>,
        _: &mut task::Context<'_>,
        dst: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(self.get_mut().0.read_vectored(dst))
    }
}
