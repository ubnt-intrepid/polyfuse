use crate::{
    kernel::fuse_dirent,
    reply::{Collector, Reply},
};
use memoffset::offset_of;
use std::{convert::TryFrom, ffi::OsStr, mem, os::unix::ffi::OsStrExt, ptr};

fn aligned(len: usize) -> usize {
    (len + mem::size_of::<u64>() - 1) & !(mem::size_of::<u64>() - 1)
}

/// A directory entry replied to the kernel.
#[derive(Debug)]
pub struct DirEntry {
    dirent_buf: Vec<u8>,
}

impl DirEntry {
    /// Create a new `DirEntry`.
    #[allow(clippy::cast_ptr_alignment, clippy::cast_lossless)]
    pub fn new(name: impl AsRef<OsStr>, ino: u64, off: u64) -> Self {
        let name = name.as_ref().as_bytes();
        let namelen = u32::try_from(name.len()).expect("the length of name is too large.");

        let entlen = mem::size_of::<fuse_dirent>() + name.len();
        let entsize = aligned(entlen);
        let padlen = entsize - entlen;

        let mut dirent_buf = Vec::<u8>::with_capacity(entsize);
        unsafe {
            let p = dirent_buf.as_mut_ptr();

            let pheader = p as *mut fuse_dirent;
            (*pheader).ino = ino;
            (*pheader).off = off;
            (*pheader).namelen = namelen;
            (*pheader).typ = libc::DT_UNKNOWN as u32;

            #[allow(clippy::unneeded_field_pattern)]
            let p = p.add(offset_of!(fuse_dirent, name));
            ptr::copy_nonoverlapping(name.as_ptr(), p, name.len());

            let p = p.add(name.len());
            ptr::write_bytes(p, 0u8, padlen);

            dirent_buf.set_len(entsize);
        }

        Self { dirent_buf }
    }

    /// Create a `DirEntry` for a directory.
    #[allow(clippy::cast_lossless)]
    pub fn dir(name: impl AsRef<OsStr>, ino: u64, off: u64) -> Self {
        let mut ent = Self::new(name, ino, off);
        ent.set_typ(libc::DT_DIR as u32);
        ent
    }

    /// Create a `DirEntry` for a regular file.
    #[allow(clippy::cast_lossless)]
    pub fn file(name: impl AsRef<OsStr>, ino: u64, off: u64) -> Self {
        let mut ent = Self::new(name, ino, off);
        ent.set_typ(libc::DT_REG as u32);
        ent
    }

    unsafe fn header(&self) -> &fuse_dirent {
        debug_assert!(self.dirent_buf.len() > mem::size_of::<fuse_dirent>());
        #[allow(clippy::cast_ptr_alignment)]
        &*(self.dirent_buf.as_ptr() as *mut fuse_dirent)
    }

    unsafe fn header_mut(&mut self) -> &mut fuse_dirent {
        debug_assert!(self.dirent_buf.len() > mem::size_of::<fuse_dirent>());
        #[allow(clippy::cast_ptr_alignment)]
        &mut *(self.dirent_buf.as_mut_ptr() as *mut fuse_dirent)
    }

    /// Return the inode number of this entry.
    pub fn nodeid(&self) -> u64 {
        unsafe { self.header().ino }
    }

    /// Set the inode number of this entry.
    pub fn set_nodeid(&mut self, ino: u64) {
        unsafe {
            self.header_mut().ino = ino;
        }
    }

    /// Return the offset value of this entry.
    pub fn offset(&self) -> u64 {
        unsafe { self.header().off }
    }

    /// Set the offset value of this entry.
    pub fn set_offset(&mut self, off: u64) {
        unsafe {
            self.header_mut().off = off;
        }
    }

    /// Return the type of this entry.
    pub fn typ(&self) -> u32 {
        unsafe { self.header().typ }
    }

    /// Set the type of this entry.
    pub fn set_typ(&mut self, typ: u32) {
        unsafe {
            self.header_mut().typ = typ;
        }
    }

    /// Returns the name of this entry.
    pub fn name(&self) -> &OsStr {
        #[allow(clippy::unneeded_field_pattern)]
        let name_offset = offset_of!(fuse_dirent, name);
        let namelen = unsafe { self.header().namelen as usize };
        OsStr::from_bytes(&self.dirent_buf[name_offset..name_offset + namelen])
    }

