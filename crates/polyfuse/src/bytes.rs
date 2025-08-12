//! Bytes facilities.

mod decoder;

pub use decoder::{DecodeError, Decoder};

use either::Either;
use std::{io, mem::MaybeUninit, os::unix::prelude::*};
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

impl Bytes for [u8; 0] {
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
                    let this = as_bytes(self);
                    if !this.is_empty() {
                        dst.put(this);
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

// ---- write_bytes ----

#[inline]
pub(crate) fn write_bytes<B, F>(bytes: B, write: F) -> io::Result<()>
where
    B: Bytes,
    F: FnOnce(&[io::IoSlice<'_>]) -> io::Result<usize>,
{
    let size = bytes.size();
    let count = bytes.count();

    let written;

    macro_rules! small_write {
        ($n:expr) => {{
            let mut vec: [MaybeUninit<io::IoSlice<'_>>; $n] =
                unsafe { MaybeUninit::uninit().assume_init() };
            bytes.fill_bytes(&mut FillWriteBytes {
                vec: &mut vec[..],
                offset: 0,
            });
            let vec = unsafe { slice_assume_init_ref(&vec[..]) };

            written = write(vec)?;
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
            let mut vec: Vec<io::IoSlice<'_>> = Vec::with_capacity(count);
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

            written = write(&*vec)?;
        }
    }

    if written < size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "written data is too short",
        ));
    }

    Ok(())
}

struct FillWriteBytes<'a, 'vec> {
    vec: &'vec mut [MaybeUninit<io::IoSlice<'a>>],
    offset: usize,
}

impl<'a, 'vec> FillBytes<'a> for FillWriteBytes<'a, 'vec> {
    fn put(&mut self, chunk: &'a [u8]) {
        self.vec[self.offset] = MaybeUninit::new(io::IoSlice::new(chunk));
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
    use std::io::prelude::*;

    fn do_write_test(bytes: impl Bytes, expected: &[u8]) {
        let mut actual = vec![];
        write_bytes(bytes, |bufs| actual.write_vectored(bufs)).expect("write_bytes");
        assert_eq!(actual[..], *expected);
    }

    #[inline]
    fn bytes(bytes: &[u8]) -> &[u8] {
        bytes
    }

    macro_rules! b {
        ($($b:expr),*$(,)?) => ( bytes(&[$($b),*]) );
    }

    #[test]
    fn test_write_bytes() {
        do_write_test([], &[]);
        do_write_test(POD(42u32), &[0x2A, 0x00, 0x00, 0x00]);
        do_write_test((b![0x04], b![0x02]), &[0x04, 0x02]);
        do_write_test(("hello", ", ", "world"), b"hello, world");
        do_write_test(&["a", "b", "c", "d"] as &[_], b"abcd");
        do_write_test(&["e", "f", "g", "h", "i"] as &[_], b"efghi");
    }
}
