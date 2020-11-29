//! Components used when processing FUSE requests.

use crate::{
    op::{self, LockOwner, SetAttrTime},
    reply,
    session::Session,
    util::{as_bytes, Decoder},
    write::ReplySender,
};
use polyfuse_kernel::{self as kernel, fuse_opcode};
use std::{
    convert::TryFrom, ffi::OsStr, fmt, io, marker::PhantomData, mem, os::unix::prelude::*, ptr,
    sync::Arc, time::Duration,
};

#[derive(Debug)]
pub struct Error(ErrorKind);

#[derive(Debug)]
enum ErrorKind {
    Decode,
    Reply(io::Error),
}

impl Error {
    fn decode() -> Self {
        Self(ErrorKind::Decode)
    }

    fn reply(err: io::Error) -> Self {
        Self(ErrorKind::Reply(err))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpError").finish()
    }
}

impl std::error::Error for Error {}

/// Context about an incoming FUSE request.
pub struct Request {
    pub(crate) buf: Vec<u8>,
    pub(crate) session: Arc<Session>,
}

impl Request {
    // TODO: add unique(), uid(), gid() and pid()

    /// Decode the argument of this request.
    pub fn operation<W>(&self, writer: W) -> Result<Operation<'_, W>, Error>
    where
        W: io::Write,
    {
        if self.session.exited() {
            return Ok(Operation::Null);
        }

        let mut decoder = Decoder::new(&self.buf[..]);

        let header = decoder
            .fetch::<kernel::fuse_in_header>()
            .ok_or_else(Error::decode)?;

        let sender = ReplySender::new(writer, header.unique);

        match fuse_opcode::try_from(header.opcode).ok() {
            // Some(fuse_opcode::FUSE_FORGET) => {
            //     let arg = decoder
            //         .fetch::<kernel::fuse_forget_in>()
            //         .map_err(Error::fatal)?;
            //     todo!()
            // }
            // Some(fuse_opcode::FUSE_BATCH_FORGET) => {
            //     let arg = decoder.fetch::<kernel::fuse_batch_forget_in>()?;
            //     let forgets = decoder.fetch_array(arg.count as usize)?;
            //     todo!()
            // }
            // Some(fuse_opcode::FUSE_INTERRUPT) => {
            //     let arg = decoder.fetch()?;
            //     todo!()
            // }
            // Some(fuse_opcode::FUSE_NOTIFY_REPLY) => {
            //     let arg = decoder.fetch()?;
            //     todo!()
            // }
            Some(fuse_opcode::FUSE_LOOKUP) => {
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Lookup {
                    op: Lookup { header, name },
                    reply: ReplyEntry {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_GETATTR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Getattr {
                    op: Getattr { header, arg },
                    reply: ReplyAttr {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_SETATTR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Setattr {
                    op: Setattr { header, arg },
                    reply: ReplyAttr {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_READLINK) => Ok(Operation::Readlink {
                op: Readlink { header },
                reply: ReplyReadlink {
                    sender,
                    _marker: PhantomData,
                },
            }),

            Some(fuse_opcode::FUSE_SYMLINK) => {
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                let link = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Symlink {
                    op: Symlink { header, name, link },
                    reply: ReplyEntry {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_MKNOD) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Mknod {
                    op: Mknod { header, arg, name },
                    reply: ReplyEntry {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_MKDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Mkdir {
                    op: Mkdir { header, arg, name },
                    reply: ReplyEntry {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_UNLINK) => {
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Unlink {
                    op: Unlink { header, name },
                    reply: ReplyOk {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_RMDIR) => {
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Rmdir {
                    op: Rmdir { header, name },
                    reply: ReplyOk {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_RENAME) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                let newname = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Rename {
                    op: Rename {
                        header,
                        arg: RenameArg::V1(arg),
                        name,
                        newname,
                    },
                    reply: ReplyOk {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }
            Some(fuse_opcode::FUSE_RENAME2) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                let newname = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Rename {
                    op: Rename {
                        header,
                        arg: RenameArg::V2(arg),
                        name,
                        newname,
                    },
                    reply: ReplyOk {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_LINK) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let newname = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Link {
                    op: Link {
                        header,
                        arg,
                        newname,
                    },
                    reply: ReplyEntry {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_OPEN) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Open {
                    op: Open { header, arg },
                    reply: ReplyOpen {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_READ) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Read {
                    op: Read { header, arg },
                    reply: ReplyData {
                        sender,
                        maxlen: arg.size as usize,
                        chunks: vec![],
                        len: 0,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_WRITE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Write {
                    op: Write { header, arg },
                    reply: ReplyWrite {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_RELEASE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Release {
                    op: Release { header, arg },
                    reply: ReplyOk {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_STATFS) => Ok(Operation::Statfs {
                op: Statfs { header },
                reply: ReplyStatfs {
                    sender,
                    _marker: PhantomData,
                },
            }),

            Some(fuse_opcode::FUSE_FSYNC) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Fsync {
                    op: Fsync { header, arg },
                    reply: ReplyOk {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_SETXATTR) => {
                let arg = decoder
                    .fetch::<kernel::fuse_setxattr_in>()
                    .ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                let value = decoder
                    .fetch_bytes(arg.size as usize)
                    .ok_or_else(Error::decode)?;
                Ok(Operation::Setxattr {
                    op: Setxattr {
                        header,
                        arg,
                        name,
                        value,
                    },
                    reply: ReplyOk {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_GETXATTR) => {
                let arg: &kernel::fuse_getxattr_in = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                if arg.size == 0 {
                    Ok(Operation::GetxattrSize {
                        op: Getxattr { header, arg, name },
                        reply: ReplyXattrSize {
                            sender,
                            _marker: PhantomData,
                        },
                    })
                } else {
                    Ok(Operation::GetxattrData {
                        op: Getxattr { header, arg, name },
                        reply: ReplyXattrData {
                            sender,
                            _marker: PhantomData,
                        },
                    })
                }
            }

            Some(fuse_opcode::FUSE_LISTXATTR) => {
                let arg: &kernel::fuse_getxattr_in = decoder.fetch().ok_or_else(Error::decode)?;
                if arg.size == 0 {
                    Ok(Operation::ListxattrSize {
                        op: Listxattr { header, arg },
                        reply: ReplyXattrSize {
                            sender,
                            _marker: PhantomData,
                        },
                    })
                } else {
                    Ok(Operation::ListxattrData {
                        op: Listxattr { header, arg },
                        reply: ReplyXattrData {
                            sender,
                            _marker: PhantomData,
                        },
                    })
                }
            }

            Some(fuse_opcode::FUSE_REMOVEXATTR) => {
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Removexattr {
                    op: Removexattr { header, name },
                    reply: ReplyOk {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_FLUSH) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Flush {
                    op: Flush { header, arg },
                    reply: ReplyOk {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_OPENDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Opendir {
                    op: Opendir { header, arg },
                    reply: ReplyOpen {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_READDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Readdir {
                    op: Readdir { header, arg },
                    reply: ReplyDirs {
                        sender,
                        maxlen: arg.size as usize,
                        entries: vec![],
                        len: 0,
                        _marker: PhantomData,
                    },
                })
            }
            Some(fuse_opcode::FUSE_READDIRPLUS) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Readdirplus {
                    op: Readdir { header, arg },
                    reply: ReplyDirsPlus {
                        sender,
                        maxlen: arg.size as usize,
                        entries: vec![],
                        len: 0,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_RELEASEDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Releasedir {
                    op: Releasedir { header, arg },
                    reply: ReplyOk {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_FSYNCDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Fsyncdir {
                    op: Fsyncdir { header, arg },
                    reply: ReplyOk {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_GETLK) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Getlk {
                    op: Getlk { header, arg },
                    reply: ReplyLk {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(opcode @ fuse_opcode::FUSE_SETLK) | Some(opcode @ fuse_opcode::FUSE_SETLKW) => {
                let arg: &kernel::fuse_lk_in = decoder.fetch().ok_or_else(Error::decode)?;
                let sleep = match opcode {
                    fuse_opcode::FUSE_SETLK => false,
                    fuse_opcode::FUSE_SETLKW => true,
                    _ => unreachable!(),
                };

                if arg.lk_flags & kernel::FUSE_LK_FLOCK == 0 {
                    Ok(Operation::Setlk {
                        op: Setlk {
                            header,
                            arg,
                            sleep: false,
                        },
                        reply: ReplyOk {
                            sender,
                            _marker: PhantomData,
                        },
                    })
                } else {
                    let op = convert_to_flock_op(arg.lk.typ, sleep).unwrap_or(0);
                    Ok(Operation::Flock {
                        op: Flock { header, arg, op },
                        reply: ReplyOk {
                            sender,
                            _marker: PhantomData,
                        },
                    })
                }
            }

            Some(fuse_opcode::FUSE_ACCESS) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Access {
                    op: Access { header, arg },
                    reply: ReplyOk {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_CREATE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Create {
                    op: Create { header, arg, name },
                    reply: ReplyCreate {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_BMAP) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Bmap {
                    op: Bmap { header, arg },
                    reply: ReplyBmap {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_FALLOCATE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Fallocate {
                    op: Fallocate { header, arg },
                    reply: ReplyOk {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_COPY_FILE_RANGE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::CopyFileRange {
                    op: CopyFileRange { header, arg },
                    reply: ReplyWrite {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            Some(fuse_opcode::FUSE_POLL) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Poll {
                    op: Poll { header, arg },
                    reply: ReplyPoll {
                        sender,
                        _marker: PhantomData,
                    },
                })
            }

            _ => {
                tracing::warn!("unsupported opcode: {}", header.opcode);
                Ok(Operation::Unsupported(Unsupported {
                    sender,
                    _marker: PhantomData,
                }))
            }
        }
    }
}

fn convert_to_flock_op(lk_type: u32, sleep: bool) -> Option<u32> {
    const F_RDLCK: u32 = libc::F_RDLCK as u32;
    const F_WRLCK: u32 = libc::F_WRLCK as u32;
    const F_UNLCK: u32 = libc::F_UNLCK as u32;

    let mut op = match lk_type {
        F_RDLCK => libc::LOCK_SH as u32,
        F_WRLCK => libc::LOCK_EX as u32,
        F_UNLCK => libc::LOCK_UN as u32,
        _ => return None,
    };

    if !sleep {
        op |= libc::LOCK_NB as u32;
    }
    Some(op)
}

/// The kind of filesystem operation requested by the kernel.
#[non_exhaustive]
pub enum Operation<'op, W> {
    Lookup {
        op: Lookup<'op>,
        reply: ReplyEntry<'op, W>,
    },
    Getattr {
        op: Getattr<'op>,
        reply: ReplyAttr<'op, W>,
    },
    Setattr {
        op: Setattr<'op>,
        reply: ReplyAttr<'op, W>,
    },
    Readlink {
        op: Readlink<'op>,
        reply: ReplyReadlink<'op, W>,
    },
    Symlink {
        op: Symlink<'op>,
        reply: ReplyEntry<'op, W>,
    },
    Mknod {
        op: Mknod<'op>,
        reply: ReplyEntry<'op, W>,
    },
    Mkdir {
        op: Mkdir<'op>,
        reply: ReplyEntry<'op, W>,
    },
    Unlink {
        op: Unlink<'op>,
        reply: ReplyOk<'op, W>,
    },
    Rmdir {
        op: Rmdir<'op>,
        reply: ReplyOk<'op, W>,
    },
    Rename {
        op: Rename<'op>,
        reply: ReplyOk<'op, W>,
    },
    Link {
        op: Link<'op>,
        reply: ReplyEntry<'op, W>,
    },
    Open {
        op: Open<'op>,
        reply: ReplyOpen<'op, W>,
    },
    Read {
        op: Read<'op>,
        reply: ReplyData<'op, W>,
    },
    Write {
        op: Write<'op>,
        reply: ReplyWrite<'op, W>,
    },
    Release {
        op: Release<'op>,
        reply: ReplyOk<'op, W>,
    },
    Statfs {
        op: Statfs<'op>,
        reply: ReplyStatfs<'op, W>,
    },
    Fsync {
        op: Fsync<'op>,
        reply: ReplyOk<'op, W>,
    },
    Setxattr {
        op: Setxattr<'op>,
        reply: ReplyOk<'op, W>,
    },
    GetxattrSize {
        op: Getxattr<'op>,
        reply: ReplyXattrSize<'op, W>,
    },
    GetxattrData {
        op: Getxattr<'op>,
        reply: ReplyXattrData<'op, W>,
    },
    ListxattrSize {
        op: Listxattr<'op>,
        reply: ReplyXattrSize<'op, W>,
    },
    ListxattrData {
        op: Listxattr<'op>,
        reply: ReplyXattrData<'op, W>,
    },
    Removexattr {
        op: Removexattr<'op>,
        reply: ReplyOk<'op, W>,
    },
    Flush {
        op: Flush<'op>,
        reply: ReplyOk<'op, W>,
    },
    Opendir {
        op: Opendir<'op>,
        reply: ReplyOpen<'op, W>,
    },
    Readdir {
        op: Readdir<'op>,
        reply: ReplyDirs<'op, W>,
    },
    Readdirplus {
        op: Readdir<'op>,
        reply: ReplyDirsPlus<'op, W>,
    },
    Releasedir {
        op: Releasedir<'op>,
        reply: ReplyOk<'op, W>,
    },
    Fsyncdir {
        op: Fsyncdir<'op>,
        reply: ReplyOk<'op, W>,
    },
    Getlk {
        op: Getlk<'op>,
        reply: ReplyLk<'op, W>,
    },
    Setlk {
        op: Setlk<'op>,
        reply: ReplyOk<'op, W>,
    },
    Flock {
        op: Flock<'op>,
        reply: ReplyOk<'op, W>,
    },
    Access {
        op: Access<'op>,
        reply: ReplyOk<'op, W>,
    },
    Create {
        op: Create<'op>,
        reply: ReplyCreate<'op, W>,
    },
    Bmap {
        op: Bmap<'op>,
        reply: ReplyBmap<'op, W>,
    },
    Fallocate {
        op: Fallocate<'op>,
        reply: ReplyOk<'op, W>,
    },
    CopyFileRange {
        op: CopyFileRange<'op>,
        reply: ReplyWrite<'op, W>,
    },
    Poll {
        op: Poll<'op>,
        reply: ReplyPoll<'op, W>,
    },

    #[doc(hidden)]
    Null,

    #[doc(hidden)]
    Unsupported(Unsupported<'op, W>),
}

impl<'op, W> Operation<'op, W>
where
    W: io::Write,
{
    pub fn unimplemented(self) -> Result<Replied, Error> {
        use crate::reply::*;

        macro_rules! f {
            ( $( $Op:ident ),* $(,)? ) => {
                match self {
                    $(
                        Operation::$Op { reply, .. } => reply.error(libc::ENOSYS),
                    )*

                    Operation::Null => Ok(Replied(())),
                    Operation::Unsupported(op) => op.send_error(),
                }
            };
        }

        f! {
            Lookup,
            Getattr,
            Setattr,
            Readlink,
            Symlink,
            Mknod,
            Mkdir,
            Unlink,
            Rmdir,
            Rename,
            Link,
            Open,
            Read,
            Write,
            Release,
            Statfs,
            Fsync,
            Setxattr,
            GetxattrSize,
            GetxattrData,
            ListxattrSize,
            ListxattrData,
            Removexattr,
            Flush,
            Opendir,
            Readdir,
            Readdirplus,
            Releasedir,
            Fsyncdir,
            Getlk,
            Setlk,
            Flock,
            Access,
            Create,
            Bmap,
            Fallocate,
            CopyFileRange,
            Poll,
        }
    }
}

#[doc(hidden)]
pub struct Unsupported<'op, W> {
    sender: ReplySender<W>,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> Unsupported<'op, W>
where
    W: io::Write,
{
    fn send_error(self) -> Result<Replied, Error> {
        self.sender
            .error(libc::ENOSYS)
            .map(Replied)
            .map_err(Error::reply)
    }
}

// ==== operations ====

pub struct Lookup<'op> {
    header: &'op kernel::fuse_in_header,
    name: &'op OsStr,
}

impl<'op> op::Lookup for Lookup<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }
}

pub struct Getattr<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_getattr_in,
}

impl<'op> op::Getattr for Getattr<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> Option<u64> {
        if self.arg.getattr_flags & kernel::FUSE_GETATTR_FH != 0 {
            Some(self.arg.fh)
        } else {
            None
        }
    }
}

pub struct Setattr<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_setattr_in,
}

impl<'op> Setattr<'op> {
    #[inline(always)]
    fn get<R>(&self, flag: u32, f: impl FnOnce(&kernel::fuse_setattr_in) -> R) -> Option<R> {
        if self.arg.valid & flag != 0 {
            Some(f(&self.arg))
        } else {
            None
        }
    }
}

impl<'op> op::Setattr for Setattr<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    #[inline]
    fn fh(&self) -> Option<u64> {
        self.get(kernel::FATTR_FH, |arg| arg.fh)
    }

    #[inline]
    fn mode(&self) -> Option<u32> {
        self.get(kernel::FATTR_MODE, |arg| arg.mode)
    }

    #[inline]
    fn uid(&self) -> Option<u32> {
        self.get(kernel::FATTR_UID, |arg| arg.uid)
    }

    #[inline]
    fn gid(&self) -> Option<u32> {
        self.get(kernel::FATTR_GID, |arg| arg.gid)
    }

    #[inline]
    fn size(&self) -> Option<u64> {
        self.get(kernel::FATTR_SIZE, |arg| arg.size)
    }

    #[inline]
    fn atime(&self) -> Option<SetAttrTime> {
        self.get(kernel::FATTR_ATIME, |arg| {
            if arg.valid & kernel::FATTR_ATIME_NOW != 0 {
                SetAttrTime::Now
            } else {
                SetAttrTime::Timespec(Duration::new(arg.atime, arg.atimensec))
            }
        })
    }

    #[inline]
    fn mtime(&self) -> Option<SetAttrTime> {
        self.get(kernel::FATTR_MTIME, |arg| {
            if arg.valid & kernel::FATTR_MTIME_NOW != 0 {
                SetAttrTime::Now
            } else {
                SetAttrTime::Timespec(Duration::new(arg.mtime, arg.mtimensec))
            }
        })
    }

    #[inline]
    fn ctime(&self) -> Option<Duration> {
        self.get(kernel::FATTR_CTIME, |arg| {
            Duration::new(arg.ctime, arg.ctimensec)
        })
    }

    #[inline]
    fn lock_owner(&self) -> Option<LockOwner> {
        self.get(kernel::FATTR_LOCKOWNER, |arg| {
            LockOwner::from_raw(arg.lock_owner)
        })
    }
}

pub struct Readlink<'op> {
    header: &'op kernel::fuse_in_header,
}

impl<'op> op::Readlink for Readlink<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }
}

pub struct Symlink<'op> {
    header: &'op kernel::fuse_in_header,
    name: &'op OsStr,
    link: &'op OsStr,
}

impl<'op> op::Symlink for Symlink<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }

    fn link(&self) -> &OsStr {
        self.link
    }
}

pub struct Mknod<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_mknod_in,
    name: &'op OsStr,
}

impl<'op> op::Mknod for Mknod<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }

    fn mode(&self) -> u32 {
        self.arg.mode
    }

    fn rdev(&self) -> u32 {
        self.arg.rdev
    }

    fn umask(&self) -> u32 {
        self.arg.umask
    }
}

pub struct Mkdir<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_mkdir_in,
    name: &'op OsStr,
}

impl<'op> op::Mkdir for Mkdir<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }

    fn mode(&self) -> u32 {
        self.arg.mode
    }

    fn umask(&self) -> u32 {
        self.arg.umask
    }
}

pub struct Unlink<'op> {
    header: &'op kernel::fuse_in_header,
    name: &'op OsStr,
}

impl<'op> op::Unlink for Unlink<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }
}

pub struct Rmdir<'op> {
    header: &'op kernel::fuse_in_header,
    name: &'op OsStr,
}

impl<'op> op::Rmdir for Rmdir<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }
}

pub struct Rename<'op> {
    header: &'op kernel::fuse_in_header,
    arg: RenameArg<'op>,
    name: &'op OsStr,
    newname: &'op OsStr,
}

enum RenameArg<'op> {
    V1(&'op kernel::fuse_rename_in),
    V2(&'op kernel::fuse_rename2_in),
}

impl<'op> op::Rename for Rename<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }

    fn newparent(&self) -> u64 {
        match self.arg {
            RenameArg::V1(arg) => arg.newdir,
            RenameArg::V2(arg) => arg.newdir,
        }
    }

    fn newname(&self) -> &OsStr {
        self.newname
    }

    fn flags(&self) -> u32 {
        match self.arg {
            RenameArg::V1(..) => 0,
            RenameArg::V2(arg) => arg.flags,
        }
    }
}

pub struct Link<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_link_in,
    newname: &'op OsStr,
}

impl<'op> op::Link for Link<'op> {
    fn ino(&self) -> u64 {
        self.arg.oldnodeid
    }

    fn newparent(&self) -> u64 {
        self.header.nodeid
    }

    fn newname(&self) -> &OsStr {
        self.newname
    }
}

pub struct Open<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_open_in,
}

impl<'op> op::Open for Open<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn flags(&self) -> u32 {
        self.arg.flags
    }
}

pub struct Read<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_read_in,
}

impl<'op> op::Read for Read<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn offset(&self) -> u64 {
        self.arg.offset
    }

    fn size(&self) -> u32 {
        self.arg.size
    }

    fn flags(&self) -> u32 {
        self.arg.flags
    }

    fn lock_owner(&self) -> Option<LockOwner> {
        if self.arg.read_flags & kernel::FUSE_READ_LOCKOWNER != 0 {
            Some(LockOwner::from_raw(self.arg.lock_owner))
        } else {
            None
        }
    }
}

pub struct Write<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_write_in,
}

impl<'op> op::Write for Write<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn offset(&self) -> u64 {
        self.arg.offset
    }

    fn size(&self) -> u32 {
        self.arg.size
    }

    fn flags(&self) -> u32 {
        self.arg.flags
    }

    fn lock_owner(&self) -> Option<LockOwner> {
        if self.arg.write_flags & kernel::FUSE_WRITE_LOCKOWNER != 0 {
            Some(LockOwner::from_raw(self.arg.lock_owner))
        } else {
            None
        }
    }
}

pub struct Release<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_release_in,
}

impl<'op> op::Release for Release<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn flags(&self) -> u32 {
        self.arg.flags
    }

    fn lock_owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.lock_owner)
    }

    fn flush(&self) -> bool {
        self.arg.release_flags & kernel::FUSE_RELEASE_FLUSH != 0
    }

    fn flock_release(&self) -> bool {
        self.arg.release_flags & kernel::FUSE_RELEASE_FLOCK_UNLOCK != 0
    }
}

pub struct Statfs<'op> {
    header: &'op kernel::fuse_in_header,
}

impl<'op> op::Statfs for Statfs<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }
}

pub struct Fsync<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_fsync_in,
}

impl<'op> op::Fsync for Fsync<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn datasync(&self) -> bool {
        self.arg.fsync_flags & kernel::FUSE_FSYNC_FDATASYNC != 0
    }
}

pub struct Setxattr<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_setxattr_in,
    name: &'op OsStr,
    value: &'op [u8],
}

impl<'op> op::Setxattr for Setxattr<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }

    fn value(&self) -> &[u8] {
        self.value
    }

    fn flags(&self) -> u32 {
        self.arg.flags
    }
}

pub struct Getxattr<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_getxattr_in,
    name: &'op OsStr,
}

impl<'op> op::Getxattr for Getxattr<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }

    fn size(&self) -> u32 {
        self.arg.size
    }
}

pub struct Listxattr<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_getxattr_in,
}

impl<'op> op::Listxattr for Listxattr<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn size(&self) -> u32 {
        self.arg.size
    }
}

