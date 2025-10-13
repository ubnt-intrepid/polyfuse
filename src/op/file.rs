use bitflags::bitflags;
use polyfuse_kernel::{
    fuse_fallocate_in, fuse_flush_in, fuse_fsync_in, fuse_lseek_in, fuse_open_in, fuse_poll_in,
    fuse_read_in, fuse_release_in, fuse_write_in, fuse_write_in_compat_8, FUSE_FSYNC_FDATASYNC,
    FUSE_POLL_SCHEDULE_NOTIFY, FUSE_READ_LOCKOWNER, FUSE_RELEASE_FLOCK_UNLOCK, FUSE_RELEASE_FLUSH,
    FUSE_WRITE_LOCKOWNER,
};

use crate::types::{FileID, LockOwnerID, NodeID, PollEvents, PollWakeupID};
use std::{fmt, marker::PhantomData, os::unix::prelude::*};

/// Open a file.
///
/// If the file is successfully opened, the filesystem must send the identifier
/// of the opened file handle to the kernel using `ReplyOpen`. This parameter is
/// set to a series of requests, such as `read` and `write`, until releasing
/// the file, and is able to be utilized as a "pointer" to the state during
/// handling the opened file.
///
/// See also the documentation of `ReplyOpen` for tuning the reply parameters.
// TODO: Description of behavior when writeback caching is enabled.
#[derive(Debug)]
#[non_exhaustive]
pub struct Open<'op> {
    /// The inode number to be opened.
    pub ino: NodeID,

    /// The access mode and creation/status flags of the opened file.
    pub options: OpenOptions,

    _marker: PhantomData<&'op ()>,
}

impl<'op> super::Op<'op> for Open<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_open_in = cx.decoder.fetch()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            options: OpenOptions::from_raw(arg.flags),
            _marker: PhantomData,
        })
    }
}

/// The compound type of the access mode and auxiliary flags of opened files.
///
/// * If the mount option contains `-o default_permissions`, the access mode
///   flags (`O_RDONLY`, `O_WRONLY` and `O_RDWR`) might be handled by the kernel
///   and in that case, these flags are omitted before issuing the request.
///   Otherwise, the filesystem should handle these flags and return an `EACCES`
///   error when provided access mode is invalid.
/// * Some parts of the creating flags (`O_CREAT`, `O_EXCL` and `O_NOCTTY`) are
///   removed and these flags are handled by the kernel.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct OpenOptions {
    raw: u32,
}

impl fmt::Debug for OpenOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenOptions")
            .field("access_mode", &self.access_mode())
            .field("flags", &self.flags())
            .finish()
    }
}

impl OpenOptions {
    pub(super) const fn from_raw(raw: u32) -> Self {
        Self { raw }
    }

    pub const fn access_mode(self) -> Option<AccessMode> {
        match self.raw as i32 & libc::O_ACCMODE {
            libc::O_RDONLY => Some(AccessMode::ReadOnly),
            libc::O_WRONLY => Some(AccessMode::WriteOnly),
            libc::O_RDWR => Some(AccessMode::ReadWrite),
            _ => None,
        }
    }

    pub const fn flags(self) -> OpenFlags {
        OpenFlags::from_bits_truncate(self.raw)
    }

    pub fn set_flags(&mut self, flags: OpenFlags) {
        self.raw &= !OpenFlags::all().bits();
        self.raw |= flags.bits();
    }
}

impl From<OpenOptions> for std::fs::OpenOptions {
    fn from(src: OpenOptions) -> Self {
        let mut options = std::fs::OpenOptions::new();
        match src.access_mode() {
            Some(AccessMode::ReadOnly) => {
                options.read(true);
            }
            Some(AccessMode::WriteOnly) => {
                options.write(true);
            }
            Some(AccessMode::ReadWrite) => {
                options.read(true).write(true);
            }
            _ => (),
        }
        options.custom_flags(src.flags().bits() as i32);
        options
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AccessMode {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

impl AccessMode {
    pub const fn into_raw(self) -> i32 {
        match self {
            Self::ReadOnly => libc::O_RDONLY,
            Self::WriteOnly => libc::O_WRONLY,
            Self::ReadWrite => libc::O_RDWR,
        }
    }
}

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct OpenFlags: u32 {
        // file creation flags.
        const CLOEXEC = libc::O_CLOEXEC as u32;
        const DIRECTORY = libc::O_DIRECTORY as u32;
        const NOFOLLOW = libc::O_NOFOLLOW as u32;
        const TMPFILE = libc::O_TMPFILE as u32;
        const TRUNC = libc::O_TRUNC as u32;

        // file status flags.
        const APPEND = libc::O_APPEND as u32;
        const ASYNC = libc::O_ASYNC as u32;
        const DIRECT = libc::O_DIRECT as u32;
        const DSYNC = libc::O_DSYNC as u32;
        const LARGEFILE = libc::O_LARGEFILE as u32;
        const NOATIME = libc::O_NOATIME as u32;
        const NONBLOCK = libc::O_NONBLOCK as u32;
        const NDELAY = libc::O_NDELAY as u32;
        const PATH = libc::O_PATH as u32;
        const SYNC = libc::O_SYNC as u32;
    }
}

/// Read data from a file.
///
/// The total amount of the replied data must be within `size`.
///
/// When the file is opened in `direct_io` mode, the result replied will be
/// reflected in the caller's result of `read` syscall.
///
/// When the file is not opened in `direct_io` mode (i.e. the page caching is enabled),
/// the filesystem should send *exactly* the specified range of file content to the
/// kernel. If the length of the passed data is shorter than `size`, the rest of
/// the data will be substituted with zeroes.
#[derive(Debug)]
#[non_exhaustive]
pub struct Read<'op> {
    /// The inode number to be read.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    /// The starting position of the content to be read.
    pub offset: u64,

    /// The length of the data to be read.
    pub size: u32,

    /// The flags specified at opening the file.
    pub options: OpenOptions,

    /// The identifier of lock owner.
    pub lock_owner: Option<LockOwnerID>,

    _marker: PhantomData<&'op ()>,
}

impl<'op> super::Op<'op> for Read<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_read_in = cx.decoder.fetch()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            fh: FileID::from_raw(arg.fh),
            offset: arg.offset,
            size: arg.size,
            options: OpenOptions::from_raw(arg.flags),
            lock_owner: (arg.read_flags & FUSE_READ_LOCKOWNER != 0)
                .then(|| LockOwnerID::from_raw(arg.lock_owner)),
            _marker: PhantomData,
        })
    }
}

