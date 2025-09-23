use crate::{
    bytes::Bytes,
    types::{FileAttr, FileID, FileLock, FileType, NodeID, PollEvents, Statfs},
};
use bitflags::bitflags;
use polyfuse_kernel::*;
use std::{ffi::OsStr, mem, os::unix::prelude::*, time::Duration};
use zerocopy::{FromZeros as _, IntoBytes as _};

fn fill_fuse_attr(slot: &mut fuse_attr, attr: &FileAttr) {
    slot.ino = attr.ino.into_raw();
    slot.size = attr.size;
    slot.blocks = attr.blocks;
    slot.atime = attr.atime.as_secs();
    slot.mtime = attr.mtime.as_secs();
    slot.ctime = attr.ctime.as_secs();
    slot.atimensec = attr.atime.subsec_nanos();
    slot.mtimensec = attr.mtime.subsec_nanos();
    slot.ctimensec = attr.ctime.subsec_nanos();
    slot.mode = attr.mode.into_raw();
    slot.nlink = attr.nlink;
    slot.uid = attr.uid.as_raw();
    slot.gid = attr.gid.as_raw();
    slot.rdev = attr.rdev.into_kernel_dev();
    slot.blksize = attr.blksize;
}

pub trait ReplySender {
    type Ok;
    type Error;

    fn send<B>(self, arg: B) -> Result<Self::Ok, Self::Error>
    where
        B: Bytes;
}

pub struct ReplyUnit<T> {
    sender: T,
}

impl<T> ReplyUnit<T>
where
    T: ReplySender,
{
    pub(crate) const fn new(sender: T) -> Self {
        Self { sender }
    }

    pub fn send(self) -> Result<T::Ok, T::Error> {
        self.sender.send(())
    }
}

pub struct ReplyEntry<T> {
    sender: T,
    out: fuse_entry_out,
}

impl<T> ReplyEntry<T>
where
    T: ReplySender,
{
    pub(crate) fn new(sender: T) -> Self {
        Self {
            sender,
            out: fuse_entry_out::new_zeroed(),
        }
    }

    /// Set the inode number of this entry.
    ///
    /// If the value is not set, it means that the entry is *negative*.
    /// Returning a negative entry is also possible with the `ENOENT` error,
    /// but the *zeroed* entries also have the ability to specify the lifetime
    /// of the entry cache by using the `ttl_entry` parameter.
    #[inline]
    pub fn ino(&mut self, ino: NodeID) {
        self.out.nodeid = ino.into_raw();
    }

    /// Fill attribute values about this entry.
    #[inline]
    pub fn attr(&mut self, attr: &FileAttr) {
        fill_fuse_attr(&mut self.out.attr, attr);
    }

    /// Set the generation of this entry.
    ///
    /// This parameter is used to distinguish the inode from the past one
    /// when the filesystem reuse inode numbers.  That is, the operations
    /// must ensure that the pair of entry's inode number and generation
    /// are unique for the lifetime of the filesystem.
    pub fn generation(&mut self, generation: u64) {
        self.out.generation = generation;
    }

    /// Set the validity timeout for inode attributes.
    ///
    /// The operations should set this value to very large
    /// when the changes of inode attributes are caused
    /// only by FUSE requests.
    pub fn ttl_attr(&mut self, ttl: Duration) {
        self.out.attr_valid = ttl.as_secs();
        self.out.attr_valid_nsec = ttl.subsec_nanos();
    }

    /// Set the validity timeout for the name.
    ///
    /// The operations should set this value to very large
    /// when the changes/deletions of directory entries are
    /// caused only by FUSE requests.
    pub fn ttl_entry(&mut self, ttl: Duration) {
        self.out.entry_valid = ttl.as_secs();
        self.out.entry_valid_nsec = ttl.subsec_nanos();
    }

    pub fn send(self) -> Result<T::Ok, T::Error> {
        self.sender.send(self.out.as_bytes())
    }
}

pub struct ReplyAttr<T> {
    sender: T,
    out: fuse_attr_out,
}

