use crate::{
    buf::Buffer,
    fs::Filesystem,
    session::{Background, Session},
};
use futures_io::{AsyncRead, AsyncWrite};
use futures_util::{
    future::{Future, FutureExt},
    select,
};
use std::io;

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

    let mut main_loop = Box::pin({
        let session = &mut session;
        let buf = &mut buf;
        let channel = &mut channel;
        let fs = &mut fs;
        let background = &mut background;
        async move {
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
    })
    .fuse();

    // FIXME: graceful shutdown the background tasks.
    select! {
        _ = main_loop => Ok(None),
        sig = sig => Ok(Some(sig)),
    }
}
