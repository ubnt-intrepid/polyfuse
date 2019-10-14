//! Replies to the kernel.

use crate::abi::{
    AttrOut, //
    BmapOut,
    EntryOut,
    GetxattrOut,
    LkOut,
    OpenOut,
    OutHeader,
    StatfsOut,
    Unique,
    WriteOut,
};
use futures::io::{AsyncWrite, AsyncWriteExt};
use smallvec::SmallVec;
use std::{
    convert::TryFrom,
    fmt,
    io::{self, IoSlice},
    mem,
    pin::Pin,
};

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
