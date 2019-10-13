use crate::{
    op::Operations,
    reply::ReplyRaw,
    request::Buffer,
    session::{Background, Session},
};
use futures::{
    future::{Future, FutureExt},
    io::{AsyncRead, AsyncWrite},
    ready,
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
    let mut sig = sig;
    let mut ops = ops;

    let mut buf = Buffer::default();

    let mut session = Session::initializer() //
        .start(&mut channel, &mut buf)
        .await?;

    let mut background = Background::new();

    MainLoop {
        session: &mut session,
        background: &mut background,
        channel: &mut channel,
        buf: &mut buf,
        sig: &mut sig,
        ops: &mut ops,
    }
    .await
}

#[allow(missing_debug_implementations)]
struct MainLoop<'a, 'b, I, S, T> {
    session: &'a mut Session,
    background: &'a mut Background<'b>,
    channel: &'b mut I,
    buf: &'a mut Buffer,
    sig: &'a mut S,
    ops: &'a mut T,
}

impl<'a, 'b, I, S, T> Future for MainLoop<'a, 'b, I, S, T>
where
    I: AsyncRead + AsyncWrite + Unpin + Clone,
    S: Future + Unpin,
    T: for<'s> Operations<&'s [u8]>,
{
    type Output = io::Result<Option<S::Output>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context) -> Poll<Self::Output> {
        let this = &mut *self;

        loop {
            if let Poll::Ready(sig) = this.sig.poll_unpin(cx) {
                // FIXME: graceful shutdown the background tasks.
                return Poll::Ready(Ok(Some(sig)));
            }

            log::trace!(
                "run background tasks (num = {})",
                this.background.num_remains()
            );
            let poll_background = this.background.poll_tasks(cx)?;

            if this.session.got_destroy() {
                continue;
            }

            match ready!(this.buf.poll_receive(cx, &mut this.channel))? {
                /* terminated = */
                true => {
                    log::debug!("connection was closed by the kernel");
                    log::trace!(
                        "remaining background tasks (num = {})",
                        this.background.num_remains()
                    );

                    if poll_background.is_ready() {
                        // FIXME: graceful shutdown the background tasks.
                    }
                    return Poll::Ready(Ok(None));
                }
                /* terminated = */
                false => {
                    let (request, data) = this.buf.extract()?;
                    log::debug!(
                        "Got a request: unique={}, opcode={:?}, arg={:?}, data={:?}",
                        request.header.unique,
                        request.header.opcode(),
                        request.arg,
                        data
                    );

                    let reply = ReplyRaw::new(request.header.unique, this.channel.clone());
                    this.session.dispatch(
                        request,
                        data,
                        reply,
                        &mut *this.ops,
                        &mut this.background,
                    );
                }
            }
        }
    }
}
