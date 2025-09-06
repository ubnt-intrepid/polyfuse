use crate::{
    bytes::{DecodeError, Decoder},
    conn::SpliceRead,
    nix::{Pipe, PipeReader},
    op::{
        self, Forget, OpenOptions, Operation, ReaddirMode, RenameFlags, SetAttrTime, SetxattrFlags,
    },
    types::{
        DeviceID, FileID, FileMode, FilePermissions, LockOwnerID, NodeID, NotifyID, RequestID, GID,
        PID, UID,
    },
};
use libc::{EINTR, ENODEV, ENOENT};
use polyfuse_kernel::*;
use std::{
    ffi::OsStr,
    io::{self, prelude::*},
    marker::PhantomData,
    mem,
    time::Duration,
};
use zerocopy::IntoBytes as _;

const FUSE_INT_REQ_BIT: u64 = 1;

// MEMO: FUSE_MIN_READ_BUFFER に到達する or 超える可能性のある opcode の一覧 (要調査)
// * FUSE_BATCH_FORGET
//    - read buffer ギリギリまで fuse_forget_one を詰めようとする
//    - 超えることはないが、場合によっては大量にメモリコピーが発生する可能性がある
// * FUSE_WRITE / FUSE_NOTIFY_REPLY
//    - これらは当然あり得る
// * FUSE_SETXATTR
//    - name については XATTR_NAME_MAX=256 で制限されているので良い
//    - value 側が非常に大きくなる可能性がある
// * その他
//   - 基本的には NAME_MAX=256, PATH_MAX=4096 による制限があるので、8096 bytes に到達することはないはず

/// The buffer to store a processing FUSE request received from the kernel driver.
pub struct RequestBuffer {
    header: fuse_in_header,
    opcode: Option<fuse_opcode>,
    mode: ReadMode,
}

enum ReadMode {
    Splice {
        // MEMO:
        // * 再アロケートされる可能性があるので Vec<u8> で持つ
        // * デフォルトの system allocator を使用している限りは alignment の心配をする必要は基本的はないはず (malloc依存)
        arg: Vec<u8>,
        pipe: Pipe,
        bufsize: usize,
    },
    Fallback {
        // どうせ再アロケートすることはないので、最初に確保した分で固定してしまう
        arg: Box<[u8]>,
        pos: usize,
    },
}

impl RequestBuffer {
    pub fn new_splice(bufsize: usize) -> io::Result<Self> {
        Ok(Self {
            header: fuse_in_header::default(),
            opcode: None,
            mode: ReadMode::Splice {
                arg: {
                    let capacity = FUSE_MIN_READ_BUFFER as usize - mem::size_of::<fuse_in_header>();
                    let mut vec = vec![0; capacity]; // ensure that the underlying buffer is zeroed.
                    vec.truncate(0);
                    vec
                },
                pipe: crate::nix::pipe()?,
                bufsize,
            },
        })
    }

    pub fn new_fallback(bufsize: usize) -> io::Result<Self> {
        Ok(Self {
            header: fuse_in_header::default(),
            opcode: None,
            mode: ReadMode::Fallback {
                arg: vec![0u8; bufsize - mem::size_of::<fuse_in_header>()].into_boxed_slice(),
                pos: 0,
            },
        })
    }

    /// Return the unique ID of the request.
    #[inline]
    pub fn unique(&self) -> RequestID {
        RequestID::from_raw(self.header.unique)
    }

    /// Return the user ID of the calling process.
    #[inline]
    pub fn uid(&self) -> UID {
        UID::from_raw(self.header.uid)
    }

    /// Return the group ID of the calling process.
    #[inline]
    pub fn gid(&self) -> GID {
        GID::from_raw(self.header.gid)
    }

    /// Return the process ID of the calling process.
    #[inline]
    pub fn pid(&self) -> PID {
        PID::from_raw(self.header.pid)
    }

    pub(crate) fn opcode(&self) -> fuse_opcode {
        self.opcode.expect("The request has not been received yet")
    }

