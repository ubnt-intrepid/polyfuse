/// A trait that represents a collection of bytes.
pub trait Bytes {
    /// Collect the *scattered* bytes in the `collector`.
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>;
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

impl Bytes for std::ffi::OsStr {
    #[inline]
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        use std::os::unix::prelude::*;
        self.as_bytes().collect(collector)
    }
}

impl Bytes for std::ffi::OsString {
    #[inline]
    fn collect<'a, T: ?Sized>(&'a self, collector: &mut T)
    where
        T: Collector<'a>,
    {
        (**self).collect(collector)
    }
}
