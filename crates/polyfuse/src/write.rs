use polyfuse_kernel::{fuse_notify_code, fuse_out_header};
use smallvec::SmallVec;
use std::{
    convert::TryInto as _,
    ffi::{OsStr, OsString},
    io, mem,
    os::unix::prelude::*,
};

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

        self.header.len = (mem::size_of::<fuse_out_header>() + collector.total_len)
            .try_into()
            .expect("too large data");

        drop(collector);

        let vec: SmallVec<[_; 4]> = Some(io::IoSlice::new(unsafe {
            crate::util::as_bytes(&self.header)
        }))
        .into_iter()
        .chain(vec.iter().map(|t| io::IoSlice::new(&*t)))
        .collect();

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
