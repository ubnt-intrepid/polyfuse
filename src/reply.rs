use crate::{
    bytes::{Bytes, POD},
    init::{KernelConfig, KernelFlags},
    types::{FileAttr, FileID, FileLock, FileType, NodeID, PollEvents, Statfs},
};
use bitflags::bitflags;
use polyfuse_kernel::*;
use rustix::io::Errno;
use std::{ffi::OsStr, io, mem, os::unix::prelude::*, time::Duration};
use zerocopy::IntoBytes as _;

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

pub trait ReplySender: Sized {
    fn config(&self) -> &KernelConfig;

    fn reply_raw<B>(self, error: Option<Errno>, bytes: B) -> io::Result<()>
    where
        B: Bytes;

    fn reply_bytes<B>(self, bytes: B) -> io::Result<()>
    where
        B: Bytes,
    {
        self.reply_raw(None, bytes)
    }

    fn reply_error(self, error: Errno) -> io::Result<()> {
        self.reply_raw(Some(error), ())
    }

    fn reply_entry(
        self,
        ino: Option<NodeID>,
        attr: impl AsRef<FileAttr>,
        generation: u64,
        attr_valid: Option<Duration>,
        entry_valid: Option<Duration>,
    ) -> io::Result<()> {
        let attr = attr.as_ref();
        let nodeid = ino.map_or(0, |ino| ino.into_raw());
        let entry_valid = entry_valid.unwrap_or_default();
        let attr_valid = attr_valid.unwrap_or_default();
        if self.config().minor <= 8 {
            self.reply_raw(
                None,
                POD(fuse_entry_out_compat_8 {
                    nodeid,
                    generation,
                    entry_valid: entry_valid.as_secs(),
                    attr_valid: attr_valid.as_secs(),
                    entry_valid_nsec: entry_valid.subsec_nanos(),
                    attr_valid_nsec: attr_valid.subsec_nanos(),
                    attr: to_fuse_attr_compat_8(attr),
                }),
            )
        } else {
            self.reply_raw(
                None,
                POD(fuse_entry_out {
                    nodeid,
                    generation,
                    entry_valid: entry_valid.as_secs(),
                    attr_valid: attr_valid.as_secs(),
                    entry_valid_nsec: entry_valid.subsec_nanos(),
                    attr_valid_nsec: attr_valid.subsec_nanos(),
                    attr: to_fuse_attr(attr),
                }),
            )
        }
    }

    fn reply_attr(self, attr: impl AsRef<FileAttr>, valid: Option<Duration>) -> io::Result<()> {
        let attr = attr.as_ref();
        let valid = valid.unwrap_or_default();
        if self.config().minor <= 8 {
            self.reply_raw(
                None,
                POD(fuse_attr_out_compat_8 {
                    attr_valid: valid.as_secs(),
                    attr_valid_nsec: valid.subsec_nanos(),
                    dummy: 0,
                    attr: to_fuse_attr_compat_8(attr),
                }),
            )
        } else {
            self.reply_raw(
                None,
                POD(fuse_attr_out {
                    attr_valid: valid.as_secs(),
                    attr_valid_nsec: valid.subsec_nanos(),
                    dummy: 0,
                    attr: to_fuse_attr(attr),
                }),
            )
        }
    }

    fn reply_write(self, size: u32) -> io::Result<()> {
        self.reply_raw(None, POD(fuse_write_out { size, padding: 0 }))
    }

    fn reply_open(self, fh: FileID, open_flags: OpenOutFlags, backing_id: i32) -> io::Result<()> {
        let enabled_passthrough =
            self.config().minor >= 40 && self.config().flags.contains(KernelFlags::PASSTHROUGH);
        self.reply_raw(
            None,
            POD(fuse_open_out {
                fh: fh.into_raw(),
                open_flags: open_flags.bits(),
                backing_id: if enabled_passthrough { backing_id } else { 0 },
            }),
        )
    }

