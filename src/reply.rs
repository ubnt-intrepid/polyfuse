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
    Unique,
    WriteOut,
};
use futures::io::{AsyncWrite, AsyncWriteExt};
use std::{
    borrow::Cow,
    fmt,
    io::{self, IoSlice},
    mem,
};

#[derive(Debug)]
enum XattrOut<'a> {
    Size(GetxattrOut),
    Value(Cow<'a, [u8]>),
}

#[repr(C)]
#[derive(Debug)]
struct CreateOut {
    entry: EntryOut,
    open: OpenOut,
}

impl AsRef<[u8]> for CreateOut {
    fn as_ref(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                self as *const Self as *const u8,
                std::mem::size_of::<Self>(),
            )
        }
    }
}

// ==== Payload ====

impl AsRef<[u8]> for XattrOut<'_> {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Size(out) => out.as_ref(),
            Self::Value(value) => &**value,
        }
    }
}

pub struct ReplyRaw<'a> {
    io: &'a mut (dyn AsyncWrite + Unpin),
    unique: Unique,
}

impl fmt::Debug for ReplyRaw<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ReplyRaw")
            .field("unique", &self.unique)
            .finish()
    }
}

impl<'a> ReplyRaw<'a> {
    pub(crate) fn new(unique: Unique, io: &'a mut (impl AsyncWrite + Unpin)) -> Self {
        Self { unique, io }
    }

    pub async fn reply(self, error: i32, data: impl AsRef<[u8]>) -> io::Result<()> {
        let data = IoSlice::new(data.as_ref());

        let out_header = OutHeader {
            unique: self.unique,
            error: -error,
            len: (mem::size_of::<OutHeader>() + data.len()) as u32,
        };
        let out_header = IoSlice::new(out_header.as_ref());

        (*self.io).write_vectored(&[out_header, data]).await?;

        Ok(())
    }

    /// Reply a byte sequence to the kernel.
    pub async fn ok(self, data: impl AsRef<[u8]>) -> io::Result<()> {
        self.reply(0, data).await
    }

    /// Reply an error code to the kernel.
    pub async fn err(self, error: i32) -> io::Result<()> {
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
        self.raw.ok(&[]).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.err(errno).await
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
        self.raw.ok(data).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.err(errno).await
    }
}

#[derive(Debug)]
#[must_use]
pub struct ReplyInit<'a> {
    raw: ReplyRaw<'a>,
    out: InitOut,
}

impl<'a> ReplyInit<'a> {
    pub(crate) fn new(raw: ReplyRaw<'a>, out: InitOut) -> Self {
        Self { raw, out }
    }

    pub fn out(&mut self) -> &mut InitOut {
        &mut self.out
    }

    pub async fn ok(self) -> io::Result<()> {
        self.raw.ok(&self.out).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.err(errno).await
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
        self.raw.ok(&attr).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.err(errno).await
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
        self.raw.ok(&entry).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.err(errno).await
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
        self.raw.ok(&out).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.err(errno).await
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

impl<'a> ReplyWrite<'a> {
    pub async fn ok(self, out: WriteOut) -> io::Result<()> {
        self.raw.ok(&out).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.err(errno).await
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
        self.raw.ok(&XattrOut::Size(out)).await
    }

    pub async fn value(self, value: &[u8]) -> io::Result<()> {
        self.raw.ok(&XattrOut::Value(value.into())).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.err(errno).await
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

impl<'a> ReplyStatfs<'a> {
    pub async fn ok(self, out: StatfsOut) -> io::Result<()> {
        self.raw.ok(&out).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.err(errno).await
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

impl<'a> ReplyLk<'a> {
    pub async fn ok(self, out: LkOut) -> io::Result<()> {
        self.raw.ok(&out).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.err(errno).await
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

impl<'a> ReplyCreate<'a> {
    pub async fn ok(self, entry: EntryOut, open: OpenOut) -> io::Result<()> {
        self.raw.ok(&CreateOut { entry, open }).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.err(errno).await
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

impl<'a> ReplyBmap<'a> {
    pub async fn ok(self, out: BmapOut) -> io::Result<()> {
        self.raw.ok(&out).await
    }

    pub async fn err(self, errno: i32) -> io::Result<()> {
        self.raw.err(errno).await
    }
}
