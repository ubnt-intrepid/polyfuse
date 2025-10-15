#![deny(unreachable_pub)]

mod attr;
mod copy_file_range;
mod create;
mod dir;
mod file;
mod forget;
mod inode;
mod interrupt;
mod lock;
mod notify_reply;
mod xattr;

pub use attr::{Getattr, SetAttrTime, Setattr};
pub use copy_file_range::CopyFileRange;
pub use create::Create;
pub use dir::{Fsyncdir, Opendir, Readdir, ReaddirMode, Releasedir};
pub use file::{
    AccessMode, Fallocate, FallocateFlags, Flush, Fsync, Lseek, Open, OpenFlags, OpenOptions, Poll,
    Read, Release, ReleaseFlags, Whence, Write,
};
pub use forget::{Forget, Forgets};
pub use inode::{
    Access, Bmap, Link, Lookup, Mkdir, Mknod, Readlink, Rename, RenameFlags, Rmdir, Statfs,
    Symlink, Unlink,
};
pub use interrupt::Interrupt;
pub use lock::{Flock, FlockOp, Getlk, Setlk};
pub use notify_reply::NotifyReply;
pub use xattr::{Getxattr, Listxattr, Removexattr, Setxattr, SetxattrFlags};

use crate::{
    bytes::{DecodeError, Decoder},
    request::RequestHeader,
    session::KernelConfig,
};
use lock::SetlkKind;
use polyfuse_kernel::fuse_opcode;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("during decoding: {}", _0)]
    Decode(#[from] DecodeError),

    #[error("unsupported opcode")]
    UnsupportedOpcode,

    #[error("invalid nodeid")]
    InvalidNodeID,

    #[error("invalid `flock(2)` operation type")]
    InvalidFlockType,

    #[error("invalid PID value")]
    InvalidPid,

    #[error("unknown `lseek(2)` whence")]
    UnknownLseekWhence,
}

