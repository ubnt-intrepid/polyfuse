//! Filesystem abstraction.

use crate::{
    async_trait, //
    context::Context,
    op::Operation,
};
use futures::io::{AsyncRead, AsyncWrite};
use std::{future::Future, io, pin::Pin};

/// A set of callbacks for FUSE filesystem implementation.
#[async_trait]
pub trait Filesystem: Sync {
    /// Reply to a FUSE request.
    ///
    /// If there is no reply in the callback, the default reply (typically
    /// an `ENOSYS` error code) is automatically sent to the kernel.
    #[allow(unused_variables)]
    async fn call<'a, 'cx, T: ?Sized>(
        &'a self,
        cx: &'a mut Context<'cx, T>,
        op: Operation<'cx>,
    ) -> io::Result<()>
    where
        T: AsyncRead + AsyncWrite + Send + Unpin,
    {
        Ok(())
    }
}

macro_rules! impl_filesystem_body {
    () => {
        #[inline]
        fn call<'a, 'cx, 'async_trait, T: ?Sized>(
            &'a self,
            cx: &'a mut Context<'cx, T>,
            op: Operation<'cx>,
        ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
        where
            'a: 'async_trait,
            'cx: 'async_trait,
            T: AsyncRead + AsyncWrite + Send + Unpin + 'async_trait,
        {
            (**self).call(cx, op)
        }
    };
}

impl<F: ?Sized> Filesystem for &F
where
    F: Filesystem,
{
    impl_filesystem_body!();
}

impl<F: ?Sized> Filesystem for Box<F>
where
    F: Filesystem,
{
    impl_filesystem_body!();
}

impl<F: ?Sized> Filesystem for std::sync::Arc<F>
where
    F: Filesystem + Send,
{
    impl_filesystem_body!();
}
