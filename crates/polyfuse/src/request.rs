//! Components used when processing FUSE requests.

use crate::{
    op::{self, SetAttrTime},
    reply::{self, EntryOptions, OpenOptions},
    session::Session,
    types::{FileAttr, FsStatistics, LockOwner},
    util::{as_bytes, Decoder},
    write,
};
use polyfuse_kernel::{self as kernel, fuse_opcode};
use std::{
    convert::TryFrom, ffi::OsStr, fmt, io, mem, os::unix::prelude::*, ptr, sync::Arc,
    time::Duration,
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
    pub fn operation<'op, W: ?Sized>(&'op self, writer: &'op W) -> Result<Operation<'op, W>, Error>
    where
        &'op W: io::Write,
    {
        if self.session.exited() {
            return Ok(Operation::Null);
        }

        let mut decoder = Decoder::new(&self.buf[..]);

        let header = decoder
            .fetch::<kernel::fuse_in_header>()
            .ok_or_else(Error::decode)?;

        let reply_entry = || ReplyEntry {
            writer,
            header,
            arg: kernel::fuse_entry_out::default(),
        };

        let reply_attr = || ReplyAttr {
            writer,
            header,
            arg: kernel::fuse_attr_out::default(),
        };

        let reply_ok = || ReplyOk { writer, header };

        let reply_data = || ReplyData { writer, header };

        let reply_open = || ReplyOpen {
            writer,
            header,
            arg: kernel::fuse_open_out::default(),
        };

        let reply_write = || ReplyWrite {
            writer,
            header,
            arg: Default::default(),
        };

        let reply_statfs = || ReplyStatfs {
            writer,
            header,
            arg: Default::default(),
        };

        let reply_xattr = || ReplyXattr {
            writer,
            header,
            arg: Default::default(),
        };

        let reply_create = || ReplyCreate {
            writer,
            header,
            arg: Default::default(),
        };

        let reply_lk = || ReplyLk {
            writer,
            header,
            arg: Default::default(),
        };

        let reply_bmap = || ReplyBmap {
            writer,
            header,
            arg: Default::default(),
        };

        let reply_poll = || ReplyPoll {
            writer,
            header,
            arg: Default::default(),
        };

        let reply_dirs = |arg| ReplyDirs {
            writer,
            header,
            arg,
            entries: vec![],
            buflen: 0,
        };

        let reply_dirs_plus = |arg| ReplyDirsPlus {
            writer,
            header,
            arg,
            entries: vec![],
            buflen: 0,
        };

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
                    reply: reply_entry(),
                })
            }

            Some(fuse_opcode::FUSE_GETATTR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Getattr {
                    op: Getattr { header, arg },
                    reply: reply_attr(),
                })
            }

            Some(fuse_opcode::FUSE_SETATTR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Setattr {
                    op: Setattr { header, arg },
                    reply: reply_attr(),
                })
            }

            Some(fuse_opcode::FUSE_READLINK) => Ok(Operation::Readlink {
                op: Readlink { header },
                reply: reply_data(),
            }),

            Some(fuse_opcode::FUSE_SYMLINK) => {
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                let link = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Symlink {
                    op: Symlink { header, name, link },
                    reply: reply_entry(),
                })
            }

            Some(fuse_opcode::FUSE_MKNOD) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Mknod {
                    op: Mknod { header, arg, name },
                    reply: reply_entry(),
                })
            }

            Some(fuse_opcode::FUSE_MKDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Mkdir {
                    op: Mkdir { header, arg, name },
                    reply: reply_entry(),
                })
            }

            Some(fuse_opcode::FUSE_UNLINK) => {
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Unlink {
                    op: Unlink { header, name },
                    reply: reply_ok(),
                })
            }

            Some(fuse_opcode::FUSE_RMDIR) => {
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Rmdir {
                    op: Rmdir { header, name },
                    reply: reply_ok(),
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
                    reply: reply_ok(),
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
                    reply: reply_ok(),
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
                    reply: reply_entry(),
                })
            }

            Some(fuse_opcode::FUSE_OPEN) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Open {
                    op: Open { header, arg },
                    reply: reply_open(),
                })
            }

            Some(fuse_opcode::FUSE_READ) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Read {
                    op: Read { header, arg },
                    reply: reply_data(),
                })
            }

            Some(fuse_opcode::FUSE_WRITE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Write {
                    op: Write { header, arg },
                    reply: reply_write(),
                })
            }

            Some(fuse_opcode::FUSE_RELEASE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Release {
                    op: Release { header, arg },
                    reply: reply_ok(),
                })
            }

            Some(fuse_opcode::FUSE_STATFS) => Ok(Operation::Statfs {
                op: Statfs { header },
                reply: reply_statfs(),
            }),

            Some(fuse_opcode::FUSE_FSYNC) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Fsync {
                    op: Fsync { header, arg },
                    reply: reply_ok(),
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
                    reply: reply_ok(),
                })
            }

            Some(fuse_opcode::FUSE_GETXATTR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Getxattr {
                    op: Getxattr { header, arg, name },
                    reply: reply_xattr(),
                })
            }

            Some(fuse_opcode::FUSE_LISTXATTR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Listxattr {
                    op: Listxattr { header, arg },
                    reply: reply_xattr(),
                })
            }

            Some(fuse_opcode::FUSE_REMOVEXATTR) => {
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Removexattr {
                    op: Removexattr { header, name },
                    reply: reply_ok(),
                })
            }

            Some(fuse_opcode::FUSE_FLUSH) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Flush {
                    op: Flush { header, arg },
                    reply: reply_ok(),
                })
            }

            Some(fuse_opcode::FUSE_OPENDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Opendir {
                    op: Opendir { header, arg },
                    reply: reply_open(),
                })
            }

            Some(fuse_opcode::FUSE_READDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Readdir {
                    op: Readdir { header, arg },
                    reply: reply_dirs(arg),
                })
            }
            Some(fuse_opcode::FUSE_READDIRPLUS) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Readdirplus {
                    op: Readdir { header, arg },
                    reply: reply_dirs_plus(arg),
                })
            }

            Some(fuse_opcode::FUSE_RELEASEDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Releasedir {
                    op: Releasedir { header, arg },
                    reply: reply_ok(),
                })
            }

            Some(fuse_opcode::FUSE_FSYNCDIR) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Fsyncdir {
                    op: Fsyncdir { header, arg },
                    reply: reply_ok(),
                })
            }

            Some(fuse_opcode::FUSE_GETLK) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Getlk {
                    op: Getlk { header, arg },
                    reply: reply_lk(),
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
                        reply: reply_ok(),
                    })
                } else {
                    let op = convert_to_flock_op(arg.lk.typ, sleep).unwrap_or(0);
                    Ok(Operation::Flock {
                        op: Flock { header, arg, op },
                        reply: reply_ok(),
                    })
                }
            }

            Some(fuse_opcode::FUSE_ACCESS) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Access {
                    op: Access { header, arg },
                    reply: reply_ok(),
                })
            }

            Some(fuse_opcode::FUSE_CREATE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                let name = decoder.fetch_str().ok_or_else(Error::decode)?;
                Ok(Operation::Create {
                    op: Create { header, arg, name },
                    reply: reply_create(),
                })
            }

            Some(fuse_opcode::FUSE_BMAP) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Bmap {
                    op: Bmap { header, arg },
                    reply: reply_bmap(),
                })
            }

            Some(fuse_opcode::FUSE_FALLOCATE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Fallocate {
                    op: Fallocate { header, arg },
                    reply: reply_ok(),
                })
            }

            Some(fuse_opcode::FUSE_COPY_FILE_RANGE) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::CopyFileRange {
                    op: CopyFileRange { header, arg },
                    reply: reply_write(),
                })
            }

            Some(fuse_opcode::FUSE_POLL) => {
                let arg = decoder.fetch().ok_or_else(Error::decode)?;
                Ok(Operation::Poll {
                    op: Poll { header, arg },
                    reply: reply_poll(),
                })
            }

            _ => {
                tracing::warn!("unsupported opcode: {}", header.opcode);
                Ok(Operation::Unsupported(Unsupported { header, writer }))
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
pub enum Operation<'op, W: ?Sized> {
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
        reply: ReplyData<'op, W>,
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
    Getxattr {
        op: Getxattr<'op>,
        reply: ReplyXattr<'op, W>,
    },
    Listxattr {
        op: Listxattr<'op>,
        reply: ReplyXattr<'op, W>,
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

impl<'op, W: ?Sized> Operation<'op, W>
where
    &'op W: io::Write,
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
            Getxattr,
            Listxattr,
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
pub struct Unsupported<'op, W: ?Sized> {
    header: &'op kernel::fuse_in_header,
    writer: &'op W,
}

impl<'op, W: ?Sized> Unsupported<'op, W>
where
    &'op W: io::Write,
{
    fn send_error(self) -> Result<Replied, Error> {
        write::send_error(self.writer, self.header.unique, libc::ENOSYS).map_err(Error::reply)?;
        Ok(Replied(()))
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

pub struct ReplyAttr<'op, W: ?Sized> {
    writer: &'op W,
    header: &'op kernel::fuse_in_header,
    arg: kernel::fuse_attr_out,
}

impl<'op, W: ?Sized> reply::ReplyAttr for ReplyAttr<'op, W>
where
    &'op W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn attr<T>(mut self, attr: T, ttl: Option<Duration>) -> Result<Self::Ok, Self::Error>
    where
        T: FileAttr,
    {
        fill_attr_out(&mut self.arg, attr, ttl);

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::reply)?;

        Ok(Replied(()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        write::send_error(self.writer, self.header.unique, code).map_err(Error::reply)?;
        Ok(Replied(()))
    }
}

pub struct ReplyEntry<'op, W: ?Sized> {
    writer: &'op W,
    header: &'op kernel::fuse_in_header,
    arg: kernel::fuse_entry_out,
}

impl<'op, W: ?Sized> reply::ReplyEntry for ReplyEntry<'op, W>
where
    &'op W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn entry<T>(mut self, attr: T, opts: &EntryOptions) -> Result<Self::Ok, Self::Error>
    where
        T: FileAttr,
    {
        fill_entry_out(&mut self.arg, attr, opts);

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::reply)?;

        Ok(Replied(()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        write::send_error(self.writer, self.header.unique, code).map_err(Error::reply)?;
        Ok(Replied(()))
    }
}

pub struct ReplyOk<'op, W: ?Sized> {
    writer: &'op W,
    header: &'op kernel::fuse_in_header,
}

