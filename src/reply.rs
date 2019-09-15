use crate::{
    abi::{fuse_attr, fuse_out_header},
    request::InHeader,
    util::{AsyncWriteVectored, AsyncWriteVectoredExt},
};
use std::{
    io::{self, IoSlice},
    mem,
};

const OUT_HEADER_SIZE: usize = mem::size_of::<fuse_out_header>();

#[repr(transparent)]
pub struct Attr(pub(crate) fuse_attr);

impl Default for Attr {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl From<libc::stat> for Attr {
    fn from(attr: libc::stat) -> Self {
        Self(fuse_attr {
            ino: attr.st_ino,
            mode: attr.st_mode,
            nlink: attr.st_nlink as u32,
            uid: attr.st_uid,
            gid: attr.st_gid,
            rdev: attr.st_gid,
            size: attr.st_size as u64,
            blksize: attr.st_blksize as u32,
            blocks: attr.st_blocks as u64,
            atime: attr.st_atime as u64,
            mtime: attr.st_mtime as u64,
            ctime: attr.st_ctime as u64,
            atimensec: attr.st_atime_nsec as u32,
            mtimensec: attr.st_mtime_nsec as u32,
            ctimensec: attr.st_ctime_nsec as u32,
            padding: 0,
        })
    }
}

pub trait Payload {
    unsafe fn to_io_slice(&self) -> IoSlice<'_>;
}

impl Payload for [u8] {
    unsafe fn to_io_slice(&self) -> IoSlice<'_> {
        IoSlice::new(&*self)
    }
}

macro_rules! impl_payload_for_abi {
    ($($t:ident,)*) => {$(
        impl Payload for crate::abi::$t {
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
    fuse_out_header,
    fuse_init_out,
    fuse_attr_out,
    fuse_open_out,
}

pub async fn reply_payload<'a, W: ?Sized, T: ?Sized>(
    writer: &'a mut W,
    in_header: &'a InHeader,
    error: i32,
    data: &'a T,
) -> io::Result<()>
where
    W: AsyncWriteVectored + Unpin,
    T: Payload,
{
    let data = unsafe { data.to_io_slice() };

    let mut out_header: fuse_out_header = unsafe { mem::zeroed() };
    out_header.unique = in_header.unique();
    out_header.error = -error;
    out_header.len = (OUT_HEADER_SIZE + data.len()) as u32;

    let out_header = unsafe { out_header.to_io_slice() };
    (*writer).write_vectored(&[out_header, data]).await?;

    Ok(())
}

pub async fn reply_none<'a, W: ?Sized>(writer: &'a mut W, in_header: &'a InHeader) -> io::Result<()>
where
    W: AsyncWriteVectored + Unpin,
{
    reply_payload(writer, in_header, 0, &[] as &[u8]).await
}

pub async fn reply_err<'a, W: ?Sized>(
    writer: &'a mut W,
    in_header: &'a InHeader,
    error: i32,
) -> io::Result<()>
where
    W: AsyncWriteVectored + Unpin,
{
    reply_payload(writer, in_header, error, &[] as &[u8]).await
}
