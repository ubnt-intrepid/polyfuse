use std::{mem, slice};

#[inline(always)]
pub unsafe fn as_bytes<T: Sized>(t: &T) -> &[u8] {
    slice::from_raw_parts(t as *const T as *const u8, mem::size_of::<T>())
}
