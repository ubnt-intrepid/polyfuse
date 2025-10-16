//! Bytes facilities.

mod decoder;

pub use decoder::{DecodeError, Decoder};

use either::Either;
use std::{
    io::{self, IoSlice},
    mem::MaybeUninit,
    os::unix::prelude::*,
};
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

impl FillBytes<'_> for Vec<u8> {
    fn put(&mut self, chunk: &'_ [u8]) {
        self.extend_from_slice(chunk);
    }
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
        Bytes::size(self.as_os_str())
    }

    #[inline]
    fn count(&self) -> usize {
        Bytes::count(self.as_os_str())
    }

    #[inline]
    fn fill_bytes<'a>(&'a self, dst: &mut dyn FillBytes<'a>) {
        Bytes::fill_bytes(self.as_os_str(), dst)
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

#[inline]
pub fn write_bytes<W, T>(mut writer: W, bytes: T) -> io::Result<()>
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

            written = writer.write_vectored(&vec)?;
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
    use polyfuse_kernel::fuse_in_header;
    use std::{
        ffi::{CString, OsString},
        mem,
    };

    use super::*;

    fn to_vec(b: impl Bytes) -> Vec<u8> {
        struct ToVec(Vec<u8>);
        impl<'a> FillBytes<'a> for ToVec {
            fn put(&mut self, chunk: &'a [u8]) {
                self.0.extend_from_slice(chunk);
            }
        }
        let mut to_vec = ToVec(Vec::with_capacity(b.size()));
        b.fill_bytes(&mut to_vec);
        to_vec.0
    }

    #[test]
    fn test_bytes_impl_for_array() {
        let array: [u8; 12] = *b"hello, world";
        assert_eq!(array.size(), 12);
        assert_eq!(array.count(), 1);
        assert_eq!(to_vec(array), b"hello, world");

        let array: [u8; 0] = [];
        assert_eq!(array.size(), 0);
        assert_eq!(array.count(), 0);
        assert_eq!(to_vec(array), []);
    }

    #[test]
    fn test_bytes_impl_for_compound_types() {
        let bytes = (
            Either::<_, ()>::Left("foo"),
            (Some("bar"), (None::<&str>, Either::<(), _>::Right("baz"))),
        );
        assert_eq!(bytes.size(), 9);
        assert_eq!(bytes.count(), 3);
        assert_eq!(to_vec(bytes), b"foobarbaz");

        let array = ["foo", "bar", "", "baz"];
        assert_eq!(array.size(), 9);
        assert_eq!(array.count(), 3);
        assert_eq!(to_vec(array), b"foobarbaz");

        let vec = vec!["foo", "bar", "", "baz"];
        assert_eq!(vec.size(), 9);
        assert_eq!(vec.count(), 3);
        assert_eq!(to_vec(vec), b"foobarbaz");
    }

    #[test]
    fn test_bytes_impl_misc() {
        let b: &[u8] = b"";
        assert_eq!(b.size(), 0);
        assert_eq!(b.count(), 0);
        assert_eq!(to_vec(b), b"");
    }

    #[test]
    fn test_bytes_impl_for_os_str() {
        let s = OsString::from("hello, world");
        assert_eq!(s.size(), 12);
        assert_eq!(s.count(), 1);
        assert_eq!(to_vec(s), b"hello, world");
    }

    #[test]
    fn test_bytes_impl_for_c_str() {
        let s = CString::new("hello, world").expect("CString::new");
        assert_eq!(s.size(), 13);
        assert_eq!(s.count(), 1);
        assert_eq!(to_vec(s), b"hello, world\0");

        let s = CString::new(b"").unwrap();
        assert_eq!(s.size(), 1);
        assert_eq!(s.count(), 1);
        assert_eq!(to_vec(s), b"\0");
    }

    #[inline]
    fn b(bytes: &[u8]) -> &[u8] {
        bytes
    }

    #[test]
    fn test_bytes_impl_for_pod() {
        let payload = POD(fuse_in_header {
            len: 42,
            opcode: 23,
            unique: 6,
            nodeid: 95,
            uid: 100,
            gid: 100,
            pid: 7,
            total_extlen: 0,
            padding: 0,
        });
        assert_eq!(payload.size(), mem::size_of::<fuse_in_header>());
        assert_eq!(payload.count(), 1);
        assert_eq!(
            to_vec(payload),
            b(&[
                42, 0, 0, 0, // len
                23, 0, 0, 0, //opcode
                6, 0, 0, 0, 0, 0, 0, 0, // unique
                95, 0, 0, 0, 0, 0, 0, 0, // nodeid
                100, 0, 0, 0, // uid
                100, 0, 0, 0, // gid
                7, 0, 0, 0, // gid
                0, 0, // total_extlen
                0, 0, // padding
            ])
        );
    }
}