/// Write data to a file.
///
/// If the data is successfully written, the filesystem must send the amount of the written
/// data using `ReplyWrite`.
///
/// When the file is opened in `direct_io` mode, the result replied will be reflected
/// in the caller's result of `write` syscall.
///
/// When the file is not opened in `direct_io` mode (i.e. the page caching is enabled),
/// the filesystem should receive *exactly* the specified range of file content from the kernel.
#[derive(Debug)]
#[non_exhaustive]
pub struct Write<'op> {
    /// The inode number to be written.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    /// The starting position of contents to be written.
    pub offset: u64,

    /// The length of contents to be written.
    pub size: u32,

    /// The flags specified at opening the file.
    pub options: OpenOptions,

    /// The identifier of lock owner.
    pub lock_owner: Option<LockOwnerID>,

    _marker: PhantomData<&'op ()>,
}

impl<'op> super::Op<'op> for Write<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        if cx.config.minor <= 8 {
            let arg: &fuse_write_in_compat_8 = cx.decoder.fetch()?;
            Ok(Self {
                ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
                fh: FileID::from_raw(arg.fh),
                offset: arg.offset,
                size: arg.size,
                options: OpenOptions::from_raw(0),
                lock_owner: None,
                _marker: PhantomData,
            })
        } else {
            let arg: &fuse_write_in = cx.decoder.fetch()?;
            Ok(Self {
                ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
                fh: FileID::from_raw(arg.fh),
                offset: arg.offset,
                size: arg.size,
                options: OpenOptions::from_raw(arg.flags),
                lock_owner: (arg.write_flags & FUSE_WRITE_LOCKOWNER != 0)
                    .then(|| LockOwnerID::from_raw(arg.lock_owner)),
                _marker: PhantomData,
            })
        }
    }
}

/// Release an opened file.
#[derive(Debug)]
#[non_exhaustive]
pub struct Release<'op> {
    /// The inode number of opened file.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    /// The flags specified at opening the file.
    pub options: OpenOptions,

    /// The identifier of lock owner.
    pub lock_owner: LockOwnerID,

    /// The flags of release operation.
    pub flags: ReleaseFlags,

    _marker: PhantomData<&'op ()>,
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct ReleaseFlags: u32 {
        /// Indicates whether the operation indicates a flush.
        const FLUSH = FUSE_RELEASE_FLUSH;

        /// Indicates whether the `flock` locks for this file should be released.
        const FLOCK_UNLOCK = FUSE_RELEASE_FLOCK_UNLOCK;
    }
}

impl<'op> super::Op<'op> for Release<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_release_in = cx.decoder.fetch()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            fh: FileID::from_raw(arg.fh),
            options: OpenOptions::from_raw(arg.flags),
            lock_owner: LockOwnerID::from_raw(arg.lock_owner),
            flags: ReleaseFlags::from_bits_truncate(arg.release_flags),
            _marker: PhantomData,
        })
    }
}

/// Synchronize the file contents.
#[derive(Debug)]
#[non_exhaustive]
pub struct Fsync<'op> {
    /// The inode number to be synchronized.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    /// Indicates whether to synchronize only the file contents.
    ///
    /// When this method returns `true`, the metadata does not have to be flushed.
    pub datasync: bool,

    _marker: PhantomData<&'op ()>,
}

impl<'op> super::Op<'op> for Fsync<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_fsync_in = cx.decoder.fetch()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            fh: FileID::from_raw(arg.fh),
            datasync: arg.fsync_flags & FUSE_FSYNC_FDATASYNC != 0,
            _marker: PhantomData,
        })
    }
}