    fn reply_statfs(self, st: impl AsRef<Statfs>) -> io::Result<()> {
        let st = st.as_ref();
        if self.config().minor <= 3 {
            self.reply_raw(
                None,
                POD(fuse_statfs_out_compat_3 {
                    st: fuse_kstatfs_compat_3 {
                        blocks: st.blocks,
                        bfree: st.bfree,
                        bavail: st.bavail,
                        files: st.files,
                        ffree: st.ffree,
                        bsize: st.bsize,
                        namelen: st.namelen,
                    },
                }),
            )
        } else {
            self.reply_raw(
                None,
                POD(fuse_statfs_out {
                    st: fuse_kstatfs {
                        blocks: st.blocks,
                        bfree: st.bfree,
                        bavail: st.bavail,
                        files: st.files,
                        ffree: st.ffree,
                        bsize: st.bsize,
                        namelen: st.namelen,
                        frsize: st.frsize,
                        padding: 0,
                        spare: [0; 6],
                    },
                }),
            )
        }
    }

    fn reply_xattr(self, size: u32) -> io::Result<()> {
        self.reply_raw(None, POD(fuse_getxattr_out { size, padding: 0 }))
    }

    fn reply_lock(self, lk: impl AsRef<FileLock>) -> io::Result<()> {
        let lk = lk.as_ref();
        self.reply_raw(
            None,
            POD(fuse_lk_out {
                lk: fuse_file_lock {
                    start: lk.start,
                    end: lk.end,
                    typ: lk.typ,
                    pid: lk.pid.as_raw_pid() as u32,
                },
            }),
        )
    }

    fn reply_bmap(self, block: u64) -> io::Result<()> {
        self.reply_raw(None, POD(fuse_bmap_out { block }))
    }

    fn reply_poll(self, revents: PollEvents) -> io::Result<()> {
        self.reply_raw(
            None,
            POD(fuse_poll_out {
                revents: revents.bits(),
                padding: 0,
            }),
        )
    }

    fn reply_lseek(self, offset: u64) -> io::Result<()> {
        self.reply_raw(None, POD(fuse_lseek_out { offset }))
    }

    fn reply_dir(self, out: &DirEntryBuf) -> io::Result<()> {
        self.reply_raw(None, &out.buf)
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

pub struct DirEntryBuf {
    buf: Vec<u8>,
}

impl DirEntryBuf {
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
    use bytes::Buf as _;
    use zerocopy::TryFromBytes as _;

    struct TestReplySender<'a> {
        buf: &'a mut Vec<u8>,
        config: KernelConfig,
    }
    impl ReplySender for TestReplySender<'_> {
        fn config(&self) -> &KernelConfig {
            &self.config
        }
        fn reply_raw<B: Bytes>(self, _: Option<Errno>, bytes: B) -> io::Result<()> {
            bytes.fill_bytes(self.buf);
            Ok(())
        }
    }

