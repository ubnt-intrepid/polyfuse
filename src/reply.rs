//! Replies to the kernel.

use crate::abi::{
    AttrOut, //
    BmapOut,
    DirEntry as DirEntryHeader,
    EntryOut,
    GetxattrOut,
    LkOut,
    Nodeid,
    OpenOut,
    OutHeader,
    StatfsOut,
    Unique,
    WriteOut,
};
use futures::io::{AsyncWrite, AsyncWriteExt};
use memoffset::offset_of;
use smallvec::SmallVec;
use std::{
    convert::TryFrom,
    ffi::OsStr,
    fmt,
    io::{self, IoSlice},
    mem,
    os::unix::ffi::OsStrExt,
    pin::Pin,
    ptr,
};

fn aligned(len: usize) -> usize {
    (len + mem::size_of::<u64>() - 1) & !(mem::size_of::<u64>() - 1)
}

#[derive(Debug)]
pub struct DirEntry {
    dirent_buf: Vec<u8>,
}

impl DirEntry {
    pub fn new(name: impl AsRef<OsStr>) -> Self {
        let name = name.as_ref().as_bytes();
        let namelen = u32::try_from(name.len()).expect("the length of name is too large.");

        let entlen = mem::size_of::<DirEntryHeader>() + name.len();
        let entsize = aligned(entlen);
        let padlen = entsize - entlen;

        let mut dirent_buf = Vec::<u8>::with_capacity(entsize);
        unsafe {
            let p = dirent_buf.as_mut_ptr();

            #[allow(clippy::cast_ptr_alignment)]
            let pheader = p as *mut DirEntryHeader;
            (*pheader).ino = Nodeid::from_raw(0);
            (*pheader).off = 0;
            (*pheader).namelen = namelen;
            (*pheader).typ = 0;

            #[allow(clippy::unneeded_field_pattern)]
            let p = p.add(offset_of!(DirEntryHeader, name));
            ptr::copy_nonoverlapping(name.as_ptr(), p, name.len());

            let p = p.add(name.len());
            ptr::write_bytes(p, 0u8, padlen);

            dirent_buf.set_len(entsize);
        }

        Self { dirent_buf }
    }

    unsafe fn header(&self) -> &DirEntryHeader {
        debug_assert!(self.dirent_buf.len() > mem::size_of::<DirEntryHeader>());
        #[allow(clippy::cast_ptr_alignment)]
        &*(self.dirent_buf.as_ptr() as *mut DirEntryHeader)
    }

    unsafe fn header_mut(&mut self) -> &mut DirEntryHeader {
        debug_assert!(self.dirent_buf.len() > mem::size_of::<DirEntryHeader>());
        #[allow(clippy::cast_ptr_alignment)]
        &mut *(self.dirent_buf.as_mut_ptr() as *mut DirEntryHeader)
    }

    pub fn nodeid(&self) -> Nodeid {
        unsafe { self.header().ino }
    }

    pub fn nodeid_mut(&mut self) -> &mut Nodeid {
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
        let name_offset = offset_of!(DirEntryHeader, name);
        let namelen = unsafe { self.header().namelen as usize };
        OsStr::from_bytes(&self.dirent_buf[name_offset..name_offset + namelen])
    }

    #[allow(clippy::cast_ptr_alignment)]
    pub fn set_name(&mut self, name: impl AsRef<OsStr>) {
        let name = name.as_ref().as_bytes();
        let namelen = u32::try_from(name.len()).expect("the length of name is too large");

        let entlen = mem::size_of::<DirEntryHeader>() + name.len();
        let entsize = aligned(entlen);
        let padlen = entsize - entlen;

        if self.dirent_buf.capacity() < entsize {
            self.dirent_buf
                .reserve_exact(entsize - self.dirent_buf.len());
        }

        unsafe {
            let p = self.dirent_buf.as_mut_ptr();

            #[allow(clippy::cast_ptr_alignment)]
            let pheader = p as *mut DirEntryHeader;
            (*pheader).namelen = namelen;

            #[allow(clippy::unneeded_field_pattern)]
            let p = p.add(offset_of!(DirEntryHeader, name));
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

pub struct ReplyRaw<'a> {
    unique: Unique,
    writer: Pin<Box<dyn AsyncWrite + Unpin + 'a>>,
}

impl fmt::Debug for ReplyRaw<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ReplyRaw")
            .field("unique", &self.unique)
            .finish()
    }
}

impl<'a> ReplyRaw<'a> {
    pub(crate) fn new(unique: Unique, writer: impl AsyncWrite + Unpin + 'a) -> Self {
        Self {
            unique,
            writer: Box::pin(writer),
        }
    }

