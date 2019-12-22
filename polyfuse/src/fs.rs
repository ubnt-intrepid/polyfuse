//! Filesystem abstraction.

use crate::{
    async_trait, //
    common::Forget,
    io::{Reader, Writer},
    op::{Interrupt, NotifyReply, Operation},
    reply::ReplyWriter,
};
use std::{future::Future, io, pin::Pin};

/// The filesystem running on the user space.
#[async_trait]
pub trait Filesystem: Sync {
    /// Reply to a FUSE request.
    ///
    /// This callback is invoked when a request is received from the kernel
    /// and ready to be processed and its reply to the kernel is performed
    /// via `writer`.
    ///
    /// If there is no reply in the callback, the default reply (typically
    /// an `ENOSYS` error code) is automatically sent to the kernel.
    #[allow(unused_variables)]
    async fn reply<'a, 'cx, 'w, R: ?Sized, W: ?Sized>(
        &'a self,
        op: Operation<'cx>,
        reader: &'a mut R,
        writer: &'a mut ReplyWriter<'w, W>,
    ) -> io::Result<()>
    where
        R: Reader + Send + Unpin,
        W: Writer + Send + Unpin,
    {
        Ok(())
    }

    /// Forget about inodes removed from the kernel's internal caches.
    #[allow(unused_variables)]
    async fn forget<'a>(&'a self, forgets: &'a [Forget]) -> io::Result<()> {
        Ok(())
    }

    /// Receive a reply for a `NOTIFY_RETRIEVE` notification.
    #[allow(unused_variables)]
    async fn notify_reply<'a, 'cx, R: ?Sized>(
        &'a self,
        arg: NotifyReply<'cx>,
        reader: &'a mut R,
    ) -> io::Result<()>
    where
        R: Reader + Send + Unpin,
    {
        Ok(())
    }

    /// Handle an INTERRUPT request.
    #[allow(unused_variables)]
    async fn interrupt<'a, 'cx, 'w, W: ?Sized>(
        &'a self,
        op: Interrupt<'cx>,
        writer: &'a mut ReplyWriter<'w, W>,
    ) -> io::Result<()>
    where
        W: Writer + Send + Unpin,
    {
        Ok(())
    }
}

macro_rules! impl_filesystem_body {
    () => {
        #[inline]
        fn reply<'a, 'cx, 'w, 'async_trait, R:?Sized, W: ?Sized>(
            &'a self,
            op: Operation<'cx>,
            reader: &'a mut R,
            writer: &'a mut ReplyWriter<'w, W>,
        ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
        where
            'a: 'async_trait,
            'cx: 'async_trait,
            'w: 'async_trait,
            R: Reader + Send + Unpin + 'async_trait,
            W: Writer + Send + Unpin + 'async_trait,
        {
            (**self).reply(op, reader, writer)
        }

        #[inline]
        fn forget<'a, 'async_trait>(
            &'a self,
            forgets: &'a [Forget],
        ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
        where
            'a: 'async_trait,
        {
            (**self).forget(forgets)
        }

        #[inline]
        fn notify_reply<'a, 'cx, 'async_trait, R:?Sized>(
            &'a self,
            arg: NotifyReply<'cx>,
            reader: &'a mut R,
        ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
        where
            'a: 'async_trait,
            'cx: 'async_trait,
            R: Reader + Send + Unpin + 'async_trait,
        {
            (**self).notify_reply(arg, reader)
        }

        #[inline]
        fn interrupt<'a, 'cx, 'w, 'async_trait, W: ?Sized>(
            &'a self,
            op: Interrupt<'cx>,
            writer: &'a mut ReplyWriter<'w, W>,
        ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'async_trait>>
        where
            'a: 'async_trait,
            'cx: 'async_trait,
            'w: 'async_trait,
            W: Writer + Send + Unpin + 'async_trait,
        {
            (**self).interrupt(op, writer)
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