    fn do_reply(f: impl FnOnce(TestReplySender<'_>) -> io::Result<()>) -> Vec<u8> {
        let mut buf = vec![];
        f(TestReplySender {
            buf: &mut buf,
            config: KernelConfig::new(),
        })
        .unwrap();
        buf
    }

    #[test]
    fn reply_raw() {
        assert_eq!(do_reply(|s| s.reply_raw(None, ())), b"");
        assert_eq!(
            do_reply(|s| s.reply_raw(None, (b"hello, ", "world"))),
            b"hello, world"
        );
    }

    #[test]
    fn reply_entry_out() {
        let bytes = do_reply(|s| {
            s.reply_entry(
                NodeID::from_raw(9876),
                &FileAttr { ..FileAttr::new() },
                42,
                Some(Duration::new(22, 19)),
                None,
            )
        });
        let out = fuse_entry_out::try_ref_from_bytes(&bytes).unwrap();
        assert_eq!(out.nodeid, 9876);
        assert_eq!(out.generation, 42);
        assert_eq!(out.entry_valid, 0);
        assert_eq!(out.attr_valid, 22);
        assert_eq!(out.entry_valid_nsec, 0);
        assert_eq!(out.attr_valid_nsec, 19);
        // TODO: check out.attr
    }

    #[test]
    fn reply_entry_out_compat() {
        let bytes = {
            let mut buf = vec![];
            TestReplySender {
                buf: &mut buf,
                config: KernelConfig {
                    minor: 5,
                    ..KernelConfig::new()
                },
            }
            .reply_entry(
                NodeID::from_raw(9876),
                &FileAttr { ..FileAttr::new() },
                42,
                Some(Duration::new(22, 19)),
                None,
            )
            .unwrap();
            buf
        };
        let out = fuse_entry_out_compat_8::try_ref_from_bytes(&bytes).unwrap();
        assert_eq!(out.nodeid, 9876);
        assert_eq!(out.generation, 42);
        assert_eq!(out.entry_valid, 0);
        assert_eq!(out.attr_valid, 22);
        assert_eq!(out.entry_valid_nsec, 0);
        assert_eq!(out.attr_valid_nsec, 19);
    }

    #[test]
    fn reply_open_out() {
        let out = do_reply(|s| s.reply_open(FileID::from_raw(22), OpenOutFlags::DIRECT_IO, 2));
        let out = fuse_open_out::try_ref_from_bytes(&out).unwrap();
        assert_eq!(out.fh, 22);
        assert_eq!(out.open_flags, FOPEN_DIRECT_IO);
        assert_eq!(out.backing_id, 0);
    }

    #[test]
    fn reply_misc_types() {
        let out = do_reply(|s| s.reply_write(9876));
        let out = fuse_write_out::try_ref_from_bytes(&out).unwrap();
        assert_eq!(out.size, 9876);
        assert_eq!(out.padding, 0);

        let out = do_reply(|s| s.reply_xattr(6));
        let out = fuse_getxattr_out::try_ref_from_bytes(&out).unwrap();
        assert_eq!(out.size, 6);
        assert_eq!(out.padding, 0);

        let out = do_reply(|s| s.reply_bmap(3314));
        let out = fuse_bmap_out::try_ref_from_bytes(&out).unwrap();
        assert_eq!(out.block, 3314);

        let out = do_reply(|s| s.reply_poll(PollEvents::HUP));
        let out = fuse_poll_out::try_ref_from_bytes(&out).unwrap();
        assert_eq!(out.revents, PollEvents::HUP.bits());
        assert_eq!(out.padding, 0);

        let out = do_reply(|s| s.reply_lseek(1192));
        let out = fuse_lseek_out::try_ref_from_bytes(&out).unwrap();
        assert_eq!(out.offset, 1192);
    }

    #[test]
    fn reply_readdir_out() {
        let mut out = DirEntryBuf::new(1024);
        out.push_entry(
            ".gitignore".as_ref(),
            NodeID::from_raw(9).unwrap(),
            Some(FileType::Regular),
            2,
        );
        out.push_entry(
            "README.md".as_ref(),
            NodeID::from_raw(18).unwrap(),
            Some(FileType::SymbolicLink),
            3,
        );
        let out = do_reply(|s| s.reply_dir(&out));
        assert_eq!(out.len(), 80);

        let mut out = &out[..];

        assert_eq!(
            &out[..mem::size_of::<fuse_dirent>()],
            fuse_dirent {
                ino: 9,
                off: 2,
                namelen: 10,
                typ: libc::DT_REG as u32,
                name: []
            }
            .as_bytes()
        );
        out.advance(mem::size_of::<fuse_dirent>());
        assert_eq!(&out[0..10], b".gitignore");
        assert_eq!(&out[10..16], [0u8; 6]);
        out.advance(16);

        assert_eq!(
            &out[..mem::size_of::<fuse_dirent>()],
            fuse_dirent {
                ino: 18,
                off: 3,
                namelen: 9,
                typ: libc::DT_LNK as u32,
                name: []
            }
            .as_bytes()
        );
        out.advance(mem::size_of::<fuse_dirent>());
        assert_eq!(&out[0..9], b"README.md");
        assert_eq!(&out[9..16], [0u8; 7]);
        out.advance(16);
    }
}
