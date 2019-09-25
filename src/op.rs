use crate::{
    reply::{AttrOut, EntryOut, InitOut, OpenOut},
    request::{
        OpFlush, //
        OpGetattr,
        OpInit,
        OpOpen,
        OpRead,
        OpRelease,
        Request,
    },
};
use async_trait::async_trait;
use libc::c_int;
use std::{borrow::Cow, ffi::OsStr};

pub type OperationResult<T> = Result<T, c_int>;

#[async_trait(?Send)]
#[allow(unused_variables)]
pub trait Operations {
    #[allow(unused_variables)]
    async fn init<'a>(
        &'a mut self,
        req: &'a Request<'a>,
        op: &'a OpInit,
        out: &'a mut InitOut,
    ) -> OperationResult<()> {
        Ok(())
    }

    async fn destroy<'a>(&'a mut self) {}

    async fn lookup<'a>(
        &'a mut self,
        req: &'a Request<'a>,
        name: &'a OsStr,
    ) -> OperationResult<EntryOut> {
        Err(libc::ENOSYS)
    }

    async fn forget<'a>(&'a mut self, req: &'a Request<'a>, nlookup: u64) {}

    async fn getattr<'a>(
        &'a mut self,
        req: &'a Request<'a>,
        op: &'a OpGetattr,
    ) -> OperationResult<AttrOut> {
        Err(libc::ENOSYS)
    }

    async fn open<'a>(
        &'a mut self,
        req: &'a Request<'a>,
        op: &'a OpOpen,
    ) -> OperationResult<OpenOut> {
        Err(libc::ENOSYS)
    }

    async fn read<'a>(
        &'a mut self,
        req: &'a Request<'a>,
        op: &'a OpRead,
    ) -> OperationResult<Cow<'a, [u8]>> {
        Err(libc::ENOSYS)
    }

    async fn flush<'a>(&'a mut self, req: &'a Request<'a>, op: &'a OpFlush) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }

    async fn release<'a>(
        &'a mut self,
        req: &'a Request<'a>,
        op: &'a OpRelease,
    ) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }
}
