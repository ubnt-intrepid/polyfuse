//! Filesystem abstraction.

use crate::{
    async_trait,
    op::Operation,
    reply::ReplyWriter,
    request::RequestHeader,
    session::{Interrupt, Session},
};
use futures::io::AsyncWrite;
use std::{fmt, future::Future, io, pin::Pin};

/// Contextural information about an incoming request.
pub struct Context<'a> {
    header: &'a RequestHeader,
    session: &'a Session,
}

impl fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Context").finish()
    }
}

impl<'a> Context<'a> {
    pub(crate) fn new(header: &'a RequestHeader, session: &'a Session) -> Self {
        Self { header, session }
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

    /// Register the request with the sesssion and get a signal
    /// that will be notified when the request is canceld by the kernel.
    pub async fn on_interrupt(&mut self) -> Interrupt {
        self.session.enable_interrupt(self.header.unique()).await
    }
}

/// The filesystem running on the user space.
#[async_trait]
pub trait Filesystem<T>: Sync {
    /// Handle a FUSE request from the kernel and reply with its result.
    #[allow(unused_variables)]
    async fn call<W: ?Sized>(
        &self,
        cx: &mut Context<'_>,
        op: Operation<'_, T>,
        writer: &mut ReplyWriter<'_, W>,
    ) -> io::Result<()>
    where
        T: Send + 'async_trait,
        W: AsyncWrite + Send + Unpin,
    {
        Ok(())
    }
}

impl<'a, F: ?Sized, T> Filesystem<T> for &'a F
where
    F: Filesystem<T>,
{
    #[inline]
    fn call<'l1, 'l2, 'l3, 'l4, 'l5, 'l6, 'async_trait, W: ?Sized>(
        &'l1 self,
        cx: &'l2 mut Context<'l3>,
        op: Operation<'l4, T>,
        writer: &'l5 mut ReplyWriter<'l6, W>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
    where
        'l1: 'async_trait,
        'l2: 'async_trait,
        'l3: 'async_trait,
        'l4: 'async_trait,
        'l5: 'async_trait,
        'l6: 'async_trait,
        T: Send + 'async_trait,
        W: AsyncWrite + Send + Unpin + 'async_trait,
    {
        (**self).call(cx, op, writer)
    }
}

impl<F: ?Sized, T> Filesystem<T> for Box<F>
where
    F: Filesystem<T>,
{
    #[inline]
    fn call<'l1, 'l2, 'l3, 'l4, 'l5, 'l6, 'async_trait, W: ?Sized>(
        &'l1 self,
        cx: &'l2 mut Context<'l3>,
        op: Operation<'l4, T>,
        writer: &'l5 mut ReplyWriter<'l6, W>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
    where
        'l1: 'async_trait,
        'l2: 'async_trait,
        'l3: 'async_trait,
        'l4: 'async_trait,
        'l5: 'async_trait,
        'l6: 'async_trait,
        T: Send + 'async_trait,
        W: AsyncWrite + Send + Unpin + 'async_trait,
    {
        (**self).call(cx, op, writer)
    }
}

impl<F: ?Sized, T> Filesystem<T> for std::sync::Arc<F>
where
    F: Filesystem<T> + Send,
{
    #[inline]
    fn call<'l1, 'l2, 'l3, 'l4, 'l5, 'l6, 'async_trait, W: ?Sized>(
        &'l1 self,
        cx: &'l2 mut Context<'l3>,
        op: Operation<'l4, T>,
        writer: &'l5 mut ReplyWriter<'l6, W>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
    where
        'l1: 'async_trait,
        'l2: 'async_trait,
        'l3: 'async_trait,
        'l4: 'async_trait,
        'l5: 'async_trait,
        'l6: 'async_trait,
        T: Send + 'async_trait,
        W: AsyncWrite + Send + Unpin + 'async_trait,
    {
        (**self).call(cx, op, writer)
    }
}
