use std::{ffi::OsStr, mem, os::unix::prelude::*, slice};

#[inline(always)]
pub unsafe fn as_bytes<T: Sized>(t: &T) -> &[u8] {
    slice::from_raw_parts(t as *const T as *const u8, mem::size_of::<T>())
}

pub(crate) struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub(crate) fn fetch_bytes(&mut self, count: usize) -> Option<&'a [u8]> {
        if self.bytes.len() < self.offset + count {
            return None;
        }
        let bytes = &self.bytes[self.offset..self.offset + count];
        self.offset += count;
        Some(bytes)
    }

    #[allow(dead_code)]
    pub(crate) fn fetch_array<T>(&mut self, count: usize) -> Option<&'a [T]> {
        // FIXME: add assertion that the provided type is aligned.
        self.fetch_bytes(mem::size_of::<T>() * count)
            .map(|bytes| unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const T, count) })
    }

    pub(crate) fn fetch_str(&mut self) -> Option<&'a OsStr> {
        let len = self.bytes[self.offset..].iter().position(|&b| b == b'\0')?;
        let s = self.fetch_bytes(len)?;
        self.offset = std::cmp::min(self.bytes.len(), self.offset + 1);
        Some(OsStr::from_bytes(s))
    }

    pub(crate) fn fetch<T>(&mut self) -> Option<&'a T> {
        // FIXME: add assertion that the provided type is aligned.
        self.fetch_bytes(mem::size_of::<T>())
            .map(|data| unsafe { &*(data.as_ptr() as *const T) })
    }
}