impl<T> ReplyAttr<T>
where
    T: ReplySender,
{
    pub(crate) fn new(sender: T) -> Self {
        Self {
            sender,
            out: fuse_attr_out::new_zeroed(),
        }
    }

    /// Return the object to fill attribute values.
    #[inline]
    pub fn attr(&mut self, attr: &FileAttr) {
        fill_fuse_attr(&mut self.out.attr, attr);
    }

    /// Set the validity timeout for this attribute.
    pub fn ttl(&mut self, ttl: Duration) {
        self.out.attr_valid = ttl.as_secs();
        self.out.attr_valid_nsec = ttl.subsec_nanos();
    }

    pub fn send(self) -> Result<T::Ok, T::Error> {
        self.sender.send(self.out.as_bytes())
    }
}

pub struct ReplyData<T> {
    sender: T,
}

impl<T> ReplyData<T>
where
    T: ReplySender,
{
    pub(crate) fn new(sender: T) -> Self {
        Self { sender }
    }

    pub fn send<B>(self, bytes: B) -> Result<T::Ok, T::Error>
    where
        B: Bytes,
    {
        self.sender.send(bytes)
    }
}

pub struct ReplyWrite<T> {
    sender: T,
}

impl<T> ReplyWrite<T>
where
    T: ReplySender,
{
    pub(crate) fn new(sender: T) -> Self {
        Self { sender }
    }

    pub fn send(self, size: u32) -> Result<T::Ok, T::Error> {
        self.sender
            .send(fuse_write_out { size, padding: 0 }.as_bytes())
    }
}

pub struct ReplyDir<T> {
    sender: T,
    buf: Vec<u8>,
}

