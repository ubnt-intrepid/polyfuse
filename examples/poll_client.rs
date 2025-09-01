use anyhow::Context as _;
use libc::{AT_EMPTY_PATH, O_NONBLOCK, O_RDONLY};
use std::{
    ffi::{CString, OsStr}, //
    io,
    os::unix::prelude::*,
    path::PathBuf,
    pin::Pin,
    task::{self, ready, Poll},
};
use tokio::io::{
    unix::AsyncFd, //
    AsyncRead,
    AsyncReadExt as _,
    ReadBuf,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();
    let is_nonblocking = args.contains("--nonblock");
    let path: PathBuf = args.opt_free_from_str()?.context("missing path")?;

    let content = if is_nonblocking {
        let mut fd = FileDesc::open(&path, O_RDONLY | O_NONBLOCK).await?;

        let mut buf = String::new();
        fd.read_to_string(&mut buf).await?;
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

struct FileDesc {
    inner: AsyncFd<RawFd>,
}

impl Drop for FileDesc {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.inner.as_raw_fd());
        }
    }
}

impl FileDesc {
    async fn open(path: impl AsRef<OsStr>, mut flags: libc::c_int) -> io::Result<Self> {
        let path = path.as_ref();
        if path.is_empty() {
            flags |= AT_EMPTY_PATH;
        }
        let c_path = CString::new(path.as_bytes())?;

        let fd =
            tokio::task::spawn_blocking(move || syscall!(open(c_path.as_ptr(), flags))).await??;

        Ok(Self {
            inner: AsyncFd::new(fd)?,
        })
    }
}

impl AsyncRead for FileDesc {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = ready!(self.inner.poll_read_ready(cx))?;

            let unfilled = buf.initialize_unfilled();
            #[allow(clippy::blocks_in_conditions)]
            match guard.try_io(|inner| {
                let len = syscall!(read(
                    inner.as_raw_fd(),
                    unfilled.as_mut_ptr().cast(),
                    unfilled.len()
                ))?;
                Ok(len as usize)
            }) {
                Ok(Ok(len)) => {
                    buf.advance(len);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_would_blodk) => continue,
            }
        }
    }
}
