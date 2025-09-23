//! A FUSE (Filesystem in Userspace) library for Rust.

#![doc(html_root_url = "https://docs.rs/polyfuse/0.4.0")]
#![forbid(clippy::todo, clippy::unimplemented)]

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

pub mod bytes;
pub mod fs;
pub mod io;
pub mod op;
pub mod raw;
pub mod reply;
pub mod types;
