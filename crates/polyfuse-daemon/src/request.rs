use crate::{
    conn::Writer,
    op::{self, Operation},
    parse::{self, Arg},
    session::Session,
    write,
};
use either::Either;
use futures::future::Future;
use std::sync::Arc;

/// Context about an incoming FUSE request.
pub struct Request {
    pub(crate) buf: Vec<u8>,
    pub(crate) session: Arc<Session>,
    pub(crate) writer: Writer,
}

impl Request {
    /// Process the request.
    pub async fn process<'a, F, Fut>(&'a self, f: F) -> anyhow::Result<()>
    where
        F: FnOnce(Operation<'a>) -> Fut,
        Fut: Future<Output = op::Result>,
    {
        if self.session.exited() {
            return Ok(());
        }

        let parse::Request { header, arg, .. } = parse::Request::parse(&self.buf[..])?;

        macro_rules! dispatch_op {
            ($Op:ident, $arg:expr) => {
                f(Operation::$Op(op::Op {
                    header,
                    arg: $arg,
                    writer: &self.writer,
                }))
                .await
            };
        }

        let res = match arg {
            Arg::Lookup(arg) => dispatch_op!(Lookup, arg),
            Arg::Getattr(arg) => dispatch_op!(Getattr, arg),
            Arg::Setattr(arg) => dispatch_op!(Setattr, arg),
            Arg::Readlink(arg) => dispatch_op!(Readlink, arg),
            Arg::Symlink(arg) => dispatch_op!(Symlink, arg),
            Arg::Mknod(arg) => dispatch_op!(Mknod, arg),
            Arg::Mkdir(arg) => dispatch_op!(Mkdir, arg),
            Arg::Unlink(arg) => dispatch_op!(Unlink, arg),
            Arg::Rmdir(arg) => dispatch_op!(Rmdir, arg),
            Arg::Rename(arg) => dispatch_op!(Rename, Either::Left(arg)),
            Arg::Rename2(arg) => dispatch_op!(Rename, Either::Right(arg)),
            Arg::Link(arg) => dispatch_op!(Link, arg),
            Arg::Open(arg) => dispatch_op!(Open, arg),
            Arg::Read(arg) => dispatch_op!(Read, arg),
            Arg::Write(arg) => dispatch_op!(Write, arg),
            Arg::Release(arg) => dispatch_op!(Release, arg),
            Arg::Statfs(arg) => dispatch_op!(Statfs, arg),
            Arg::Fsync(arg) => dispatch_op!(Fsync, arg),
            Arg::Setxattr(arg) => dispatch_op!(Setxattr, arg),
            Arg::Getxattr(arg) => dispatch_op!(Getxattr, arg),
            Arg::Listxattr(arg) => dispatch_op!(Listxattr, arg),
            Arg::Removexattr(arg) => dispatch_op!(Removexattr, arg),
            Arg::Flush(arg) => dispatch_op!(Flush, arg),
            Arg::Opendir(arg) => dispatch_op!(Opendir, arg),
            Arg::Readdir(arg) => dispatch_op!(Readdir, arg),
            Arg::Readdirplus(arg) => dispatch_op!(Readdirplus, arg),
            Arg::Releasedir(arg) => dispatch_op!(Releasedir, arg),
            Arg::Fsyncdir(arg) => dispatch_op!(Fsyncdir, arg),
            Arg::Getlk(arg) => dispatch_op!(Getlk, arg),
            Arg::Setlk(arg) => dispatch_op!(Setlk, arg),
            Arg::Flock(arg) => dispatch_op!(Flock, arg),
            Arg::Access(arg) => dispatch_op!(Access, arg),
            Arg::Create(arg) => dispatch_op!(Create, arg),
            Arg::Bmap(arg) => dispatch_op!(Bmap, arg),
            Arg::Fallocate(arg) => dispatch_op!(Fallocate, arg),
            Arg::CopyFileRange(arg) => dispatch_op!(CopyFileRange, arg),
            Arg::Poll(arg) => dispatch_op!(Poll, arg),

            op => {
                tracing::warn!("unsupported operation: {:?}", op);
                write::send_error(&self.writer, header.unique, libc::ENOSYS)?;
                return Ok(());
            }
        };

        if let Err(err) = res {
            match err.code() {
                Some(code) => {
                    write::send_error(&self.writer, header.unique, code)?;
                }
                None => return Err(err.into()),
            }
        }

        Ok(())
    }
}
