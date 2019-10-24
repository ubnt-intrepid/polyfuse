use crate::{
    buf::Buffer,
    fs::Filesystem,
    session::{Background, Session},
};
use futures_io::{AsyncRead, AsyncWrite};
use futures_util::{
    future::{Future, FutureExt},
    ready, select,
};
use std::{
    io,
    pin::Pin,
    task::{self, Poll},
};

/// Run a main loop of FUSE filesystem.
pub async fn main_loop<T, I, S>(fs: T, channel: I, sig: S) -> io::Result<Option<S::Output>>
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

    let mut main_loop = MainLoop {
        session: &mut session,
        background: &mut background,
        channel: &mut channel,
        buf: &mut buf,
        fs: &mut fs,
    }
    .fuse();

    // FIXME: graceful shutdown the background tasks.
    select! {
        _ = main_loop => Ok(None),
        sig = sig => Ok(Some(sig)),
    }
}

#[allow(missing_debug_implementations)]
struct MainLoop<'a, I, T> {
    session: &'a mut Session,
    background: &'a mut Background,
    channel: &'a mut I,
    buf: &'a mut Buffer,
    fs: &'a mut T,
}

impl<'a, I, T> Future for MainLoop<'a, I, T>
where
    I: AsyncRead + AsyncWrite + Unpin + Clone + 'static,
    T: for<'s> Filesystem<&'s [u8]>,
{
    type Output = io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context) -> Poll<Self::Output> {
        let this = &mut *self;

        loop {
            if this.session.exited() {
                log::debug!("Got a DESTROY request and session was exited");
                return Poll::Ready(Ok(()));
            }

            log::trace!(
                "run background tasks (num = {})",
                this.background.num_remains()
            );
            let _ = this.background.poll_tasks(cx)?;

            let terminated = ready!(this.buf.poll_receive(cx, &mut this.channel))?;
            if terminated {
                log::debug!("connection was closed by the kernel");
                log::trace!(
                    "remaining background tasks (num = {})",
                    this.background.num_remains()
                );
                return Poll::Ready(Ok(()));
            }

            this.session.dispatch(
                &mut *this.buf,
                &*this.channel,
                &mut *this.fs,
                &mut this.background,
            )?;
        }
    }
}