pub struct Removexattr<'op> {
    header: &'op kernel::fuse_in_header,
    name: &'op OsStr,
}

impl<'op> op::Removexattr for Removexattr<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }
}

pub struct Flush<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_flush_in,
}

impl<'op> op::Flush for Flush<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn lock_owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.lock_owner)
    }
}

pub struct Opendir<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_open_in,
}

impl<'op> op::Opendir for Opendir<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn flags(&self) -> u32 {
        self.arg.flags
    }
}

pub struct Readdir<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_read_in,
}

impl<'op> op::Readdir for Readdir<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn offset(&self) -> u64 {
        self.arg.offset
    }

    fn size(&self) -> u32 {
        self.arg.size
    }
}

pub struct Releasedir<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_release_in,
}

impl<'op> op::Releasedir for Releasedir<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn flags(&self) -> u32 {
        self.arg.flags
    }
}

pub struct Fsyncdir<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_fsync_in,
}

impl<'op> op::Fsyncdir for Fsyncdir<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn datasync(&self) -> bool {
        self.arg.fsync_flags & kernel::FUSE_FSYNC_FDATASYNC != 0
    }
}

pub struct Getlk<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_lk_in,
}

impl<'op> op::Getlk for Getlk<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.owner)
    }

    fn typ(&self) -> u32 {
        self.arg.lk.typ
    }

    fn start(&self) -> u64 {
        self.arg.lk.start
    }

    fn end(&self) -> u64 {
        self.arg.lk.end
    }

    fn pid(&self) -> u32 {
        self.arg.lk.pid
    }
}

