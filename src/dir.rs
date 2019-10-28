use memoffset::offset_of;
use polyfuse_sys::abi::fuse_dirent;
use std::{convert::TryFrom, ffi::OsStr, mem, os::unix::ffi::OsStrExt, ptr};

fn aligned(len: usize) -> usize {
    (len + mem::size_of::<u64>() - 1) & !(mem::size_of::<u64>() - 1)
}

#[derive(Debug)]
pub struct DirEntry {
    dirent_buf: Vec<u8>,
}

impl DirEntry {
    pub fn new(name: impl AsRef<OsStr>, ino: u64, off: u64, typ: u32) -> Self {
        let name = name.as_ref().as_bytes();
        let namelen = u32::try_from(name.len()).expect("the length of name is too large.");

        let entlen = mem::size_of::<fuse_dirent>() + name.len();
        let entsize = aligned(entlen);
        let padlen = entsize - entlen;

        let mut dirent_buf = Vec::<u8>::with_capacity(entsize);
        unsafe {
            let p = dirent_buf.as_mut_ptr();

            #[allow(clippy::cast_ptr_alignment)]
            let pheader = p as *mut fuse_dirent;
            (*pheader).ino = ino;
            (*pheader).off = off;
            (*pheader).namelen = namelen;
            (*pheader).typ = typ;

            #[allow(clippy::unneeded_field_pattern)]
            let p = p.add(offset_of!(fuse_dirent, name));
            ptr::copy_nonoverlapping(name.as_ptr(), p, name.len());

            let p = p.add(name.len());
            ptr::write_bytes(p, 0u8, padlen);

            dirent_buf.set_len(entsize);
        }

        Self { dirent_buf }
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

    pub fn nodeid(&self) -> u64 {
        unsafe { self.header().ino }
    }

    pub fn nodeid_mut(&mut self) -> &mut u64 {
        unsafe { &mut self.header_mut().ino }
    }

    pub fn offset(&self) -> u64 {
        unsafe { self.header().off }
    }

    pub fn offset_mut(&mut self) -> &mut u64 {
        unsafe { &mut self.header_mut().off }
    }

    pub fn type_(&self) -> u32 {
        unsafe { self.header().typ }
    }

    pub fn type_mut(&mut self) -> &mut u32 {
        unsafe { &mut self.header_mut().typ }
    }

    pub fn name(&self) -> &OsStr {
        #[allow(clippy::unneeded_field_pattern)]
        let name_offset = offset_of!(fuse_dirent, name);
        let namelen = unsafe { self.header().namelen as usize };
        OsStr::from_bytes(&self.dirent_buf[name_offset..name_offset + namelen])
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_direntry() {
        let dirent = DirEntry::new("hello", 1, 42, 0);
        assert_eq!(dirent.nodeid(), 1u64);
        assert_eq!(dirent.offset(), 42u64);
        assert_eq!(dirent.type_(), 0u32);
        assert_eq!(dirent.name(), "hello");

        assert_eq!(dirent.as_ref().len(), 32usize);
        assert_eq!(
            dirent.as_ref(),
            &*vec![
                1u8, 0, 0, 0, 0, 0, 0, 0, // ino
                42, 0, 0, 0, 0, 0, 0, 0, // off
                5, 0, 0, 0, // namlen
                0, 0, 0, 0, // typ
                104, 101, 108, 108, 111, // name
                0, 0, 0 // padding
            ]
        );
    }

    #[test]
    fn test_direntry_set_long_name() {
        let mut dirent = DirEntry::new("hello", 1, 42, 0);
        dirent.set_name("good evening");
        assert_eq!(dirent.nodeid(), 1u64);
        assert_eq!(dirent.offset(), 42u64);
        assert_eq!(dirent.type_(), 0u32);
        assert_eq!(dirent.name(), "good evening");

        assert_eq!(dirent.as_ref().len(), 40usize);
        assert_eq!(
            dirent.as_ref(),
            &*vec![
                1u8, 0, 0, 0, 0, 0, 0, 0, // ino
                42, 0, 0, 0, 0, 0, 0, 0, // off
                12, 0, 0, 0, // namelen
                0, 0, 0, 0, // typ
                103, 111, 111, 100, 32, 101, 118, 101, 110, 105, 110, 103, // name
                0, 0, 0, 0 // padding
            ]
        );
    }

    #[test]
    fn test_direntry_set_short_name() {
        let mut dirent = DirEntry::new("good morning", 1, 42, 0);
        dirent.set_name("bye");
        assert_eq!(dirent.nodeid(), 1u64);
        assert_eq!(dirent.offset(), 42u64);
        assert_eq!(dirent.type_(), 0u32);
        assert_eq!(dirent.name(), "bye");

        assert_eq!(dirent.as_ref().len(), 32usize);
        assert_eq!(
            dirent.as_ref(),
            &*vec![
                1u8, 0, 0, 0, 0, 0, 0, 0, // ino
                42, 0, 0, 0, 0, 0, 0, 0, // off
                3, 0, 0, 0, // namelen
                0, 0, 0, 0, // off
                98, 121, 101, // name
                0, 0, 0, 0, 0 // padding
            ]
        );
    }
}