    /// Set the name of this entry.
    #[allow(clippy::cast_ptr_alignment)]
    pub fn set_name(&mut self, name: impl AsRef<OsStr>) {
        let name = name.as_ref().as_bytes();
        let namelen = u32::try_from(name.len()).expect("the length of name is too large");

        let entlen = mem::size_of::<fuse_dirent>() + name.len();
        let entsize = aligned(entlen);
        let padlen = entsize - entlen;

        if self.dirent_buf.capacity() < entsize {
            self.dirent_buf
                .reserve_exact(entsize - self.dirent_buf.len());
        }

        unsafe {
            let p = self.dirent_buf.as_mut_ptr();

            #[allow(clippy::cast_ptr_alignment)]
            let pheader = p as *mut fuse_dirent;
            (*pheader).namelen = namelen;

            #[allow(clippy::unneeded_field_pattern)]
            let p = p.add(offset_of!(fuse_dirent, name));
            ptr::copy_nonoverlapping(name.as_ptr(), p, name.len());

            let p = p.add(name.len());
            ptr::write_bytes(p, 0u8, padlen);

            self.dirent_buf.set_len(entsize);
        }
    }
}

impl AsRef<[u8]> for DirEntry {
    fn as_ref(&self) -> &[u8] {
        self.dirent_buf.as_ref()
    }
}

impl Reply for DirEntry {
    #[inline]
    fn collect_bytes<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        collector.append(self.as_ref());
    }
}

#[allow(clippy::cast_lossless)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_cases() {
        assert_eq!(aligned(1), 8);
        assert_eq!(aligned(7), 8);
        assert_eq!(aligned(8), 8);
        assert_eq!(aligned(9), 16);
        assert_eq!(aligned(15), 16);
        assert_eq!(aligned(16), 16);
        assert_eq!(aligned(17), 24);
        assert_eq!(aligned(23), 24);
        assert_eq!(aligned(24), 24);
        assert_eq!(aligned(25), 32);

        assert_eq!(aligned(5), 8);
    }

    #[test]
    fn smoke_debug() {
        dbg!(DirEntry::new("hello", 1, 0));
    }

    #[test]
    fn new_dirent() {
        let dirent = DirEntry::new("hello", 1, 42);
        assert_eq!(dirent.nodeid(), 1u64);
        assert_eq!(dirent.offset(), 42u64);
        assert_eq!(dirent.typ(), libc::DT_UNKNOWN as u32);
        assert_eq!(dirent.name(), "hello");

        assert_eq!(dirent.as_ref().len(), 32usize);
        assert_eq!(
            dirent.as_ref(),
            &*vec![
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // ino
                0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // off
                0x05, 0x00, 0x00, 0x00, // namlen
                0x00, 0x00, 0x00, 0x00, // typ
                0x68, 0x65, 0x6c, 0x6c, 0x6f, // name
                0x00, 0x00, 0x00, // padding
            ]
        );
    }

    #[allow(clippy::cast_lossless)]
    #[test]
    fn set_attributes() {
        let mut dirent = DirEntry::new("hello", 1, 0);

        dirent.set_nodeid(2);
        dirent.set_offset(90);
        dirent.set_typ(libc::DT_DIR as u32);

        assert_eq!(dirent.nodeid(), 2u64);
        assert_eq!(dirent.offset(), 90u64);
        assert_eq!(dirent.typ(), libc::DT_DIR as u32);
    }

    #[test]
    fn set_long_name() {
        let mut dirent = DirEntry::new("hello", 1, 42);
        dirent.set_name("good evening");
        assert_eq!(dirent.nodeid(), 1u64);
        assert_eq!(dirent.offset(), 42u64);
        assert_eq!(dirent.typ(), libc::DT_UNKNOWN as u32);
        assert_eq!(dirent.name(), "good evening");

        assert_eq!(dirent.as_ref().len(), 40usize);
        assert_eq!(
            dirent.as_ref(),
            &*vec![
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // ino
                0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // off
                0x0c, 0x00, 0x00, 0x00, // namelen
                0x00, 0x00, 0x00, 0x00, // typ
                0x67, 0x6f, 0x6f, 0x64, 0x20, 0x65, 0x76, 0x65, 0x6e, 0x69, 0x6e,
                0x67, // name
                0x00, 0x00, 0x00, 0x00, // padding
            ]
        );
    }

    #[test]
    fn set_short_name() {
        let mut dirent = DirEntry::new("good morning", 1, 42);
        dirent.set_name("bye");
        assert_eq!(dirent.nodeid(), 1u64);
        assert_eq!(dirent.offset(), 42u64);
        assert_eq!(dirent.typ(), libc::DT_UNKNOWN as u32);
        assert_eq!(dirent.name(), "bye");

        assert_eq!(dirent.as_ref().len(), 32usize);
        assert_eq!(
            dirent.as_ref(),
            &*vec![
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // ino
                0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // off
                0x03, 0x00, 0x00, 0x00, // namelen
                0x00, 0x00, 0x00, 0x00, // typ
                0x62, 0x79, 0x65, // name
                0x00, 0x00, 0x00, 0x00, 0x00, // padding
            ]
        );
    }
}
