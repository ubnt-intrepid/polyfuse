//! Replies to the kernel.

use fuse_async_abi::{
    AttrOut, //
    BmapOut,
    EntryOut,
    GetxattrOut,
    InitOut,
    LkOut,
    OpenOut,
    OutHeader,
    StatfsOut,
    WriteOut,
};
use futures::io::{AsyncWrite, AsyncWriteExt};
use std::{
    borrow::Cow,
    ffi::OsStr,
    io::{self, IoSlice},
    mem,
    os::unix::ffi::OsStrExt,
};

#[derive(Debug)]
pub enum XattrOut<'a> {
    Size(GetxattrOut),
    Value(Cow<'a, [u8]>),
}

#[repr(C)]
#[derive(Debug)]
pub struct CreateOut {
    pub entry: EntryOut,
    pub open: OpenOut,
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
    OutHeader,
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

pub async fn reply_raw<'a, W: ?Sized, T: ?Sized>(
    writer: &'a mut W,
    unique: u64,
    error: i32,
    data: &'a T,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Payload,
{
    let data = unsafe { data.to_io_slice() };
    let out_header = OutHeader::new(unique, error, data.len());
    let out_header = unsafe { out_header.to_io_slice() };

    (*writer).write_vectored(&[out_header, data]).await?;

    Ok(())
}

pub async fn reply_payload<'a, W: ?Sized, T: ?Sized>(
    writer: &'a mut W,
    unique: u64,
    data: &'a T,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Payload,
{
    reply_raw(writer, unique, 0, data).await
}

pub async fn reply_unit<W: ?Sized>(writer: &mut W, unique: u64) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    reply_raw(writer, unique, 0, &[] as &[u8]).await
}

pub async fn reply_err<W: ?Sized>(writer: &mut W, unique: u64, error: i32) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    reply_raw(writer, unique, error, &[] as &[u8]).await
}