    /// Decode the argument of this request.
    pub fn operation(&self) -> Result<(Operation<impl op::Op + '_>, RemainingData<'_>), Error> {
        let (arg, data) = match &self.mode {
            ReadMode::Splice { arg, pipe, .. } => (&arg[..], RemainingData::Splice(&pipe.reader)),
            ReadMode::Fallback { arg, pos } => {
                let (arg, remaining) = arg.split_at(*pos);
                (arg, RemainingData::Fallback(remaining))
            }
        };
        let op = decode(&self.header, self.opcode(), arg)?;
        Ok((op, data))
    }

    pub(crate) fn clear(&mut self) -> io::Result<()> {
        self.header = fuse_in_header::default(); // opcode=0 means that the buffer is not valid.
        match &mut self.mode {
            ReadMode::Splice { arg, pipe, .. } => {
                arg.truncate(0);
                if pipe.reader.remaining_bytes()? > 0 {
                    tracing::warn!(
                        "The remaining data of request(unique={}) is destroyed",
                        self.header.unique
                    );
                    let _ = mem::replace(pipe, crate::nix::pipe()?);
                }
            }
            ReadMode::Fallback { pos, .. } => {
                *pos = 0;
            }
        }
        Ok(())
    }

    pub(crate) fn try_receive<T>(&mut self, mut conn: T) -> Result<(), ReceiveError>
    where
        T: SpliceRead,
    {
        match &mut self.mode {
            ReadMode::Splice { arg, pipe, bufsize } => {
                tracing::debug!("use splice(2)");
                let len = conn
                    .splice_read(&pipe.writer, *bufsize)
                    .map_err(ReceiveError::from_read_operation)?;

                if len < mem::size_of::<fuse_in_header>() {
                    Err(ReceiveError::invalid_data(
                        "dequeued request message is too short",
                    ))?
                }
                pipe.reader.read_exact(self.header.as_mut_bytes())?;

                if len != self.header.len as usize {
                    Err(ReceiveError::invalid_data(
                        "The value in_header.len is mismatched to the result of splice(2)",
                    ))?
                }

                let opcode = fuse_opcode::try_from(self.header.opcode)
                    .map_err(|_| ReceiveError::UnrecognizedOpcode(self.header.opcode))?;
                self.opcode = Some(opcode);

                let arglen = arg_len(&self.header, opcode);
                arg.resize(arglen, 0); // MEMO: FUSE_SETXATTR において、非常に大きいサイズの値が設定されたときに再アロケートされる可能性がある
                pipe.reader.read_exact(&mut arg[..])?;
            }

            ReadMode::Fallback { arg, pos } => {
                tracing::debug!("use fallback");
                let len = conn
                    .read_vectored(&mut [
                        io::IoSliceMut::new(self.header.as_mut_bytes()),
                        io::IoSliceMut::new(&mut arg[..]),
                    ])
                    .map_err(ReceiveError::from_read_operation)?;

                if len != self.header.len as usize {
                    Err(ReceiveError::invalid_data(
                        "The value in_header.len is mismatched to the result of splice(2)",
                    ))?
                }

                let opcode = fuse_opcode::try_from(self.header.opcode)
                    .map_err(|_| ReceiveError::UnrecognizedOpcode(self.header.opcode))?;
                self.opcode = Some(opcode);

                *pos = arg_len(&self.header, opcode);
            }
        }
        Ok(())
    }
}

fn arg_len(header: &fuse_in_header, opcode: fuse_opcode) -> usize {
    match opcode {
        fuse_opcode::FUSE_WRITE | fuse_opcode::FUSE_NOTIFY_REPLY => mem::size_of::<fuse_write_in>(),
        _ => header.len as usize - mem::size_of::<fuse_in_header>(),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReceiveError {
    #[error("The connection is disconnected")]
    Disconnected,

    #[error("The read syscall is interrupted")]
    Interrupted,

    #[error(
        "The opcode `{}' is not recognized by the current version of `polyfuse`",
        _0
    )]
    UnrecognizedOpcode(u32),

    #[error("Unrecoverable I/O error: {}", _0)]
    Fatal(#[from] io::Error),
}

impl ReceiveError {
    fn from_read_operation(err: io::Error) -> Self {
        // ref: https://github.com/libfuse/libfuse/blob/fuse-3.10.5/lib/fuse_lowlevel.c#L2865
        match err.raw_os_error() {
            Some(ENODEV) => Self::Disconnected,
            Some(ENOENT) | Some(EINTR) => Self::Interrupted,
            _ => Self::Fatal(err),
        }
    }

    fn invalid_data<T>(source: T) -> Self
    where
        T: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        Self::Fatal(io::Error::new(io::ErrorKind::InvalidData, source))
    }
}

