use crate::{
    bytes::{Bytes, POD},
    session::{KernelConfig, KernelFlags},
    types::{FileAttr, FileID, FileLock, FileType, NodeID, PollEvents, Statfs},
};
use bitflags::bitflags;
use polyfuse_kernel::*;
use std::{borrow::Cow, ffi::OsStr, mem, os::unix::prelude::*, time::Duration};
use zerocopy::{Immutable, IntoBytes, KnownLayout};

const fn to_fuse_attr(attr: &FileAttr) -> fuse_attr {
    fuse_attr {
        ino: attr.ino.into_raw(),
        size: attr.size,
        blocks: attr.blocks,
        atime: attr.atime.as_secs(),
        mtime: attr.mtime.as_secs(),
        ctime: attr.ctime.as_secs(),
        atimensec: attr.atime.subsec_nanos(),
        mtimensec: attr.mtime.subsec_nanos(),
        ctimensec: attr.ctime.subsec_nanos(),
        mode: attr.mode.into_raw(),
        nlink: attr.nlink,
        uid: attr.uid.as_raw(),
        gid: attr.gid.as_raw(),
        rdev: attr.rdev.into_kernel_dev(),
        blksize: attr.blksize,
        flags: 0,
    }
}

const fn to_fuse_attr_compat_8(attr: &FileAttr) -> fuse_attr_compat_8 {
    fuse_attr_compat_8 {
        ino: attr.ino.into_raw(),
        size: attr.size,
        blocks: attr.blocks,
        atime: attr.atime.as_secs(),
        mtime: attr.mtime.as_secs(),
        ctime: attr.ctime.as_secs(),
        atimensec: attr.atime.subsec_nanos(),
        mtimensec: attr.mtime.subsec_nanos(),
        ctimensec: attr.ctime.subsec_nanos(),
        mode: attr.mode.into_raw(),
        nlink: attr.nlink,
        uid: attr.uid.as_raw(),
        gid: attr.gid.as_raw(),
        rdev: attr.rdev.into_kernel_dev(),
    }
}

pub trait ReplySender {
    type Error;
    fn config(&self) -> &KernelConfig;
    fn send_bytes<B: Bytes>(self, bytes: B) -> Result<(), Self::Error>;
}

pub trait ReplyArg {
    fn reply<T>(self, sender: T) -> Result<(), T::Error>
    where
        T: ReplySender;
}

impl ReplyArg for () {
    #[inline]
    fn reply<T>(self, sender: T) -> Result<(), T::Error>
    where
        T: ReplySender,
    {
        sender.send_bytes(())
    }
}

pub struct Raw<B: Bytes>(pub B);

impl<B> ReplyArg for Raw<B>
where
    B: Bytes,
{
    fn reply<T>(self, sender: T) -> Result<(), T::Error>
    where
        T: ReplySender,
    {
        sender.send_bytes(self.0)
    }
}

#[derive(Debug)]
pub struct EntryOut<'a> {
    /// The inode number of this entry.
    ///
    /// If the value is not set, it means that the entry is *negative*.
    /// Returning a negative entry is also possible with the `ENOENT` error,
    /// but the *zeroed* entries also have the ability to specify the lifetime
    /// of the entry cache by using the `ttl_entry` parameter.
    pub ino: Option<NodeID>,

    /// The Attribute values about this entry.
    pub attr: Cow<'a, FileAttr>,

    /// The generation of this entry.
    ///
    /// This parameter is used to distinguish the inode from the past one
    /// when the filesystem reuse inode numbers.  That is, the operations
    /// must ensure that the pair of entry's inode number and generation
    /// are unique for the lifetime of the filesystem.
    pub generation: u64,

    /// Set the validity timeout for inode attributes.
    ///
    /// The operations should set this value to very large
    /// when the changes of inode attributes are caused
    /// only by FUSE requests.
    pub attr_valid: Option<Duration>,

    /// The validity timeout for the entry name.
    ///
    /// The operations should set this value to very large
    /// when the changes/deletions of directory entries are
    /// caused only by FUSE requests.
    pub entry_valid: Option<Duration>,
}

