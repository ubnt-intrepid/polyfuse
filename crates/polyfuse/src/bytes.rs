use smallvec::SmallVec;
use std::{
    io::{self, IoSlice},
    os::unix::prelude::*,
};

/// A trait that represents a collection of bytes.
pub trait Bytes {
    /// Return the total size of bytes.
    fn size(&self) -> usize;

    /// Collect the *scattered* bytes in the `collector`.
    fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>);
}

/// Container for collecting the scattered bytes.
pub trait Collector<'a> {
    /// Append a chunk of bytes into itself.
    fn append(&mut self, buf: &'a [u8]);
}

// ==== pointer types ====

macro_rules! impl_reply_body_for_pointers {
    () => {
        #[inline]
        fn size(&self) -> usize {
            (**self).size()
        }

        #[inline]
        fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>) {
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
    fn size(&self) -> usize {
        0
    }

    #[inline]
    fn collect<'a>(&'a self, _: &mut dyn Collector<'a>) {}
}

impl Bytes for [u8; 0] {
    #[inline]
    fn size(&self) -> usize {
        0
    }

    #[inline]
    fn collect<'a>(&'a self, _: &mut dyn Collector<'a>) {}
}

// ==== compound types ====

macro_rules! impl_reply_for_tuple {
    ($($T:ident),+ $(,)?) => {
        #[allow(nonstandard_style)]
        impl<$($T),+> Bytes for ($($T,)+)
        where
            $( $T: Bytes, )+
        {
            #[inline]
            fn size(&self) -> usize {
                let ($($T,)+) = self;
                let mut size = 0;
                $(
                    size += $T.size();
                )+
                size
            }

            #[inline]
            fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>) {
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
    fn size(&self) -> usize {
        self.iter().map(|chunk| chunk.size()).sum()
    }

    #[inline]
    fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>) {
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
    fn size(&self) -> usize {
        self.iter().map(|chunk| chunk.size()).sum()
    }

    #[inline]
    fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>) {
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
    fn size(&self) -> usize {
        self.as_ref().map_or(0, |b| b.size())
    }

    #[inline]
    fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>) {
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
                fn size(&self) -> usize {
                    as_bytes(self).len()
                }

                #[inline]
                fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>) {
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

impl Bytes for std::ffi::OsStr {
    #[inline]
    fn size(&self) -> usize {
        self.as_bytes().len()
    }

    #[inline]
    fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>) {
        Bytes::collect(self.as_bytes(), collector)
    }
}

impl Bytes for std::ffi::OsString {
    #[inline]
    fn size(&self) -> usize {
        self.as_bytes().len()
    }

    #[inline]
    fn collect<'a>(&'a self, collector: &mut dyn Collector<'a>) {
        (**self).collect(collector)
    }
}

pub fn write_bytes<W, T>(mut writer: W, bytes: T) -> io::Result<()>
where
    W: io::Write,
    T: Bytes,
{
    let mut vec = SmallVec::new();
    let mut collector = VecCollector {
        vec: &mut vec,
        total_len: 0,
    };
    bytes.collect(&mut collector);
    assert_eq!(
        collector.total_len,
        bytes.size(),
        "incorrect Bytes::size implementation"
    );

    let count = writer.write_vectored(&*vec)?;
    if count < bytes.size() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "written data is too short",
        ));
    }

    Ok(())
}

struct VecCollector<'a, 'v> {
    vec: &'v mut SmallVec<[IoSlice<'a>; 4]>,
    total_len: usize,
}

impl<'a> Collector<'a> for VecCollector<'a, '_> {
    fn append(&mut self, chunk: &'a [u8]) {
        self.vec.push(IoSlice::new(chunk));
        self.total_len += chunk.len();
    }
}