#[derive(Debug)]
pub enum RemainingData<'req> {
    Splice(&'req PipeReader),
    Fallback(&'req [u8]),
}

impl io::Read for RemainingData<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Splice(pipe) => pipe.read(buf),
            Self::Fallback(vec) => vec.read(buf),
        }
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        match self {
            Self::Splice(pipe) => pipe.read_vectored(bufs),
            Self::Fallback(vec) => vec.read_vectored(bufs),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("during decoding: {}", _0)]
    Decode(#[from] DecodeError),

    #[error("unsupported opcode")]
    UnsupportedOpcode,
}

struct Op<'req> {
    _marker: PhantomData<&'req ()>,
}

impl<'req> crate::op::Op for Op<'req> {
    type Lookup = Lookup<'req>;
    type Getattr = Getattr<'req>;
    type Setattr = Setattr<'req>;
    type Readlink = Readlink<'req>;
    type Symlink = Symlink<'req>;
    type Mknod = Mknod<'req>;
    type Mkdir = Mkdir<'req>;
    type Unlink = Unlink<'req>;
    type Rmdir = Rmdir<'req>;
    type Rename = Rename<'req>;
    type Link = Link<'req>;
    type Open = Open<'req>;
    type Read = ReadFile<'req>;
    type Write = WriteFile<'req>;
    type Release = Release<'req>;
    type Statfs = Statfs<'req>;
    type Fsync = Fsync<'req>;
    type Setxattr = Setxattr<'req>;
    type Getxattr = Getxattr<'req>;
    type Listxattr = Listxattr<'req>;
    type Removexattr = Removexattr<'req>;
    type Flush = Flush<'req>;
    type Opendir = Opendir<'req>;
    type Readdir = Readdir<'req>;
    type Releasedir = Releasedir<'req>;
    type Fsyncdir = Fsyncdir<'req>;

    type Forgets = Forgets<'req>;
    type Interrupt = Interrupt<'req>;
    type NotifyReply = NotifyReply<'req>;
}

enum Forgets<'op> {
    Single(fuse_forget_one),
    Batch(&'op [fuse_forget_one]),
}
impl<'op> AsRef<[Forget]> for Forgets<'op> {
    #[inline]
    fn as_ref(&self) -> &[Forget] {
        let (ptr, len) = match self {
            Self::Single(forget) => (forget as *const fuse_forget_one, 1),
            Self::Batch(forgets) => (forgets.as_ptr(), forgets.len()),
        };
        unsafe {
            // Safety: Forget has the same layout with fuse_forget_one
            std::slice::from_raw_parts(ptr as *const Forget, len)
        }
    }
}

struct Interrupt<'op> {
    arg: &'op fuse_interrupt_in,
}
impl<'op> op::Interrupt for Interrupt<'op> {
    fn interrupted(&self) -> RequestID {
        RequestID::from_raw(self.arg.unique & !FUSE_INT_REQ_BIT)
    }
}

struct NotifyReply<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_notify_retrieve_in,
}
impl<'op> op::NotifyReply for NotifyReply<'op> {
    fn unique(&self) -> NotifyID {
        NotifyID::from_raw(self.header.unique)
    }
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn offset(&self) -> u64 {
        self.arg.offset
    }
    fn size(&self) -> u32 {
        self.arg.size
    }
}

struct Lookup<'req> {
    header: &'req fuse_in_header,
    name: &'req OsStr,
}
impl op::Lookup for Lookup<'_> {
    fn parent(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }

    fn name(&self) -> &OsStr {
        self.name
    }
}