/// The kind of filesystem operation requested by the kernel.
#[derive(Debug)]
#[non_exhaustive]
pub enum Operation<'op> {
    Lookup(Lookup<'op>),
    Getattr(Getattr<'op>),
    Setattr(Setattr<'op>),
    Readlink(Readlink<'op>),
    Symlink(Symlink<'op>),
    Mknod(Mknod<'op>),
    Mkdir(Mkdir<'op>),
    Unlink(Unlink<'op>),
    Rmdir(Rmdir<'op>),
    Rename(Rename<'op>),
    Link(Link<'op>),
    Open(Open<'op>),
    Read(Read<'op>),
    Write(Write<'op>),
    Release(Release<'op>),
    Statfs(Statfs<'op>),
    Fsync(Fsync<'op>),
    Setxattr(Setxattr<'op>),
    Getxattr(Getxattr<'op>),
    Listxattr(Listxattr<'op>),
    Removexattr(Removexattr<'op>),
    Flush(Flush<'op>),
    Opendir(Opendir<'op>),
    Readdir(Readdir<'op>),
    Releasedir(Releasedir<'op>),
    Fsyncdir(Fsyncdir<'op>),
    Getlk(Getlk<'op>),
    Setlk(Setlk<'op>),
    Flock(Flock<'op>),
    Access(Access<'op>),
    Create(Create<'op>),
    Bmap(Bmap<'op>),
    Fallocate(Fallocate<'op>),
    CopyFileRange(CopyFileRange<'op>),
    Poll(Poll<'op>),
    Lseek(Lseek<'op>),

    Forget(Forgets<'op>),
    Interrupt(Interrupt<'op>),
    NotifyReply(NotifyReply<'op>),
}

impl<'op> Operation<'op> {
    pub fn decode(
        config: &'op KernelConfig,
        header: &'op RequestHeader,
        arg: &'op [u8],
    ) -> Result<Self, Error> {
        let opcode = header.opcode().map_err(|_| Error::UnsupportedOpcode)?;
        let mut cx = Context {
            config,
            header,
            opcode,
            decoder: Decoder::new(arg),
        };

        match opcode {
            fuse_opcode::FUSE_FORGET | fuse_opcode::FUSE_BATCH_FORGET => {
                Forgets::decode(&mut cx).map(Operation::Forget)
            }
            fuse_opcode::FUSE_INTERRUPT => Interrupt::decode(&mut cx).map(Operation::Interrupt),
            fuse_opcode::FUSE_NOTIFY_REPLY => {
                NotifyReply::decode(&mut cx).map(Operation::NotifyReply)
            }
            fuse_opcode::FUSE_LOOKUP => Lookup::decode(&mut cx).map(Operation::Lookup),
            fuse_opcode::FUSE_GETATTR => Getattr::decode(&mut cx).map(Operation::Getattr),
            fuse_opcode::FUSE_SETATTR => Setattr::decode(&mut cx).map(Operation::Setattr),
            fuse_opcode::FUSE_READLINK => Readlink::decode(&mut cx).map(Operation::Readlink),
            fuse_opcode::FUSE_SYMLINK => Symlink::decode(&mut cx).map(Operation::Symlink),
            fuse_opcode::FUSE_MKNOD => Mknod::decode(&mut cx).map(Operation::Mknod),
            fuse_opcode::FUSE_MKDIR => Mkdir::decode(&mut cx).map(Operation::Mkdir),
            fuse_opcode::FUSE_UNLINK => Unlink::decode(&mut cx).map(Operation::Unlink),
            fuse_opcode::FUSE_RMDIR => Rmdir::decode(&mut cx).map(Operation::Rmdir),
            fuse_opcode::FUSE_RENAME | fuse_opcode::FUSE_RENAME2 => {
                Rename::decode(&mut cx).map(Operation::Rename)
            }
            fuse_opcode::FUSE_LINK => Link::decode(&mut cx).map(Operation::Link),

            fuse_opcode::FUSE_OPEN => Open::decode(&mut cx).map(Operation::Open),
            fuse_opcode::FUSE_READ => Read::decode(&mut cx).map(Operation::Read),
            fuse_opcode::FUSE_WRITE => Write::decode(&mut cx).map(Operation::Write),
            fuse_opcode::FUSE_RELEASE => Release::decode(&mut cx).map(Operation::Release),
            fuse_opcode::FUSE_FSYNC => Fsync::decode(&mut cx).map(Operation::Fsync),
            fuse_opcode::FUSE_FLUSH => Flush::decode(&mut cx).map(Operation::Flush),

            fuse_opcode::FUSE_STATFS => Statfs::decode(&mut cx).map(Operation::Statfs),

            fuse_opcode::FUSE_SETXATTR => Setxattr::decode(&mut cx).map(Operation::Setxattr),
            fuse_opcode::FUSE_GETXATTR => Getxattr::decode(&mut cx).map(Operation::Getxattr),
            fuse_opcode::FUSE_LISTXATTR => Listxattr::decode(&mut cx).map(Operation::Listxattr),
            fuse_opcode::FUSE_REMOVEXATTR => {
                Removexattr::decode(&mut cx).map(Operation::Removexattr)
            }

            fuse_opcode::FUSE_OPENDIR => Opendir::decode(&mut cx).map(Operation::Opendir),
            fuse_opcode::FUSE_READDIR | fuse_opcode::FUSE_READDIRPLUS => {
                Readdir::decode(&mut cx).map(Operation::Readdir)
            }
            fuse_opcode::FUSE_RELEASEDIR => Releasedir::decode(&mut cx).map(Operation::Releasedir),
            fuse_opcode::FUSE_FSYNCDIR => Fsyncdir::decode(&mut cx).map(Operation::Fsyncdir),

            fuse_opcode::FUSE_GETLK => Getlk::decode(&mut cx).map(Operation::Getlk),
            fuse_opcode::FUSE_SETLK | fuse_opcode::FUSE_SETLKW => {
                SetlkKind::decode(&mut cx).map(|kind| match kind {
                    SetlkKind::Posix(op) => Operation::Setlk(op),
                    SetlkKind::Flock(op) => Operation::Flock(op),
                })
            }

            fuse_opcode::FUSE_ACCESS => Access::decode(&mut cx).map(Operation::Access),
            fuse_opcode::FUSE_CREATE => Create::decode(&mut cx).map(Operation::Create),
            fuse_opcode::FUSE_BMAP => Bmap::decode(&mut cx).map(Operation::Bmap),
            fuse_opcode::FUSE_FALLOCATE => Fallocate::decode(&mut cx).map(Operation::Fallocate),
            fuse_opcode::FUSE_COPY_FILE_RANGE => {
                CopyFileRange::decode(&mut cx).map(Operation::CopyFileRange)
            }
            fuse_opcode::FUSE_POLL => Poll::decode(&mut cx).map(Operation::Poll),
            fuse_opcode::FUSE_LSEEK => Lseek::decode(&mut cx).map(Operation::Lseek),

            fuse_opcode::FUSE_INIT | fuse_opcode::FUSE_DESTROY => {
                // should be handled by the upstream process.
                unreachable!()
            }

            fuse_opcode::FUSE_IOCTL
            | fuse_opcode::FUSE_SETUPMAPPING
            | fuse_opcode::FUSE_REMOVEMAPPING
            | fuse_opcode::FUSE_SYNCFS
            | fuse_opcode::FUSE_TMPFILE
            | fuse_opcode::FUSE_STATX
            | fuse_opcode::CUSE_INIT => Err(Error::UnsupportedOpcode),
        }
    }
}

trait Op<'op>: Sized {
    fn decode(cx: &mut Context<'op>) -> Result<Self, Error>;
}

struct Context<'op> {
    config: &'op KernelConfig,
    header: &'op RequestHeader,
    opcode: fuse_opcode,
    decoder: Decoder<'op>,
}
