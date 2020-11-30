use crate::bytes::{Bytes, Collector};
use polyfuse_kernel::{fuse_notify_code, fuse_out_header};
use smallvec::SmallVec;
use std::{convert::TryInto as _, io, mem};

pub(crate) struct ReplySender<W> {
    writer: W,
    header: fuse_out_header,
}

impl<W> ReplySender<W>
where
    W: io::Write,
{
    #[inline]
    pub(crate) fn new(writer: W, unique: u64) -> Self {
        Self {
            writer,
            header: fuse_out_header {
                len: 0,
                error: 0,
                unique,
            },
        }
    }

    #[inline]
    pub(crate) fn reply<T>(self, data: T) -> io::Result<()>
    where
        T: Bytes,
    {
        self.send_msg(0, data)
    }

    #[inline]
    pub(crate) fn error(self, code: i32) -> io::Result<()> {
        self.send_msg(-code, &[])
    }

    #[allow(dead_code)]
    #[inline]
    pub(crate) fn notify<T>(self, code: fuse_notify_code, data: T) -> io::Result<()>
    where
        T: Bytes,
    {
        self.send_msg(unsafe { mem::transmute::<_, i32>(code) }, data)
    }

    fn send_msg<T>(mut self, error: i32, data: T) -> io::Result<()>
    where
        T: Bytes,
    {
        self.header.error = error;

        let mut vec = SmallVec::new();
        vec.push(io::IoSlice::new(&[])); // dummy

        {
            let mut collector = VecCollector {
                vec: &mut vec,
                total_len: 0,
            };
            data.collect(&mut collector);

            self.header.len = (mem::size_of::<fuse_out_header>() + collector.total_len)
                .try_into()
                .expect("too large data");
        }

        vec[0] = io::IoSlice::new(unsafe { crate::util::as_bytes(&self.header) });

        let count = self.writer.write_vectored(&*vec)?;
        if count < self.header.len as usize {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "written data is too short",
            ));
        }

        Ok(())
    }
}

struct VecCollector<'a, 'v> {
    vec: &'v mut SmallVec<[io::IoSlice<'a>; 4]>,
    total_len: usize,
}

impl<'a> Collector<'a> for VecCollector<'a, '_> {
    fn append(&mut self, bytes: &'a [u8]) {
        self.vec.push(io::IoSlice::new(bytes));
        self.total_len += bytes.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[inline]
    fn bytes(bytes: &[u8]) -> &[u8] {
        bytes
    }
    macro_rules! b {
        ($($b:expr),*$(,)?) => ( *bytes(&[$($b),*]) );
    }

    #[test]
    fn send_msg_empty() {
        let mut buf = vec![0u8; 0];
        ReplySender::new(&mut buf, 42).send_msg(4, &[]).unwrap();
        assert_eq!(buf[0..4], b![0x10, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(buf[4..8], b![0x04, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            buf[8..16],
            b![0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
    }

    #[test]
    fn send_msg_single_data() {
        let mut buf = vec![0u8; 0];
        ReplySender::new(&mut buf, 42).send_msg(0, "hello").unwrap();
        assert_eq!(buf[0..4], b![0x15, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(buf[4..8], b![0x00, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            buf[8..16],
            b![0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
        assert_eq!(buf[16..], b![0x68, 0x65, 0x6c, 0x6c, 0x6f], "payload");
    }

    #[test]
    fn send_msg_chunked_data() {
        let payload: &[&[u8]] = &[
            "hello, ".as_ref(), //
            "this ".as_ref(),
            "is a ".as_ref(),
            "message.".as_ref(),
        ];
        let mut buf = vec![0u8; 0];
        ReplySender::new(&mut buf, 26).send_msg(0, payload).unwrap();
        assert_eq!(buf[0..4], b![0x29, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(buf[4..8], b![0x00, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            buf[8..16],
            b![0x1a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
        assert_eq!(buf[16..], *b"hello, this is a message.", "payload");
    }
}
