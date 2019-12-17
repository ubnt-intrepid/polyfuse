//! I/O abstraction specialized for FUSE.

use crate::kernel::{
    fuse_in_header, //
    fuse_notify_retrieve_in,
    fuse_opcode,
    fuse_out_header,
    fuse_write_in,
};
use futures::{
    future::Future,
    task::{self, Poll},
};
use std::{convert::TryFrom, io, mem, pin::Pin};

/// The reader of incoming FUSE request messages.
///
/// The role of this trait is similar to `AsyncRead`, except that the message data
/// is transferred to a specific buffer instance instead of the in-memory buffer.
///
pub trait Reader {
    /// The buffer holding a transferred FUSE request message.
    type Buffer: ?Sized;

    /// Receive one FUSE request message from the kernel and store it
    /// to the buffer.
    fn poll_receive_msg(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut Self::Buffer,
    ) -> Poll<io::Result<()>>;
}

impl<R: ?Sized> Reader for &mut R
where
    R: Reader + Unpin,
{
    type Buffer = R::Buffer;

    #[inline]
    fn poll_receive_msg(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut Self::Buffer,
    ) -> Poll<io::Result<()>> {
        let me = Pin::new(&mut **self.get_mut());
        me.poll_receive_msg(cx, buf)
    }
}

pub(crate) trait ReaderExt: Reader {
    fn receive_msg<'r>(
        &'r mut self,
        buf: &'r mut Self::Buffer,
    ) -> ReceiveMsg<'r, Self, Self::Buffer>
    where
        Self: Unpin,
    {
        ReceiveMsg { reader: self, buf }
    }
}

impl<R: Reader + ?Sized> ReaderExt for R {}

#[must_use]
pub(crate) struct ReceiveMsg<'r, R: ?Sized, B: ?Sized> {
    reader: &'r mut R,
    buf: &'r mut B,
}

impl<R: ?Sized, B: ?Sized> Future for ReceiveMsg<'_, R, B>
where
    R: Reader<Buffer = B> + Unpin,
{
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let me = self.get_mut();
        Pin::new(&mut *me.reader).poll_receive_msg(cx, &mut *me.buf)
    }
}

/// A received FUSE message to be processed.
///
/// It holds the raw data for a single FUSE request message received from the kernel.
pub trait Buffer {
    /// The rest of the request message.
    type Data;

    /// Return a reference to `InHeader` corresponding to this request.
    fn header(&self) -> &InHeader;

    /// Extract the request message data.
    ///
    /// The extracted data consists of three parts: the header part,
    /// the raw data of argument part, and additional data that may
    /// not have been transferred from the kernel space.
    fn extract(&mut self) -> (&InHeader, &[u8], Option<Self::Data>);
}

impl<B: ?Sized> Buffer for &mut B
where
    B: Buffer + Unpin,
{
    type Data = B::Data;

    #[inline]
    fn header(&self) -> &InHeader {
        (**self).header()
    }

    #[inline]
    fn extract(&mut self) -> (&InHeader, &[u8], Option<Self::Data>) {
        (**self).extract()
    }
}

/// The header part of FUSE request messages.
///
/// This type is ABI-compatible with `fuse_in_header.
#[derive(Debug)]
#[repr(transparent)]
pub struct InHeader(fuse_in_header);

#[doc(hidden)]
impl AsMut<[u8]> for InHeader {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        unsafe { crate::util::as_bytes_mut(self) }
    }
}

#[allow(clippy::len_without_is_empty)]
impl InHeader {
    #[doc(hidden)]
    pub fn len(&self) -> u32 {
        self.0.len
    }

    #[doc(hidden)]
    pub fn unique(&self) -> u64 {
        self.0.unique
    }

    #[doc(hidden)]
    pub fn opcode(&self) -> Option<fuse_opcode> {
        fuse_opcode::try_from(self.0.opcode).ok()
    }

    #[doc(hidden)]
    pub fn nodeid(&self) -> u64 {
        self.0.nodeid
    }

    #[doc(hidden)]
    pub fn uid(&self) -> u32 {
        self.0.uid
    }

    #[doc(hidden)]
    pub fn gid(&self) -> u32 {
        self.0.gid
    }

    #[doc(hidden)]
    pub fn pid(&self) -> u32 {
        self.0.pid
    }

    /// Return the argument part length in the corresponding request message.
    pub fn arg_len(&self) -> usize {
        match self.opcode() {
            Some(fuse_opcode::FUSE_WRITE) => mem::size_of::<fuse_write_in>(),
            Some(fuse_opcode::FUSE_NOTIFY_REPLY) => mem::size_of::<fuse_notify_retrieve_in>(), // = size_of::<fuse_write_in>()
            _ => self.len() as usize - mem::size_of::<InHeader>(),
        }
    }
}

