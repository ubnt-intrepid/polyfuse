//! A small facilities of Unix syscalls.

use bitflags::bitflags;
use libc::{c_int, c_void, iovec};
use std::{
    cmp, io,
    os::{fd::BorrowedFd, unix::prelude::*},
    ptr,
};

macro_rules! syscall {
    ($fn:ident ( $($arg:expr),* $(,)* ) ) => {{
        #[allow(unused_unsafe)]
        let res = unsafe { libc::$fn($($arg),*) };
        if res == -1 {
            return Err(std::io::Error::last_os_error());
        }
        res
    }};
}

pub fn read(fd: BorrowedFd<'_>, buf: &mut [u8]) -> io::Result<usize> {
    let len = syscall! {
        read(
            fd.as_raw_fd(), //
            buf.as_mut_ptr() as *mut c_void,
            buf.len(),
        )
    };
    Ok(len as usize)
}

pub fn readv(fd: BorrowedFd<'_>, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
    let len = syscall! {
        readv(
            fd.as_raw_fd(), //
            bufs.as_mut_ptr() as *mut iovec,
            cmp::min(bufs.len(), c_int::max_value() as usize) as c_int,
        )
    };
    Ok(len as usize)
}

pub fn write(fd: BorrowedFd<'_>, buf: &[u8]) -> io::Result<usize> {
    let res = syscall! {
        write(
            fd.as_raw_fd(), //
            buf.as_ptr() as *const c_void,
            buf.len(),
        )
    };
    Ok(res as usize)
}

pub fn writev(fd: BorrowedFd<'_>, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
    let res = syscall! {
        writev(
            fd.as_raw_fd(), //
            bufs.as_ptr() as *const iovec,
            cmp::min(bufs.len(), c_int::max_value() as usize) as c_int,
        )
    };
    Ok(res as usize)
}

pub fn splice(
    fd_in: BorrowedFd<'_>,
    mut off_in: Option<i64>,
    fd_out: BorrowedFd<'_>,
    mut off_out: Option<i64>,
    len: usize,
    flags: SpliceFlags,
) -> io::Result<usize> {
    let off_in = off_in
        .as_mut()
        .map_or_else(ptr::null_mut, |off| ptr::addr_of_mut!(*off));
    let off_out = off_out
        .as_mut()
        .map_or_else(ptr::null_mut, |off| ptr::addr_of_mut!(*off));
    let amount = syscall! {
        splice(
            fd_in.as_raw_fd(),
            off_in,
            fd_out.as_raw_fd(),
            off_out,
            len,
            flags.bits(),
        )
    };
    Ok(amount as usize)
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct SpliceFlags: u32 {
        const MOVE = libc::SPLICE_F_MOVE;
        const NONBLOCK = libc::SPLICE_F_NONBLOCK;
        const MORE = libc::SPLICE_F_MORE;
        const GIFT = libc::SPLICE_F_GIFT;
    }
}
