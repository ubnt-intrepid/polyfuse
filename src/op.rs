use crate::{
    error::{Error, Result},
    reply::{CreateOut, XattrOut},
};
use async_trait::async_trait;
use fuse_async_abi::{
    AttrOut, //
    BmapOut,
    EntryOut,
    InHeader,
    InitOut,
    LkOut,
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
    OpenOut,
    StatfsOut,
    WriteOut,
};
use std::{borrow::Cow, ffi::OsStr};

#[async_trait(?Send)]
#[allow(unused_variables)]
pub trait Operations {
    async fn init<'a>(
        &'a mut self,
        header: &'a InHeader,
        op: &'a OpInit,
        out: &'a mut InitOut,
    ) -> Result<()> {
        Ok(())
    }

    async fn destroy<'a>(&'a mut self) {}

    async fn lookup<'a>(&'a mut self, header: &'a InHeader, name: &'a OsStr) -> Result<EntryOut> {
        Err(Error::NOSYS)
    }

    async fn forget<'a>(&'a mut self, header: &'a InHeader, op: &'a OpForget) {}

    async fn getattr<'a>(&'a mut self, header: &'a InHeader, op: &'a OpGetattr) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn setattr<'a>(&'a mut self, header: &'a InHeader, op: &'a OpSetattr) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn readlink<'a>(&'a mut self, header: &'a InHeader) -> Result<Cow<'a, OsStr>> {
        Err(Error::NOSYS)
    }

    async fn symlink<'a>(
        &'a mut self,
        header: &'a InHeader,
        name: &'a OsStr,
        link: &'a OsStr,
    ) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn mknod<'a>(
        &'a mut self,
        header: &'a InHeader,
        op: &'a OpMknod,
        name: &'a OsStr,
    ) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn mkdir<'a>(
        &'a mut self,
        header: &'a InHeader,
        op: &'a OpMkdir,
        name: &'a OsStr,
    ) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn unlink<'a>(&'a mut self, header: &'a InHeader, name: &'a OsStr) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn rmdir<'a>(&'a mut self, header: &'a InHeader, name: &'a OsStr) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn rename<'a>(
        &'a mut self,
        header: &'a InHeader,
        op: &'a OpRename,
        name: &'a OsStr,
        newname: &'a OsStr,
    ) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn link<'a>(
        &'a mut self,
        header: &'a InHeader,
        op: &'a OpLink,
        newname: &'a OsStr,
    ) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn open<'a>(&'a mut self, header: &'a InHeader, op: &'a OpOpen) -> Result<OpenOut> {
        Err(Error::NOSYS)
    }

    async fn read<'a>(&'a mut self, header: &'a InHeader, op: &'a OpRead) -> Result<Cow<'a, [u8]>> {
        Err(Error::NOSYS)
    }

    async fn write<'a>(
        &'a mut self,
        header: &'a InHeader,
        op: &'a OpWrite,
        data: &'a [u8],
    ) -> Result<WriteOut> {
        Err(Error::NOSYS)
    }

    async fn release<'a>(&'a mut self, header: &'a InHeader, op: &'a OpRelease) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn statfs<'a>(&'a mut self, header: &'a InHeader) -> Result<StatfsOut> {
        Err(Error::NOSYS)
    }

    async fn fsync<'a>(&'a mut self, header: &'a InHeader, op: &'a OpFsync) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn setxattr<'a>(
        &'a mut self,
        header: &'a InHeader,
        op: &'a OpSetxattr,
        name: &'a OsStr,
        value: &'a [u8],
    ) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn getxattr<'a>(
        &'a mut self,
        header: &'a InHeader,
        op: &'a OpGetxattr,
        name: &'a OsStr,
    ) -> Result<XattrOut<'a>> {
        Err(Error::NOSYS)
    }

    async fn listxattr<'a>(
        &'a mut self,
        header: &'a InHeader,
        op: &'a OpGetxattr,
    ) -> Result<XattrOut<'a>> {
        Err(Error::NOSYS)
    }

    async fn removexattr<'a>(&'a mut self, header: &'a InHeader, name: &'a OsStr) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn flush<'a>(&'a mut self, header: &'a InHeader, op: &'a OpFlush) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn opendir<'a>(&'a mut self, header: &'a InHeader, op: &'a OpOpen) -> Result<OpenOut> {
        Err(Error::NOSYS)
    }

    async fn readdir<'a>(
        &'a mut self,
        header: &'a InHeader,
        op: &'a OpRead,
    ) -> Result<Cow<'a, [u8]>> {
        Err(Error::NOSYS)
    }

    async fn releasedir<'a>(&'a mut self, header: &'a InHeader, op: &'a OpRelease) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn fsyncdir<'a>(&'a mut self, header: &'a InHeader, op: &'a OpFsync) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn getlk<'a>(&'a mut self, header: &'a InHeader, op: &'a OpLk) -> Result<LkOut> {
        Err(Error::NOSYS)
    }

    async fn setlk<'a>(
        &'a mut self,
        header: &'a InHeader,
        op: &'a OpLk,
        sleep: bool,
    ) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn access<'a>(&'a mut self, header: &'a InHeader, op: &'a OpAccess) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn create<'a>(&'a mut self, header: &'a InHeader, op: &'a OpCreate) -> Result<CreateOut> {
        Err(Error::NOSYS)
    }

    async fn bmap<'a>(&'a mut self, header: &'a InHeader, op: &'a OpBmap) -> Result<BmapOut> {
        Err(Error::NOSYS)
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