/// The writer of FUSE responses and notifications.
pub trait Writer {
    /// Send a FUSE response or notification message to the kernel.
    fn poll_write_msg(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        header: &OutHeader,
        payload: &[&[u8]],
    ) -> Poll<io::Result<()>>;
}

impl<W: ?Sized> Writer for &mut W
where
    W: Writer + Unpin,
{
    #[inline]
    fn poll_write_msg(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        header: &OutHeader,
        payload: &[&[u8]],
    ) -> Poll<io::Result<()>> {
        let me = Pin::new(&mut **self.get_mut());
        me.poll_write_msg(cx, header, payload)
    }
}

/// The header part of FUSE response or notification messages.
///
/// This type is ABI-compatible with `fuse_out_header.
#[derive(Debug)]
#[repr(transparent)]
pub struct OutHeader(fuse_out_header);

impl AsRef<[u8]> for OutHeader {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        unsafe { crate::util::as_bytes(self) }
    }
}

#[allow(clippy::len_without_is_empty)]
#[doc(hidden)]
impl OutHeader {
    #[inline]
    pub fn unique(&self) -> u64 {
        self.0.unique
    }

    #[inline]
    pub fn error(&self) -> i32 {
        self.0.error
    }

    #[inline]
    pub fn len(&self) -> u32 {
        self.0.len
    }
}

pub(crate) trait WriterExt: Writer {
    fn send_msg<'w, 'a>(
        &'w mut self,
        unique: u64,
        error: i32,
        data: &'w [&'a [u8]],
    ) -> SendMsg<'w, 'a, Self> {
        let data_len: usize = data.iter().map(|t| t.len()).sum();
        let len = u32::try_from(mem::size_of::<fuse_out_header>() + data_len).unwrap();
        let header = OutHeader(fuse_out_header { unique, error, len });

        SendMsg {
            writer: self,
            header,
            data,
        }
    }
}

impl<W: Writer + ?Sized> WriterExt for W {}

pub(crate) struct SendMsg<'w, 'a, W: ?Sized> {
    writer: &'w mut W,
    header: OutHeader,
    data: &'w [&'a [u8]],
}

impl<W: ?Sized> Future for SendMsg<'_, '_, W>
where
    W: Writer + Unpin,
{
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let me = self.get_mut();
        futures::ready!(Pin::new(&mut me.writer).poll_write_msg(cx, &me.header, &me.data))?;
        tracing::debug!(
            "Reply to kernel: unique={}: error={}",
            me.header.unique(),
            me.header.error()
        );
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{
        executor::block_on,
        task::{self, Poll},
    };
    use std::{ops::Index, pin::Pin};

    #[inline]
    fn bytes(bytes: &[u8]) -> &[u8] {
        bytes
    }
    macro_rules! b {
        ($($b:expr),*$(,)?) => ( *bytes(&[$($b),*]) );
    }

    #[derive(Default)]
    struct DummyWriter(Vec<u8>);

    impl<I> Index<I> for DummyWriter
    where
        Vec<u8>: Index<I>,
    {
        type Output = <Vec<u8> as Index<I>>::Output;

        fn index(&self, index: I) -> &Self::Output {
            self.0.index(index)
        }
    }

    impl Writer for DummyWriter {
        fn poll_write_msg(
            self: Pin<&mut Self>,
            _: &mut task::Context<'_>,
            out_header: &OutHeader,
            payload: &[&[u8]],
        ) -> Poll<io::Result<()>> {
            let me = self.get_mut();
            me.0.extend_from_slice(out_header.as_ref());
            for chunk in payload {
                me.0.extend_from_slice(chunk);
            }
            Poll::Ready(Ok(()))
        }
    }

    #[test]
    fn send_msg_empty() {
        let mut writer = DummyWriter::default();
        block_on(writer.send_msg(42, 4, &[])).unwrap();
        assert_eq!(writer[0..4], b![0x10, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(writer[4..8], b![0x04, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            writer[8..16],
            b![0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
    }

    #[test]
    fn send_msg_single_data() {
        let mut writer = DummyWriter::default();
        block_on(writer.send_msg(42, 0, &["hello".as_ref()])).unwrap();
        assert_eq!(writer[0..4], b![0x15, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(writer[4..8], b![0x00, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            writer[8..16],
            b![0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
        assert_eq!(writer[16..], b![0x68, 0x65, 0x6c, 0x6c, 0x6f], "payload");
    }

    #[test]
    fn send_msg_chunked_data() {
        let payload: &[&[u8]] = &[
            "hello, ".as_ref(), //
            "this ".as_ref(),
            "is a ".as_ref(),
            "message.".as_ref(),
        ];
        let mut writer = DummyWriter::default();
        block_on(writer.send_msg(26, 0, payload)).unwrap();
        assert_eq!(writer[0..4], b![0x29, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(writer[4..8], b![0x00, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            writer[8..16],
            b![0x1a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
        assert_eq!(writer[16..], *b"hello, this is a message.", "payload");
    }
}
