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
    reply::{Collector, Reply},
};
use futures::{
    future::Future,
    io::{AsyncRead, AsyncWrite},
    task::{self, Poll},
};
use pin_project_lite::pin_project;
use smallvec::SmallVec;
use std::{
    convert::TryFrom,
    io::{self, IoSlice, IoSliceMut},
    mem,
    ops::DerefMut,
    pin::Pin,
};

/// A reader for an FUSE request message.
pub trait Reader: AsyncRead {}

impl<R: ?Sized> Reader for &mut R where R: Reader + Unpin {}

impl<R: ?Sized> Reader for Box<R> where R: Reader + Unpin {}

impl<P, R: ?Sized> Reader for Pin<P>
where
    P: DerefMut<Target = R> + Unpin,
    R: Reader,
{
}

impl Reader for &[u8] {}

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

impl<W: ?Sized> Writer for &mut W where W: Writer + Unpin {}

impl<W: ?Sized> Writer for Box<W> where W: Writer + Unpin {}

impl<P, W: ?Sized> Writer for Pin<P>
where
    P: DerefMut<Target = W> + Unpin,
    W: Writer,
{
}

impl Writer for Vec<u8> {}

pub(crate) trait WriterExt: Writer {
    fn send_msg<'w, T: ?Sized>(
        &'w mut self,
        unique: u64,
        error: i32,
        data: &'w T,
    ) -> SendMsg<'w, Self>
    where
        Self: Unpin,
        T: Reply,
    {
        struct VecCollector<'a, 'v> {
            vec: &'v mut Vec<&'a [u8]>,
            total_len: usize,
        }

        impl<'a> Collector<'a> for VecCollector<'a, '_> {
            fn append(&mut self, bytes: &'a [u8]) {
                self.vec.push(bytes);
                self.total_len += bytes.len();
            }
        }

        let mut vec = Vec::new();
        let mut collector = VecCollector {
            vec: &mut vec,
            total_len: 0,
        };
        data.collect_bytes(&mut collector);

        let len = u32::try_from(mem::size_of::<fuse_out_header>() + collector.total_len).unwrap();
        let header = fuse_out_header { unique, error, len };

        drop(collector);

        SendMsg {
            writer: self,
            header,
            vec,
        }
    }
}

impl<W: Writer + ?Sized> WriterExt for W {}

pub(crate) struct SendMsg<'w, W: ?Sized> {
    writer: &'w mut W,
    header: fuse_out_header,
    vec: Vec<&'w [u8]>,
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
                .chain(me.vec.iter().map(|t| IoSlice::new(&*t)))
                .collect();

        let count = futures::ready!(Pin::new(&mut *me.writer).poll_write_vectored(cx, &*vec))?;
        if count < me.header.len as usize {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::Other,
                "written data is too short",
            )));
        }

        Poll::Ready(Ok(()))
    }
}

/// Unite a pair of `Reader` and `Writer` as an I/O.
pub fn unite<R, W>(reader: R, writer: W) -> Unite<R, W>
where
    R: Reader,
    W: Writer,
{
    Unite { reader, writer }
}

pin_project! {
    /// The united I/O of a pair of `Reader` and `Writer`.
    #[derive(Debug)]
    pub struct Unite<R, W> {
        #[pin]
        reader: R,
        #[pin]
        writer: W,
    }
}

impl<R, W> AsyncRead for Unite<R, W>
where
    R: AsyncRead,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.project().reader.poll_read(cx, dst)
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        dst: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        self.project().reader.poll_read_vectored(cx, dst)
    }
}

impl<R, W> AsyncWrite for Unite<R, W>
where
    W: AsyncWrite,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.project().writer.poll_write(cx, src)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        src: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.project().writer.poll_write_vectored(cx, src)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        self.project().writer.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        self.project().writer.poll_close(cx)
    }
}

impl<R, W> Reader for Unite<R, W> where R: Reader {}

