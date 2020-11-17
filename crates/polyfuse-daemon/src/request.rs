use crate::{
    conn::Writer,
    op::{self, Operation},
    parse::{self, Arg},
    session::Session,
    write,
};
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
