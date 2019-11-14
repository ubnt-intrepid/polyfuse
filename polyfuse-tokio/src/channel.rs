//! Establish connection with FUSE kernel driver.

use crate::mount::{Connection, MountOptions};
use futures::{
    io::AsyncRead,
    ready,
    task::{self, Poll},
};
use mio::{unix::UnixReady, Ready};
use polyfuse::reply::{ReplyHeader, ReplyWriter};
use std::{
    io::{self, IoSlice, IoSliceMut},
    path::Path,
    pin::Pin,
};
use tokio::{
    net::util::PollEvented,
    sync::semaphore::{Permit, Semaphore},
};

/// Asynchronous I/O object that communicates with the FUSE kernel driver.
#[derive(Debug)]
pub struct Channel(PollEvented<Connection>);

impl Channel {
    /// Establish a connection with the FUSE kernel driver.
    pub fn open(mountpoint: &Path, mountopts: &MountOptions) -> io::Result<Self> {
        let conn = Connection::open(mountpoint, mountopts)?;
        let evented = PollEvented::new(conn)?;
        Ok(Self(evented))
    }

    fn poll_read_with<F, R>(&self, cx: &mut task::Context<'_>, f: F) -> Poll<io::Result<R>>
    where
        F: FnOnce(&Connection) -> io::Result<R>,
    {
        let mut ready = Ready::readable();
        ready.insert(UnixReady::error());
        ready!(self.0.poll_read_ready(cx, ready))?;

        match f(self.0.get_ref()) {
            Ok(ret) => Poll::Ready(Ok(ret)),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.0.clear_read_ready(cx, ready)?;
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_write_with<F, R>(&self, cx: &mut task::Context<'_>, f: F) -> Poll<io::Result<R>>
    where
        F: FnOnce(&Connection) -> io::Result<R>,
    {
        ready!(self.0.poll_write_ready(cx))?;

        match f(self.0.get_ref()) {
            Ok(ret) => Poll::Ready(Ok(ret)),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.0.clear_write_ready(cx)?;
                Poll::Pending
            }
            Err(e) => {
                log::debug!("write error: {}", e);
                Poll::Ready(Err(e))
            }
        }
    }

    /// Attempt to create a clone of this channel.
    pub fn try_clone(&self, ioc_clone: bool) -> io::Result<Self> {
        let conn = self.0.get_ref().try_clone(ioc_clone)?;
        Ok(Self(PollEvented::new(conn)?))
    }

    pub fn shared_writer(&self, ioc_clone: bool) -> io::Result<SharedWriter> {
        let channel = self.try_clone(ioc_clone)?;
        Ok(SharedWriter::new2(channel))
    }
}

impl AsyncRead for Channel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_with(cx, |fd| fd.read(dst))
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [IoSliceMut],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_with(cx, |fd| fd.read_vectored(dst))
    }
}

impl ReplyWriter for Channel {
    type Permit = ();

    fn poll_acquire_lock(&self, _: &mut task::Context, _: &mut ()) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_write_reply(
        &self,
        cx: &mut task::Context<'_>,
        header: &ReplyHeader,
        src: &[&[u8]],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_with(cx, |fd| {
            let vec = Some(IoSlice::new(header.as_ref()))
                .into_iter()
                .chain(src.iter().copied().map(IoSlice::new))
                .collect::<Vec<_>>();
            fd.write_vectored(&vec[..])
        })
    }

    fn release_lock(&self, _: &mut ()) {}
}

#[derive(Debug)]
pub struct SharedWriter {
    channel: Channel,
    semaphore: Semaphore,
}

impl SharedWriter {
    fn new2(channel: Channel) -> Self {
        Self {
            channel,
            semaphore: Semaphore::new(1),
        }
    }
}

impl ReplyWriter for SharedWriter {
    type Permit = Permit;

    fn poll_acquire_lock(
        &self,
        cx: &mut task::Context,
        permit: &mut Permit,
    ) -> Poll<io::Result<()>> {
        futures::ready!(permit.poll_acquire(cx, &self.semaphore))
            .unwrap_or_else(|_| unreachable!());
        Poll::Ready(Ok(()))
    }

    fn poll_write_reply(
        &self,
        cx: &mut task::Context<'_>,
        header: &ReplyHeader,
        src: &[&[u8]],
    ) -> Poll<io::Result<usize>> {
        self.channel.poll_write_reply(cx, header, src)
    }

    fn release_lock(&self, permit: &mut Permit) {
        permit.release(&self.semaphore);
    }
}