impl<R, W> Writer for Unite<R, W> where W: Writer {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::fuse_init_in;
    use futures::{
        executor::block_on,
        task::{self, Poll},
    };
    use std::{
        io::{self, IoSlice},
        ops::Index,
        pin::Pin,
    };

    #[inline]
    fn bytes(bytes: &[u8]) -> &[u8] {
        bytes
    }
    macro_rules! b {
        ($($b:expr),*$(,)?) => ( *bytes(&[$($b),*]) );
    }

    #[test]
    fn read_request_simple() {
        #[allow(clippy::cast_possible_truncation)]
        let in_header = fuse_in_header {
            len: (mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_init_in>()) as u32,
            opcode: crate::kernel::FUSE_INIT,
            unique: 2,
            nodeid: 0,
            uid: 100,
            gid: 100,
            pid: 12,
            padding: 0,
        };
        let init_in = fuse_init_in {
            major: 7,
            minor: 23,
            max_readahead: 40,
            flags: crate::kernel::FUSE_AUTO_INVAL_DATA | crate::kernel::FUSE_DO_READDIRPLUS,
        };

        let mut input = vec![];
        input.extend_from_slice(unsafe { crate::util::as_bytes(&in_header) });
        input.extend_from_slice(unsafe { crate::util::as_bytes(&init_in) });

        let mut reader = &input[..];

        let request = block_on(reader.read_request()).expect("parser failed");
        assert_eq!(request.header().len, in_header.len);
        assert_eq!(request.header().opcode, in_header.opcode);
        assert_eq!(request.header().unique, in_header.unique);
        assert_eq!(request.header().nodeid, in_header.nodeid);
        assert_eq!(request.header().uid, in_header.uid);
        assert_eq!(request.header().gid, in_header.gid);
        assert_eq!(request.header().pid, in_header.pid);

        let arg = request.arg().expect("failed to parse argument");
        match arg {
            OperationKind::Init { arg, .. } => {
                assert_eq!(arg.major, init_in.major);
                assert_eq!(arg.minor, init_in.minor);
                assert_eq!(arg.max_readahead, init_in.max_readahead);
                assert_eq!(arg.flags, init_in.flags);
            }
            _ => panic!("operation mismachd"),
        }
    }

    #[test]
    fn read_request_too_short() {
        #[allow(clippy::cast_possible_truncation)]
        let in_header = fuse_in_header {
            len: (mem::size_of::<fuse_in_header>() + mem::size_of::<fuse_init_in>()) as u32,
            opcode: crate::kernel::FUSE_INIT,
            unique: 2,
            nodeid: 0,
            uid: 100,
            gid: 100,
            pid: 12,
            padding: 0,
        };

        let mut input = vec![];
        input.extend_from_slice(unsafe { &crate::util::as_bytes(&in_header)[0..10] });

        let mut reader = &input[..];

        assert!(
            block_on(reader.read_request()).is_err(),
            "parser should fail"
        );
    }

    pin_project! {
        #[derive(Default)]
        struct DummyWriter {
            #[pin]
            vec: Vec<u8>,
        }
    }

    impl<I> Index<I> for DummyWriter
    where
        Vec<u8>: Index<I>,
    {
        type Output = <Vec<u8> as Index<I>>::Output;

        fn index(&self, index: I) -> &Self::Output {
            self.vec.index(index)
        }
    }

    impl AsyncWrite for DummyWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            src: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.project().vec.poll_write(cx, src)
        }

        fn poll_write_vectored(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            src: &[IoSlice<'_>],
        ) -> Poll<io::Result<usize>> {
            self.project().vec.poll_write_vectored(cx, src)
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
            self.project().vec.poll_flush(cx)
        }

        fn poll_close(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
            self.project().vec.poll_close(cx)
        }
    }

    impl Writer for DummyWriter {}

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
        block_on(writer.send_msg(42, 0, "hello")).unwrap();
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
