#![allow(clippy::needless_lifetimes)]

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
    async fn init(&mut self, header: &InHeader, arg: &InitIn, out: &mut InitOut) -> Result<()> {
        Ok(())
    }

    async fn destroy(&mut self) {}

    async fn lookup(&mut self, header: &InHeader, name: &OsStr) -> Result<EntryOut> {
        Err(Error::NOSYS)
    }

    async fn forget(&mut self, header: &InHeader, arg: &ForgetIn) {}

    async fn getattr(&mut self, header: &InHeader, arg: &GetattrIn) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn setattr(&mut self, header: &InHeader, arg: &SetattrIn) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn readlink<'s>(&'s mut self, header: &InHeader) -> Result<Cow<'s, OsStr>> {
        Err(Error::NOSYS)
    }

    async fn symlink(&mut self, header: &InHeader, name: &OsStr, link: &OsStr) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn mknod(&mut self, header: &InHeader, arg: &MknodIn, name: &OsStr) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn mkdir(&mut self, header: &InHeader, arg: &MkdirIn, name: &OsStr) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn unlink(&mut self, header: &InHeader, name: &OsStr) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn rmdir(&mut self, header: &InHeader, name: &OsStr) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn rename(
        &mut self,
        header: &InHeader,
        arg: &RenameIn,
        name: &OsStr,
        newname: &OsStr,
    ) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn link(&mut self, header: &InHeader, arg: &LinkIn, newname: &OsStr) -> Result<AttrOut> {
        Err(Error::NOSYS)
    }

    async fn open(&mut self, header: &InHeader, arg: &OpenIn) -> Result<OpenOut> {
        Err(Error::NOSYS)
    }

    async fn read<'s>(&'s mut self, header: &InHeader, arg: &ReadIn) -> Result<Cow<'s, [u8]>> {
        Err(Error::NOSYS)
    }

    async fn write(&mut self, header: &InHeader, arg: &WriteIn, data: &[u8]) -> Result<WriteOut> {
        Err(Error::NOSYS)
    }

    async fn release(&mut self, header: &InHeader, arg: &ReleaseIn) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn statfs(&mut self, header: &InHeader) -> Result<StatfsOut> {
        Err(Error::NOSYS)
    }

    async fn fsync(&mut self, header: &InHeader, arg: &FsyncIn) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn setxattr(
        &mut self,
        header: &InHeader,
        arg: &SetxattrIn,
        name: &OsStr,
        value: &[u8],
    ) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn getxattr<'s>(
        &'s mut self,
        header: &InHeader,
        arg: &GetxattrIn,
        name: &OsStr,
    ) -> Result<XattrOut<'s>> {
        Err(Error::NOSYS)
    }

    async fn listxattr<'s>(
        &'s mut self,
        header: &InHeader,
        arg: &GetxattrIn,
    ) -> Result<XattrOut<'s>> {
        Err(Error::NOSYS)
    }

    async fn removexattr(&mut self, header: &InHeader, name: &OsStr) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn flush(&mut self, header: &InHeader, arg: &FlushIn) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn opendir(&mut self, header: &InHeader, arg: &OpenIn) -> Result<OpenOut> {
        Err(Error::NOSYS)
    }

    async fn readdir<'s>(&'s mut self, header: &InHeader, arg: &ReadIn) -> Result<Cow<'s, [u8]>> {
        Err(Error::NOSYS)
    }

    async fn releasedir(&mut self, header: &InHeader, arg: &ReleaseIn) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn fsyncdir(&mut self, header: &InHeader, arg: &FsyncIn) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn getlk(&mut self, header: &InHeader, arg: &LkIn) -> Result<LkOut> {
        Err(Error::NOSYS)
    }

    async fn setlk(&mut self, header: &InHeader, arg: &LkIn, sleep: bool) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn access(&mut self, header: &InHeader, arg: &AccessIn) -> Result<()> {
        Err(Error::NOSYS)
    }

    async fn create(&mut self, header: &InHeader, arg: &CreateIn) -> Result<CreateOut> {
        Err(Error::NOSYS)
    }

    async fn bmap(&mut self, header: &InHeader, arg: &BmapIn) -> Result<BmapOut> {
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
