use crate::{
    buf::Buffer,
    channel::Channel,
    conn::MountOptions,
    fs::Filesystem,
    session::{Background, Session},
};
use futures_io::{AsyncRead, AsyncWrite};
use futures_util::{
    future::{Future, FutureExt},
    select,
    stream::StreamExt,
};
use libc::c_int;
use std::io;
use std::path::Path;
use tokio_net::signal::unix::{signal, SignalKind};

/// Run a FUSE filesystem.
pub async fn run<T, I, S>(fs: T, channel: I, sig: S) -> io::Result<Option<S::Output>>
where
    T: for<'a> Filesystem<&'a [u8]>,
    I: AsyncRead + AsyncWrite + Unpin + Clone + 'static,
    S: Future + Unpin,
{
    let mut channel = channel;
    let mut sig = sig.fuse();
    let mut fs = fs;

    let mut buf = Buffer::default();

    let mut session = Session::initializer() //
        .start(&mut channel, &mut buf)
        .await?;

    let mut background = Background::new();

    let mut main_loop = Box::pin(main_loop(
        &mut session,
        &mut buf,
        &mut channel,
        &mut fs,
        &mut background,
    ))
    .fuse();

    // FIXME: graceful shutdown the background tasks.
    select! {
        _ = main_loop => Ok(None),
        sig = sig => Ok(Some(sig)),
    }
}

async fn main_loop<I, T>(
    session: &mut Session,
    buf: &mut Buffer,
    channel: &mut I,
    fs: &mut T,
    background: &mut Background,
) -> io::Result<()>
where
    T: for<'a> Filesystem<&'a [u8]>,
    I: AsyncRead + AsyncWrite + Unpin + Clone + 'static,
{
    loop {
        let terminated = buf.receive(&mut *channel).await?;
        if terminated {
            log::debug!("connection was closed by the kernel");
            return Ok::<_, io::Error>(());
        }

        session
            .process(&mut *buf, &*channel, &mut *fs, &mut *background)
            .await?;
    }
}

/// Run a FUSE filesystem mounted on the specified path.
pub async fn mount<T>(
    fs: T,
    mointpoint: impl AsRef<Path>,
    mountopts: MountOptions,
) -> io::Result<()>
where
    T: for<'a> Filesystem<&'a [u8]>,
{
    let channel = Channel::open(mointpoint, mountopts)?;
    let sig = default_shutdown_signal()?;

    run(fs, channel, sig).await?;
    Ok(())
}

fn default_shutdown_signal() -> io::Result<impl Future<Output = c_int> + Unpin> {
    let mut sighup = signal(SignalKind::hangup())?.into_future();
    let mut sigint = signal(SignalKind::interrupt())?.into_future();
    let mut sigterm = signal(SignalKind::terminate())?.into_future();
    let mut sigpipe = signal(SignalKind::pipe())?.into_future();

    Ok(Box::pin(async move {
        loop {
            select! {
                _ = sighup => {
                    log::debug!("Got SIGHUP");
                    return libc::SIGHUP;
                },
                _ = sigint => {
                    log::debug!("Got SIGINT");
                    return libc::SIGINT;
                },
                _ = sigterm => {
                    log::debug!("Got SIGTERM");
                    return libc::SIGTERM;
                },
                _ = sigpipe => {
                    log::debug!("Got SIGPIPE (and ignored)");
                    continue
                }
            }
        }
    }))
}
