//! Filesystem abstraction.

use crate::{common::Forget, op};
use futures::future::Future;
use std::pin::Pin;

macro_rules! dispatch_ops {
    ($mac:ident) => {
        $mac! {
            lookup => Lookup,
            getattr => Getattr,
            setattr => Setattr,
            readlink => Readlink,
            symlink => Symlink,
            mknod => Mknod,
            mkdir => Mkdir,
            unlink => Unlink,
            rmdir => Rmdir,
            rename => Rename,
            link => Link,
            open => Open,
            read => Read,
            write => Write,
            release => Release,
            statfs => Statfs,
            fsync => Fsync,
            setxattr => Setxattr,
            getxattr => Getxattr,
            listxattr => Listxattr,
            removexattr => Removexattr,
            flush => Flush,
            opendir => Opendir,
            readdir => Readdir,
            releasedir => Releasedir,
            fsyncdir => Fsyncdir,
            getlk => Getlk,
            setlk => Setlk,
            flock => Flock,
            access => Access,
            create => Create,
            bmap => Bmap,
            fallocate => Fallocate,
            copy_file_range => CopyFileRange,
            poll => Poll,
            interrupt => Interrupt,
            notify_reply => NotifyReply,
        }
    };
}

macro_rules! define_filesystem_op {
    ($( $name:ident => $Op:ident, )*) => {$(
        #[must_use]
        fn $name<'a, 'async_trait, Op>(
            &'a self,
            op: Op,
        ) -> Pin<Box<dyn Future<Output = Result<Op::Ok, Op::Error>> + Send + 'async_trait>>
        where
            'a: 'async_trait,
            Op: op::$Op + Send + 'async_trait;
    )*};
}

/// A set of callbacks for FUSE filesystem implementation.
#[allow(missing_docs)]
pub trait Filesystem: Sync {
    dispatch_ops!(define_filesystem_op);

    #[must_use]
    fn forget<'a, 'async_trait, I>(
        &'a self,
        forgets: I,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'async_trait>>
    where
        I: IntoIterator + Send + 'async_trait,
        I::Item: AsRef<Forget> + Send + 'async_trait,
        I::IntoIter: Send + 'async_trait;
}

macro_rules! impl_filesystem_op {
    ($( $name:ident => $Op:ident, )*) => {$(
        #[inline]
        fn $name<'a, 'async_trait, Op>(
            &'a self,
            op: Op,
        ) -> Pin<Box<dyn Future<Output = Result<Op::Ok, Op::Error>> + Send + 'async_trait>>
        where
            'a: 'async_trait,
            Op: op::$Op + Send + 'async_trait,
        {
            (**self).$name(op)
        }
    )*};
}

macro_rules! impl_filesystem_body {
    () => {
        dispatch_ops!(impl_filesystem_op);

        #[inline]
        fn forget<'a, 'async_trait, I>(
            &'a self,
            forgets: I,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'async_trait>>
        where
            I: IntoIterator + Send + 'async_trait,
            I::Item: AsRef<Forget> + Send + 'async_trait,
            I::IntoIter: Send + 'async_trait,
        {
            (**self).forget(forgets)
        }

        // TODO: interrupt, notify_reply
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
