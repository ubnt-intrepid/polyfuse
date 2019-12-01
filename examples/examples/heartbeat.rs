//! test script:
//!
//! ```shell-session
//! $ for i in {1..1000}; do clear; cat /path/to/heartbeat; usleep 500000; done
//! ```

#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

use polyfuse_examples::prelude::*;

use chrono::Local;
use futures::lock::Mutex;
use polyfuse::{
    reply::{ReplyAttr, ReplyOpen},
    FileAttr,
};
use polyfuse_tokio::{Notifier, Server};
use std::{io, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    examples::init_tracing()?;

    let mountpoint = examples::get_mountpoint()?;
    ensure!(
        mountpoint.is_file(),
        "the mountpoint must be a regular file"
    );

    let mut args = pico_args::Arguments::from_vec(std::env::args_os().skip(2).collect());
    let with_notify = !args.contains("--no-notify");

    let heartbeat = Arc::new(Heartbeat::now());

    // It is necessary to use the primitive server APIs in order to obtain
    // the instance of `Notifier` associated with the server.
    let mut server = Server::mount(mountpoint, Default::default()).await?;

    // Spawn a task that beats the heart.
    tokio::task::spawn({
        let mut notifier = if with_notify {
            server.notifier().map(Some)?
        } else {
            None
        };
        let heartbeat = heartbeat.clone();
        async move {
            loop {
                if let Err(err) = heartbeat.heartbeat(notifier.as_mut()).await {
                    tracing::error!("heartbeat error: {}", err);
                    return;
                }
                tokio::time::delay_for(Duration::from_secs(2)).await;
            }
        }
    });

    // Run the filesystem daemon on the foreground.
    server.run(heartbeat).await?;
    Ok(())
}

struct Heartbeat {
    inner: Mutex<Inner>,
}

struct Inner {
    content: Arc<String>,
    attr: FileAttr,
}

impl Heartbeat {
    fn now() -> Self {
        let content = Arc::new(Local::now().to_rfc3339());

        let mut attr = FileAttr::default();
        attr.set_ino(1);
        attr.set_mode(libc::S_IFREG | 0o444);
        attr.set_size(content.len() as u64);

        Self {
            inner: Mutex::new(Inner { content, attr }),
        }
    }

    async fn heartbeat(&self, notifier: Option<&mut Notifier>) -> io::Result<()> {
        let mut inner = self.inner.lock().await;

        let content = Arc::new(Local::now().to_rfc3339());
        inner.attr.set_size(content.len() as u64);
        inner.content = content.clone();

        tracing::info!("heartbeat: {:?}", content);

        if let Some(notifier) = notifier {
            tracing::info!("send notify_store: {:?}", content);
            notifier.store(1, 0, &[content.as_bytes()]).await?;

            //
            let retrieve = notifier.retrieve(1, 0, 1024).await?;
            let (_offset, data) = retrieve.await;
            tracing::info!(" --> retrieved: {:?}", data);
            if data[..content.len()] != *content.as_bytes() {
                tracing::error!("mismatched data");
            }
        }

        Ok(())
    }
}

#[async_trait]
impl<T> Filesystem<T> for Heartbeat {
    async fn call<W: ?Sized>(&self, cx: &mut Context<'_, W>, op: Operation<'_, T>) -> io::Result<()>
    where
        T: Send + 'async_trait,
        W: AsyncWrite + Unpin + Send,
    {
        match op {
            Operation::Getattr(op) => match op.ino() {
                1 => {
                    let inner = self.inner.lock().await;
                    op.reply(cx, ReplyAttr::new(inner.attr)).await?;
                }
                _ => cx.reply_err(libc::ENOENT).await?,
            },
            Operation::Open(op) => match op.ino() {
                1 => {
                    let mut reply = ReplyOpen::new(0);
                    reply.keep_cache(true);
                    op.reply(cx, reply).await?;
                }
                _ => cx.reply_err(libc::ENOENT).await?,
            },
            Operation::Read(op) => match op.ino() {
                1 => {
                    let inner = self.inner.lock().await;

                    let offset = op.offset() as usize;
                    if offset >= inner.content.len() {
                        op.reply(cx, &[]).await?;
                    } else {
                        let size = op.size() as usize;
                        let data = &inner.content.as_bytes()[offset..];
                        let data = &data[..std::cmp::min(data.len(), size)];
                        op.reply(cx, data).await?;
                    }
                }
                _ => cx.reply_err(libc::ENOENT).await?,
            },
            _ => (),
        }

        Ok(())
    }
}
