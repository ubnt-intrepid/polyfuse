use polyfuse_kernel::{fuse_notify_code, fuse_out_header};
use smallvec::SmallVec;
use std::convert::TryFrom;
use std::ffi::{OsStr, OsString};
use std::io;
use std::mem;
use std::os::unix::prelude::*;

#[inline]
pub fn send_reply<W, T>(writer: W, unique: u64, data: T) -> io::Result<()>
where
    W: io::Write,
    T: Bytes,
{
    send_msg(writer, unique, 0, data)
}

#[inline]
pub fn send_error<W>(writer: W, unique: u64, error: i32) -> io::Result<()>
where
    W: io::Write,
{
    send_msg(writer, unique, -error, &[])
}

#[allow(dead_code)]
#[inline]
pub fn send_notify<W, T>(writer: W, code: fuse_notify_code, data: T) -> io::Result<()>
where
    W: io::Write,
    T: Bytes,
{
    send_msg(writer, 0, unsafe { mem::transmute::<_, i32>(code) }, data)
}

fn send_msg<W, T>(mut writer: W, unique: u64, error: i32, data: T) -> io::Result<()>
where
    W: io::Write,
    T: Bytes,
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
    data.collect(&mut collector);

    let len = u32::try_from(mem::size_of::<fuse_out_header>() + collector.total_len).unwrap();
    let header = fuse_out_header { unique, error, len };

    drop(collector);

    let vec: SmallVec<[_; 4]> = Some(io::IoSlice::new(unsafe { crate::util::as_bytes(&header) }))
        .into_iter()
        .chain(vec.iter().map(|t| io::IoSlice::new(&*t)))
        .collect();

    let count = writer.write_vectored(&*vec)?;
    if count < header.len as usize {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "written data is too short",
        ));
    }

    Ok(())
}

/// A trait that represents a collection of bytes.
pub trait Bytes {
    /// Collect the *scattered* bytes in the `collector`.
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>;
}

// ==== pointer types ====

macro_rules! impl_reply_body_for_pointers {
    () => {
        #[inline]
        fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
        where
            T: Collector<'a>,
        {
            (**self).collect(collector)
        }
    };
}

impl<R: ?Sized> Bytes for &R
where
    R: Bytes,
{
    impl_reply_body_for_pointers!();
}

impl<R: ?Sized> Bytes for &mut R
where
    R: Bytes,
{
    impl_reply_body_for_pointers!();
}

impl<R: ?Sized> Bytes for Box<R>
where
    R: Bytes,
{
    impl_reply_body_for_pointers!();
}

impl<R: ?Sized> Bytes for std::rc::Rc<R>
where
    R: Bytes,
{
    impl_reply_body_for_pointers!();
}

impl<R: ?Sized> Bytes for std::sync::Arc<R>
where
    R: Bytes,
{
    impl_reply_body_for_pointers!();
}

// ==== empty bytes ====

impl Bytes for () {
    #[inline]
    fn collect<'a, T: ?Sized>(&'a self, _: &mut T)
    where
        T: Collector<'a>,
    {
    }
}

impl Bytes for [u8; 0] {
    #[inline]
    fn collect<'a, T: ?Sized>(&'a self, _: &mut T)
    where
        T: Collector<'a>,
    {
    }
}

// ==== compound types ====

macro_rules! impl_reply_for_tuple {
    ($($T:ident),+ $(,)?) => {
        impl<$($T),+> Bytes for ($($T,)+)
        where
            $( $T: Bytes, )+
        {
            #[allow(nonstandard_style)]
            #[inline]
            fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
            where
                T: Collector<'a>,
            {
                let ($($T,)+) = self;
                $(
                    $T.collect(collector);
                )+
            }
        }
    }
}

impl_reply_for_tuple!(T1);
impl_reply_for_tuple!(T1, T2);
impl_reply_for_tuple!(T1, T2, T3);
impl_reply_for_tuple!(T1, T2, T3, T4);
impl_reply_for_tuple!(T1, T2, T3, T4, T5);

impl<R> Bytes for [R]
where
    R: Bytes,
{
    #[inline]
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        for t in self {
            t.collect(collector);
        }
    }
}

impl<R> Bytes for Vec<R>
where
    R: Bytes,
{
    #[inline]
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        for t in self {
            t.collect(collector);
        }
    }
}

// ==== Option<T> ====

impl<T> Bytes for Option<T>
where
    T: Bytes,
{
    #[inline]
    fn collect<'a, C: ?Sized>(&'a self, collector: &mut C)
    where
        C: Collector<'a>,
    {
        if let Some(ref reply) = self {
            reply.collect(collector);
        }
    }
}

// ==== continuous bytes ====

mod impl_scattered_bytes_for_cont {
    use super::*;

    #[inline(always)]
    fn as_bytes(t: &(impl AsRef<[u8]> + ?Sized)) -> &[u8] {
        t.as_ref()
    }

    macro_rules! impl_reply {
        ($($t:ty),*$(,)?) => {$(
            impl Bytes for $t {
                #[inline]
                fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
                where
                    T: Collector<'a>,
                {
                    let this = as_bytes(self);
                    if !this.is_empty() {
                        collector.append(this);
                    }
                }
            }
        )*};
    }

    impl_reply! {
        [u8],
        str,
        String,
        Vec<u8>,
        std::borrow::Cow<'_, [u8]>,
    }
}

impl Bytes for OsStr {
    #[inline]
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        self.as_bytes().collect(collector)
    }
}

impl Bytes for OsString {
    #[inline]
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        (**self).collect(collector)
    }
}

/// Container for collecting the scattered bytes.
pub trait Collector<'a> {
    /// Append a chunk of bytes into itself.
    fn append(&mut self, buf: &'a [u8]);
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
        send_msg(&mut buf, 42, 4, &[]).unwrap();
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
        send_msg(&mut buf, 42, 0, "hello").unwrap();
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
        send_msg(&mut buf, 26, 0, payload).unwrap();
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
