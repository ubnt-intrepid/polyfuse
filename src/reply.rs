use crate::{
    abi::{
        fuse_attr_out, //
        fuse_bmap_out,
        fuse_entry_out,
        fuse_getxattr_out,
        fuse_init_out,
        fuse_lk_out,
        fuse_open_out,
        fuse_out_header,
        fuse_statfs_out,
        fuse_write_out,
        FUSE_KERNEL_MINOR_VERSION,
        FUSE_KERNEL_VERSION,
    },
    common::{Attr, CapFlags, FileLock, Statfs},
    io::{AsyncWriteVectored, AsyncWriteVectoredExt},
};
use bitflags::bitflags;
use std::{
    borrow::Cow,
    ffi::OsStr,
    io::{self, IoSlice},
    mem,
    os::unix::ffi::OsStrExt,
};

const OUT_HEADER_SIZE: usize = mem::size_of::<fuse_out_header>();

#[repr(transparent)]
struct Header(fuse_out_header);

impl Header {
    fn new(unique: u64, error: i32, data_len: usize) -> Self {
        Self(fuse_out_header {
            unique,
            error: -error,
            len: (OUT_HEADER_SIZE + data_len) as u32,
        })
    }
}

#[repr(transparent)]
pub struct AttrOut(fuse_attr_out);

impl Default for AttrOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl From<Attr> for AttrOut {
    fn from(attr: Attr) -> Self {
        let mut attr_out = Self::default();
        attr_out.set_attr(attr);
        attr_out
    }
}

impl From<libc::stat> for AttrOut {
    fn from(attr: libc::stat) -> Self {
        Self::from(Attr::from(attr))
    }
}

impl AttrOut {
    pub fn set_attr(&mut self, attr: impl Into<Attr>) {
        self.0.attr = attr.into().0;
    }

    pub fn set_attr_valid(&mut self, sec: u64, nsec: u32) {
        self.0.attr_valid = sec;
        self.0.attr_valid_nsec = nsec;
    }
}

#[repr(transparent)]
pub struct EntryOut(pub(crate) fuse_entry_out);

impl Default for EntryOut {
    fn default() -> Self {
        Self(fuse_entry_out {
            nodeid: 0,
            generation: 0,
            entry_valid: 0,
            attr_valid: 0,
            entry_valid_nsec: 0,
            attr_valid_nsec: 0,
            attr: Attr::default().0,
        })
    }
}

impl EntryOut {
    pub fn set_nodeid(&mut self, nodeid: u64) {
        self.0.nodeid = nodeid;
    }

    pub fn set_generation(&mut self, generation: u64) {
        self.0.generation = generation;
    }

    pub fn set_entry_valid(&mut self, sec: u64, nsec: u32) {
        self.0.entry_valid = sec;
        self.0.entry_valid_nsec = nsec;
    }

    pub fn set_attr_valid(&mut self, sec: u64, nsec: u32) {
        self.0.attr_valid = sec;
        self.0.attr_valid_nsec = nsec;
    }

    pub fn set_attr(&mut self, attr: impl Into<Attr>) {
        self.0.attr = attr.into().0;
    }
}

#[repr(transparent)]
pub struct InitOut(fuse_init_out);

impl Default for InitOut {
    fn default() -> Self {
        let mut init_out: fuse_init_out = unsafe { mem::zeroed() };
        init_out.major = FUSE_KERNEL_VERSION;
        init_out.minor = FUSE_KERNEL_MINOR_VERSION;
        Self(init_out)
    }
}

impl InitOut {
    pub fn set_flags(&mut self, flags: CapFlags) {
        self.0.flags = flags.bits();
    }

    pub fn max_readahead(&self) -> u32 {
        self.0.max_readahead
    }

    pub fn set_max_readahead(&mut self, max_readahead: u32) {
        self.0.max_readahead = max_readahead;
    }

    pub fn max_write(&self) -> u32 {
        self.0.max_write
    }

    pub fn set_max_write(&mut self, max_write: u32) {
        self.0.max_write = max_write;
    }
}

#[repr(transparent)]
pub struct GetxattrOut(fuse_getxattr_out);

impl Default for GetxattrOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl GetxattrOut {
    pub fn set_size(&mut self, size: u32) {
        self.0.size = size;
    }
}

pub enum XattrOut<'a> {
    Size(GetxattrOut),
    Value(Cow<'a, [u8]>),
}