struct Getattr<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_getattr_in,
}
impl<'op> op::Getattr for Getattr<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn fh(&self) -> Option<FileID> {
        if self.arg.getattr_flags & FUSE_GETATTR_FH != 0 {
            Some(FileID::from_raw(self.arg.fh))
        } else {
            None
        }
    }
}

struct Setattr<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_setattr_in,
}
impl<'op> Setattr<'op> {
    #[inline(always)]
    fn get<R>(&self, flag: u32, f: impl FnOnce(&fuse_setattr_in) -> R) -> Option<R> {
        if self.arg.valid & flag != 0 {
            Some(f(self.arg))
        } else {
            None
        }
    }
}
impl<'op> op::Setattr for Setattr<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn fh(&self) -> Option<FileID> {
        self.get(FATTR_FH, |arg| FileID::from_raw(arg.fh))
    }
    fn mode(&self) -> Option<FileMode> {
        self.get(FATTR_MODE, |arg| FileMode::from_raw(arg.mode))
    }
    fn uid(&self) -> Option<UID> {
        self.get(FATTR_UID, |arg| UID::from_raw(arg.uid))
    }
    fn gid(&self) -> Option<GID> {
        self.get(FATTR_GID, |arg| GID::from_raw(arg.gid))
    }
    fn size(&self) -> Option<u64> {
        self.get(FATTR_SIZE, |arg| arg.size)
    }
    fn atime(&self) -> Option<SetAttrTime> {
        self.get(FATTR_ATIME, |arg| {
            if arg.valid & FATTR_ATIME_NOW != 0 {
                SetAttrTime::Now
            } else {
                SetAttrTime::Timespec(Duration::new(arg.atime, arg.atimensec))
            }
        })
    }
    fn mtime(&self) -> Option<SetAttrTime> {
        self.get(FATTR_MTIME, |arg| {
            if arg.valid & FATTR_MTIME_NOW != 0 {
                SetAttrTime::Now
            } else {
                SetAttrTime::Timespec(Duration::new(arg.mtime, arg.mtimensec))
            }
        })
    }
    fn ctime(&self) -> Option<Duration> {
        self.get(FATTR_CTIME, |arg| Duration::new(arg.ctime, arg.ctimensec))
    }
    fn lock_owner(&self) -> Option<LockOwnerID> {
        self.get(FATTR_LOCKOWNER, |arg| LockOwnerID::from_raw(arg.lock_owner))
    }
}

struct Readlink<'op> {
    header: &'op fuse_in_header,
}
impl<'op> op::Readlink for Readlink<'op> {
    /// Return the inode number to be read the link value.
    #[inline]
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
}