/// Close a file descriptor.
///
/// This operation is issued on each `close(2)` syscall
/// for a file descriptor.
///
/// Do not confuse this operation with `Release`.
/// Since the file descriptor could be duplicated, the multiple
/// flush operations might be issued for one `Open`.
/// Also, it is not guaranteed that flush will always be issued
/// after some writes.
#[derive(Debug)]
#[non_exhaustive]
pub struct Flush<'op> {
    /// The inode number of target file.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    /// The identifier of lock owner.
    pub lock_owner: LockOwnerID,

    _marker: PhantomData<&'op ()>,
}

impl<'op> super::Op<'op> for Flush<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_flush_in = cx.decoder.fetch()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            fh: FileID::from_raw(arg.fh),
            lock_owner: LockOwnerID::from_raw(arg.lock_owner),
            _marker: PhantomData,
        })
    }
}

/// Allocate requested space.
///
/// If this operation is successful, the filesystem shall not report
/// the error caused by the lack of free spaces to subsequent write
/// requests.
#[derive(Debug)]
#[non_exhaustive]
pub struct Fallocate<'op> {
    /// The number of target inode to be allocated the space.
    pub ino: NodeID,

    /// The handle for opened file.
    pub fh: FileID,

    /// The starting point of region to be allocated.
    pub offset: u64,

    /// The length of region to be allocated.
    pub length: u64,

    /// The mode that specifies how to allocate the region.
    ///
    /// See [`fallocate(2)`][fallocate] for details.
    ///
    /// [fallocate]: http://man7.org/linux/man-pages/man2/fallocate.2.html
    pub mode: FallocateFlags,

    _marker: PhantomData<&'op ()>,
}

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct FallocateFlags: u32 {
        const KEEP_SIZE = libc::FALLOC_FL_KEEP_SIZE as u32;
        const UNSHARE_RANGE = libc::FALLOC_FL_UNSHARE_RANGE as u32;
        const PUNCH_HOLE = libc::FALLOC_FL_PUNCH_HOLE as u32;
        const COLLAPSE_RANGE = libc::FALLOC_FL_COLLAPSE_RANGE as u32;
        const ZERO_RANGE = libc::FALLOC_FL_ZERO_RANGE as u32;
        const INSERT_RANGE = libc::FALLOC_FL_INSERT_RANGE as u32;
    }
}

impl<'op> super::Op<'op> for Fallocate<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_fallocate_in = cx.decoder.fetch()?;
        Ok(Self {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            fh: FileID::from_raw(arg.fh),
            offset: arg.offset,
            length: arg.length,
            mode: FallocateFlags::from_bits_truncate(arg.mode),
            _marker: PhantomData,
        })
    }
}

/// Poll for readiness.
///
/// The mask of ready poll events must be replied using `ReplyPoll`.
#[derive(Debug)]
#[non_exhaustive]
pub struct Poll<'op> {
    /// The inode number to check the I/O readiness.
    pub ino: NodeID,

    /// The handle of opened file.
    pub fh: FileID,

    /// The requested poll events.
    pub events: PollEvents,

    /// The handle to this poll.
    ///
    /// If the returned value is not `None`, the filesystem should send the notification
    /// when the corresponding I/O will be ready.
    pub kh: Option<PollWakeupID>,

    _marker: PhantomData<&'op ()>,
}

impl<'op> super::Op<'op> for Poll<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_poll_in = cx.decoder.fetch()?;
        Ok(Poll {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            fh: FileID::from_raw(arg.fh),
            events: PollEvents::from_bits_truncate(arg.events),
            kh: (arg.flags & FUSE_POLL_SCHEDULE_NOTIFY != 0)
                .then(|| PollWakeupID::from_raw(arg.kh)),
            _marker: PhantomData,
        })
    }
}

/// Reposition the offset of read/write operations.
///
/// See [`lseek(2)`][lseek] for details.
///
/// [lseek]: https://man7.org/linux/man-pages/man2/lseek.2.html
#[derive(Debug)]
#[non_exhaustive]
pub struct Lseek<'op> {
    pub ino: NodeID,
    pub fh: FileID,
    pub offset: u64,
    pub whence: Whence,
    _marker: PhantomData<&'op ()>,
}

impl<'op> super::Op<'op> for Lseek<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_lseek_in = cx.decoder.fetch()?;
        Ok(Lseek {
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            fh: FileID::from_raw(arg.fh),
            offset: arg.offset,
            whence: match arg.whence as i32 {
                libc::SEEK_SET => Whence::Set,
                libc::SEEK_CUR => Whence::Current,
                libc::SEEK_END => Whence::End,
                libc::SEEK_DATA => Whence::Data,
                libc::SEEK_HOLE => Whence::Hole,
                _ => return Err(super::Error::UnknownLseekWhence),
            },
            _marker: PhantomData,
        })
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Whence {
    Set,
    Current,
    End,
    Data,
    Hole,
}
