use mio::{unix::EventedFd, Evented, Poll, PollOpt, Ready, Token};
use pico_args::Arguments;
use std::{
    ffi::{CString, OsStr},
    io::{self, IoSliceMut, Read},
    os::unix::prelude::*,
    path::PathBuf,
};
use tokio::io::{AsyncReadExt, PollEvented};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = Arguments::from_env();
    let is_nonblocking = args.contains("--nonblock");
    let path: PathBuf = args
        .free_from_str()?
        .ok_or_else(|| anyhow::anyhow!("missing path"))?;

    let content = if is_nonblocking {
        let fd = FileDesc::open(&path, libc::O_RDONLY | libc::O_NONBLOCK).await?;
        let mut evented = PollEvented::new(fd)?;
        let mut buf = String::new();
        evented.read_to_string(&mut buf).await?;
        buf
    } else {
        tokio::fs::read_to_string(&path).await?
    };

    println!("read: {:?}", content);

    Ok(())
}

// copied from https://github.com/tokio-rs/mio/blob/master/mio/src/sys/unix/mod.rs
macro_rules! syscall {
    ($name:ident ( $($args:expr),* $(,)? )) => {
        match unsafe { libc::$name($($args),*) } {
            -1 => Err(io::Error::last_os_error()),
            ret => Ok(ret),
        }
    }
}

// copied from https://github.com/tokio-rs/tokio/blob/master/tokio/src/fs/mod.rs
async fn asyncify<F, T>(f: F) -> io::Result<T>
where
    F: FnOnce() -> io::Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(res) => res,
        Err(..) => Err(io::Error::new(
            io::ErrorKind::Other,
            "background task failed",
        )),
    }
}

struct FileDesc(RawFd);

impl Drop for FileDesc {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

impl FileDesc {
    async fn open(path: impl AsRef<OsStr>, mut flags: libc::c_int) -> io::Result<Self> {
        let path = path.as_ref();
        if path.is_empty() {
            flags |= libc::AT_EMPTY_PATH;
        }
        let c_path = CString::new(path.as_bytes())?;
        let fd = asyncify(move || syscall!(open(c_path.as_ptr(), flags))).await?;
        Ok(Self(fd))
    }
}

impl Read for FileDesc {
    fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        let len = syscall!(read(self.0, dst.as_mut_ptr().cast(), dst.len()))?;
        Ok(len as usize)
    }

    fn read_vectored(&mut self, dst: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        let len = syscall!(readv(
            self.0,
            dst.as_mut_ptr().cast(),
            dst.len() as libc::c_int
        ))?;
        Ok(len as usize)
    }
}

impl Evented for FileDesc {
    fn register(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.0).register(poll, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.0).reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &Poll) -> io::Result<()> {
        EventedFd(&self.0).deregister(poll)
    }
}