impl<'op, W: ?Sized> reply::ReplyOk for ReplyOk<'op, W>
where
    &'op W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn ok(self) -> Result<Self::Ok, Self::Error> {
        write::send_reply(self.writer, self.header.unique, &[]).map_err(Error::reply)?;
        Ok(Replied(()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        write::send_error(self.writer, self.header.unique, code).map_err(Error::reply)?;
        Ok(Replied(()))
    }
}

pub struct ReplyData<'op, W: ?Sized> {
    writer: &'op W,
    header: &'op kernel::fuse_in_header,
}

impl<'op, W: ?Sized> reply::ReplyData for ReplyData<'op, W>
where
    &'op W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn data<T>(self, data: T) -> Result<Self::Ok, Self::Error>
    where
        T: AsRef<[u8]>,
    {
        write::send_reply(self.writer, self.header.unique, data.as_ref()).map_err(Error::reply)?;

        Ok(Replied(()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        write::send_error(self.writer, self.header.unique, code).map_err(Error::reply)?;
        Ok(Replied(()))
    }
}

pub struct ReplyOpen<'op, W: ?Sized> {
    writer: &'op W,
    header: &'op kernel::fuse_in_header,
    arg: kernel::fuse_open_out,
}

impl<'op, W: ?Sized> reply::ReplyOpen for ReplyOpen<'op, W>
where
    &'op W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn open(mut self, fh: u64, opts: &OpenOptions) -> Result<Self::Ok, Self::Error> {
        fill_open_out(&mut self.arg, fh, opts);

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::reply)?;

        Ok(Replied(()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        write::send_error(self.writer, self.header.unique, code).map_err(Error::reply)?;
        Ok(Replied(()))
    }
}

