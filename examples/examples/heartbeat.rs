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

const ROOT_INO: u64 = 1;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    examples::init_tracing()?;

    let mountpoint = examples::get_mountpoint()?;
    ensure!(
        mountpoint.is_file(),
        "the mountpoint must be a regular file"
    );

    let mut args = pico_args::Arguments::from_vec(std::env::args_os().skip(2).collect());

    let no_notify = args.contains("--no-notify");
    let notify_kind = args
        .opt_value_from_os_str("--notify-kind", |s| match s.to_str() {
            Some("store") => Ok(NotifyKind::Store),
            Some("invalidate") => Ok(NotifyKind::Invalidate),
            s => Err(anyhow::anyhow!("invalid notify kind: {:?}", s)),
        })?
        .unwrap_or(NotifyKind::Store);
    let update_interval: u64 = args.value_from_str("--update-interval")?;

    let heartbeat = Arc::new(Heartbeat::now());

    // It is necessary to use the primitive server APIs in order to obtain
    // the instance of `Notifier` associated with the server.
    let mut server = Server::mount(mountpoint, &[]).await?;
    let mut notifier = if !no_notify {
        Some(server.notifier()?)
    } else {
        None
    };

    // Spawn a task that beats the heart.
    {
        let heartbeat = heartbeat.clone();

        let _: tokio::task::JoinHandle<io::Result<()>> = tokio::task::spawn(async move {
            loop {
                tracing::info!("heartbeat");

                heartbeat.update_content().await;

                if let Some(ref mut notifier) = notifier {
                    match notify_kind {
                        NotifyKind::Store => heartbeat.notify_store(notifier).await?,
                        NotifyKind::Invalidate => heartbeat.notify_inval_inode(notifier).await?,
                    }
                }

                tokio::time::delay_for(Duration::from_secs(update_interval)).await;
            }
        });
    }

    // Run the filesystem daemon on the foreground.
    server.run(heartbeat).await?;
    Ok(())
}

#[derive(Debug, Copy, Clone, PartialEq)]
enum NotifyKind {
    Store,
    Invalidate,
}

struct Heartbeat {
    inner: Mutex<Inner>,
}

struct Inner {
    content: String,
    attr: FileAttr,
}

impl Heartbeat {
    fn now() -> Self {
        let content = Local::now().to_rfc3339();

        let mut attr = FileAttr::default();
        attr.set_ino(ROOT_INO);
        attr.set_mode(libc::S_IFREG | 0o444);
        attr.set_size(content.len() as u64);

        Self {
            inner: Mutex::new(Inner { content, attr }),
        }
    }

    async fn update_content(&self) {
        let mut inner = self.inner.lock().await;
        let content = Local::now().to_rfc3339();
        inner.attr.set_size(content.len() as u64);
        inner.content = content;
    }

    async fn notify_store(&self, notifier: &mut Notifier) -> io::Result<()> {
        let inner = self.inner.lock().await;
        let content = &inner.content;

        tracing::info!("send notify_store(data={:?})", content);
        notifier.store(ROOT_INO, 0, &[content.as_bytes()]).await?;

        // To check if the cache is updated correctly, pull the
        // content from the kernel using notify_retrieve.
        tracing::info!("send notify_retrieve");
        let data = notifier.retrieve(ROOT_INO, 0, 1024).await?;
        tracing::info!("--> content={:?}", data);

        if data[..content.len()] != *content.as_bytes() {
            tracing::error!("mismatched data");
        }

        Ok(())
    }

    async fn notify_inval_inode(&self, notifier: &mut Notifier) -> io::Result<()> {
        tracing::info!("send notify_invalidate_inode");
        notifier.inval_inode(ROOT_INO, 0, 0).await?;
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
                ROOT_INO => {
                    let inner = self.inner.lock().await;
                    op.reply(cx, ReplyAttr::new(inner.attr)).await?;
                }
                _ => cx.reply_err(libc::ENOENT).await?,
            },
            Operation::Open(op) => match op.ino() {
                ROOT_INO => {
                    let mut reply = ReplyOpen::new(0);
                    reply.keep_cache(true);
                    op.reply(cx, reply).await?;
                }
                _ => cx.reply_err(libc::ENOENT).await?,
            },
            Operation::Read(op) => match op.ino() {
                ROOT_INO => {
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
