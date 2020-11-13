//! Filesystem abstraction.

use crate::op;
use futures::future::BoxFuture;

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
        }
    };
}

macro_rules! define_filesystem_op {
    ($( $name:ident => $Op:ident, )*) => {$(
        paste::paste! {
            #[doc = "See [the documentation of `" $Op "`](op::" $Op ") for details."]
            #[must_use]
            fn $name<'op, Op>(
                &'op self,
                op: Op,
            ) -> BoxFuture<'op, Result<Op::Ok, Op::Error>>
            where
                Op: op::$Op + Send + 'op;
        }
    )*};
}

/// The FUSE filesystem.
pub trait Filesystem: Sync {
    dispatch_ops!(define_filesystem_op);

    /// Forget about inodes removed from the kenrel's internal caches.
    #[allow(unused_variables)]
    #[must_use]
    fn forget<'op, I>(&'op self, forgets: I) -> BoxFuture<'op, ()>
    where
        I: IntoIterator + Send + 'op,
        I::IntoIter: Send + 'op,
        I::Item: op::Forget + Send + 'op;
}

macro_rules! impl_filesystem_op {
    ($( $name:ident => $Op:ident, )*) => {$(
        #[inline]
        fn $name<'op, Op>(&'op self, op: Op) -> BoxFuture<'op, Result<Op::Ok, Op::Error>>
        where
            Op: op::$Op + Send + 'op,
        {
            (**self).$name(op)
        }
    )*};
}

macro_rules! impl_filesystem_body {
    () => {
        dispatch_ops!(impl_filesystem_op);

        #[inline]
        fn forget<'op, I>(&'op self, forgets: I) -> BoxFuture<'op, ()>
        where
            I: IntoIterator + Send + 'op,
            I::IntoIter: Send + 'op,
            I::Item: op::Forget + Send + 'op,
        {
            (**self).forget(forgets)
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
