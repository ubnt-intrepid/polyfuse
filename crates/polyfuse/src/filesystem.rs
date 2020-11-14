//! Filesystem abstraction.

use crate::{op, reply};
use futures::future::{BoxFuture, LocalBoxFuture};

/// Information about FUSE request.
pub trait Request {
    /// Return the unique ID of the request.
    fn unique(&self) -> u64;

    /// Return the user ID of the calling process.
    fn uid(&self) -> u32;

    /// Return the group ID of the calling process.
    fn gid(&self) -> u32;

    /// Return the process ID of the calling process.
    fn pid(&self) -> u32;
}

macro_rules! dispatch_ops {
    ($mac:ident) => {
        $mac! {
            lookup => (Lookup, ReplyEntry),
            getattr => (Getattr, ReplyAttr),
            setattr => (Setattr, ReplyAttr),
            readlink => (Readlink, ReplyBytes),
            symlink => (Symlink, ReplyEntry),
            mknod => (Mknod, ReplyEntry),
            mkdir => (Mkdir, ReplyEntry),
            unlink => (Unlink, ReplyOk),
            rmdir => (Rmdir, ReplyOk),
            rename => (Rename, ReplyOk),
            link => (Link, ReplyEntry),
            open => (Open, ReplyOpen),
            read => (Read, ReplyBytes),
            write => (Write, ReplyWrite),
            release => (Release, ReplyOk),
            statfs => (Statfs, ReplyStatfs),
            fsync => (Fsync, ReplyOk),
            setxattr => (Setxattr, ReplyOk),
            getxattr => (Getxattr, ReplyXattr),
            listxattr => (Listxattr, ReplyXattr),
            removexattr => (Removexattr, ReplyOk),
            flush => (Flush, ReplyOk),
            opendir => (Opendir, ReplyOpen),
            readdir => (Readdir, ReplyBytes),
            releasedir => (Releasedir, ReplyOk),
            fsyncdir => (Fsyncdir, ReplyOk),
            getlk => (Getlk, ReplyLk),
            setlk => (Setlk, ReplyOk),
            flock => (Flock, ReplyOk),
            access => (Access, ReplyOk),
            create => (Create, ReplyCreate),
            bmap => (Bmap, ReplyBmap),
            fallocate => (Fallocate, ReplyOk),
            copy_file_range => (CopyFileRange, ReplyWrite),
            poll => (Poll, ReplyPoll),
        }
    };
}

macro_rules! define_filesystem_op {
    ($( $name:ident => ($Op:ident, $Reply:ident), )*) => {$(
        paste::paste! {
            #[doc = "See [the documentation of `" $Op "`](op::" $Op ") for details."]
            #[allow(unused_variables)]
            #[must_use]
            fn $name<'a, R>(
                &'a self,
                request: impl Request + Send + Sync + 'a,
                op: impl op::$Op + Send + Sync + 'a,
                reply: R,
            ) -> BoxFuture<'a, Result<R::Ok, R::Error>>
            where
                R: reply::$Reply + Send + 'a,
            {
                Box::pin(async move {
                    reply.unimplemented()
                })
            }
        }
    )*};
}

/// The FUSE filesystem.
///
/// Each operation callback is asynchronous, and the returned future
/// must be converted to [`BoxFuture`](futures::future::BoxFuture) due
/// to the restriction of current Rust trait system.
pub trait Filesystem: Sync {
    dispatch_ops!(define_filesystem_op);

    /// Forget about inodes removed from the kenrel's internal caches.
    #[allow(unused_variables)]
    #[must_use]
    fn forget<'a, I>(
        &'a self,
        request: impl Request + Send + Sync + 'a,
        forgets: I,
    ) -> BoxFuture<'a, ()>
    where
        I: IntoIterator + Send + 'a,
        I::IntoIter: Send + 'a,
        I::Item: op::Forget + Send + Sync + 'a,
    {
        Box::pin(async {})
    }
}

