//! Error representation

use std::io;

/// The error type returned from the filesystem.
pub trait Error {
    /// Construct the error value from an I/O error.
    fn from_io_error(io_error: io::Error) -> Self;

    /// Construct the error value from an OS error code.
    #[inline]
    fn from_code(code: i32) -> Self
    where
        Self: Sized,
    {
        Self::from_io_error(io::Error::from_raw_os_error(code))
    }
}

/// A shortcut to `<E as Error>::from_io_error(io_error)`.
#[inline]
pub fn io<E>(io_error: io::Error) -> E
where
    E: Error,
{
    E::from_io_error(io_error)
}

/// A shortcut to `<E as Error>::from_code(code)`.
#[inline]
pub fn code<E>(code: i32) -> E
where
    E: Error,
{
    E::from_code(code)
}
