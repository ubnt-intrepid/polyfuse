use crate::channel::Channel;
use futures::{
    io::{AsyncRead, AsyncReadExt},
    task::{self, Poll},
};
use polyfuse::io::Reader;
use std::{
    io::{self, IoSliceMut, Read},
    pin::Pin,
};

#[derive(Debug)]
pub struct RequestBuffer(io::Cursor<Vec<u8>>);

impl RequestBuffer {
    pub fn new(bufsize: usize) -> Self {
        Self(io::Cursor::new(vec![0; bufsize]))
    }

    pub fn reset_position(&mut self) {
        self.0.set_position(0);
    }

    pub async fn receive_from(&mut self, channel: &mut Channel) -> io::Result<bool> {
        loop {
            match channel.read(&mut self.0.get_mut()[..]).await {
                Ok(_) => return Ok(false),
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

impl Reader for RequestBuffer {}