#[repr(transparent)]
pub struct OpenOut(fuse_open_out);

impl Default for OpenOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl OpenOut {
    pub fn set_fh(&mut self, fh: u64) {
        self.0.fh = fh;
    }

    pub fn set_flags(&mut self, flags: OpenFlags) {
        self.0.open_flags = flags.bits();
    }
}

bitflags! {
    pub struct OpenFlags: u32 {
        const DIRECT_IO = crate::abi::FOPEN_DIRECT_IO;
        const KEEP_CACHE = crate::abi::FOPEN_KEEP_CACHE;
        const NONSEEKABLE = crate::abi::FOPEN_NONSEEKABLE;
        //const CACHE_DIR = crate::abi::FOPEN_CACHE_DIR;
    }
}

#[repr(transparent)]
pub struct WriteOut(fuse_write_out);

impl Default for WriteOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl WriteOut {
    pub fn set_size(&mut self, size: u32) {
        self.0.size = size;
    }
}

#[repr(transparent)]
pub struct StatfsOut(fuse_statfs_out);

impl Default for StatfsOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl StatfsOut {
    pub fn set_st(&mut self, st: impl Into<Statfs>) {
        self.0.st = st.into().0;
    }
}

#[repr(transparent)]
pub struct LkOut(fuse_lk_out);

impl Default for LkOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl LkOut {
    pub fn set_lk(&mut self, lk: impl Into<FileLock>) {
        self.0.lk = lk.into().0;
    }
}

#[repr(C)]
pub struct CreateOut {
    pub entry: EntryOut,
    pub open: OpenOut,
}

#[repr(transparent)]
pub struct BmapOut(fuse_bmap_out);

impl Default for BmapOut {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl BmapOut {
    pub fn set_block(&mut self, block: u64) {
        self.0.block = block;
    }
}

// ==== Payload ====

pub trait Payload {
    unsafe fn to_io_slice(&self) -> IoSlice<'_>;
}

impl Payload for [u8] {
    unsafe fn to_io_slice(&self) -> IoSlice<'_> {
        IoSlice::new(&*self)
    }
}

impl Payload for Cow<'_, [u8]> {
    unsafe fn to_io_slice(&self) -> IoSlice<'_> {
        IoSlice::new(&**self)
    }
}

impl Payload for Cow<'_, OsStr> {
    unsafe fn to_io_slice(&self) -> IoSlice<'_> {
        IoSlice::new((**self).as_bytes())
    }
}

impl Payload for XattrOut<'_> {
    unsafe fn to_io_slice(&self) -> IoSlice<'_> {
        match self {
            Self::Size(out) => out.to_io_slice(),
            Self::Value(value) => IoSlice::new(&**value),
        }
    }
}

macro_rules! impl_payload_for_abi {
    ($($t:ty,)*) => {$(
        impl Payload for $t {
            unsafe fn to_io_slice(&self) -> IoSlice<'_> {
                IoSlice::new(std::slice::from_raw_parts(
                    self as *const Self as *const u8,
                    mem::size_of::<Self>(),
                ))
            }
        }
    )*}
}

impl_payload_for_abi! {
    Header,
    InitOut,
    OpenOut,
    AttrOut,
    EntryOut,
    GetxattrOut,
    WriteOut,
    StatfsOut,
    LkOut,
    CreateOut,
    BmapOut,
}

pub async fn reply_payload<'a, W: ?Sized, T: ?Sized>(
    writer: &'a mut W,
    unique: u64,
    error: i32,
    data: &'a T,
) -> io::Result<()>
where
    W: AsyncWriteVectored + Unpin,
    T: Payload,
{
    let data = unsafe { data.to_io_slice() };
    let out_header = Header::new(unique, error, data.len());
    let out_header = unsafe { out_header.to_io_slice() };

    (*writer).write_vectored(&[out_header, data]).await?;

    Ok(())
}

pub async fn reply_unit<'a, W: ?Sized>(writer: &'a mut W, unique: u64) -> io::Result<()>
where
    W: AsyncWriteVectored + Unpin,
{
    reply_payload(writer, unique, 0, &[] as &[u8]).await
}

pub async fn reply_err<'a, W: ?Sized>(writer: &'a mut W, unique: u64, error: i32) -> io::Result<()>
where
    W: AsyncWriteVectored + Unpin,
{
    reply_payload(writer, unique, error, &[] as &[u8]).await
}