pub struct ReplyWrite<'op, W: ?Sized> {
    writer: &'op W,
    header: &'op kernel::fuse_in_header,
    arg: kernel::fuse_write_out,
}

impl<'op, W: ?Sized> reply::ReplyWrite for ReplyWrite<'op, W>
where
    &'op W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn size(mut self, size: u32) -> Result<Self::Ok, Self::Error> {
        self.arg.size = size;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::reply)?;

        Ok(Replied(()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        write::send_error(self.writer, self.header.unique, code).map_err(Error::reply)?;
        Ok(Replied(()))
    }
}

pub struct ReplyStatfs<'op, W: ?Sized> {
    writer: &'op W,
    header: &'op kernel::fuse_in_header,
    arg: kernel::fuse_statfs_out,
}

impl<'op, W: ?Sized> reply::ReplyStatfs for ReplyStatfs<'op, W>
where
    &'op W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn stat<S>(mut self, stat: S) -> Result<Self::Ok, Self::Error>
    where
        S: FsStatistics,
    {
        fill_statfs(&mut self.arg.st, stat);

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::reply)?;

        Ok(Replied(()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        write::send_error(self.writer, self.header.unique, code).map_err(Error::reply)?;
        Ok(Replied(()))
    }
}

pub struct ReplyXattr<'op, W: ?Sized> {
    writer: &'op W,
    header: &'op kernel::fuse_in_header,
    arg: kernel::fuse_getxattr_out,
}

