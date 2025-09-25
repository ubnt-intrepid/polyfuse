use anyhow::Context as _;
use rustix::fs::{Mode, OFlags};
use std::{
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
        let mut fd = FileDesc::open(path, OFlags::RDONLY | OFlags::NONBLOCK, Mode::empty()).await?;

        let mut buf = String::new();
        fd.read_to_string(&mut buf).await?;
        buf
    } else {
        tokio::fs::read_to_string(&path).await?
    };

    println!("read: {:?}", content);

    Ok(())
}

struct FileDesc {
    inner: AsyncFd<OwnedFd>,
}

impl FileDesc {
    async fn open<A>(path: A, flags: OFlags, mode: Mode) -> io::Result<Self>
    where
        A: rustix::path::Arg + Send + 'static,
    {
        let fd = tokio::task::spawn_blocking(move || rustix::fs::open(path, flags, mode)).await??;

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
            match guard
                .try_io(|inner| rustix::io::read(inner.get_ref(), unfilled).map_err(Into::into))
            {
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
