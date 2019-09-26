use crate::{
    reply::{
        AttrOut, //
        BmapOut,
        CreateOut,
        EntryOut,
        InitOut,
        LkOut,
        OpenOut,
        StatfsOut,
        WriteOut,
        XattrOut,
    },
    request::{
        Header, //
        OpAccess,
        OpBmap,
        OpCreate,
        OpFlush,
        OpForget,
        OpFsync,
        OpGetattr,
        OpGetxattr,
        OpInit,
        OpLink,
        OpLk,
        OpMkdir,
        OpMknod,
        OpOpen,
        OpRead,
        OpRelease,
        OpRename,
        OpSetattr,
        OpSetxattr,
        OpWrite,
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

    async fn write<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpWrite,
        data: &'a [u8],
    ) -> OperationResult<WriteOut> {
        Err(libc::ENOSYS)
    }

    async fn release<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpRelease,
    ) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }

    async fn statfs<'a>(&'a mut self, header: &'a Header) -> OperationResult<StatfsOut> {
        Err(libc::ENOSYS)
    }

    async fn fsync<'a>(&'a mut self, header: &'a Header, op: &'a OpFsync) -> OperationResult<()> {
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

    async fn flush<'a>(&'a mut self, header: &'a Header, op: &'a OpFlush) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }

    async fn opendir<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpOpen,
    ) -> OperationResult<OpenOut> {
        Err(libc::ENOSYS)
    }

    async fn readdir<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpRead,
    ) -> OperationResult<Cow<'a, [u8]>> {
        Err(libc::ENOSYS)
    }

    async fn releasedir<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpRelease,
    ) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }

    async fn fsyncdir<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpFsync,
    ) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }

    async fn getlk<'a>(&'a mut self, header: &'a Header, op: &'a OpLk) -> OperationResult<LkOut> {
        Err(libc::ENOSYS)
    }

    async fn setlk<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpLk,
        sleep: bool,
    ) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }

    async fn access<'a>(&'a mut self, header: &'a Header, op: &'a OpAccess) -> OperationResult<()> {
        Err(libc::ENOSYS)
    }

    async fn create<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpCreate,
    ) -> OperationResult<CreateOut> {
        Err(libc::ENOSYS)
    }

    async fn bmap<'a>(
        &'a mut self,
        header: &'a Header,
        op: &'a OpBmap,
    ) -> OperationResult<BmapOut> {
        Err(libc::ENOSYS)
    }

    // interrupt
    // ioctl
    // poll
    // notify_reply
    // batch_forget
    // fallocate
    // readdirplus
    // rename2
    // lseek
    // copy_file_range
}