    /// Repy the specified data to the kernel.
    pub async fn reply(self, error: i32, data: &[&[u8]]) -> io::Result<()> {
        let mut this = self;

        let data_len: usize = data.iter().map(|t| t.len()).sum();

        let out_header = OutHeader {
            unique: this.unique,
            error: -error,
            len: u32::try_from(mem::size_of::<OutHeader>() + data_len).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("the total length of data is too long: {}", e),
                )
            })?,
        };

        let vec: SmallVec<[_; 4]> = Some(IoSlice::new(out_header.as_ref()))
            .into_iter()
            .chain(data.iter().map(|t| IoSlice::new(&*t)))
            .collect();

        (*this.writer).write_vectored(&*vec).await?;

        Ok(())
    }

    /// Reply an error code to the kernel.
    pub async fn reply_err(self, error: i32) -> io::Result<()> {
        self.reply(error, &[]).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyUnit<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyUnit<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl ReplyUnit<'_> {
    pub async fn ok(self) -> io::Result<()> {
        self.raw.reply(0, &[]).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyData<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyData<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl<'a> ReplyData<'a> {
    pub async fn ok(self, data: &[u8]) -> io::Result<()> {
        self.ok_vectored(&[data]).await
    }

    pub async fn ok_vectored(self, data: &[&[u8]]) -> io::Result<()> {
        self.raw.reply(0, data).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyAttr<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyAttr<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl ReplyAttr<'_> {
    pub async fn ok(self, attr: AttrOut) -> io::Result<()> {
        self.raw.reply(0, &[attr.as_ref()]).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyEntry<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyEntry<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl ReplyEntry<'_> {
    pub async fn ok(self, entry: EntryOut) -> io::Result<()> {
        self.raw.reply(0, &[entry.as_ref()]).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyOpen<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyOpen<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl ReplyOpen<'_> {
    pub async fn ok(self, out: OpenOut) -> io::Result<()> {
        self.raw.reply(0, &[out.as_ref()]).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyWrite<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyWrite<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl ReplyWrite<'_> {
    pub async fn ok(self, out: WriteOut) -> io::Result<()> {
        self.raw.reply(0, &[out.as_ref()]).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyXattr<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyXattr<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl<'a> ReplyXattr<'a> {
    pub async fn size(self, out: GetxattrOut) -> io::Result<()> {
        self.raw.reply(0, &[out.as_ref()]).await
    }

    pub async fn value(self, value: &[u8]) -> io::Result<()> {
        self.raw.reply(0, &[value]).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyStatfs<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyStatfs<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl ReplyStatfs<'_> {
    pub async fn ok(self, out: StatfsOut) -> io::Result<()> {
        self.raw.reply(0, &[out.as_ref()]).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyLk<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyLk<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl ReplyLk<'_> {
    pub async fn ok(self, out: LkOut) -> io::Result<()> {
        self.raw.reply(0, &[out.as_ref()]).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyCreate<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyCreate<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl ReplyCreate<'_> {
    pub async fn ok(self, entry: EntryOut, open: OpenOut) -> io::Result<()> {
        self.raw.reply(0, &[entry.as_ref(), open.as_ref()]).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyBmap<'a> {
    raw: ReplyRaw<'a>,
}

impl<'a> From<ReplyRaw<'a>> for ReplyBmap<'a> {
    fn from(raw: ReplyRaw<'a>) -> Self {
        Self { raw }
    }
}

impl ReplyBmap<'_> {
    pub async fn ok(self, out: BmapOut) -> io::Result<()> {
        self.raw.reply(0, &[out.as_ref()]).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.reply_err(errno).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_direntry() {
        let dirent = DirEntry::new("hello");
        assert_eq!(dirent.nodeid(), Nodeid::from_raw(0));
        assert_eq!(dirent.offset(), 0u64);
        assert_eq!(dirent.type_(), 0u32);
        assert_eq!(dirent.name(), "hello");

        assert_eq!(dirent.as_ref().len(), 32usize);
        assert_eq!(
            dirent.as_ref(),
            &*vec![
                0u8, 0, 0, 0, 0, 0, 0, 0, // ino
                0, 0, 0, 0, 0, 0, 0, 0, // off
                5, 0, 0, 0, // namlen
                0, 0, 0, 0, // typ
                104, 101, 108, 108, 111, // name
                0, 0, 0 // padding
            ]
        );
    }

    #[test]
    fn test_direntry_set_long_name() {
        let mut dirent = DirEntry::new("hello");
        dirent.set_name("good evening");
        assert_eq!(dirent.nodeid(), Nodeid::from_raw(0));
        assert_eq!(dirent.offset(), 0u64);
        assert_eq!(dirent.type_(), 0u32);
        assert_eq!(dirent.name(), "good evening");

        assert_eq!(dirent.as_ref().len(), 40usize);
        assert_eq!(
            dirent.as_ref(),
            &*vec![
                0u8, 0, 0, 0, 0, 0, 0, 0, // ino
                0, 0, 0, 0, 0, 0, 0, 0, // off
                12, 0, 0, 0, // namelen
                0, 0, 0, 0, // typ
                103, 111, 111, 100, 32, 101, 118, 101, 110, 105, 110, 103, // name
                0, 0, 0, 0 // padding
            ]
        );
    }

    #[test]
    fn test_direntry_set_short_name() {
        let mut dirent = DirEntry::new("good morning");
        dirent.set_name("bye");
        assert_eq!(dirent.nodeid(), Nodeid::from_raw(0));
        assert_eq!(dirent.offset(), 0u64);
        assert_eq!(dirent.type_(), 0u32);
        assert_eq!(dirent.name(), "bye");

        assert_eq!(dirent.as_ref().len(), 32usize);
        assert_eq!(
            dirent.as_ref(),
            &*vec![
                0u8, 0, 0, 0, 0, 0, 0, 0, // ino
                0, 0, 0, 0, 0, 0, 0, 0, // off
                3, 0, 0, 0, // namelen
                0, 0, 0, 0, // off
                98, 121, 101, // name
                0, 0, 0, 0, 0 // padding
            ]
        );
    }
}
