//! I/O abstraction specialized for FUSE.

use crate::{
    kernel::{
        fuse_in_header, //
        fuse_notify_retrieve_in,
        fuse_opcode,
        fuse_out_header,
        fuse_write_in,
    },
    op::OperationKind,
};
use futures::{
    future::Future,
    io::{AsyncRead, AsyncWrite},
    task::{self, Poll},
};
use smallvec::SmallVec;
use std::{
    convert::TryFrom,
    io::{self, IoSlice},
    mem,
    pin::Pin,
};

/// A reader for an FUSE request message.
pub trait Reader: AsyncRead {}

impl<R: AsyncRead + ?Sized> Reader for R {}

pub(crate) trait ReaderExt: Reader {
    fn read_request(&mut self) -> ReadRequest<'_, Self>
    where
        Self: Unpin,
    {
        ReadRequest {
            reader: self,
            header: None,
            arg: None,
            state: ReadRequestState::Init,
        }
    }
}

impl<R: Reader + ?Sized> ReaderExt for R {}

#[allow(missing_debug_implementations)]
pub(crate) struct ReadRequest<'r, R: ?Sized> {
    reader: &'r mut R,
    header: Option<fuse_in_header>,
    arg: Option<Vec<u8>>,
    state: ReadRequestState,
}

#[derive(Copy, Clone)]
enum ReadRequestState {
    Init,
    ReadingHeader,
    ReadingArg,
    Done,
}

impl<R: ?Sized> Future for ReadRequest<'_, R>
where
    R: Reader + Unpin,
{
    type Output = io::Result<Request>;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let me = self.get_mut();
        loop {
            match me.state {
                ReadRequestState::Init => {
                    me.header
                        .get_or_insert_with(|| unsafe { mem::MaybeUninit::zeroed().assume_init() });
                    me.state = ReadRequestState::ReadingHeader;
                    continue;
                }
                ReadRequestState::ReadingHeader => {
                    let header = me.header.as_mut().expect("header is empty");
                    let count = futures::ready!(Pin::new(&mut me.reader)
                        .poll_read(cx, unsafe { crate::util::as_bytes_mut(header) }))?;
                    if count < mem::size_of::<fuse_in_header>() {
                        return Poll::Ready(Err(io::Error::from_raw_os_error(libc::EINVAL)));
                    }
                    me.state = ReadRequestState::ReadingArg;
                    let arg_len = match fuse_opcode::try_from(header.opcode).ok() {
                        Some(fuse_opcode::FUSE_WRITE) => mem::size_of::<fuse_write_in>(),
                        Some(fuse_opcode::FUSE_NOTIFY_REPLY) => {
                            mem::size_of::<fuse_notify_retrieve_in>()
                        } // = size_of::<fuse_write_in>()
                        _ => header.len as usize - mem::size_of::<fuse_in_header>(),
                    };
                    me.arg.get_or_insert_with(|| Vec::with_capacity(arg_len));
                    continue;
                }
                ReadRequestState::ReadingArg => {
                    {
                        struct Guard<'a>(&'a mut Vec<u8>);
                        impl Drop for Guard<'_> {
                            fn drop(&mut self) {
                                unsafe {
                                    self.0.set_len(0);
                                }
                            }
                        }

                        let arg = Guard(me.arg.as_mut().expect("arg is empty"));
                        unsafe {
                            arg.0.set_len(arg.0.capacity());
                        }

                        let count = futures::ready!(
                            Pin::new(&mut me.reader) //
                                .poll_read(cx, &mut arg.0[..])
                        )?;
                        if count < arg.0.len() {
                            return Poll::Ready(Err(io::Error::from_raw_os_error(libc::EINVAL)));
                        }

                        unsafe {
                            arg.0.set_len(count);
                        }
                        mem::forget(arg);
                    }

                    me.state = ReadRequestState::Done;
                    let header = me.header.take().unwrap();
                    let arg = me.arg.take().unwrap();

                    return Poll::Ready(Ok(Request { header, arg }));
                }
                ReadRequestState::Done => unreachable!(),
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct Request {
    header: fuse_in_header,
    arg: Vec<u8>,
}

impl Request {
    pub(crate) fn header(&self) -> &fuse_in_header {
        &self.header
    }

    pub(crate) fn arg(&self) -> io::Result<OperationKind<'_>> {
        OperationKind::parse(&self.header, &self.arg)
    }
}

/// The writer of FUSE responses and notifications.
pub trait Writer: AsyncWrite {}

impl<W: AsyncWrite + ?Sized> Writer for W {}

pub(crate) trait WriterExt: Writer {
    fn send_msg<'w>(
        &'w mut self,
        unique: u64,
        error: i32,
        data: &'w [&'w [u8]],
    ) -> SendMsg<'w, Self>
    where
        Self: Unpin,
    {
        let data_len: usize = data.iter().map(|t| t.len()).sum();
        let len = u32::try_from(mem::size_of::<fuse_out_header>() + data_len).unwrap();
        let header = fuse_out_header { unique, error, len };

        SendMsg {
            writer: self,
            header,
            data,
        }
    }
}

impl<W: Writer + ?Sized> WriterExt for W {}

pub(crate) struct SendMsg<'w, W: ?Sized> {
    writer: &'w mut W,
    header: fuse_out_header,
    data: &'w [&'w [u8]],
}

impl<W: ?Sized> Future for SendMsg<'_, W>
where
    W: Writer + Unpin,
{
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let me = self.get_mut();

        // Unfortunately, IoSlice<'_> does not implement Send and
        // the data vector must be created in `poll` function.
        let vec: SmallVec<[_; 4]> =
            Some(IoSlice::new(unsafe { crate::util::as_bytes(&me.header) }))
                .into_iter()
                .chain(me.data.iter().map(|t| IoSlice::new(&*t)))
                .collect();

        let count = futures::ready!(Pin::new(&mut *me.writer).poll_write_vectored(cx, &*vec))?;
        if count < me.header.len as usize {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::Other,
                "written data is too short",
            )));
        }

        tracing::debug!(
            "Reply to kernel: unique={}: error={}",
            me.header.unique,
            me.header.error
        );

        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;

    #[inline]
    fn bytes(bytes: &[u8]) -> &[u8] {
        bytes
    }
    macro_rules! b {
        ($($b:expr),*$(,)?) => ( *bytes(&[$($b),*]) );
    }

    #[test]
    fn send_msg_empty() {
        let mut writer = Vec::<u8>::new();
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
        let mut writer = Vec::<u8>::new();
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
        let mut writer = Vec::<u8>::new();
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
