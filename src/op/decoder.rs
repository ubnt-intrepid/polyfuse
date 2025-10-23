use std::{ffi::OsStr, mem, os::unix::prelude::*};
use zerocopy::{Immutable, KnownLayout, TryFromBytes};

use super::DecodeError;

pub struct Decoder<'a> {
    bytes: &'a [u8],
}

impl<'a> Decoder<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub fn fetch_bytes(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        if self.bytes.len() < count {
            return Err(DecodeError::UnexpectedEof);
        }

        let (bytes, remaining) = self.bytes.split_at(count);
        self.bytes = remaining;

        debug_assert!(bytes.len() >= count);

        Ok(bytes)
    }

    /// Fetch a value of Plain-Old-Data (POD) type by reference.
    pub fn fetch<T>(&mut self) -> Result<&'a T, DecodeError>
    where
        T: TryFromBytes + KnownLayout + Immutable,
    {
        let bytes = self.fetch_bytes(mem::size_of::<T>())?;
        TryFromBytes::try_ref_from_bytes(bytes).map_err(|_err| DecodeError::Unaligned)
    }

    /// Fetch an array of Plain-Old Data (POD) type by reference.
    pub fn fetch_array<T>(&mut self, count: usize) -> Result<&'a [T], DecodeError>
    where
        T: TryFromBytes + KnownLayout + Immutable,
    {
        let bytes = self.fetch_bytes(mem::size_of::<T>() * count)?;
        TryFromBytes::try_ref_from_bytes(bytes).map_err(|_err| DecodeError::Unaligned)
    }

    /// Fetch a zero-terminated OS string by reference.
    pub fn fetch_str(&mut self) -> Result<&'a OsStr, DecodeError> {
        let len = self
            .bytes
            .iter()
            .position(|&b| b == b'\0')
            .ok_or(DecodeError::MissingNulCharacter)?;
        let bytes = self
            .fetch_bytes(len + 1)
            .expect("invalid null terminator position");
        let bytes = &bytes[..bytes.len() - 1];
        Ok(OsStr::from_bytes(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_str() {
        let input = {
            let mut v = vec![0u8; 0];
            v.extend_from_slice(b"foo\0");
            v.extend_from_slice(b"bar\0");
            v
        };

        let mut decoder = Decoder::new(&input[..]);
        assert_eq!(decoder.fetch_str().ok(), Some(OsStr::from_bytes(b"foo")));
        assert_eq!(decoder.fetch_str().ok(), Some(OsStr::from_bytes(b"bar")));
    }

    #[test]
    fn unexpected_eof() {
        const INPUT: &[u8] = &[3, 1, 4, 1, 5, 9, 2, 6, 5];
        assert_eq!(Decoder::new(INPUT).fetch_bytes(9).unwrap(), INPUT);
        assert!(Decoder::new(INPUT).fetch_bytes(10).is_err());

        let mut decoder = Decoder::new(INPUT);
        assert!(decoder.fetch_bytes(8).is_ok());
        assert!(decoder.fetch_bytes(1).is_ok());
        assert!(decoder.fetch_bytes(0).is_ok());
        assert!(decoder.fetch_bytes(1).is_err());

        assert!(Decoder::new(INPUT).fetch::<[u8; 10]>().is_err());
    }

    #[test]
    fn unaligned() {
        let input = [42u64, 0u64];
        let input = unsafe {
            std::slice::from_raw_parts(
                input.as_ptr() as *const u8, //
                input.len() * mem::size_of::<u64>(),
            )
        };

        // Successfully decoded if the input bytes is properly aligned.
        assert!(Decoder::new(input).fetch::<[u64; 1]>().is_ok());

        // Decoding will fail if the alignment of input bytes is wrong.
        let input = &input[2..];
        assert!(input.as_ptr() as usize % mem::align_of::<u64>() != 0);
        assert!(matches!(
            Decoder::new(input).fetch::<[u64; 1]>().err(),
            Some(DecodeError::Unaligned)
        ));
    }

    #[test]
    fn unaligned_array() {
        let input = [42u64, 0u64, 0u64];
        let input = unsafe {
            std::slice::from_raw_parts(
                input.as_ptr() as *const u8, //
                input.len() * mem::size_of::<u64>(),
            )
        };

        assert!(Decoder::new(input).fetch_array::<u64>(2).is_ok());

        let input = &input[2..];
        assert!(input.as_ptr() as usize % mem::align_of::<u64>() != 0);
        assert!(matches!(
            Decoder::new(input).fetch_array::<u64>(2).err(),
            Some(DecodeError::Unaligned)
        ));
    }

    #[test]
    fn missing_nul_terminator() {
        let input = {
            let mut v = vec![0u8; 0];
            v.extend_from_slice(b"foo");
            v
        };

        let mut decoder = Decoder::new(&input[..]);
        assert!(matches!(
            decoder.fetch_str().err(),
            Some(DecodeError::MissingNulCharacter)
        ));
    }
}