impl<'op, W: ?Sized> reply::ReplyXattr for ReplyXattr<'op, W>
where
    &'op W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn size(mut self, size: u32) -> Result<Self::Ok, Self::Error> {
        self.arg.size = size;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::reply)?;

        Ok(Replied(()))
    }

    fn data<T>(self, data: T) -> Result<Self::Ok, Self::Error>
    where
        T: AsRef<[u8]>,
    {
        write::send_reply(self.writer, self.header.unique, data.as_ref()).map_err(Error::reply)?;

        Ok(Replied(()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        write::send_error(self.writer, self.header.unique, code).map_err(Error::reply)?;
        Ok(Replied(()))
    }
}

pub struct ReplyLk<'op, W: ?Sized> {
    writer: &'op W,
    header: &'op kernel::fuse_in_header,
    arg: kernel::fuse_lk_out,
}

impl<'op, W: ?Sized> reply::ReplyLk for ReplyLk<'op, W>
where
    &'op W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn lk(mut self, typ: u32, start: u64, end: u64, pid: u32) -> Result<Self::Ok, Self::Error> {
        self.arg.lk.typ = typ;
        self.arg.lk.start = start;
        self.arg.lk.end = end;
        self.arg.lk.pid = pid;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::reply)?;

        Ok(Replied(()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        write::send_error(self.writer, self.header.unique, code).map_err(Error::reply)?;
        Ok(Replied(()))
    }
}

pub struct ReplyCreate<'op, W: ?Sized> {
    writer: &'op W,
    header: &'op kernel::fuse_in_header,
    arg: CreateArg,
}

#[derive(Default)]
#[repr(C)]
struct CreateArg {
    entry_out: kernel::fuse_entry_out,
    open_out: kernel::fuse_open_out,
}

impl<'op, W: ?Sized> reply::ReplyCreate for ReplyCreate<'op, W>
where
    &'op W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn create<T>(
        mut self,
        fh: u64,
        attr: T,
        entry_opts: &EntryOptions,
        open_opts: &OpenOptions,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: FileAttr,
    {
        fill_entry_out(&mut self.arg.entry_out, attr, entry_opts);
        fill_open_out(&mut self.arg.open_out, fh, open_opts);

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::reply)?;

        Ok(Replied(()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        write::send_error(self.writer, self.header.unique, code).map_err(Error::reply)?;
        Ok(Replied(()))
    }
}

pub struct ReplyBmap<'op, W: ?Sized> {
    writer: &'op W,
    header: &'op kernel::fuse_in_header,
    arg: kernel::fuse_bmap_out,
}

impl<'op, W: ?Sized> reply::ReplyBmap for ReplyBmap<'op, W>
where
    &'op W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn block(mut self, block: u64) -> Result<Self::Ok, Self::Error> {
        self.arg.block = block;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::reply)?;

        Ok(Replied(()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        write::send_error(self.writer, self.header.unique, code).map_err(Error::reply)?;
        Ok(Replied(()))
    }
}

