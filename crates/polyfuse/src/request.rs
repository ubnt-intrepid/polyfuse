use crate::{
    conn::Writer,
    op::{self, Operation},
    parse::{self, Arg},
    reply::{
        self, ReplyAttr, ReplyBmap, ReplyCreate, ReplyData, ReplyEntry, ReplyLk, ReplyOk,
        ReplyOpen, ReplyPoll, ReplyStatfs, ReplyWrite, ReplyXattr,
    },
    session::Session,
    write,
};
use either::Either;
use futures::future::Future;
use polyfuse_kernel as kernel;
use std::sync::Arc;

/// Context about an incoming FUSE request.
pub struct Request {
    pub(crate) buf: Vec<u8>,
    pub(crate) session: Arc<Session>,
    pub(crate) writer: Writer,
}

impl Request {
    // TODO: add unique(), uid(), gid() and pid()

    /// Process the request.
    pub async fn process<'req, F, Fut>(&'req self, f: F) -> anyhow::Result<()>
    where
        F: FnOnce(Operation<'req>) -> Fut,
        Fut: Future<Output = reply::Result>,
    {
        if self.session.exited() {
            return Ok(());
        }

        let parse::Request { header, arg, .. } = parse::Request::parse(&self.buf[..])?;

        let reply_entry = || ReplyEntry {
            writer: &self.writer,
            header,
            arg: kernel::fuse_entry_out::default(),
        };

        let reply_attr = || ReplyAttr {
            writer: &self.writer,
            header,
            arg: kernel::fuse_attr_out::default(),
        };

        let reply_ok = || ReplyOk {
            writer: &self.writer,
            header,
        };

        let reply_data = || ReplyData {
            writer: &self.writer,
            header,
        };

        let reply_open = || ReplyOpen {
            writer: &self.writer,
            header,
            arg: kernel::fuse_open_out::default(),
        };

        let reply_write = || ReplyWrite {
            writer: &self.writer,
            header,
            arg: Default::default(),
        };

        let reply_statfs = || ReplyStatfs {
            writer: &self.writer,
            header,
            arg: Default::default(),
        };

        let reply_xattr = || ReplyXattr {
            writer: &self.writer,
            header,
            arg: Default::default(),
        };

        let reply_create = || ReplyCreate {
            writer: &self.writer,
            header,
            arg: Default::default(),
        };

        let reply_lk = || ReplyLk {
            writer: &self.writer,
            header,
            arg: Default::default(),
        };

        let reply_bmap = || ReplyBmap {
            writer: &self.writer,
            header,
            arg: Default::default(),
        };

        let reply_poll = || ReplyPoll {
            writer: &self.writer,
            header,
            arg: Default::default(),
        };

        macro_rules! dispatch_op {
            ($Op:ident, $arg:expr, $reply:expr) => {
                f(Operation::$Op {
                    op: op::Op { header, arg: $arg },
                    reply: $reply,
                })
                .await
            };
        }

        let res = match arg {
            Arg::Lookup(arg) => dispatch_op!(Lookup, arg, reply_entry()),
            Arg::Getattr(arg) => dispatch_op!(Getattr, arg, reply_attr()),
            Arg::Setattr(arg) => dispatch_op!(Setattr, arg, reply_attr()),
            Arg::Readlink(arg) => dispatch_op!(Readlink, arg, reply_data()),
            Arg::Symlink(arg) => dispatch_op!(Symlink, arg, reply_entry()),
            Arg::Mknod(arg) => dispatch_op!(Mknod, arg, reply_entry()),
            Arg::Mkdir(arg) => dispatch_op!(Mkdir, arg, reply_entry()),
            Arg::Unlink(arg) => dispatch_op!(Unlink, arg, reply_ok()),
            Arg::Rmdir(arg) => dispatch_op!(Rmdir, arg, reply_ok()),
            Arg::Rename(arg) => dispatch_op!(Rename, Either::Left(arg), reply_ok()),
            Arg::Rename2(arg) => dispatch_op!(Rename, Either::Right(arg), reply_ok()),
            Arg::Link(arg) => dispatch_op!(Link, arg, reply_entry()),
            Arg::Open(arg) => dispatch_op!(Open, arg, reply_open()),
            Arg::Read(arg) => dispatch_op!(Read, arg, reply_data()),
            Arg::Write(arg) => dispatch_op!(Write, arg, reply_write()),
            Arg::Release(arg) => dispatch_op!(Release, arg, reply_ok()),
            Arg::Statfs(arg) => dispatch_op!(Statfs, arg, reply_statfs()),
            Arg::Fsync(arg) => dispatch_op!(Fsync, arg, reply_ok()),
            Arg::Setxattr(arg) => dispatch_op!(Setxattr, arg, reply_ok()),
            Arg::Getxattr(arg) => dispatch_op!(Getxattr, arg, reply_xattr()),
            Arg::Listxattr(arg) => dispatch_op!(Listxattr, arg, reply_xattr()),
            Arg::Removexattr(arg) => dispatch_op!(Removexattr, arg, reply_ok()),
            Arg::Flush(arg) => dispatch_op!(Flush, arg, reply_ok()),
            Arg::Opendir(arg) => dispatch_op!(Opendir, arg, reply_open()),
            Arg::Readdir(arg) => dispatch_op!(Readdir, arg, reply_data()),
            Arg::Readdirplus(arg) => dispatch_op!(Readdirplus, arg, reply_data()),
            Arg::Releasedir(arg) => dispatch_op!(Releasedir, arg, reply_ok()),
            Arg::Fsyncdir(arg) => dispatch_op!(Fsyncdir, arg, reply_ok()),
            Arg::Getlk(arg) => dispatch_op!(Getlk, arg, reply_lk()),
            Arg::Setlk(arg) => dispatch_op!(Setlk, arg, reply_ok()),
            Arg::Flock(arg) => dispatch_op!(Flock, arg, reply_ok()),
            Arg::Access(arg) => dispatch_op!(Access, arg, reply_ok()),
            Arg::Create(arg) => dispatch_op!(Create, arg, reply_create()),
            Arg::Bmap(arg) => dispatch_op!(Bmap, arg, reply_bmap()),
            Arg::Fallocate(arg) => dispatch_op!(Fallocate, arg, reply_ok()),
            Arg::CopyFileRange(arg) => dispatch_op!(CopyFileRange, arg, reply_write()),
            Arg::Poll(arg) => dispatch_op!(Poll, arg, reply_poll()),
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