struct Symlink<'op> {
    header: &'op fuse_in_header,
    name: &'op OsStr,
    link: &'op OsStr,
}
impl<'op> op::Symlink for Symlink<'op> {
    fn parent(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn name(&self) -> &OsStr {
        self.name
    }
    fn link(&self) -> &OsStr {
        self.link
    }
}

struct Mknod<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_mknod_in,
    name: &'op OsStr,
}
impl<'op> op::Mknod for Mknod<'op> {
    fn parent(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn name(&self) -> &OsStr {
        self.name
    }
    fn mode(&self) -> FileMode {
        FileMode::from_raw(self.arg.mode)
    }
    fn rdev(&self) -> DeviceID {
        DeviceID::from_kernel_dev(self.arg.rdev)
    }
    fn umask(&self) -> u32 {
        self.arg.umask
    }
}

struct Mkdir<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_mkdir_in,
    name: &'op OsStr,
}
impl<'op> op::Mkdir for Mkdir<'op> {
    fn parent(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn name(&self) -> &OsStr {
        self.name
    }
    fn permissions(&self) -> FilePermissions {
        FilePermissions::from_bits_truncate(self.arg.mode)
    }
    fn umask(&self) -> u32 {
        self.arg.umask
    }
}

struct Unlink<'op> {
    header: &'op fuse_in_header,
    name: &'op OsStr,
}
impl<'op> op::Unlink for Unlink<'op> {
    fn parent(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn name(&self) -> &OsStr {
        self.name
    }
}

struct Rmdir<'op> {
    header: &'op fuse_in_header,
    name: &'op OsStr,
}
impl<'op> op::Rmdir for Rmdir<'op> {
    fn parent(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn name(&self) -> &OsStr {
        self.name
    }
}

struct Rename<'op> {
    header: &'op fuse_in_header,
    arg: RenameArg<'op>,
    name: &'op OsStr,
    newname: &'op OsStr,
}
enum RenameArg<'op> {
    V1(&'op fuse_rename_in),
    V2(&'op fuse_rename2_in),
}
impl<'op> op::Rename for Rename<'op> {
    fn parent(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn name(&self) -> &OsStr {
        self.name
    }
    fn newparent(&self) -> NodeID {
        match self.arg {
            RenameArg::V1(arg) => NodeID::from_raw(arg.newdir),
            RenameArg::V2(arg) => NodeID::from_raw(arg.newdir),
        }
    }
    fn newname(&self) -> &OsStr {
        self.newname
    }
    fn flags(&self) -> RenameFlags {
        match self.arg {
            RenameArg::V1(..) => RenameFlags::empty(),
            RenameArg::V2(arg) => RenameFlags::from_bits_truncate(arg.flags),
        }
    }
}

struct Link<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_link_in,
    newname: &'op OsStr,
}
impl<'op> op::Link for Link<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.arg.oldnodeid)
    }
    fn newparent(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn newname(&self) -> &OsStr {
        self.newname
    }
}

struct Open<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_open_in,
}
impl<'op> op::Open for Open<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn options(&self) -> OpenOptions {
        OpenOptions::from_raw(self.arg.flags)
    }
}

struct ReadFile<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_read_in,
}
impl<'op> op::Read for ReadFile<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn fh(&self) -> FileID {
        FileID::from_raw(self.arg.fh)
    }
    fn offset(&self) -> u64 {
        self.arg.offset
    }
    fn size(&self) -> u32 {
        self.arg.size
    }
    fn options(&self) -> OpenOptions {
        OpenOptions::from_raw(self.arg.flags)
    }
    fn lock_owner(&self) -> Option<LockOwnerID> {
        if self.arg.read_flags & FUSE_READ_LOCKOWNER != 0 {
            Some(LockOwnerID::from_raw(self.arg.lock_owner))
        } else {
            None
        }
    }
}

struct WriteFile<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_write_in,
}
impl<'op> op::Write for WriteFile<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn fh(&self) -> FileID {
        FileID::from_raw(self.arg.fh)
    }
    fn offset(&self) -> u64 {
        self.arg.offset
    }
    fn size(&self) -> u32 {
        self.arg.size
    }
    fn options(&self) -> OpenOptions {
        OpenOptions::from_raw(self.arg.flags)
    }
    fn lock_owner(&self) -> Option<LockOwnerID> {
        if self.arg.write_flags & FUSE_WRITE_LOCKOWNER != 0 {
            Some(LockOwnerID::from_raw(self.arg.lock_owner))
        } else {
            None
        }
    }
}

struct Release<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_release_in,
}
impl<'op> op::Release for Release<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn fh(&self) -> FileID {
        FileID::from_raw(self.arg.fh)
    }
    fn options(&self) -> OpenOptions {
        OpenOptions::from_raw(self.arg.flags)
    }
    fn lock_owner(&self) -> LockOwnerID {
        LockOwnerID::from_raw(self.arg.lock_owner)
    }
    fn flush(&self) -> bool {
        self.arg.release_flags & FUSE_RELEASE_FLUSH != 0
    }
    fn flock_release(&self) -> bool {
        self.arg.release_flags & FUSE_RELEASE_FLOCK_UNLOCK != 0
    }
}

struct Statfs<'op> {
    header: &'op fuse_in_header,
}
impl<'op> op::Statfs for Statfs<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
}

struct Fsync<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_fsync_in,
}
impl<'op> op::Fsync for Fsync<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn fh(&self) -> FileID {
        FileID::from_raw(self.arg.fh)
    }
    fn datasync(&self) -> bool {
        self.arg.fsync_flags & FUSE_FSYNC_FDATASYNC != 0
    }
}