pub struct Setlk<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_lk_in,
    sleep: bool,
}

impl<'op> op::Setlk for Setlk<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.owner)
    }

    fn typ(&self) -> u32 {
        self.arg.lk.typ
    }

    fn start(&self) -> u64 {
        self.arg.lk.start
    }

    fn end(&self) -> u64 {
        self.arg.lk.end
    }

    fn pid(&self) -> u32 {
        self.arg.lk.pid
    }

    fn sleep(&self) -> bool {
        self.sleep
    }
}

pub struct Flock<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_lk_in,
    op: u32,
}

impl<'op> op::Flock for Flock<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn owner(&self) -> LockOwner {
        LockOwner::from_raw(self.arg.owner)
    }

    fn op(&self) -> Option<u32> {
        Some(self.op)
    }
}

pub struct Access<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_access_in,
}

impl<'op> op::Access for Access<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn mask(&self) -> u32 {
        self.arg.mask
    }
}

pub struct Create<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_create_in,
    name: &'op OsStr,
}

impl<'op> op::Create for Create<'op> {
    fn parent(&self) -> u64 {
        self.header.nodeid
    }

    fn name(&self) -> &OsStr {
        self.name
    }

    fn mode(&self) -> u32 {
        self.arg.mode
    }

    fn open_flags(&self) -> u32 {
        self.arg.flags
    }