impl ReplyArg for EntryOut<'_> {
    fn reply<T>(self, sender: T) -> Result<(), T::Error>
    where
        T: ReplySender,
    {
        let nodeid = self.ino.map_or(0, |ino| ino.into_raw());
        let entry_valid = self.entry_valid.unwrap_or_default();
        let attr_valid = self.attr_valid.unwrap_or_default();
        if sender.config().minor <= 8 {
            sender.send_bytes(POD(fuse_entry_out_compat_8 {
                nodeid,
                generation: self.generation,
                entry_valid: entry_valid.as_secs(),
                attr_valid: attr_valid.as_secs(),
                entry_valid_nsec: entry_valid.subsec_nanos(),
                attr_valid_nsec: attr_valid.subsec_nanos(),
                attr: to_fuse_attr_compat_8(&self.attr),
            }))
        } else {
            sender.send_bytes(POD(fuse_entry_out {
                nodeid,
                generation: self.generation,
                entry_valid: entry_valid.as_secs(),
                attr_valid: attr_valid.as_secs(),
                entry_valid_nsec: entry_valid.subsec_nanos(),
                attr_valid_nsec: attr_valid.subsec_nanos(),
                attr: to_fuse_attr(&self.attr),
            }))
        }
    }
}

#[derive(Debug)]
pub struct AttrOut<'a> {
    pub attr: Cow<'a, FileAttr>,
    pub valid: Option<Duration>,
}

impl ReplyArg for AttrOut<'_> {
    fn reply<T>(self, sender: T) -> Result<(), T::Error>
    where
        T: ReplySender,
    {
        let valid = self.valid.unwrap_or_default();
        if sender.config().minor <= 8 {
            sender.send_bytes(POD(fuse_attr_out_compat_8 {
                attr_valid: valid.as_secs(),
                attr_valid_nsec: valid.subsec_nanos(),
                dummy: 0,
                attr: to_fuse_attr_compat_8(&self.attr),
            }))
        } else {
            sender.send_bytes(POD(fuse_attr_out {
                attr_valid: valid.as_secs(),
                attr_valid_nsec: valid.subsec_nanos(),
                dummy: 0,
                attr: to_fuse_attr(&self.attr),
            }))
        }
    }
}

#[derive(IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct WriteOut {
    raw: fuse_write_out,
}

impl ReplyArg for WriteOut {
    fn reply<T>(self, sender: T) -> Result<(), T::Error>
    where
        T: ReplySender,
    {
        sender.send_bytes(self.raw.as_bytes())
    }
}

impl WriteOut {
    pub const fn new(size: u32) -> Self {
        Self {
            raw: fuse_write_out { size, padding: 0 },
        }
    }
}

#[derive(Debug)]
pub struct OpenOut {
    /// The handle of opened file.
    pub fh: FileID,

    /// The flags for the opened file.
    pub open_flags: OpenOutFlags,

    pub backing_id: i32,
}

impl ReplyArg for OpenOut {
    fn reply<T>(self, sender: T) -> Result<(), T::Error>
    where
        T: ReplySender,
    {
        let backing_id = if sender.config().minor >= 40
            && sender.config().flags.contains(KernelFlags::PASSTHROUGH)
        {
            self.backing_id
        } else {
            0
        };
        sender.send_bytes(POD(fuse_open_out {
            fh: self.fh.into_raw(),
            open_flags: self.open_flags.bits(),
            backing_id,
        }))
    }
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct OpenOutFlags: u32 {
        /// Indicates that the direct I/O is used on this file.
        const DIRECT_IO = FOPEN_DIRECT_IO;

        /// Indicates that the currently cached file data in the kernel
        /// need not be invalidated.
        const KEEP_CACHE = FOPEN_KEEP_CACHE;

        /// Indicates that the opened file is not seekable.
        const NONSEEKABLE = FOPEN_NONSEEKABLE;

        /// Enable caching of entries returned by `readdir`.
        ///
        /// This flag is meaningful only for `opendir` operations.
        const CACHE_DIR = FOPEN_CACHE_DIR;

        const STREAM = FOPEN_STREAM;
        const NOFLUSH = FOPEN_NOFLUSH;
        const PARALLEL_DIRECT_WRITES = FOPEN_PARALLEL_DIRECT_WRITES;
        const PASSTHROUGH = FOPEN_PASSTHROUGH;
    }
}