struct Setxattr<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_setxattr_in,
    name: &'op OsStr,
    value: &'op [u8],
}
impl<'op> op::Setxattr for Setxattr<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn name(&self) -> &OsStr {
        self.name
    }
    fn value(&self) -> &[u8] {
        self.value
    }
    fn flags(&self) -> SetxattrFlags {
        SetxattrFlags::from_bits_truncate(self.arg.flags)
    }
}

struct Getxattr<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_getxattr_in,
    name: &'op OsStr,
}
impl<'op> op::Getxattr for Getxattr<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn name(&self) -> &OsStr {
        self.name
    }
    fn size(&self) -> u32 {
        self.arg.size
    }
}

struct Listxattr<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_getxattr_in,
}
impl<'op> op::Listxattr for Listxattr<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn size(&self) -> u32 {
        self.arg.size
    }
}

struct Removexattr<'op> {
    header: &'op fuse_in_header,
    name: &'op OsStr,
}
impl<'op> op::Removexattr for Removexattr<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn name(&self) -> &OsStr {
        self.name
    }
}

struct Flush<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_flush_in,
}
impl<'op> op::Flush for Flush<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn fh(&self) -> FileID {
        FileID::from_raw(self.arg.fh)
    }
    fn lock_owner(&self) -> LockOwnerID {
        LockOwnerID::from_raw(self.arg.lock_owner)
    }
}

struct Opendir<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_open_in,
}
impl<'op> op::Opendir for Opendir<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn options(&self) -> OpenOptions {
        OpenOptions::from_raw(self.arg.flags)
    }
}

struct Readdir<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_read_in,
    mode: ReaddirMode,
}
impl<'op> op::Readdir for Readdir<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn fh(&self) -> FileID {
        FileID::from_raw(self.arg.fh)
    }
    fn offset(&self) -> u64 {
        self.arg.offset
    }
    fn size(&self) -> u32 {
        self.arg.size
    }
    fn mode(&self) -> ReaddirMode {
        self.mode
    }
}

struct Releasedir<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_release_in,
}
impl<'op> op::Releasedir for Releasedir<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn fh(&self) -> FileID {
        FileID::from_raw(self.arg.fh)
    }
    fn options(&self) -> OpenOptions {
        OpenOptions::from_raw(self.arg.flags)
    }
}

struct Fsyncdir<'op> {
    header: &'op fuse_in_header,
    arg: &'op fuse_fsync_in,
}
impl<'op> op::Fsyncdir for Fsyncdir<'op> {
    fn ino(&self) -> NodeID {
        NodeID::from_raw(self.header.nodeid)
    }
    fn fh(&self) -> FileID {
        FileID::from_raw(self.arg.fh)
    }
    fn datasync(&self) -> bool {
        self.arg.fsync_flags & FUSE_FSYNC_FDATASYNC != 0
    }
}