    fn umask(&self) -> u32 {
        self.arg.umask
    }
}

pub struct Bmap<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_bmap_in,
}

impl<'op> op::Bmap for Bmap<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn block(&self) -> u64 {
        self.arg.block
    }

    fn blocksize(&self) -> u32 {
        self.arg.blocksize
    }
}

pub struct Fallocate<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_fallocate_in,
}

impl<'op> op::Fallocate for Fallocate<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn offset(&self) -> u64 {
        self.arg.offset
    }

    fn length(&self) -> u64 {
        self.arg.length
    }

    fn mode(&self) -> u32 {
        self.arg.mode
    }
}

pub struct CopyFileRange<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_copy_file_range_in,
}

impl<'op> op::CopyFileRange for CopyFileRange<'op> {
    fn ino_in(&self) -> u64 {
        self.header.nodeid
    }

    fn fh_in(&self) -> u64 {
        self.arg.fh_in
    }

    fn offset_in(&self) -> u64 {
        self.arg.off_in
    }

    fn ino_out(&self) -> u64 {
        self.arg.nodeid_out
    }

    fn fh_out(&self) -> u64 {
        self.arg.fh_out
    }

    fn offset_out(&self) -> u64 {
        self.arg.off_out
    }

    fn length(&self) -> u64 {
        self.arg.len
    }

