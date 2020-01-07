use futures_intrusive::sync::ManualResetEvent;
use pico_args::Arguments;
use polyfuse::{
    io::{Reader, Writer},
    reply::{ReplyAttr, ReplyOpen, ReplyPoll},
    Context, FileAttr, Filesystem, Operation,
};
use polyfuse_tokio::Server;
use std::{io, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;

const CONTENT: &str = "Hello, world!\n";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = Arguments::from_env();

    let wakeup_interval = args.opt_value_from_str("--interval")?.unwrap_or(5);
    let wakeup_interval = Duration::from_secs(wakeup_interval);

    let mountpoint: PathBuf = args
        .free_from_str()?
        .ok_or_else(|| anyhow::anyhow!("missing mountpoint"))?;
    anyhow::ensure!(
        mountpoint.is_file(),
        "the mountpoint must be a regular file"
    );

    let mut server = polyfuse_tokio::Builder::default()
        .mount(mountpoint, &[])
        .await?;

    let fs = PollFS::new(&server, wakeup_interval)?;
    server.run(fs).await?;

    Ok(())
}

struct PollFS {
    handle: Arc<Mutex<Option<OpenHandle>>>,
    is_ready: Arc<ManualResetEvent>,
    server: Arc<Mutex<Server>>,
    wakeup_interval: Duration,
}

impl PollFS {
    fn new(server: &Server, wakeup_interval: Duration) -> io::Result<Self> {
        let server = server.try_clone()?;
        Ok(Self {
            handle: Arc::new(Mutex::new(None)),
            is_ready: Arc::new(ManualResetEvent::new(false)),
            server: Arc::new(Mutex::new(server)),
            wakeup_interval,
        })
    }

    fn start_reading(&self) {
        let handle = self.handle.clone();
        let is_ready = self.is_ready.clone();
        let server = self.server.clone();
        let wakeup_interval = self.wakeup_interval;
        tokio::task::spawn(async move {
            tokio::time::delay_for(wakeup_interval).await;

            tracing::info!("background task is completed");
            is_ready.set();

            let mut handle = handle.lock().await;
            if let Some(handle) = handle.as_mut() {
                if let Some(kh) = handle.kh.take() {
                    tracing::info!("sending wakeup notification to kh={}", kh);
                    if let Err(err) = server.lock().await.notify_poll_wakeup(kh).await {
                        tracing::error!("failed to send poll notification: {}", err);
                    }
                }
            }
        });
    }
}

#[polyfuse::async_trait]
impl Filesystem for PollFS {
    #[allow(clippy::cognitive_complexity)]
    async fn call<'a, 'cx, T: ?Sized>(
        &'a self,
        cx: &'a mut Context<'cx, T>,
        op: Operation<'cx>,
    ) -> io::Result<()>
    where
        T: Reader + Writer + Unpin + Send,
    {
        match op {
            Operation::Getattr(op) => {
                let mut attr = FileAttr::default();
                attr.set_ino(1);
                attr.set_nlink(1);
                attr.set_size(0);
                attr.set_mode(libc::S_IFREG | 0o444);
                attr.set_uid(unsafe { libc::getuid() });
                attr.set_gid(unsafe { libc::getgid() });

                op.reply(cx, {
                    ReplyAttr::new(attr) //
                        .ttl_attr(Duration::from_secs(u64::max_value() / 2))
                })
                .await
            }

            Operation::Open(op) => {
                if op.flags() as i32 & libc::O_ACCMODE != libc::O_RDONLY {
                    return cx.reply_err(libc::EACCES).await;
                }

                tracing::info!("start reading task");
                self.is_ready.reset();
                self.start_reading();

                {
                    let mut handle = self.handle.lock().await;
                    if handle.is_some() {
                        return cx.reply_err(libc::EBUSY).await;
                    }
                    *handle = Some(OpenHandle {
                        is_nonblock: op.flags() as i32 & libc::O_NONBLOCK != 0,
                        kh: None,
                    });
                }

                op.reply(cx, {
                    ReplyOpen::new(0) //
                        .direct_io(true)
                        .nonseekable(true)
                })
                .await
            }

            Operation::Read(op) => {
                if !self.is_ready.is_set() {
                    tracing::info!("the background task has not finished yet.");

                    {
                        let mut handle = self.handle.lock().await;
                        let handle = handle.as_mut().expect("open handle is empty");
                        if handle.is_nonblock {
                            tracing::info!("send EAGAIN immediately");
                            return cx.reply_err(libc::EAGAIN).await;
                        }
                    }

                    tracing::info!("wait for the completion of background task");
                    self.is_ready.wait().await;
                }

                let offset = op.offset() as usize;
                let bufsize = op.size() as usize;
                let content = CONTENT.as_bytes().get(offset..).unwrap_or(&[]);
                op.reply(cx, &content[..std::cmp::min(content.len(), bufsize)])
                    .await
            }

            Operation::Poll(op) => {
                if self.is_ready.is_set() {
                    tracing::info!("the background task is completed and ready to read");
                    let revents = op.events() & libc::POLLIN as u32;
                    return op.reply(cx, ReplyPoll::new(revents)).await;
                }

                if let Some(kh) = op.kh() {
                    tracing::info!("register the poll handle for notification: kh={}", kh);
                    let mut handle = self.handle.lock().await;
                    let handle = handle.as_mut().expect("open handle is empty");
                    handle.kh.replace(kh);
                }

                op.reply(cx, ReplyPoll::new(0)).await
            }

            Operation::Release(op) => {
                let mut handle = self.handle.lock().await;
                handle.take();
                op.reply(cx).await
            }
            _ => Ok(()),
        }
    }
}

struct OpenHandle {
    is_nonblock: bool,
    kh: Option<u64>,
}
