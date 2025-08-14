//! Bytes facilities.

mod decoder;

pub use decoder::{DecodeError, Decoder};

use either::Either;
use std::os::unix::prelude::*;
use zerocopy::{Immutable, IntoBytes};

/// A trait that represents a collection of bytes.
///
/// The role of this trait is similar to [`Buf`] provided by [`bytes`] crate,
/// but it focuses on the situation where all byte chunks are written in a *single*
/// operation.
/// This difference is due to the requirement of FUSE kernel driver that all data in
/// a reply message must be passed in a single `write(2)` syscall.
///
/// [`bytes`]: https://docs.rs/bytes/0.6/bytes
/// [`Buf`]: https://docs.rs/bytes/0.6/bytes/trait.Buf.html
pub trait Bytes {
    /// Return the total amount of bytes contained in this data.
    fn size(&self) -> usize;

    /// Return the number of byte chunks.
    fn count(&self) -> usize;

    /// Fill with potentially multiple slices in this data.
    ///
    /// This method corresonds to [`Buf::bytes_vectored`][bytes_vectored], except that
    /// the number of byte chunks is acquired from `Bytes::count` and the implementation
    /// needs to add all chunks in `dst`.
    ///
    /// [bytes_vectored]: https://docs.rs/bytes/0.6/bytes/trait.Buf.html#method.bytes_vectored
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>);
}

/// The container of scattered bytes.
pub trait FillBytes<'a> {
    /// Put a chunk of bytes into this container.
    fn put(&mut self, chunk: &'a [u8]);
}

// ==== pointer types ====

macro_rules! impl_reply_body_for_pointers {
    () => {
        #[inline]
        fn size(&self) -> usize {
            (**self).size()
        }

        #[inline]
        fn count(&self) -> usize {
            (**self).count()
        }

        #[inline]
        fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
            (**self).fill_bytes(dst)
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
    fn count(&self) -> usize {
        0
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, _: &mut dyn FillBytes<'a>) {}
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
            fn count(&self) -> usize {
                let ($($T,)+) = self;
                let mut count = 0;
                $(
                    count += $T.count();
                )+
                count
            }

            #[inline]
            fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
                let ($($T,)+) = self;
                $(
                    Bytes::fill_bytes($T, dst);
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
impl_reply_for_tuple!(T1, T2, T3, T4, T5, T6);
impl_reply_for_tuple!(T1, T2, T3, T4, T5, T6, T7);
impl_reply_for_tuple!(T1, T2, T3, T4, T5, T6, T7, T8);
impl_reply_for_tuple!(T1, T2, T3, T4, T5, T6, T7, T8, T9);
impl_reply_for_tuple!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);

impl<R, const N: usize> Bytes for [R; N]
where
    R: Bytes,
{
    fn size(&self) -> usize {
        self.as_slice().size()
    }

    fn count(&self) -> usize {
        self.as_slice().count()
    }

    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        self.as_slice().fill_bytes(dst)
    }
}

impl<R> Bytes for [R]
where
    R: Bytes,
{
    #[inline]
    fn size(&self) -> usize {
        self.iter().map(|chunk| chunk.size()).sum()
    }

    #[inline]
    fn count(&self) -> usize {
        self.iter().map(|chunk| chunk.count()).sum()
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        for t in self {
            Bytes::fill_bytes(t, dst);
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
    fn count(&self) -> usize {
        self.iter().map(|chunk| chunk.count()).sum()
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        for t in self {
            Bytes::fill_bytes(t, dst);
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
    fn count(&self) -> usize {
        self.as_ref().map_or(0, |b| b.count())
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        if let Some(t) = self {
            Bytes::fill_bytes(t, dst)
        }
    }
}

// ==== Either<L, R> ====

impl<L, R> Bytes for Either<L, R>
where
    L: Bytes,
    R: Bytes,
{
    #[inline]
    fn size(&self) -> usize {
        match self {
            Either::Left(l) => l.size(),
            Either::Right(r) => r.size(),
        }
    }

    #[inline]
    fn count(&self) -> usize {
        match self {
            Either::Left(l) => l.count(),
            Either::Right(r) => r.count(),
        }
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        match self {
            Either::Left(l) => Bytes::fill_bytes(l, dst),
            Either::Right(r) => Bytes::fill_bytes(r, dst),
        }
    }
}

// ==== continuous bytes ====

impl Bytes for [u8] {
    #[inline]
    fn size(&self) -> usize {
        self.len()
    }

    #[inline]
    fn count(&self) -> usize {
        if self.is_empty() {
            0
        } else {
            1
        }
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        if !self.is_empty() {
            dst.put(self);
        }
    }
}

impl<const N: usize> Bytes for [u8; N] {
    #[inline]
    fn size(&self) -> usize {
        if N == 0 {
            0
        } else {
            self.len()
        }
    }

    #[inline]
    fn count(&self) -> usize {
        if N == 0 || self.is_empty() {
            0
        } else {
            1
        }
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        if N > 0 && !self.is_empty() {
            dst.put(self);
        }
    }
}

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
                fn count(&self) -> usize {
                    if as_bytes(self).is_empty() {
                        0
                    } else {
                        1
                    }
                }

                #[inline]
                fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
                    as_bytes(self).fill_bytes(dst)
                }
            }
        )*};
    }

    impl_reply! {
        str,
        String,
        Vec<u8>,
        std::borrow::Cow<'_, [u8]>,
    }
}

impl Bytes for std::ffi::OsStr {
    #[inline]
    fn size(&self) -> usize {
        Bytes::size(self.as_bytes())
    }

    #[inline]
    fn count(&self) -> usize {
        Bytes::count(self.as_bytes())
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        Bytes::fill_bytes(self.as_bytes(), dst)
    }
}

impl Bytes for std::ffi::OsString {
    #[inline]
    fn size(&self) -> usize {
        Bytes::size(self.as_bytes())
    }

    #[inline]
    fn count(&self) -> usize {
        Bytes::count(self.as_bytes())
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        Bytes::fill_bytes(self.as_bytes(), dst)
    }
}

impl Bytes for std::ffi::CStr {
    fn size(&self) -> usize {
        self.to_bytes_with_nul().len()
    }

    fn count(&self) -> usize {
        1 // &CStr always contains null terminator ('\0') and not empty
    }

    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        dst.put(self.to_bytes_with_nul())
    }
}

impl Bytes for std::ffi::CString {
    #[inline]
    fn size(&self) -> usize {
        Bytes::size(self.as_c_str())
    }

    #[inline]
    fn count(&self) -> usize {
        Bytes::count(self.as_c_str())
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        Bytes::fill_bytes(self.as_c_str(), dst);
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct POD<T>(pub T);

impl<T> Bytes for POD<T>
where
    T: IntoBytes + Immutable,
{
    #[inline]
    fn count(&self) -> usize {
        1
    }

    #[inline]
    fn size(&self) -> usize {
        self.0.as_bytes().len()
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        dst.put(self.0.as_bytes());
    }
}