    fn flags(&self) -> u64 {
        self.arg.flags
    }
}

pub struct Poll<'op> {
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_poll_in,
}

impl<'op> op::Poll for Poll<'op> {
    fn ino(&self) -> u64 {
        self.header.nodeid
    }

    fn fh(&self) -> u64 {
        self.arg.fh
    }

    fn events(&self) -> u32 {
        self.arg.events
    }

    fn kh(&self) -> Option<u64> {
        if self.arg.flags & kernel::FUSE_POLL_SCHEDULE_NOTIFY != 0 {
            Some(self.arg.kh)
        } else {
            None
        }
    }
}

// === reply ====

#[derive(Debug)]
#[must_use]
pub struct Replied(());

#[inline]
fn cvt(res: io::Result<()>) -> Result<Replied, Error> {
    res.map(Replied).map_err(Error::reply)
}

pub struct ReplyAttr<'op, W> {
    sender: ReplySender<W>,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyAttr for ReplyAttr<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn send<F>(self, f: F) -> Result<Self::Ok, Self::Error>
    where
        F: FnOnce(&mut dyn reply::AttrOut),
    {
        let mut out = kernel::fuse_attr_out::default();
        f(&mut AttrOutFiller { out: &mut out });
        cvt(self.sender.reply(unsafe { as_bytes(&out) }))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyEntry<'op, W> {
    sender: ReplySender<W>,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyEntry for ReplyEntry<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn send<F>(self, f: F) -> Result<Self::Ok, Self::Error>
    where
        F: FnOnce(&mut dyn reply::EntryOut),
    {
        let mut out = kernel::fuse_entry_out::default();
        f(&mut EntryOutFiller { out: &mut out });
        cvt(self.sender.reply(unsafe { as_bytes(&out) }))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyReadlink<'op, W> {
    sender: ReplySender<W>,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyReadlink for ReplyReadlink<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn send<T>(self, link: T) -> Result<Self::Ok, Self::Error>
    where
        T: AsRef<OsStr> + Send + 'static,
    {
        cvt(self.sender.reply(link.as_ref()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyOk<'op, W> {
    sender: ReplySender<W>,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyOk for ReplyOk<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn send(self) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.reply(&[]))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyData<'op, W> {
    sender: ReplySender<W>,
    maxlen: usize,
    chunks: Vec<Box<dyn AsRef<[u8]> + Send>>,
    len: usize,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyData for ReplyData<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    #[inline]
    fn remaining(&self) -> usize {
        self.maxlen - self.len
    }

    #[inline]
    fn chunk<T>(&mut self, chunk: T) -> Option<T>
    where
        T: AsRef<[u8]> + Send + 'static,
    {
        let chunk_len = chunk.as_ref().len();
        if chunk_len > self.remaining() {
            return Some(chunk);
        }
        self.chunks.push(Box::new(chunk));
        None
    }

    fn send(self) -> Result<Self::Ok, Self::Error> {
        let data: Vec<&[u8]> = self.chunks.iter().map(|chunk| (**chunk).as_ref()).collect();
        cvt(self.sender.reply(data))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyOpen<'op, W> {
    sender: ReplySender<W>,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyOpen for ReplyOpen<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn send<F>(self, f: F) -> Result<Self::Ok, Self::Error>
    where
        F: FnOnce(&mut dyn reply::OpenOut),
    {
        let mut out = kernel::fuse_open_out::default();
        f(&mut OpenOutFiller { out: &mut out });

        cvt(self.sender.reply(unsafe { as_bytes(&out) }))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyWrite<'op, W> {
    sender: ReplySender<W>,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyWrite for ReplyWrite<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn send(self, size: u32) -> Result<Self::Ok, Self::Error> {
        let out = kernel::fuse_write_out {
            size,
            ..Default::default()
        };

        cvt(self.sender.reply(unsafe { as_bytes(&out) }))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyStatfs<'op, W> {
    sender: ReplySender<W>,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyStatfs for ReplyStatfs<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn send<F>(self, f: F) -> Result<Self::Ok, Self::Error>
    where
        F: FnOnce(&mut dyn reply::StatfsOut),
    {
        let mut out = kernel::fuse_statfs_out::default();
        f(&mut StatfsOutFiller { out: &mut out });

        cvt(self.sender.reply(unsafe { as_bytes(&out) }))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyXattrSize<'op, W> {
    sender: ReplySender<W>,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyXattrSize for ReplyXattrSize<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn send(self, size: u32) -> Result<Self::Ok, Self::Error> {
        let out = kernel::fuse_getxattr_out {
            size,
            ..Default::default()
        };

        cvt(self.sender.reply(unsafe { as_bytes(&out) }))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyXattrData<'op, W> {
    sender: ReplySender<W>,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyXattrData for ReplyXattrData<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn send<T>(self, data: T) -> Result<Self::Ok, Self::Error>
    where
        T: AsRef<[u8]> + Send + 'static,
    {
        cvt(self.sender.reply(data.as_ref()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyLk<'op, W> {
    sender: ReplySender<W>,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyLk for ReplyLk<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn send<F>(self, f: F) -> Result<Self::Ok, Self::Error>
    where
        F: FnOnce(&mut dyn reply::LkOut),
    {
        let mut out = kernel::fuse_lk_out::default();
        f(&mut LkOutFiller { out: &mut out });

        cvt(self.sender.reply(unsafe { as_bytes(&out) }))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyCreate<'op, W> {
    sender: ReplySender<W>,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyCreate for ReplyCreate<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn send<F>(self, f: F) -> Result<Self::Ok, Self::Error>
    where
        F: FnOnce(&mut dyn reply::EntryOut, &mut dyn reply::OpenOut),
    {
        let mut entry_out = kernel::fuse_entry_out::default();
        let mut open_out = kernel::fuse_open_out::default();
        f(
            &mut EntryOutFiller {
                out: &mut entry_out,
            },
            &mut OpenOutFiller { out: &mut open_out },
        );
        cvt(self
            .sender
            .reply(unsafe { &[as_bytes(&entry_out), as_bytes(&open_out)] as &[_] }))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyBmap<'op, W> {
    sender: ReplySender<W>,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyBmap for ReplyBmap<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn send(self, block: u64) -> Result<Self::Ok, Self::Error> {
        let out = kernel::fuse_bmap_out { block };
        cvt(self.sender.reply(unsafe { as_bytes(&out) }))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyPoll<'op, W> {
    sender: ReplySender<W>,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyPoll for ReplyPoll<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn send(self, revents: u32) -> Result<Self::Ok, Self::Error> {
        let out = kernel::fuse_poll_out {
            revents,
            ..Default::default()
        };

        cvt(self.sender.reply(unsafe { as_bytes(&out) }))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyDirs<'op, W> {
    sender: ReplySender<W>,
    maxlen: usize,
    entries: Vec<(kernel::fuse_dirent, Vec<u8>)>,
    len: usize,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyDirs for ReplyDirs<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn entry<F>(&mut self, name: &OsStr, f: F) -> bool
    where
        F: FnOnce(&mut dyn reply::DirEntry),
    {
        let name = name.as_bytes();
        let namelen = u32::try_from(name.len()).unwrap();

        let entlen = mem::size_of::<kernel::fuse_dirent>() + name.len();
        let entsize = aligned(entlen);
        let padlen = entsize - entlen;

        if self.len + entsize > self.maxlen {
            return true;
        }

        let mut dirent = kernel::fuse_dirent {
            namelen,
            name: [],
            ..Default::default()
        };
        f(&mut DirEntryOutFiller {
            dirent: &mut dirent,
        });

        let mut padded_name = Vec::with_capacity(name.len() + padlen);
        unsafe {
            ptr::copy_nonoverlapping(name.as_ptr(), padded_name.as_mut_ptr(), name.len());
            ptr::write_bytes(padded_name.as_mut_ptr().add(name.len()), 0, padlen);
            padded_name.set_len(name.len() + padlen);
        }

        self.entries.push((dirent, padded_name));
        self.len += entsize;

        false
    }

    fn send(self) -> Result<Self::Ok, Self::Error> {
        // FIXME: avoid redundant allocations.
        let data: Vec<_> = self
            .entries
            .iter()
            .flat_map(|(header, padded_name)| vec![unsafe { as_bytes(header) }, &padded_name[..]])
            .collect();
        cvt(self.sender.reply(data))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

pub struct ReplyDirsPlus<'op, W> {
    sender: ReplySender<W>,
    maxlen: usize,
    entries: Vec<(kernel::fuse_direntplus, Vec<u8>)>,
    len: usize,
    _marker: PhantomData<&'op ()>,
}

impl<'op, W> reply::ReplyDirsPlus for ReplyDirsPlus<'op, W>
where
    W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn entry<F>(&mut self, name: &OsStr, f: F) -> bool
    where
        F: FnOnce(&mut dyn reply::EntryOut, &mut dyn reply::DirEntry),
    {
        let name = name.as_bytes();
        let namelen = u32::try_from(name.len()).unwrap();

        let entlen = mem::size_of::<kernel::fuse_dirent>() + name.len();
        let entsize = aligned(entlen);
        let padlen = entsize - entlen;

        if self.len + entsize + mem::size_of::<kernel::fuse_entry_out>() > self.maxlen {
            return true;
        }

        let mut entry_header = kernel::fuse_direntplus {
            dirent: kernel::fuse_dirent {
                namelen,
                name: [],
                ..Default::default()
            },
            ..Default::default()
        };
        f(
            &mut EntryOutFiller {
                out: &mut entry_header.entry_out,
            },
            &mut DirEntryOutFiller {
                dirent: &mut entry_header.dirent,
            },
        );

        let mut padded_name = Vec::with_capacity(name.len() + padlen);
        padded_name.copy_from_slice(name);
        padded_name.resize(name.len() + padlen, 0);

        self.entries.push((entry_header, padded_name));
        self.len += entsize;

        false
    }

    fn send(self) -> Result<Self::Ok, Self::Error> {
        // FIXME: avoid redundant allocations.
        let mut data = Vec::with_capacity(self.entries.len() * 2);
        for (header, padded_name) in &self.entries {
            data.push(unsafe { as_bytes(header) });
            data.push(&padded_name[..]);
        }

        cvt(self.sender.reply(data))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        cvt(self.sender.error(code))
    }
}

struct AttrOutFiller<'a> {
    out: &'a mut kernel::fuse_attr_out,
}

impl reply::FileAttr for AttrOutFiller<'_> {
    fn ino(&mut self, ino: u64) {
        self.out.attr.ino = ino;
    }

    fn size(&mut self, size: u64) {
        self.out.attr.size = size;
    }

    fn mode(&mut self, mode: u32) {
        self.out.attr.mode = mode;
    }

    fn nlink(&mut self, nlink: u32) {
        self.out.attr.nlink = nlink;
    }

    fn uid(&mut self, uid: u32) {
        self.out.attr.uid = uid;
    }

    fn gid(&mut self, gid: u32) {
        self.out.attr.gid = gid;
    }

    fn rdev(&mut self, rdev: u32) {
        self.out.attr.rdev = rdev;
    }

    fn blksize(&mut self, blksize: u32) {
        self.out.attr.blksize = blksize;
    }

    fn blocks(&mut self, blocks: u64) {
        self.out.attr.blocks = blocks;
    }

    fn atime(&mut self, sec: u64, nsec: u32) {
        self.out.attr.atime = sec;
        self.out.attr.atimensec = nsec;
    }

    fn mtime(&mut self, sec: u64, nsec: u32) {
        self.out.attr.mtime = sec;
        self.out.attr.mtimensec = nsec;
    }

    fn ctime(&mut self, sec: u64, nsec: u32) {
        self.out.attr.ctime = sec;
        self.out.attr.ctimensec = nsec;
    }
}

impl reply::AttrOut for AttrOutFiller<'_> {
    fn attr(&mut self) -> &mut dyn reply::FileAttr {
        self
    }

    fn ttl(&mut self, ttl: Duration) {
        self.out.attr_valid = ttl.as_secs();
        self.out.attr_valid_nsec = ttl.subsec_nanos();
    }
}

struct EntryOutFiller<'a> {
    out: &'a mut kernel::fuse_entry_out,
}

impl reply::FileAttr for EntryOutFiller<'_> {
    fn ino(&mut self, ino: u64) {
        self.out.attr.ino = ino;
    }

    fn size(&mut self, size: u64) {
        self.out.attr.size = size;
    }

    fn mode(&mut self, mode: u32) {
        self.out.attr.mode = mode;
    }

    fn nlink(&mut self, nlink: u32) {
        self.out.attr.nlink = nlink;
    }

    fn uid(&mut self, uid: u32) {
        self.out.attr.uid = uid;
    }

    fn gid(&mut self, gid: u32) {
        self.out.attr.gid = gid;
    }

    fn rdev(&mut self, rdev: u32) {
        self.out.attr.rdev = rdev;
    }

    fn blksize(&mut self, blksize: u32) {
        self.out.attr.blksize = blksize;
    }

    fn blocks(&mut self, blocks: u64) {
        self.out.attr.blocks = blocks;
    }

    fn atime(&mut self, sec: u64, nsec: u32) {
        self.out.attr.atime = sec;
        self.out.attr.atimensec = nsec;
    }

    fn mtime(&mut self, sec: u64, nsec: u32) {
        self.out.attr.mtime = sec;
        self.out.attr.mtimensec = nsec;
    }

    fn ctime(&mut self, sec: u64, nsec: u32) {
        self.out.attr.ctime = sec;
        self.out.attr.ctimensec = nsec;
    }
}

impl reply::EntryOut for EntryOutFiller<'_> {
    fn attr(&mut self) -> &mut dyn reply::FileAttr {
        self
    }

    fn ino(&mut self, ino: u64) {
        self.out.nodeid = ino;
    }

    fn generation(&mut self, generation: u64) {
        self.out.generation = generation;
    }

    fn ttl_attr(&mut self, ttl: Duration) {
        self.out.attr_valid = ttl.as_secs();
        self.out.attr_valid_nsec = ttl.subsec_nanos();
    }

    fn ttl_entry(&mut self, ttl: Duration) {
        self.out.entry_valid = ttl.as_secs();
        self.out.entry_valid_nsec = ttl.subsec_nanos();
    }
}

struct OpenOutFiller<'a> {
    out: &'a mut kernel::fuse_open_out,
}

impl OpenOutFiller<'_> {
    fn set_flag(&mut self, flag: u32, enabled: bool) {
        if enabled {
            self.out.open_flags |= flag;
        } else {
            self.out.open_flags &= !flag;
        }
    }
}

impl reply::OpenOut for OpenOutFiller<'_> {
    fn fh(&mut self, fh: u64) {
        self.out.fh = fh;
    }

    fn direct_io(&mut self, enabled: bool) {
        self.set_flag(kernel::FOPEN_DIRECT_IO, enabled);
    }

    fn keep_cache(&mut self, enabled: bool) {
        self.set_flag(kernel::FOPEN_KEEP_CACHE, enabled);
    }

    fn nonseekable(&mut self, enabled: bool) {
        self.set_flag(kernel::FOPEN_NONSEEKABLE, enabled);
    }

    fn cache_dir(&mut self, enabled: bool) {
        self.set_flag(kernel::FOPEN_CACHE_DIR, enabled);
    }
}

struct StatfsOutFiller<'a> {
    out: &'a mut kernel::fuse_statfs_out,
}

impl reply::Statfs for StatfsOutFiller<'_> {
    fn bsize(&mut self, bsize: u32) {
        self.out.st.bsize = bsize;
    }

    fn frsize(&mut self, frsize: u32) {
        self.out.st.frsize = frsize;
    }

    fn blocks(&mut self, blocks: u64) {
        self.out.st.blocks = blocks;
    }

    fn bfree(&mut self, bfree: u64) {
        self.out.st.bfree = bfree;
    }

    fn bavail(&mut self, bavail: u64) {
        self.out.st.bavail = bavail;
    }

    fn files(&mut self, files: u64) {
        self.out.st.files = files;
    }

    fn ffree(&mut self, ffree: u64) {
        self.out.st.ffree = ffree;
    }

    fn namelen(&mut self, namelen: u32) {
        self.out.st.namelen = namelen;
    }
}

impl reply::StatfsOut for StatfsOutFiller<'_> {
    #[inline]
    fn statfs(&mut self) -> &mut dyn reply::Statfs {
        self
    }
}

struct LkOutFiller<'a> {
    out: &'a mut kernel::fuse_lk_out,
}

impl reply::FileLock for LkOutFiller<'_> {
    fn typ(&mut self, typ: u32) {
        self.out.lk.typ = typ;
    }

    fn start(&mut self, start: u64) {
        self.out.lk.start = start;
    }

    fn end(&mut self, end: u64) {
        self.out.lk.end = end;
    }

    fn pid(&mut self, pid: u32) {
        self.out.lk.pid = pid;
    }
}

impl reply::LkOut for LkOutFiller<'_> {
    fn flock(&mut self) -> &mut dyn reply::FileLock {
        self
    }
}

struct DirEntryOutFiller<'a> {
    dirent: &'a mut kernel::fuse_dirent,
}

impl reply::DirEntry for DirEntryOutFiller<'_> {
    fn ino(&mut self, ino: u64) {
        self.dirent.ino = ino;
    }

    fn typ(&mut self, typ: u32) {
        self.dirent.typ = typ;
    }

    fn offset(&mut self, offset: u64) {
        self.dirent.off = offset;
    }
}

#[inline]
const fn aligned(len: usize) -> usize {
    (len + mem::size_of::<u64>() - 1) & !(mem::size_of::<u64>() - 1)
}
