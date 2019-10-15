use crate::{
    buf::Buffer,
    op::Operations,
    session::{Background, Session},
};
use futures::{
    future::{Future, FutureExt},
    io::{AsyncRead, AsyncWrite},
    ready, select,
};
use std::{
    io,
    pin::Pin,
    task::{self, Poll},
};

/// Run a main loop of FUSE filesystem.
pub async fn main_loop<I, S, T>(channel: I, sig: S, ops: T) -> io::Result<Option<S::Output>>
where
    I: AsyncRead + AsyncWrite + Unpin + Clone,
    S: Future + Unpin,
    T: for<'a> Operations<&'a [u8]>,
{
    let mut channel = channel;
    let mut sig = sig.fuse();
    let mut ops = ops;

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
        ops: &mut ops,
    }
    .fuse();

    // FIXME: graceful shutdown the background tasks.
    select! {
        _ = main_loop => Ok(None),
        sig = sig => Ok(Some(sig)),
    }
}

#[allow(missing_debug_implementations)]
struct MainLoop<'a, 'b, I, T> {
    session: &'a mut Session,
    background: &'a mut Background<'b>,
    channel: &'b mut I,
    buf: &'a mut Buffer,
    ops: &'a mut T,
}

impl<'a, 'b, I, T> Future for MainLoop<'a, 'b, I, T>
where
    I: AsyncRead + AsyncWrite + Unpin + Clone,
    T: for<'s> Operations<&'s [u8]>,
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
                this.channel.clone(),
                &mut *this.ops,
                &mut this.background,
            )?;
        }
    }
}
