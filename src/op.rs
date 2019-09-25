use crate::{
    reply::Attr,
    request::{
        CapFlags, //
        OpFlush,
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
use std::borrow::Cow;

pub type OperationResult<T> = Result<T, c_int>;

#[async_trait(?Send)]
#[allow(unused_variables)]
pub trait Operations {
    #[allow(unused_variables)]
    async fn init<'a>(
        &'a mut self,
        req: &'a Request<'a>,
        op: &'a OpInit,
    ) -> OperationResult<CapFlags> {
        Ok(CapFlags::all())
    }

    async fn destroy<'a>(&'a mut self) {}

    async fn getattr<'a>(
        &'a mut self,
        req: &'a Request<'a>,
        op: &'a OpGetattr,
    ) -> OperationResult<(Attr, u64, u32)> {
        Err(libc::ENOSYS)
    }

    async fn open<'a>(
        &'a mut self,
        req: &'a Request<'a>,
        op: &'a OpOpen,
    ) -> OperationResult<(u64, u32)> {
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
