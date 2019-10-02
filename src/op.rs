#![allow(clippy::needless_lifetimes)]

use crate::reply::{
    ReplyAttr, //
    ReplyBmap,
    ReplyCreate,
    ReplyData,
    ReplyEntry,
    ReplyInit,
    ReplyLk,
    ReplyOpen,
    ReplyStatfs,
    ReplyUnit,
    ReplyWrite,
    ReplyXattr,
};
use fuse_async_abi::{
    AccessIn, //
    BmapIn,
    CreateIn,
    FlushIn,
    ForgetIn,
    FsyncIn,
    GetattrIn,
    GetxattrIn,
    InHeader,
    InitIn,
    LinkIn,
    LkIn,
    MkdirIn,
    MknodIn,
    OpenIn,
    ReadIn,
    ReleaseIn,
    RenameIn,
    SetattrIn,
    SetxattrIn,
    WriteIn,
};
use std::future::Future;
use std::{ffi::OsStr, io, pin::Pin};

type ImplFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[allow(unused_variables)]
pub trait Operations {
    fn init<'a>(
        &mut self,
        header: &InHeader,
        arg: &InitIn,
        reply: ReplyInit<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.ok())
    }

    fn destroy(&mut self) {}

    fn lookup<'a>(
        &mut self,
        header: &InHeader,
        name: &OsStr,
        reply: ReplyEntry<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn forget(&mut self, header: &InHeader, arg: &ForgetIn) {}

    fn getattr<'a>(
        &mut self,
        header: &InHeader,
        arg: &GetattrIn,
        reply: ReplyAttr<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn setattr<'a>(
        &mut self,
        header: &InHeader,
        arg: &SetattrIn,
        reply: ReplyAttr<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn readlink<'a>(
        &mut self,
        header: &InHeader,
        reply: ReplyData<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn symlink<'a>(
        &mut self,
        header: &InHeader,
        name: &OsStr,
        link: &OsStr,
        reply: ReplyAttr<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn mknod<'a>(
        &mut self,
        header: &InHeader,
        arg: &MknodIn,
        name: &OsStr,
        reply: ReplyAttr<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn mkdir<'a>(
        &mut self,
        header: &InHeader,
        arg: &MkdirIn,
        name: &OsStr,
        reply: ReplyAttr<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn unlink<'a>(
        &mut self,
        header: &InHeader,
        name: &OsStr,
        reply: ReplyUnit<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn rmdir<'a>(
        &mut self,
        header: &InHeader,
        name: &OsStr,
        reply: ReplyUnit<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn rename<'a>(
        &mut self,
        header: &InHeader,
        arg: &RenameIn,
        name: &OsStr,
        newname: &OsStr,
        reply: ReplyUnit<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn link<'a>(
        &mut self,
        header: &InHeader,
        arg: &LinkIn,
        newname: &OsStr,
        reply: ReplyAttr<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn open<'a>(
        &mut self,
        header: &InHeader,
        arg: &OpenIn,
        reply: ReplyOpen<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn read<'a>(
        &mut self,
        header: &InHeader,
        arg: &ReadIn,
        reply: ReplyData<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn write<'a>(
        &mut self,
        header: &InHeader,
        arg: &WriteIn,
        data: &[u8],
        reply: ReplyWrite<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn release<'a>(
        &mut self,
        header: &InHeader,
        arg: &ReleaseIn,
        reply: ReplyUnit<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn statfs<'a>(
        &mut self,
        header: &InHeader,
        reply: ReplyStatfs<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn fsync<'a>(
        &mut self,
        header: &InHeader,
        arg: &FsyncIn,
        reply: ReplyUnit<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn setxattr<'a>(
        &mut self,
        header: &InHeader,
        arg: &SetxattrIn,
        name: &OsStr,
        value: &[u8],
        reply: ReplyUnit<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn getxattr<'a>(
        &mut self,
        header: &InHeader,
        arg: &GetxattrIn,
        name: &OsStr,
        reply: ReplyXattr<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn listxattr<'a>(
        &mut self,
        header: &InHeader,
        arg: &GetxattrIn,
        reply: ReplyXattr<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn removexattr<'a>(
        &mut self,
        header: &InHeader,
        name: &OsStr,
        reply: ReplyUnit<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn flush<'a>(
        &mut self,
        header: &InHeader,
        arg: &FlushIn,
        reply: ReplyUnit<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn opendir<'a>(
        &mut self,
        header: &InHeader,
        arg: &OpenIn,
        reply: ReplyOpen<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn readdir<'a>(
        &mut self,
        header: &InHeader,
        arg: &ReadIn,
        reply: ReplyData<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn releasedir<'a>(
        &mut self,
        header: &InHeader,
        arg: &ReleaseIn,
        reply: ReplyUnit<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn fsyncdir<'a>(
        &mut self,
        header: &InHeader,
        arg: &FsyncIn,
        reply: ReplyUnit<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn getlk<'a>(
        &mut self,
        header: &InHeader,
        arg: &LkIn,
        reply: ReplyLk<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn setlk<'a>(
        &mut self,
        header: &InHeader,
        arg: &LkIn,
        sleep: bool,
        reply: ReplyUnit<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn access<'a>(
        &mut self,
        header: &InHeader,
        arg: &AccessIn,
        reply: ReplyUnit<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn create<'a>(
        &mut self,
        header: &InHeader,
        arg: &CreateIn,
        reply: ReplyCreate<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
    }

    fn bmap<'a>(
        &mut self,
        header: &InHeader,
        arg: &BmapIn,
        reply: ReplyBmap<'a>,
    ) -> ImplFuture<'a, io::Result<()>> {
        Box::pin(reply.err(libc::ENOSYS))
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
