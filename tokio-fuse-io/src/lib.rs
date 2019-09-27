//! Additional I/O primitives.

mod fd_source;
mod misc;
mod read_vectored;
mod write_vectored;

pub use crate::fd_source::FdSource;
pub use crate::misc::set_nonblocking;
pub use crate::read_vectored::{AsyncReadVectored, AsyncReadVectoredExt};
pub use crate::write_vectored::{AsyncWriteVectored, AsyncWriteVectoredExt};