impl<T> ReplyDir<T>
where
    T: ReplySender,
{
    pub(crate) fn new(sender: T, capacity: usize) -> Self {
        Self {
            sender,
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

    pub fn send(self) -> Result<T::Ok, T::Error> {
        self.sender.send(&self.buf[..])
    }
}

#[inline]
const fn aligned(len: usize) -> usize {
    (len + mem::size_of::<u64>() - 1) & !(mem::size_of::<u64>() - 1)
}

pub struct ReplyOpen<T> {
    sender: T,
    out: fuse_open_out,
}

impl<T> ReplyOpen<T>
where
    T: ReplySender,
{
    pub(crate) fn new(sender: T) -> Self {
        Self {
            sender,
            out: fuse_open_out::new_zeroed(),
        }
    }

    /// Set the handle of opened file.
    pub fn fh(&mut self, fh: FileID) {
        self.out.fh = fh.into_raw();
    }

    /// Specify the flags for the opened file.
    pub fn flags(&mut self, flags: OpenOutFlags) {
        self.out.open_flags = flags.bits();
    }

    pub fn send(self) -> Result<T::Ok, T::Error> {
        self.sender.send(self.out.as_bytes())
    }
}

pub struct ReplyCreate<T> {
    sender: T,
    entry_out: fuse_entry_out,
    open_out: fuse_open_out,
}

impl<T> ReplyCreate<T>
where
    T: ReplySender,
{
    pub(crate) fn new(sender: T) -> Self {
        Self {
            sender,
            entry_out: fuse_entry_out::new_zeroed(),
            open_out: fuse_open_out::new_zeroed(),
        }
    }

    /// Set the inode number of this entry.
    ///
    /// See [`ReplyEntry::ino`] for details.
    #[inline]
    pub fn ino(&mut self, ino: NodeID) {
        self.entry_out.nodeid = ino.into_raw();
    }

    /// Fill attribute values about this entry.
    #[inline]
    pub fn attr(&mut self, attr: &FileAttr) {
        fill_fuse_attr(&mut self.entry_out.attr, attr);
    }

    /// Set the generation of this entry.
    ///
    /// See [`ReplyEntry::generation`] for details.
    pub fn generation(&mut self, generation: u64) {
        self.entry_out.generation = generation;
    }

    /// Set the validity timeout for inode attributes.
    ///
    /// See [`ReplyEntry::ttl_attr`] for details.
    pub fn ttl_attr(&mut self, ttl: Duration) {
        self.entry_out.attr_valid = ttl.as_secs();
        self.entry_out.attr_valid_nsec = ttl.subsec_nanos();
    }

    /// Set the validity timeout for the name.
    ///
    /// See [`ReplyEntry::ttl_entry`] for details.
    pub fn ttl_entry(&mut self, ttl: Duration) {
        self.entry_out.entry_valid = ttl.as_secs();
        self.entry_out.entry_valid_nsec = ttl.subsec_nanos();
    }

    /// Set the handle of opened file.
    pub fn fh(&mut self, fh: FileID) {
        self.open_out.fh = fh.into_raw();
    }

    /// Specify the flags for the opened file.
    pub fn flags(&mut self, flags: OpenOutFlags) {
        self.open_out.open_flags = flags.bits();
    }

    pub fn send(self) -> Result<T::Ok, T::Error> {
        self.sender
            .send((self.entry_out.as_bytes(), self.open_out.as_bytes()))
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
    }
}

pub struct ReplyStatfs<T> {
    sender: T,
}

impl<T> ReplyStatfs<T>
where
    T: ReplySender,
{
    pub(crate) fn new(sender: T) -> Self {
        Self { sender }
    }

    pub fn send(self, st: &Statfs) -> Result<T::Ok, T::Error> {
        self.sender.send(
            fuse_statfs_out {
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
            }
            .as_bytes(),
        )
    }
}

pub struct ReplyXattr<T> {
    sender: T,
}

impl<T> ReplyXattr<T>
where
    T: ReplySender,
{
    pub(crate) fn new(sender: T) -> Self {
        Self { sender }
    }

    pub fn send_size(self, size: u32) -> Result<T::Ok, T::Error> {
        self.sender
            .send(fuse_getxattr_out { size, padding: 0 }.as_bytes())
    }

    pub fn send_value<B>(self, data: B) -> Result<T::Ok, T::Error>
    where
        B: Bytes,
    {
        self.sender.send(data)
    }
}

pub struct ReplyLock<T> {
    sender: T,
}

impl<T> ReplyLock<T>
where
    T: ReplySender,
{
    pub(crate) fn new(sender: T) -> Self {
        Self { sender }
    }

    pub fn send(self, lk: &FileLock) -> Result<T::Ok, T::Error> {
        self.sender.send(
            fuse_lk_out {
                lk: fuse_file_lock {
                    typ: lk.typ,
                    start: lk.start,
                    end: lk.end,
                    pid: lk.pid.as_raw_pid() as u32,
                },
            }
            .as_bytes(),
        )
    }
}

pub struct ReplyBmap<T> {
    sender: T,
}

impl<T> ReplyBmap<T>
where
    T: ReplySender,
{
    pub(crate) fn new(sender: T) -> Self {
        Self { sender }
    }

    pub fn send(self, block: u64) -> Result<T::Ok, T::Error> {
        self.sender.send(fuse_bmap_out { block }.as_bytes())
    }
}

pub struct ReplyPoll<T> {
    sender: T,
}

impl<T> ReplyPoll<T>
where
    T: ReplySender,
{
    pub(crate) fn new(sender: T) -> Self {
        Self { sender }
    }

    pub fn send(self, revents: PollEvents) -> Result<T::Ok, T::Error> {
        self.sender.send(
            fuse_poll_out {
                revents: revents.bits(),
                padding: 0,
            }
            .as_bytes(),
        )
    }
}

pub struct ReplyLseek<T> {
    sender: T,
}

impl<T> ReplyLseek<T>
where
    T: ReplySender,
{
    pub(crate) fn new(sender: T) -> Self {
        Self { sender }
    }

    pub fn send(self, offset: u64) -> Result<T::Ok, T::Error> {
        self.sender.send(fuse_lseek_out { offset }.as_bytes())
    }
}