fn decode<'req>(
    header: &'req fuse_in_header,
    opcode: fuse_opcode,
    arg: &'req [u8],
) -> Result<Operation<Op<'req>>, Error> {
    let mut decoder = Decoder::new(arg);

    match opcode {
        fuse_opcode::FUSE_FORGET => {
            let arg: &fuse_forget_in = decoder.fetch()?;
            let forget = fuse_forget_one {
                nodeid: header.nodeid,
                nlookup: arg.nlookup,
            };
            Ok(Operation::Forget(Forgets::Single(forget)))
        }

        fuse_opcode::FUSE_BATCH_FORGET => {
            let arg: &fuse_batch_forget_in = decoder.fetch()?;
            let forgets = decoder.fetch_array::<fuse_forget_one>(arg.count as usize)?;
            Ok(Operation::Forget(Forgets::Batch(forgets)))
        }

        fuse_opcode::FUSE_INTERRUPT => {
            let arg = decoder.fetch()?;
            Ok(Operation::Interrupt(Interrupt { arg }))
        }

        fuse_opcode::FUSE_NOTIFY_REPLY => {
            let arg = decoder.fetch()?;
            Ok(Operation::NotifyReply(NotifyReply { header, arg }))
        }

        fuse_opcode::FUSE_LOOKUP => {
            let name = decoder.fetch_str()?;
            Ok(Operation::Lookup(Lookup { header, name }))
        }

        fuse_opcode::FUSE_GETATTR => {
            let arg = decoder.fetch()?;
            Ok(Operation::Getattr(Getattr { header, arg }))
        }

        fuse_opcode::FUSE_SETATTR => {
            let arg = decoder.fetch()?;
            Ok(Operation::Setattr(Setattr { header, arg }))
        }

        fuse_opcode::FUSE_READLINK => Ok(Operation::Readlink(Readlink { header })),

        fuse_opcode::FUSE_SYMLINK => {
            let name = decoder.fetch_str()?;
            let link = decoder.fetch_str()?;
            Ok(Operation::Symlink(Symlink { header, name, link }))
        }

        fuse_opcode::FUSE_MKNOD => {
            let arg = decoder.fetch()?;
            let name = decoder.fetch_str()?;
            Ok(Operation::Mknod(Mknod { header, arg, name }))
        }

        fuse_opcode::FUSE_MKDIR => {
            let arg = decoder.fetch()?;
            let name = decoder.fetch_str()?;
            Ok(Operation::Mkdir(Mkdir { header, arg, name }))
        }

        fuse_opcode::FUSE_UNLINK => {
            let name = decoder.fetch_str()?;
            Ok(Operation::Unlink(Unlink { header, name }))
        }

        fuse_opcode::FUSE_RMDIR => {
            let name = decoder.fetch_str()?;
            Ok(Operation::Rmdir(Rmdir { header, name }))
        }

        fuse_opcode::FUSE_RENAME => {
            let arg = decoder.fetch()?;
            let name = decoder.fetch_str()?;
            let newname = decoder.fetch_str()?;
            Ok(Operation::Rename(Rename {
                header,
                arg: RenameArg::V1(arg),
                name,
                newname,
            }))
        }

        fuse_opcode::FUSE_RENAME2 => {
            let arg = decoder.fetch()?;
            let name = decoder.fetch_str()?;
            let newname = decoder.fetch_str()?;
            Ok(Operation::Rename(Rename {
                header,
                arg: RenameArg::V2(arg),
                name,
                newname,
            }))
        }

        fuse_opcode::FUSE_LINK => {
            let arg = decoder.fetch()?;
            let newname = decoder.fetch_str()?;
            Ok(Operation::Link(Link {
                header,
                arg,
                newname,
            }))
        }

        fuse_opcode::FUSE_OPEN => {
            let arg = decoder.fetch()?;
            Ok(Operation::Open(Open { header, arg }))
        }

        fuse_opcode::FUSE_READ => {
            let arg = decoder.fetch()?;
            Ok(Operation::Read(ReadFile { header, arg }))
        }

        fuse_opcode::FUSE_WRITE => {
            let arg = decoder.fetch()?;
            Ok(Operation::Write(WriteFile { header, arg }))
        }

        fuse_opcode::FUSE_RELEASE => {
            let arg = decoder.fetch()?;
            Ok(Operation::Release(Release { header, arg }))
        }

        fuse_opcode::FUSE_STATFS => Ok(Operation::Statfs(Statfs { header })),

        fuse_opcode::FUSE_FSYNC => {
            let arg = decoder.fetch()?;
            Ok(Operation::Fsync(Fsync { header, arg }))
        }

        fuse_opcode::FUSE_SETXATTR => {
            let arg = decoder.fetch::<fuse_setxattr_in>()?;
            let name = decoder.fetch_str()?;
            let value = decoder.fetch_bytes(arg.size as usize)?;
            Ok(Operation::Setxattr(Setxattr {
                header,
                arg,
                name,
                value,
            }))
        }

        fuse_opcode::FUSE_GETXATTR => {
            let arg = decoder.fetch()?;
            let name = decoder.fetch_str()?;
            Ok(Operation::Getxattr(Getxattr { header, arg, name }))
        }

        fuse_opcode::FUSE_LISTXATTR => {
            let arg = decoder.fetch()?;
            Ok(Operation::Listxattr(Listxattr { header, arg }))
        }

        fuse_opcode::FUSE_REMOVEXATTR => {
            let name = decoder.fetch_str()?;
            Ok(Operation::Removexattr(Removexattr { header, name }))
        }

        fuse_opcode::FUSE_FLUSH => {
            let arg = decoder.fetch()?;
            Ok(Operation::Flush(Flush { header, arg }))
        }

        fuse_opcode::FUSE_OPENDIR => {
            let arg = decoder.fetch()?;
            Ok(Operation::Opendir(Opendir { header, arg }))
        }

        fuse_opcode::FUSE_READDIR => {
            let arg = decoder.fetch()?;
            Ok(Operation::Readdir(Readdir {
                header,
                arg,
                mode: ReaddirMode::Normal,
            }))
        }

        fuse_opcode::FUSE_READDIRPLUS => {
            let arg = decoder.fetch()?;
            Ok(Operation::Readdir(Readdir {
                header,
                arg,
                mode: ReaddirMode::Plus,
            }))
        }

        fuse_opcode::FUSE_RELEASEDIR => {
            let arg = decoder.fetch()?;
            Ok(Operation::Releasedir(Releasedir { header, arg }))
        }

        fuse_opcode::FUSE_FSYNCDIR => {
            let arg = decoder.fetch()?;
            Ok(Operation::Fsyncdir(Fsyncdir { header, arg }))
        }

        // fuse_opcode::FUSE_GETLK => {
        //     let arg = decoder.fetch()?;
        //     Ok(Operation::Getlk(Getlk { header, arg }))
        // }

        // opcode @ fuse_opcode::FUSE_SETLK | opcode @ fuse_opcode::FUSE_SETLKW => {
        //     let arg: &fuse_lk_in = decoder.fetch()?;
        //     let sleep = match opcode {
        //         fuse_opcode::FUSE_SETLK => false,
        //         fuse_opcode::FUSE_SETLKW => true,
        //         _ => unreachable!(),
        //     };

        //     if arg.lk_flags & FUSE_LK_FLOCK == 0 {
        //         Ok(Operation::Setlk(Setlk { header, arg, sleep }))
        //     } else {
        //         Ok(Operation::Flock(Flock { header, arg, sleep }))
        //     }
        // }

        // fuse_opcode::FUSE_ACCESS => {
        //     let arg = decoder.fetch()?;
        //     Ok(Operation::Access(Access { header, arg }))
        // }

        // fuse_opcode::FUSE_CREATE => {
        //     let arg = decoder.fetch()?;
        //     let name = decoder.fetch_str()?;
        //     Ok(Operation::Create(Create { header, arg, name }))
        // }

        // fuse_opcode::FUSE_BMAP => {
        //     let arg = decoder.fetch()?;
        //     Ok(Operation::Bmap(Bmap { header, arg }))
        // }

        // fuse_opcode::FUSE_FALLOCATE => {
        //     let arg = decoder.fetch()?;
        //     Ok(Operation::Fallocate(Fallocate { header, arg }))
        // }

        // fuse_opcode::FUSE_COPY_FILE_RANGE => {
        //     let arg = decoder.fetch()?;
        //     Ok(Operation::CopyFileRange(CopyFileRange { header, arg }))
        // }

        // fuse_opcode::FUSE_POLL => {
        //     let arg = decoder.fetch()?;
        //     Ok(Operation::Poll(Poll { header, arg }))
        // }

        // fuse_opcode::FUSE_LSEEK => {
        //     let arg = decoder.fetch()?;
        //     Ok(Operation::Lseek(Lseek { header, arg }))
        // }

        // fuse_opcode::FUSE_INIT | fuse_opcode::FUSE_DESTROY => {
        //     // should be handled by the upstream process.
        //     unreachable!()
        // }

        // fuse_opcode::FUSE_IOCTL | fuse_opcode::CUSE_INIT => Err(Error::UnsupportedOpcode),
        _ => todo!(),
    }
}
