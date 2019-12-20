//! Filesystem abstraction.

use crate::{
    async_trait,
    common::Forget,
    io::{InHeader, Writer},
    op::Operation,
    reply::ReplyWriter,
};
use std::{fmt, future::Future, io, pin::Pin};

/// Contextural information about an incoming request.
pub struct Context<'a> {
    header: &'a InHeader,
}

impl fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Context").finish()
    }
}

impl<'a> Context<'a> {
    pub(crate) fn new(header: &'a InHeader) -> Self {
        Self { header }
    }

    /// Return the user ID of the calling process.
    pub fn uid(&self) -> u32 {
        self.header.uid()
    }

    /// Return the group ID of the calling process.
    pub fn gid(&self) -> u32 {
        self.header.gid()
    }

    /// Return the process ID of the calling process.
    pub fn pid(&self) -> u32 {
        self.header.pid()
    }
}

/// The filesystem running on the user space.
#[async_trait]
pub trait Filesystem<T>: Sync {
    /// Reply to a FUSE request.
    ///
    /// This callback is invoked when a request is received from the kernel
    /// and ready to be processed and its reply to the kernel is performed
    /// via `writer`.
    ///
    /// If there is no reply in the callback, the default reply (typically
    /// an `ENOSYS` error code) is automatically sent to the kernel.
    #[allow(unused_variables)]
    async fn reply<'a, 'cx, 'w, W: ?Sized>(
        &'a self,
        cx: &'a mut Context<'cx>,
        op: Operation<'cx, T>,
        writer: &'a mut ReplyWriter<'w, W>,
    ) -> io::Result<()>
    where
        T: Send + 'async_trait,
        W: Writer + Send + Unpin,
    {
        Ok(())
    }

    /// Forget about inodes removed from the kernel's internal caches.
    #[allow(unused_variables)]
    async fn forget<'a, 'cx>(
        &'a self,
        cx: &'a mut Context<'cx>,
        forgets: &'a [Forget],
    ) -> io::Result<()>
    where
        T: 'async_trait,
    {
        Ok(())
    }
}

macro_rules! impl_filesystem_body {
    () => {
        #[inline]
        fn reply<'a, 'cx, 'w, 'async_trait, W: ?Sized>(
            &'a self,
            cx: &'a mut Context<'cx>,
            op: Operation<'cx, T>,
            writer: &'a mut ReplyWriter<'w, W>,
        ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
        where
            'a: 'async_trait,
            'cx: 'async_trait,
            'w: 'async_trait,
            T: Send + 'async_trait,
            W: Writer + Send + Unpin + 'async_trait,
        {
            (**self).reply(cx, op, writer)
        }

        #[inline]
        fn forget<'a, 'cx, 'async_trait>(
            &'a self,
            cx: &'a mut Context<'cx>,
            forgets: &'a [Forget],
        ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
        where
            'a: 'async_trait,
            'cx: 'async_trait,
            T: 'async_trait,
        {
            (**self).forget(cx, forgets)
        }
    };
}

impl<F: ?Sized, T> Filesystem<T> for &F
where
    F: Filesystem<T>,
{
    impl_filesystem_body!();
}

impl<F: ?Sized, T> Filesystem<T> for Box<F>
where
    F: Filesystem<T>,
{
    impl_filesystem_body!();
}

impl<F: ?Sized, T> Filesystem<T> for std::sync::Arc<F>
where
    F: Filesystem<T> + Send,
{
    impl_filesystem_body!();
}
