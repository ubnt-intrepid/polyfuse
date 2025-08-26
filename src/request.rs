use crate::{
    conn::SpliceRead,
    nix::{pipe, Pipe},
    Operation,
};
use libc::{EINTR, ENODEV, ENOENT};
use polyfuse_kernel::{fuse_in_header, fuse_opcode, fuse_write_in};
use std::{
    io::{self, prelude::*},
    mem,
};
use zerocopy::IntoBytes as _;

/// Context about an incoming FUSE request.
pub struct Request {
    header: fuse_in_header,
    arg: Vec<u8>,
    bufsize: usize,
    opcode: Option<fuse_opcode>,
    pipe: Pipe,
}

impl Request {
    pub(crate) fn new(bufsize: usize) -> io::Result<Self> {
        Ok(Self {
            header: fuse_in_header::default(),
            arg: vec![0; bufsize - mem::size_of::<fuse_in_header>()],
            bufsize,
            opcode: None,
            pipe: pipe()?,
        })
    }

    /// Return the unique ID of the request.
    #[inline]
    pub fn unique(&self) -> u64 {
        self.header.unique
    }

    /// Return the user ID of the calling process.
    #[inline]
    pub fn uid(&self) -> u32 {
        self.header.uid
    }

    /// Return the group ID of the calling process.
    #[inline]
    pub fn gid(&self) -> u32 {
        self.header.gid
    }

    /// Return the process ID of the calling process.
    #[inline]
    pub fn pid(&self) -> u32 {
        self.header.pid
    }

    /// Decode the argument of this request.
    pub fn operation(&self) -> Result<Operation<'_>, crate::op::Error> {
        Operation::decode(&self.header, self.opcode(), &self.arg[..])
    }

    pub(crate) fn clear(&mut self) -> io::Result<()> {
        self.header = fuse_in_header::default();
        self.arg
            .resize(self.bufsize - mem::size_of::<fuse_in_header>(), 0);
        if self.pipe.reader.remaining_bytes()? > 0 {
            tracing::warn!(
                "The remaining data of request(unique={}) is destroyed",
                self.unique()
            );
            self.pipe = pipe()?;
        }
        self.opcode = None;
        Ok(())
    }

    pub(crate) fn opcode(&self) -> fuse_opcode {
        self.opcode.expect("The request has not been received yet")
    }

    pub(crate) fn try_receive<T>(&mut self, mut conn: T) -> Result<(), ReceiveError>
    where
        T: SpliceRead,
    {
        let len = conn
            .splice_read(&self.pipe.writer, self.bufsize)
            .map_err(|err| {
                // ref: https://github.com/libfuse/libfuse/blob/fuse-3.10.5/lib/fuse_lowlevel.c#L2865
                match err.raw_os_error() {
                    Some(ENODEV) => ReceiveError::Disconnected,
                    Some(ENOENT) | Some(EINTR) => ReceiveError::Interrupted,
                    _ => ReceiveError::Fatal(err),
                }
            })?;

        if len < mem::size_of::<fuse_in_header>() {
            Err(ReceiveError::invalid_data(
                "dequeued request message is too short",
            ))?
        }

        self.pipe.reader.read_exact(self.header.as_mut_bytes())?;
        if len != self.header.len as usize {
            Err(ReceiveError::invalid_data(
                "The value in_header.len is mismatched to the result of splice(2)",
            ))?
        }

        let opcode = self
            .header
            .opcode
            .try_into()
            .map_err(|_| ReceiveError::UnrecognizedOpcode(self.header.opcode))?;
        self.opcode = Some(opcode);

        let arglen = match self.opcode {
            Some(fuse_opcode::FUSE_WRITE) | Some(fuse_opcode::FUSE_NOTIFY_REPLY) => {
                mem::size_of::<fuse_write_in>()
            }
            _ => self.header.len as usize - mem::size_of::<fuse_in_header>(),
        };
        self.arg.resize(arglen, 0);
        self.pipe.reader.read_exact(&mut self.arg[..])?;

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReceiveError {
    #[error("The connection is disconnected")]
    Disconnected,

    #[error("The read syscall is interrupted")]
    Interrupted,

    #[error(
        "The opcode `{}' is not recognized by the current version of `polfyfuse`",
        _0
    )]
    UnrecognizedOpcode(u32),

    #[error("Unrecoverable I/O error: {}", _0)]
    Fatal(#[from] io::Error),
}

impl ReceiveError {
    fn invalid_data<T>(source: T) -> Self
    where
        T: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        Self::Fatal(io::Error::new(io::ErrorKind::InvalidData, source))
    }
}

impl io::Read for Request {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buf)
    }
    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        (&*self).read_vectored(bufs)
    }
}

impl io::Read for &Request {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.pipe.reader).read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        (&self.pipe.reader).read_vectored(bufs)
    }
}
