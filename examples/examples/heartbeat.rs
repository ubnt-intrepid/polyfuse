//! test script:
//!
//! ```shell-session
//! $ for i in {1..1000}; do clear; cat /path/to/heartbeat; usleep 500000; done
//! ```

#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented)]

use polyfuse_examples::prelude::*;

use chrono::Local;
use futures::{channel::oneshot, io::AsyncReadExt, lock::Mutex};
use polyfuse::{
    reply::{ReplyAttr, ReplyOpen},
    FileAttr,
};
use polyfuse_tokio::Server;
use std::{collections::HashMap, io, sync::Arc, time::Duration};

const ROOT_INO: u64 = 1;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

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

    // Spawn a task that beats the heart.
    {
        let heartbeat = heartbeat.clone();
        let mut server = if !no_notify {
            Some(server.try_clone()?)
        } else {
            None
        };

        let _: tokio::task::JoinHandle<io::Result<()>> = tokio::task::spawn(async move {
            loop {
                tracing::info!("heartbeat");

                heartbeat.update_content().await;

                if let Some(ref mut server) = server {
                    match notify_kind {
                        NotifyKind::Store => heartbeat.notify_store(server).await?,
                        NotifyKind::Invalidate => heartbeat.notify_inval_inode(server).await?,
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
    retrieves: Mutex<HashMap<u64, oneshot::Sender<Vec<u8>>>>,
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
            retrieves: Mutex::default(),
        }
    }

    async fn update_content(&self) {
        let mut inner = self.inner.lock().await;
        let content = Local::now().to_rfc3339();
        inner.attr.set_size(content.len() as u64);
        inner.content = content;
    }

    async fn notify_store(&self, server: &mut Server) -> io::Result<()> {
        let inner = self.inner.lock().await;
        let content = &inner.content;

        tracing::info!("send notify_store(data={:?})", content);
        server
            .notify_store(ROOT_INO, 0, &[content.as_bytes()])
            .await?;

        // To check if the cache is updated correctly, pull the
        // content from the kernel using notify_retrieve.
        tracing::info!("send notify_retrieve");
        let data = {
            let unique = server.notify_retrieve(ROOT_INO, 0, 1024).await?;
            let (tx, rx) = oneshot::channel();
            self.retrieves.lock().await.insert(unique, tx);
            rx.await.unwrap()
        };
        tracing::info!("--> content={:?}", data);

        if data[..content.len()] != *content.as_bytes() {
            tracing::error!("mismatched data");
        }

        Ok(())
    }

    async fn notify_inval_inode(&self, server: &mut Server) -> io::Result<()> {
        tracing::info!("send notify_invalidate_inode");
        server.notify_inval_inode(ROOT_INO, 0, 0).await?;
        Ok(())
    }
}

#[async_trait]
impl Filesystem for Heartbeat {
    async fn call<'a, 'cx, T: ?Sized>(
        &'a self,
        cx: &'a mut Context<'cx, T>,
        op: Operation<'cx>,
    ) -> io::Result<()>
    where
        T: Reader + Writer + Unpin + Send,
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
                    op.reply(cx, {
                        ReplyOpen::new(0) //
                            .keep_cache(true)
                    })
                    .await?;
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
            Operation::NotifyReply(op) => {
                if let Some(tx) = self.retrieves.lock().await.remove(&op.unique()) {
                    let mut data = vec![0u8; op.size() as usize];
                    cx.reader().read_exact(&mut data).await?;
                    let _ = tx.send(data);
                }
            }
            _ => (),
        }

        Ok(())
    }
}
