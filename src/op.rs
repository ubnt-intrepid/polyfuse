use crate::{
    error::{Error, Result},
    reply::{CreateOut, XattrOut},
};
use async_trait::async_trait;
use fuse_async_abi::{
    AccessIn,
    AttrOut, //
    BmapIn,
    BmapOut,
    CreateIn,
    EntryOut,
    FlushIn,
    ForgetIn,
    FsyncIn,
    GetattrIn,
    GetxattrIn,
    InHeader,
    InitIn,
    InitOut,
    LinkIn,
    LkIn,
    LkOut,
    MkdirIn,
    MknodIn,
    OpenIn,
    OpenOut,
    ReadIn,
    ReleaseIn,
    RenameIn,
    SetattrIn,
    SetxattrIn,
    StatfsOut,
    WriteIn,
    WriteOut,
};
use std::{borrow::Cow, ffi::OsStr};

#[async_trait]
#[allow(unused_variables)]
pub trait Operations {
    async fn init<'a>(
        &'a mut self,
        header: &'a InHeader,
        arg: &'a InitIn,
        out: &'a mut InitOut,
    ) -> Result<()> {
        Ok(())
    }

    async fn destroy<'a>(&'a mut self) {}

    async fn lookup<'a>(&'a mut self, header: &'a InHeader, name: &'a OsStr) -> Result<EntryOut> {
        Err(Error::NOSYS)
    }

    async fn forget<'a>(&'a mut self, header: &'a InHeader, arg: &'a ForgetIn) {}

    async fn getattr<'a>(
        &'a mut self,
        header: &'a InHeader,
        arg: &'a GetattrIn,
    ) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn setattr<'a>(
        &'a mut self,
        header: &'a InHeader,
        arg: &'a SetattrIn,
    ) -> Result<AttrOut> {
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
        arg: &'a MknodIn,
        name: &'a OsStr,
    ) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn mkdir<'a>(
        &'a mut self,
        header: &'a InHeader,
        arg: &'a MkdirIn,
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
        arg: &'a RenameIn,
        name: &'a OsStr,
        newname: &'a OsStr,
    ) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn link<'a>(
        &'a mut self,
        header: &'a InHeader,
        arg: &'a LinkIn,
        newname: &'a OsStr,
    ) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn open<'a>(&'a mut self, header: &'a InHeader, arg: &'a OpenIn) -> Result<OpenOut> {
        Err(Error::NOSYS)
    }

    async fn read<'a>(
        &'a mut self,
        header: &'a InHeader,
        arg: &'a ReadIn,
    ) -> Result<Cow<'a, [u8]>> {
        Err(Error::NOSYS)
    }

    async fn write<'a>(
        &'a mut self,
        header: &'a InHeader,
        arg: &'a WriteIn,
        data: &'a [u8],
    ) -> Result<WriteOut> {
        Err(Error::NOSYS)
    }

    async fn release<'a>(&'a mut self, header: &'a InHeader, arg: &'a ReleaseIn) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn statfs<'a>(&'a mut self, header: &'a InHeader) -> Result<StatfsOut> {
        Err(Error::NOSYS)
    }

    async fn fsync<'a>(&'a mut self, header: &'a InHeader, arg: &'a FsyncIn) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn setxattr<'a>(
        &'a mut self,
        header: &'a InHeader,
        arg: &'a SetxattrIn,
        name: &'a OsStr,
        value: &'a [u8],
    ) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn getxattr<'a>(
        &'a mut self,
        header: &'a InHeader,
        arg: &'a GetxattrIn,
        name: &'a OsStr,
    ) -> Result<XattrOut<'a>> {
        Err(Error::NOSYS)
    }

    async fn listxattr<'a>(
        &'a mut self,
        header: &'a InHeader,
        arg: &'a GetxattrIn,
    ) -> Result<XattrOut<'a>> {
        Err(Error::NOSYS)
    }

    async fn removexattr<'a>(&'a mut self, header: &'a InHeader, name: &'a OsStr) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn flush<'a>(&'a mut self, header: &'a InHeader, arg: &'a FlushIn) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn opendir<'a>(&'a mut self, header: &'a InHeader, arg: &'a OpenIn) -> Result<OpenOut> {
        Err(Error::NOSYS)
    }

    async fn readdir<'a>(
        &'a mut self,
        header: &'a InHeader,
        arg: &'a ReadIn,
    ) -> Result<Cow<'a, [u8]>> {
        Err(Error::NOSYS)
    }

    async fn releasedir<'a>(&'a mut self, header: &'a InHeader, arg: &'a ReleaseIn) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn fsyncdir<'a>(&'a mut self, header: &'a InHeader, arg: &'a FsyncIn) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn getlk<'a>(&'a mut self, header: &'a InHeader, arg: &'a LkIn) -> Result<LkOut> {
        Err(Error::NOSYS)
    }

    async fn setlk<'a>(
        &'a mut self,
        header: &'a InHeader,
        arg: &'a LkIn,
        sleep: bool,
    ) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn access<'a>(&'a mut self, header: &'a InHeader, arg: &'a AccessIn) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn create<'a>(
        &'a mut self,
        header: &'a InHeader,
        arg: &'a CreateIn,
    ) -> Result<CreateOut> {
        Err(Error::NOSYS)
    }

    async fn bmap<'a>(&'a mut self, header: &'a InHeader, arg: &'a BmapIn) -> Result<BmapOut> {
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
