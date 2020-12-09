use polyfuse::{Connection, MountOptions};

use async_io::Async;
use futures::{
    io::AsyncRead,
    task::{self, Poll},
};
use std::{
    io,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Weak},
};

pub struct AsyncConnection {
    inner: Arc<Async<Connection>>,
}

impl AsyncConnection {
    pub async fn open(mountpoint: PathBuf, mountopts: MountOptions) -> io::Result<Self> {
        let conn = async_std::task::spawn_blocking(move || Connection::open(mountpoint, mountopts))
            .await?;
        let inner = Async::new(conn)?;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    pub fn writer(&self) -> Writer {
        Writer {
            conn: Arc::downgrade(&self.inner),
        }
    }
}

impl AsyncRead for &AsyncConnection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.get_mut().inner).poll_read(cx, buf)
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        bufs: &mut [io::IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.get_mut().inner).poll_read_vectored(cx, bufs)
    }
}

impl io::Write for &AsyncConnection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.get_ref().write(buf)
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.inner.get_ref().write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.get_ref().flush()
    }
}

pub struct Writer {
    conn: Weak<Async<Connection>>,
}

impl io::Write for &Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(conn) = self.conn.upgrade() {
            conn.get_ref().write(buf)
        } else {
            Ok(0)
        }
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        if let Some(conn) = self.conn.upgrade() {
            conn.get_ref().write_vectored(bufs)
        } else {
            Ok(0)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(conn) = self.conn.upgrade() {
            conn.get_ref().flush()
        } else {
            Ok(())
        }
    }
}
