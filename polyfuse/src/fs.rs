//! Filesystem abstraction.

use crate::{
    op::Operation,
    reply::send_msg,
    request::RequestHeader,
    session::{Interrupt, Session},
};
use async_trait::async_trait;
use futures::io::AsyncWrite;
use std::{fmt, future::Future, io, pin::Pin};

/// Contextural information about an incoming request.
pub struct Context<'a, W: ?Sized> {
    header: &'a RequestHeader,
    writer: Option<&'a mut W>,
    session: &'a Session,
}

impl<W: ?Sized> fmt::Debug for Context<'_, W> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Context").finish()
    }
}

impl<'a, W: ?Sized> Context<'a, W> {
    pub(crate) fn new(header: &'a RequestHeader, writer: &'a mut W, session: &'a Session) -> Self {
        Self {
            header,
            writer: Some(writer),
            session,
        }
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

    #[inline]
    pub(crate) async fn reply(&mut self, data: &[u8]) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        self.reply_vectored(&[data]).await
    }

    #[inline]
    pub(crate) async fn reply_vectored(&mut self, data: &[&[u8]]) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        if let Some(ref mut writer) = self.writer.take() {
            send_msg(writer, self.header.unique(), 0, data).await?;
        }
        Ok(())
    }

    /// Reply to the kernel with an error code.
    pub async fn reply_err(&mut self, error: i32) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        if let Some(ref mut writer) = self.writer.take() {
            send_msg(writer, self.header.unique(), -error, &[]).await?;
        }
        Ok(())
    }

    /// Register the request with the sesssion and get a signal
    /// that will be notified when the request is canceld by the kernel.
    pub async fn on_interrupt(&mut self) -> Interrupt {
        self.session.enable_interrupt(self.header.unique()).await
    }

    pub(crate) fn is_replied(&self) -> bool {
        self.writer.is_none()
    }
}

/// The filesystem running on the user space.
#[async_trait]
pub trait Filesystem<T>: Sync {
    /// Handle a FUSE request from the kernel and reply with its result.
    #[allow(unused_variables)]
    async fn call<W: ?Sized>(&self, cx: &mut Context<'_, W>, op: Operation<'_, T>) -> io::Result<()>
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
    fn call<'l1, 'l2, 'l3, 'l4, 'async_trait, W: ?Sized>(
        &'l1 self,
        cx: &'l2 mut Context<'l3, W>,
        op: Operation<'l4, T>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
    where
        'l1: 'async_trait,
        'l2: 'async_trait,
        'l3: 'async_trait,
        'l4: 'async_trait,
        T: Send + 'async_trait,
        W: AsyncWrite + Send + Unpin + 'async_trait,
    {
        (**self).call(cx, op)
    }
}

impl<F: ?Sized, T> Filesystem<T> for Box<F>
where
    F: Filesystem<T>,
{
    #[inline]
    fn call<'l1, 'l2, 'l3, 'l4, 'async_trait, W: ?Sized>(
        &'l1 self,
        cx: &'l2 mut Context<'l3, W>,
        op: Operation<'l4, T>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
    where
        'l1: 'async_trait,
        'l2: 'async_trait,
        'l3: 'async_trait,
        'l4: 'async_trait,
        T: Send + 'async_trait,
        W: AsyncWrite + Send + Unpin + 'async_trait,
    {
        (**self).call(cx, op)
    }
}

impl<F: ?Sized, T> Filesystem<T> for std::sync::Arc<F>
where
    F: Filesystem<T> + Send,
{
    #[inline]
    fn call<'l1, 'l2, 'l3, 'l4, 'async_trait, W: ?Sized>(
        &'l1 self,
        cx: &'l2 mut Context<'l3, W>,
        op: Operation<'l4, T>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
    where
        'l1: 'async_trait,
        'l2: 'async_trait,
        'l3: 'async_trait,
        'l4: 'async_trait,
        T: Send + 'async_trait,
        W: AsyncWrite + Send + Unpin + 'async_trait,
    {
        (**self).call(cx, op)
    }
}
