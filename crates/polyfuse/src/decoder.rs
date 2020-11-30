use std::{ffi::OsStr, mem, os::unix::prelude::*};
use zerocopy::{FromBytes, LayoutVerified};

#[derive(Debug)]
pub(crate) enum DecodeError {
    UnexpectedEof,
    MissingNulCharacter,
    Unaligned,
}

pub(crate) struct Decoder<'a> {
    bytes: &'a [u8],
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub(crate) fn fetch_bytes(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        if self.bytes.len() < count {
            return Err(DecodeError::UnexpectedEof);
        }

        let (bytes, remaining) = self.bytes.split_at(count);
        self.bytes = remaining;

        debug_assert!(bytes.len() >= count);

        Ok(bytes)
    }

    /// Fetch a value of Plain-Old-Data (POD) type by reference.
    pub(crate) fn fetch<T>(&mut self) -> Result<&'a T, DecodeError>
    where
        T: FromBytes,
    {
        let bytes = self.fetch_bytes(mem::size_of::<T>())?;
        let verified = LayoutVerified::<_, T>::new(bytes).ok_or(DecodeError::Unaligned)?;
        Ok(verified.into_ref())
    }

    /// Fetch an array of Plain-Old Data (POD) type by reference.
    #[allow(dead_code)]
    pub(crate) fn fetch_array<T>(&mut self, count: usize) -> Result<&'a [T], DecodeError>
    where
        T: FromBytes,
    {
        let bytes = self.fetch_bytes(mem::size_of::<T>() * count)?;
        let verified = LayoutVerified::<_, [T]>::new_slice(bytes) //
            .ok_or(DecodeError::Unaligned)?;
        Ok(verified.into_slice())
    }

    /// Fetch a zero-terminated OS string by reference.
    pub(crate) fn fetch_str(&mut self) -> Result<&'a OsStr, DecodeError> {
        let len = self
            .bytes
            .iter()
            .position(|&b| b == b'\0')
            .ok_or(DecodeError::MissingNulCharacter)?;
        let bytes = self.fetch_bytes(len + 1)?;
        let bytes = &bytes[..bytes.len() - 2];
        Ok(OsStr::from_bytes(bytes))
    }
}