impl OpenOutFlags {
    pub const PASSTHROUGH_MASK: Self = Self::PASSTHROUGH
        .union(Self::DIRECT_IO)
        .union(Self::PARALLEL_DIRECT_WRITES)
        .union(Self::NOFLUSH);
}

#[derive(IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct StatfsOut {
    raw: fuse_statfs_out,
}

impl ReplyArg for StatfsOut {
    fn reply<T>(self, sender: T) -> Result<(), T::Error>
    where
        T: ReplySender,
    {
        if sender.config().minor <= 3 {
            sender.send_bytes(POD(fuse_statfs_out_compat_3 {
                st: fuse_kstatfs_compat_3 {
                    blocks: self.raw.st.blocks,
                    bfree: self.raw.st.bfree,
                    bavail: self.raw.st.bavail,
                    files: self.raw.st.files,
                    ffree: self.raw.st.ffree,
                    bsize: self.raw.st.bsize,
                    namelen: self.raw.st.namelen,
                },
            }))
        } else {
            sender.send_bytes(self.raw.as_bytes())
        }
    }
}

impl StatfsOut {
    pub const fn new(st: &Statfs) -> Self {
        Self {
            raw: fuse_statfs_out {
                st: fuse_kstatfs {
                    bsize: st.bsize,
                    frsize: st.frsize,
                    blocks: st.blocks,
                    bfree: st.bfree,
                    bavail: st.bavail,
                    files: st.files,
                    ffree: st.ffree,
                    namelen: st.namelen,
                    padding: 0,
                    spare: [0; 6],
                },
            },
        }
    }
}

#[derive(IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct XattrOut {
    raw: fuse_getxattr_out,
}

impl ReplyArg for XattrOut {
    fn reply<T>(self, sender: T) -> Result<(), T::Error>
    where
        T: ReplySender,
    {
        sender.send_bytes(self.raw.as_bytes())
    }
}

impl XattrOut {
    pub const fn new(size: u32) -> Self {
        Self {
            raw: fuse_getxattr_out { size, padding: 0 },
        }
    }
}

#[derive(IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct LkOut {
    raw: fuse_lk_out,
}

impl ReplyArg for LkOut {
    fn reply<T>(self, sender: T) -> Result<(), T::Error>
    where
        T: ReplySender,
    {
        sender.send_bytes(self.raw.as_bytes())
    }
}

impl LkOut {
    pub const fn new(lk: &FileLock) -> Self {
        Self {
            raw: fuse_lk_out {
                lk: fuse_file_lock {
                    typ: lk.typ,
                    start: lk.start,
                    end: lk.end,
                    pid: lk.pid.as_raw_pid() as u32,
                },
            },
        }
    }
}

#[derive(IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct BmapOut {
    raw: fuse_bmap_out,
}

impl ReplyArg for BmapOut {
    fn reply<T>(self, sender: T) -> Result<(), T::Error>
    where
        T: ReplySender,
    {
        sender.send_bytes(self.raw.as_bytes())
    }
}

impl BmapOut {
    pub const fn new(block: u64) -> Self {
        Self {
            raw: fuse_bmap_out { block },
        }
    }
}

#[derive(IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct PollOut {
    raw: fuse_poll_out,
}

impl ReplyArg for PollOut {
    fn reply<T>(self, sender: T) -> Result<(), T::Error>
    where
        T: ReplySender,
    {
        sender.send_bytes(self.raw.as_bytes())
    }
}

impl PollOut {
    pub const fn new(revents: PollEvents) -> Self {
        Self {
            raw: fuse_poll_out {
                revents: revents.bits(),
                padding: 0,
            },
        }
    }
}

#[repr(transparent)]
pub struct LseekOut {
    raw: fuse_lseek_out,
}

