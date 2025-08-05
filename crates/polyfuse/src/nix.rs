//! A small facilities of Unix syscalls.

use libc::{c_int, c_void, iovec};
use std::{
    cmp, io,
    os::{fd::BorrowedFd, unix::prelude::*},
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
