use crate::{
    conn::Connection,
    request::Request,
    session::{CapabilityFlags, Session, SessionInitializer},
};
use async_io::Async;
use futures::io::AsyncReadExt as _;
use std::{ffi::OsStr, io, path::Path, sync::Arc};

#[derive(Default)]
pub struct Builder {
    session: SessionInitializer,
}

impl Builder {
    /// Return a reference to the capability flags.
    pub fn flags(&mut self) -> &mut CapabilityFlags {
        &mut self.session.flags
    }

    /// Set the maximum readahead.
    pub fn max_readahead(&mut self, value: u32) -> &mut Self {
        self.session.max_readahead = value;
        self
    }

    /// Set the maximum size of the write buffer.
    // ///
    // /// # Panic
    // /// It causes an assertion panic if the setting value is
    // /// less than the absolute minimum.
    pub fn max_write(&mut self, value: u32) -> &mut Self {
        // assert!(
        //     value >= MIN_MAX_WRITE,
        //     "max_write must be greater or equal to {}",
        //     MIN_MAX_WRITE,
        // );
        self.session.max_write = value;
        self
    }

    /// Return the maximum number of pending *background* requests.
    pub fn max_background(&mut self, max_background: u16) -> &mut Self {
        self.session.max_background = max_background;
        self
    }

    /// Set the threshold number of pending background requests
    /// that the kernel marks the filesystem as *congested*.
    ///
    /// If the setting value is 0, the value is automatically
    /// calculated by using max_background.
    ///
    /// # Panics
    /// It cause a panic if the setting value is greater than `max_background`.
    pub fn congestion_threshold(&mut self, mut threshold: u16) -> &mut Self {
        assert!(
            threshold <= self.session.max_background,
            "The congestion_threshold must be less or equal to max_background"
        );
        if threshold == 0 {
            threshold = self.session.max_background * 3 / 4;
            tracing::debug!(congestion_threshold = threshold);
        }
        self.session.congestion_threshold = threshold;
        self
    }

    /// Set the timestamp resolution supported by the filesystem.
    ///
    /// The setting value has the nanosecond unit and should be a power of 10.
    ///
    /// The default value is 1.
    pub fn time_gran(&mut self, time_gran: u32) -> &mut Self {
        self.session.time_gran = time_gran;
        self
    }

    /// Start a FUSE daemon mount on the specified path.
    pub async fn mount(
        &self,
        mountpoint: impl AsRef<Path>,
        mountopts: &[&OsStr],
    ) -> io::Result<Daemon> {
        // FIXME: make Connection::open async.
        let conn = Connection::open(mountpoint.as_ref(), mountopts)?;
        let conn = Async::new(conn)?;

        let session = self.session.init(&conn, conn.get_ref()).await?;

        Ok(Daemon {
            session: Arc::new(session),
            conn,
        })
    }
}

/// The instance of FUSE daemon for interaction with the kernel driver.
pub struct Daemon {
    conn: Async<Connection>,
    session: Arc<Session>,
}

impl Daemon {
    /// Start a FUSE daemon mount on the specified path.
    pub async fn mount(mountpoint: impl AsRef<Path>, mountopts: &[&OsStr]) -> io::Result<Self> {
        Builder::default().mount(mountpoint, mountopts).await
    }

    /// Receive an incoming FUSE request from the kernel.
    pub async fn next_request(&mut self) -> io::Result<Option<Request>> {
        let mut buf = vec![0u8; self.session.buffer_size()];

        loop {
            match self.conn.read(&mut buf[..]).await {
                Ok(len) => {
                    unsafe {
                        buf.set_len(len);
                    }
                    break;
                }

                Err(err) => match err.raw_os_error() {
                    Some(libc::ENODEV) => {
                        tracing::debug!("ENODEV");
                        return Ok(None);
                    }
                    Some(libc::ENOENT) => {
                        tracing::debug!("ENOENT");
                        continue;
                    }
                    _ => return Err(err),
                },
            }
        }

        Ok(Some(Request {
            buf,
            session: self.session.clone(),
            writer: self.conn.get_ref().writer(),
        }))
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.session.exit();
    }
}