pub struct ReplyPoll<'op, W: ?Sized> {
    writer: &'op W,
    header: &'op kernel::fuse_in_header,
    arg: kernel::fuse_poll_out,
}

impl<'op, W: ?Sized> reply::ReplyPoll for ReplyPoll<'op, W>
where
    &'op W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn revents(mut self, revents: u32) -> Result<Self::Ok, Self::Error> {
        self.arg.revents = revents;

        write::send_reply(self.writer, self.header.unique, unsafe {
            as_bytes(&self.arg)
        })
        .map_err(Error::reply)?;

        Ok(Replied(()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        write::send_error(self.writer, self.header.unique, code).map_err(Error::reply)?;
        Ok(Replied(()))
    }
}

pub struct ReplyDirs<'op, W: ?Sized> {
    writer: &'op W,
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_read_in,
    entries: Vec<(kernel::fuse_dirent, Vec<u8>)>,
    buflen: usize,
}

impl<'op, W: ?Sized> reply::ReplyDirs for ReplyDirs<'op, W>
where
    &'op W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn add(&mut self, ino: u64, typ: u32, name: &OsStr, offset: u64) -> bool {
        let name = name.as_bytes();
        let namelen = u32::try_from(name.len()).unwrap();

        let entlen = mem::size_of::<kernel::fuse_dirent>() + name.len();
        let entsize = aligned(entlen);
        let padlen = entsize - entlen;

        if self.buflen + entsize > self.arg.size as usize {
            return true;
        }

        let dirent = kernel::fuse_dirent {
            off: offset,
            ino,
            typ,
            namelen,
            name: [],
        };

        let mut padded_name = Vec::with_capacity(name.len() + padlen);
        unsafe {
            ptr::copy_nonoverlapping(name.as_ptr(), padded_name.as_mut_ptr(), name.len());
            ptr::write_bytes(padded_name.as_mut_ptr().add(name.len()), 0, padlen);
            padded_name.set_len(name.len() + padlen);
        }

        self.entries.push((dirent, padded_name));
        self.buflen += entsize;

        false
    }

    fn send(self) -> Result<Self::Ok, Self::Error> {
        // FIXME: avoid redundant allocations.
        let data: Vec<_> = self
            .entries
            .iter()
            .flat_map(|(header, padded_name)| vec![unsafe { as_bytes(header) }, &padded_name[..]])
            .collect();

        write::send_reply(self.writer, self.header.unique, data) //
            .map_err(Error::reply)?;

        Ok(Replied(()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        write::send_error(self.writer, self.header.unique, code).map_err(Error::reply)?;
        Ok(Replied(()))
    }
}

pub struct ReplyDirsPlus<'op, W: ?Sized> {
    writer: &'op W,
    header: &'op kernel::fuse_in_header,
    arg: &'op kernel::fuse_read_in,
    entries: Vec<(kernel::fuse_dirent, Vec<u8>, kernel::fuse_entry_out)>,
    buflen: usize,
}

impl<'op, W: ?Sized> reply::ReplyDirsPlus for ReplyDirsPlus<'op, W>
where
    &'op W: io::Write,
{
    type Ok = Replied;
    type Error = Error;

    fn add<A>(
        &mut self,
        ino: u64,
        typ: u32,
        name: &OsStr,
        offset: u64,
        attr: A,
        opts: &EntryOptions,
    ) -> bool
    where
        A: FileAttr,
    {
        let name = name.as_bytes();
        let namelen = u32::try_from(name.len()).unwrap();

        let entlen = mem::size_of::<kernel::fuse_dirent>() + name.len();
        let entsize = aligned(entlen);
        let padlen = entsize - entlen;

        if self.buflen + entsize + mem::size_of::<kernel::fuse_entry_out>() > self.arg.size as usize
        {
            return true;
        }

        let dirent = kernel::fuse_dirent {
            off: offset,
            ino,
            typ,
            namelen,
            name: [],
        };

        let mut padded_name = Vec::with_capacity(name.len() + padlen);
        padded_name.copy_from_slice(name);
        padded_name.resize(name.len() + padlen, 0);

        let mut entry_out = kernel::fuse_entry_out::default();
        fill_entry_out(&mut entry_out, attr, opts);

        self.entries.push((dirent, padded_name, entry_out));
        self.buflen += entsize;

        false
    }

    fn send(self) -> Result<Self::Ok, Self::Error> {
        // FIXME: avoid redundant allocations.
        let data: Vec<_> = self
            .entries
            .iter()
            .flat_map(|(header, padded_name, entry_out)| {
                vec![unsafe { as_bytes(header) }, &padded_name[..], unsafe {
                    as_bytes(entry_out)
                }]
            })
            .collect();

        write::send_reply(self.writer, self.header.unique, data) //
            .map_err(Error::reply)?;

        Ok(Replied(()))
    }

    fn error(self, code: i32) -> Result<Self::Ok, Self::Error> {
        write::send_error(self.writer, self.header.unique, code).map_err(Error::reply)?;
        Ok(Replied(()))
    }
}

