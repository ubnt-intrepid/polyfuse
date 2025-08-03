use libc::{c_int, c_void, iovec};
use std::{cmp, io, os::fd::AsRawFd};

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

pub fn read(fd: &impl AsRawFd, buf: &mut [u8]) -> io::Result<usize> {
    let len = syscall! {
        read(
            fd.as_raw_fd(), //
            buf.as_mut_ptr() as *mut c_void,
            buf.len(),
        )
    };
    Ok(len as usize)
}

pub fn read_vectored(fd: &impl AsRawFd, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
    let len = syscall! {
        readv(
            fd.as_raw_fd(), //
            bufs.as_mut_ptr() as *mut iovec,
            cmp::min(bufs.len(), c_int::max_value() as usize) as c_int,
        )
    };
    Ok(len as usize)
}

pub fn write(fd: &impl AsRawFd, buf: &[u8]) -> io::Result<usize> {
    let res = syscall! {
        write(
            fd.as_raw_fd(), //
            buf.as_ptr() as *const c_void,
            buf.len(),
        )
    };
    Ok(res as usize)
}

pub fn write_vectored(fd: &impl AsRawFd, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
    let res = syscall! {
        writev(
            fd.as_raw_fd(), //
            bufs.as_ptr() as *const iovec,
            cmp::min(bufs.len(), c_int::max_value() as usize) as c_int,
        )
    };
    Ok(res as usize)
}
