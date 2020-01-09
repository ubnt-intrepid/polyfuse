use crate::{
    io::{Reader, Writer, WriterExt},
    kernel::fuse_in_header,
    reply::Reply,
};
use std::{fmt, io};

/// The context of FUSE callbacks.
pub struct Context<'cx, T: ?Sized> {
    header: &'cx fuse_in_header,
    io: &'cx mut T,
    written: bool,
}

impl<T: ?Sized> fmt::Debug for Context<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context")
            .field("unique", &self.unique())
            .field("uid", &self.uid())
            .field("gid", &self.gid())
            .field("pid", &self.pid())
            .field("replied", &self.replied())
            .finish()
    }
}

impl<'cx, T: ?Sized> Context<'cx, T> {
    pub(crate) fn new(header: &'cx fuse_in_header, io: &'cx mut T) -> Self {
        Self {
            header,
            io,
            written: false,
        }
    }

    /// Return the unique ID of the request.
    #[inline]
    pub fn unique(&self) -> u64 {
        self.header.unique
    }

    /// Return the user ID of the calling process.
    #[inline]
    pub fn uid(&self) -> u32 {
        self.header.uid
    }

    /// Return the group ID of the calling process.
    #[inline]
    pub fn gid(&self) -> u32 {
        self.header.gid
    }

    /// Return the process ID of the calling process.
    #[inline]
    pub fn pid(&self) -> u32 {
        self.header.pid
    }

    /// Return the instance of `Reader` for reading the rest of the request message.
    #[inline]
    pub fn reader(&mut self) -> impl Reader + '_
    where
        T: Reader + Unpin,
    {
        &mut *self.io
    }

    #[doc(hidden)]
    #[deprecated(since = "0.3.3", note = "use `reply` instead.")]
    #[inline]
    pub async fn reply_raw<'a>(&'a mut self, data: &'a [&'a [u8]]) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        self.send_reply(0, data).await
    }

    /// Reply to the kernel with an arbitrary bytes of data.
    #[inline]
    pub async fn reply<R>(&mut self, reply: R) -> io::Result<()>
    where
        T: Writer + Unpin,
        R: Reply,
    {
        self.send_reply(0, &reply).await
    }

    /// Reply to the kernel with an error code.
    #[inline]
    pub async fn reply_err(&mut self, error: i32) -> io::Result<()>
    where
        T: Writer + Unpin,
    {
        self.send_reply(error, &[]).await
    }

    /// Return whether the writer has already sent a reply to the kernel or not.
    #[inline]
    pub fn replied(&self) -> bool {
        self.written
    }

    async fn send_reply<R: ?Sized>(&mut self, error: i32, data: &R) -> io::Result<()>
    where
        T: Writer + Unpin,
        R: Reply,
    {
        if !self.written {
            self.written = true;
            self.io.send_msg(self.header.unique, -error, data).await?;
        }
        Ok(())
    }

    #[inline]
    pub(crate) fn disable_writer(&mut self) {
        self.written = true;
    }
}