macro_rules! impl_filesystem_op {
    ($( $name:ident => ($Op:ident, $Reply:ident), )*) => {$(
        #[inline]
        fn $name<'a, R>(
            &'a self,
            request: impl Request + Send + Sync + 'a,
            op: impl op::$Op + Send + Sync + 'a,
            reply: R,
        ) -> BoxFuture<'a, Result<R::Ok, R::Error>>
        where
            R: reply::$Reply + Send + 'a,
        {
            (**self).$name(request, op, reply)
        }
    )*};
}

macro_rules! impl_filesystem_body {
    () => {
        dispatch_ops!(impl_filesystem_op);

        #[inline]
        fn forget<'a, I>(
            &'a self,
            request: impl Request + Send + Sync + 'a,
            forgets: I,
        ) -> BoxFuture<'a, ()>
        where
            I: IntoIterator + Send + 'a,
            I::IntoIter: Send + 'a,
            I::Item: op::Forget + Send + Sync + 'a,
        {
            (**self).forget(request, forgets)
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

macro_rules! define_local_filesystem_op {
    ($( $name:ident => ($Op:ident, $Reply:ident), )*) => {$(
        paste::paste! {
            #[doc = "See [the documentation of `" $Op "`](op::" $Op ") for details."]
            #[allow(unused_variables)]
            #[must_use]
            fn $name<'a, R>(
                &'a self,
                request: impl Request + 'a,
                op: impl op::$Op + 'a,
                reply: R,
            ) -> LocalBoxFuture<'a, Result<R::Ok, R::Error>>
            where
                R: reply::$Reply + 'a,
            {
                Box::pin(async move {
                    reply.unimplemented()
                })
            }
        }
    )*};
}

/// The FUSE filesystem that does not satisfy thread safety.
pub trait LocalFilesystem {
    dispatch_ops!(define_local_filesystem_op);

    /// Forget about inodes removed from the kenrel's internal caches.
    #[allow(unused_variables)]
    #[must_use]
    fn forget<'a, I>(&'a self, request: impl Request + 'a, forgets: I) -> LocalBoxFuture<'a, ()>
    where
        I: IntoIterator + 'a,
        I::IntoIter: 'a,
        I::Item: op::Forget + 'a,
    {
        Box::pin(async {})
    }
}

macro_rules! impl_local_filesystem_op {
    ($( $name:ident => ($Op:ident, $Reply:ident), )*) => {$(
        #[inline]
        fn $name<'a, R>(
            &'a self,
            request: impl Request + 'a,
            op: impl op::$Op + 'a,
            reply: R,
        ) -> LocalBoxFuture<'a, Result<R::Ok, R::Error>>
        where
            R: reply::$Reply + 'a,
        {
            (**self).$name(request, op, reply)
        }
    )*};
}

macro_rules! impl_local_filesystem_body {
    () => {
        dispatch_ops!(impl_local_filesystem_op);

        #[inline]
        fn forget<'a, I>(&'a self, request: impl Request + 'a, forgets: I) -> LocalBoxFuture<'a, ()>
        where
            I: IntoIterator + 'a,
            I::IntoIter: 'a,
            I::Item: op::Forget + 'a,
        {
            (**self).forget(request, forgets)
        }
    };
}

impl<F: ?Sized> LocalFilesystem for &F
where
    F: LocalFilesystem,
{
    impl_local_filesystem_body!();
}

impl<F: ?Sized> LocalFilesystem for Box<F>
where
    F: LocalFilesystem,
{
    impl_local_filesystem_body!();
}

impl<F: ?Sized> LocalFilesystem for std::rc::Rc<F>
where
    F: LocalFilesystem,
{
    impl_local_filesystem_body!();
}

impl<F: ?Sized> LocalFilesystem for std::sync::Arc<F>
where
    F: LocalFilesystem,
{
    impl_local_filesystem_body!();
}
