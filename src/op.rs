use crate::{
    reply::{AttrOut, EntryOut, InitOut, OpenOut, XattrOut},
    request::{
        Header, //
        OpFlush,
        OpForget,
        OpGetattr,
        OpGetxattr,
        OpInit,
        OpLink,
        OpMkdir,
        OpMknod,
        OpOpen,
        OpRead,
        OpRelease,
        OpRename,
        OpSetattr,
        OpSetxattr,
    },
};
use async_trait::async_trait;
use libc::c_int;
use std::{borrow::Cow, ffi::OsStr};

pub type OperationResult<T> = Result<T, c_int>;

#[async_trait(?Send)]
#[allow(unused_variables)]
pub trait Operations {
    async fn init<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpInit,
        out: &'a mut InitOut,
    ) -> OperationResult<()> {
        Ok(())
    }

    async fn destroy<'a>(&'a mut self) {}

    async fn lookup<'a>(
        &'a mut self,
        header: &'a Header,
        name: &'a OsStr,
    ) -> OperationResult<EntryOut> {
        Err(libc::ENOSYS)
    }

    async fn forget<'a>(&'a mut self, header: &'a Header, op: &'a OpForget) {}

    async fn getattr<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpGetattr,
    ) -> OperationResult<AttrOut> {
        Err(libc::ENOSYS)
    }

    async fn setattr<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpSetattr,
    ) -> OperationResult<AttrOut> {
        Err(libc::ENOSYS)
    }

    async fn readlink<'a>(&'a mut self, header: &'a Header) -> OperationResult<Cow<'a, OsStr>> {
        Err(libc::ENOSYS)
    }

    async fn symlink<'a>(
        &'a mut self,
        header: &'a Header,
        name: &'a OsStr,
        link: &'a OsStr,
    ) -> OperationResult<AttrOut> {
        Err(libc::ENOSYS)
    }

    async fn mknod<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpMknod,
        name: &'a OsStr,
    ) -> OperationResult<AttrOut> {
        Err(libc::ENOSYS)
    }

    async fn mkdir<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpMkdir,
        name: &'a OsStr,
    ) -> OperationResult<AttrOut> {
        Err(libc::ENOSYS)
    }

    async fn unlink<'a>(&'a mut self, header: &'a Header, name: &'a OsStr) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }

    async fn rmdir<'a>(&'a mut self, header: &'a Header, name: &'a OsStr) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }

    async fn rename<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpRename,
        name: &'a OsStr,
        newname: &'a OsStr,
    ) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }

    async fn link<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpLink,
        newname: &'a OsStr,
    ) -> OperationResult<AttrOut> {
        Err(libc::ENOSYS)
    }

    async fn open<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpOpen,
    ) -> OperationResult<OpenOut> {
        Err(libc::ENOSYS)
    }

    async fn read<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpRead,
    ) -> OperationResult<Cow<'a, [u8]>> {
        Err(libc::ENOSYS)
    }

    async fn flush<'a>(&'a mut self, header: &'a Header, op: &'a OpFlush) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }

    async fn release<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpRelease,
    ) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }

    async fn setxattr<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpSetxattr,
        name: &'a OsStr,
        value: &'a [u8],
    ) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }

    async fn getxattr<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpGetxattr,
        name: &'a OsStr,
    ) -> OperationResult<XattrOut<'a>> {
        Err(libc::ENOSYS)
    }

    async fn listxattr<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpGetxattr,
    ) -> OperationResult<XattrOut<'a>> {
        Err(libc::ENOSYS)
    }

    async fn removexattr<'a>(
        &'a mut self,
        header: &'a Header,
        name: &'a OsStr,
    ) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }
}