impl ReplyArg for LseekOut {
    fn reply<T>(self, sender: T) -> Result<(), T::Error>
    where
        T: ReplySender,
    {
        sender.send_bytes(self.raw.as_bytes())
    }
}

impl LseekOut {
    pub const fn new(offset: u64) -> Self {
        Self {
            raw: fuse_lseek_out { offset },
        }
    }
}

pub struct ReaddirOut {
    buf: Vec<u8>,
}

impl ReplyArg for ReaddirOut {
    fn reply<T>(self, sender: T) -> Result<(), T::Error>
    where
        T: ReplySender,
    {
        sender.send_bytes(self.buf)
    }
}

impl ReaddirOut {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
        }
    }

    pub fn push_entry(
        &mut self,
        name: &OsStr,
        ino: NodeID,
        typ: Option<FileType>,
        offset: u64,
    ) -> bool {
        let name = name.as_bytes();
        let remaining = self.buf.capacity() - self.buf.len();

        let entry_size = mem::size_of::<fuse_dirent>() + name.len();
        let aligned_entry_size = aligned(entry_size);

        if remaining < aligned_entry_size {
            return true;
        }

        let typ = match typ {
            Some(FileType::BlockDevice) => libc::DT_BLK,
            Some(FileType::CharacterDevice) => libc::DT_CHR,
            Some(FileType::Directory) => libc::DT_DIR,
            Some(FileType::Fifo) => libc::DT_FIFO,
            Some(FileType::SymbolicLink) => libc::DT_LNK,
            Some(FileType::Regular) => libc::DT_REG,
            Some(FileType::Socket) => libc::DT_SOCK,
            None => libc::DT_UNKNOWN,
        };

        let dirent = fuse_dirent {
            ino: ino.into_raw(),
            off: offset,
            namelen: name.len().try_into().expect("name length is too long"),
            typ: typ as u32,
            name: [],
        };
        let lenbefore = self.buf.len();
        self.buf.extend_from_slice(dirent.as_bytes());
        self.buf.extend_from_slice(name);
        self.buf.resize(lenbefore + aligned_entry_size, 0);

        false
    }
}

#[inline]
const fn aligned(len: usize) -> usize {
    (len + mem::size_of::<u64>() - 1) & !(mem::size_of::<u64>() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocopy::TryFromBytes as _;

    struct TestReplySender<'a> {
        buf: &'a mut Vec<u8>,
        config: KernelConfig,
    }
    impl ReplySender for TestReplySender<'_> {
        type Error = std::convert::Infallible;
        fn config(&self) -> &KernelConfig {
            &self.config
        }
        fn send_bytes<B: Bytes>(self, bytes: B) -> Result<(), Self::Error> {
            bytes.fill_bytes(self.buf);
            Ok(())
        }
    }

    fn do_reply<T: ReplyArg>(arg: T) -> Vec<u8> {
        let mut buf = vec![];
        ReplyArg::reply(
            arg,
            TestReplySender {
                buf: &mut buf,
                config: KernelConfig::new(),
            },
        )
        .unwrap();
        buf
    }

    #[test]
    fn reply_unit() {
        assert_eq!(do_reply(()), b"");
    }

    #[test]
    fn reply_raw() {
        assert_eq!(do_reply(Raw((b"hello, ", "world"))), b"hello, world");
    }

    #[test]
    fn reply_entry_out() {
        let entry_out = EntryOut {
            ino: NodeID::from_raw(9876),
            attr: Cow::Owned(FileAttr { ..FileAttr::new() }),
            generation: 42,
            attr_valid: Some(Duration::new(22, 19)),
            entry_valid: None,
        };
        let bytes = do_reply(entry_out);
        let out = fuse_entry_out::try_ref_from_bytes(&bytes).unwrap();
        assert_eq!(out.nodeid, 9876);
        assert_eq!(out.generation, 42);
        assert_eq!(out.entry_valid, 0);
        assert_eq!(out.attr_valid, 22);
        assert_eq!(out.entry_valid_nsec, 0);
        assert_eq!(out.attr_valid_nsec, 19);
        // TODO: check out.attr
    }
}