fn fill_attr_out(dst: &mut kernel::fuse_attr_out, attr: impl FileAttr, ttl: Option<Duration>) {
    fill_attr(&mut dst.attr, attr);

    if let Some(ttl) = ttl {
        dst.attr_valid = ttl.as_secs();
        dst.attr_valid_nsec = ttl.subsec_nanos();
    } else {
        dst.attr_valid = u64::MAX;
        dst.attr_valid_nsec = u32::MAX;
    }
}

fn fill_entry_out(dst: &mut kernel::fuse_entry_out, attr: impl FileAttr, opts: &EntryOptions) {
    fill_attr(&mut dst.attr, attr);

    dst.nodeid = opts.ino;
    dst.generation = opts.generation;

    if let Some(ttl) = opts.ttl_attr {
        dst.attr_valid = ttl.as_secs();
        dst.attr_valid_nsec = ttl.subsec_nanos();
    } else {
        dst.attr_valid = u64::MAX;
        dst.attr_valid_nsec = u32::MAX;
    }

    if let Some(ttl) = opts.ttl_entry {
        dst.entry_valid = ttl.as_secs();
        dst.entry_valid_nsec = ttl.subsec_nanos();
    } else {
        dst.entry_valid = u64::MAX;
        dst.entry_valid_nsec = u32::MAX;
    }
}

fn fill_open_out(dst: &mut kernel::fuse_open_out, fh: u64, opts: &OpenOptions) {
    dst.fh = fh;

    if opts.direct_io {
        dst.open_flags |= kernel::FOPEN_DIRECT_IO;
    }

    if opts.keep_cache {
        dst.open_flags |= kernel::FOPEN_KEEP_CACHE;
    }

    if opts.nonseekable {
        dst.open_flags |= kernel::FOPEN_NONSEEKABLE;
    }

    if opts.cache_dir {
        dst.open_flags |= kernel::FOPEN_CACHE_DIR;
    }
}

fn fill_attr(dst: &mut kernel::fuse_attr, src: impl FileAttr) {
    dst.ino = src.ino();
    dst.size = src.size();
    dst.mode = src.mode();
    dst.nlink = src.nlink();
    dst.uid = src.uid();
    dst.gid = src.gid();
    dst.rdev = src.rdev();
    dst.blksize = src.blksize();
    dst.blocks = src.blocks();
    dst.atime = src.atime();
    dst.atimensec = src.atime_nsec();
    dst.mtime = src.mtime();
    dst.mtimensec = src.mtime_nsec();
    dst.ctime = src.ctime();
    dst.ctimensec = src.ctime_nsec();
}

fn fill_statfs(dst: &mut kernel::fuse_kstatfs, src: impl FsStatistics) {
    dst.bsize = src.bsize();
    dst.frsize = src.frsize();
    dst.blocks = src.blocks();
    dst.bfree = src.bfree();
    dst.bavail = src.bavail();
    dst.files = src.files();
    dst.ffree = src.ffree();
    dst.namelen = src.namelen();
}

#[inline]
const fn aligned(len: usize) -> usize {
    (len + mem::size_of::<u64>() - 1) & !(mem::size_of::<u64>() - 1)
}
