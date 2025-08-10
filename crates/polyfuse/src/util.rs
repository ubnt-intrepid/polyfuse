use crate::bytes::{Bytes, FillBytes};
use polyfuse_kernel::{fuse_notify_code, fuse_out_header};
use std::{
    io::{self, IoSlice},
    mem::{self, MaybeUninit},
};
use zerocopy::IntoBytes;

pub(crate) struct Reply<T> {
    header: fuse_out_header,
    arg: T,
}

impl<T> Reply<T>
where
    T: Bytes,
{
    #[inline]
    pub(crate) fn new(unique: u64, error: i32, arg: T) -> Self {
        let len = (mem::size_of::<fuse_out_header>() + arg.size())
            .try_into()
            .expect("Argument size is too large");
        Self {
            header: fuse_out_header {
                len,
                error: -error,
                unique,
            },
            arg,
        }
    }
}

impl<T> Bytes for Reply<T>
where
    T: Bytes,
{
    #[inline]
    fn size(&self) -> usize {
        self.header.len as usize
    }

    #[inline]
    fn count(&self) -> usize {
        self.arg.count() + 1
    }

    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        dst.put(self.header.as_bytes());
        self.arg.fill_bytes(dst);
    }
}

pub(crate) struct Notify<T> {
    header: fuse_out_header,
    arg: T,
}

impl<T> Notify<T>
where
    T: Bytes,
{
    #[inline]
    pub(crate) fn new(code: fuse_notify_code, arg: T) -> Self {
        let len = (mem::size_of::<fuse_out_header>() + arg.size())
            .try_into()
            .expect("Argument size is too large");
        Self {
            header: fuse_out_header {
                len,
                error: code as i32,
                unique: 0,
            },
            arg,
        }
    }
}

impl<T> Bytes for Notify<T>
where
    T: Bytes,
{
    #[inline]
    fn size(&self) -> usize {
        self.header.len as usize
    }

    #[inline]
    fn count(&self) -> usize {
        self.arg.count() + 1
    }

    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        dst.put(self.header.as_bytes());
        self.arg.fill_bytes(dst);
    }
}

#[inline]
pub(crate) fn write_bytes<W, T>(mut writer: W, bytes: T) -> io::Result<()>
where
    W: io::Write,
    T: Bytes,
{
    let size = bytes.size();
    let count = bytes.count();

    let written;

    macro_rules! small_write {
        ($n:expr) => {{
            let mut vec: [MaybeUninit<IoSlice<'_>>; $n] =
                unsafe { MaybeUninit::uninit().assume_init() };
            bytes.fill_bytes(&mut FillWriteBytes {
                vec: &mut vec[..],
                offset: 0,
            });
            let vec = unsafe { slice_assume_init_ref(&vec[..]) };

            written = writer.write_vectored(vec)?;
        }};
    }

    match count {
        // Skip writing.
        0 => return Ok(()),

        // Avoid heap allocation if count is small.
        1 => small_write!(1),
        2 => small_write!(2),
        3 => small_write!(3),
        4 => small_write!(4),

        count => {
            let mut vec: Vec<IoSlice<'_>> = Vec::with_capacity(count);
            unsafe {
                let dst = std::slice::from_raw_parts_mut(
                    vec.as_mut_ptr().cast(), //
                    count,
                );
                bytes.fill_bytes(&mut FillWriteBytes {
                    vec: dst,
                    offset: 0,
                });
                vec.set_len(count);
            }

            written = writer.write_vectored(&*vec)?;
        }
    }

    if written < size {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "written data is too short",
        ));
    }

    Ok(())
}

struct FillWriteBytes<'a, 'vec> {
    vec: &'vec mut [MaybeUninit<IoSlice<'a>>],
    offset: usize,
}

impl<'a, 'vec> FillBytes<'a> for FillWriteBytes<'a, 'vec> {
    fn put(&mut self, chunk: &'a [u8]) {
        self.vec[self.offset] = MaybeUninit::new(IoSlice::new(chunk));
        self.offset += 1;
    }
}

// FIXME: replace with stabilized MaybeUninit::slice_assume_init_ref.
#[inline(always)]
unsafe fn slice_assume_init_ref<T>(slice: &[MaybeUninit<T>]) -> &[T] {
    #[allow(unused_unsafe)]
    unsafe {
        &*(slice as *const [MaybeUninit<T>] as *const [T])
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
        write_bytes(&mut buf, Reply::new(42, -4, &[])).unwrap();
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
        write_bytes(&mut buf, Reply::new(42, 0, "hello")).unwrap();
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
        write_bytes(&mut buf, Reply::new(26, 0, payload)).unwrap();
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
