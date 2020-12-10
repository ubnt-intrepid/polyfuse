use polyfuse::{
    reply::{AttrOut, OpenOut, PollOut, ReplyWriter},
    Operation, Request, Session,
};
use polyfuse_example_async_std_support::{AsyncConnection, Writer};

use anyhow::{ensure, Context as _, Result};
use async_std::sync::Mutex;
use futures_intrusive::sync::ManualResetEvent;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tracing::Instrument as _;

const CONTENT: &str = "Hello, world!\n";

#[async_std::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let wakeup_interval = Duration::from_secs(
        args //
            .opt_value_from_str("--interval")?
            .unwrap_or(5),
    );

    let mountpoint: PathBuf = args.free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    let conn = AsyncConnection::open(mountpoint, Default::default()).await?;
    let session = Session::start(&conn, &conn, Default::default()).await?;

    let fs = PollFS::new(
        Notifier {
            session: session.clone(),
            writer: conn.writer(),
        },
        wakeup_interval,
    );

    while let Some(req) = session.next_request(&conn).await? {
        fs.handle_request(&req, &conn).await?;
    }

    Ok(())
}

struct PollFS {
    handle: Arc<Mutex<Option<OpenHandle>>>,
    is_ready: Arc<ManualResetEvent>,
    notifier: Arc<Notifier>,
    wakeup_interval: Duration,
}

impl PollFS {
    fn new(notifier: Notifier, wakeup_interval: Duration) -> Self {
        Self {
            handle: Arc::new(Mutex::new(None)),
            is_ready: Arc::new(ManualResetEvent::new(false)),
            notifier: Arc::new(notifier),
            wakeup_interval,
        }
    }

    fn start_reading(&self) {
        let handle = self.handle.clone();
        let is_ready = self.is_ready.clone();
        let notifier = self.notifier.clone();
        let wakeup_interval = self.wakeup_interval;

        let span = tracing::debug_span!("wakeup");
        async_std::task::spawn(
            (async move {
                async_std::task::sleep(wakeup_interval).await;

                tracing::info!("background task is completed");
                is_ready.set();

                let mut handle = handle.lock().await;
                if let Some(handle) = handle.as_mut() {
                    if let Some(kh) = handle.kh.take() {
                        tracing::info!("sending wakeup notification to kh={}", kh);
                        if let Err(err) = notifier.session.notify_poll_wakeup(&notifier.writer, kh)
                        {
                            tracing::error!("failed to send poll notification: {}", err);
                        }
                    }
                }
            })
            .instrument(span),
        );
    }

    async fn handle_request(&self, req: &Request, conn: &AsyncConnection) -> anyhow::Result<()> {
        let span = tracing::debug_span!("handle_request", unique = req.unique());
        (async {
            let op = req.operation()?;
            tracing::debug!(?op);

            let reply = ReplyWriter::new(conn, req.unique());

            match op {
                Operation::Getattr(..) => {
                    let mut out = AttrOut::default();
                    out.attr().ino(1);
                    out.attr().nlink(1);
                    out.attr().mode(libc::S_IFREG | 0o444);
                    out.attr().uid(unsafe { libc::getuid() });
                    out.attr().gid(unsafe { libc::getgid() });
                    out.ttl(Duration::from_secs(u64::max_value() / 2));

                    reply.reply(out)?;
                }

                Operation::Open(op) => {
                    if op.flags() as i32 & libc::O_ACCMODE != libc::O_RDONLY {
                        return reply.error(libc::EACCES).map_err(Into::into);
                    }

                    tracing::info!("start reading task");
                    self.is_ready.reset();
                    self.start_reading();

                    {
                        let mut handle = self.handle.lock().await;
                        if handle.is_some() {
                            return reply.error(libc::EBUSY).map_err(Into::into);
                        }
                        *handle = Some(OpenHandle {
                            is_nonblock: op.flags() as i32 & libc::O_NONBLOCK != 0,
                            kh: None,
                        });
                    }

                    let mut out = OpenOut::default();
                    out.direct_io(true);
                    out.nonseekable(true);

                    reply.reply(out)?;
                }

                Operation::Read(op) => {
                    if !self.is_ready.is_set() {
                        tracing::info!("the background task has not finished yet.");

                        {
                            let mut handle = self.handle.lock().await;
                            let handle = handle.as_mut().expect("open handle is empty");
                            if handle.is_nonblock {
                                tracing::info!("send EAGAIN immediately");
                                return reply.error(libc::EAGAIN).map_err(Into::into);
                            }
                        }

                        tracing::info!("wait for the completion of background task");
                        self.is_ready.wait().await;
                    }

                    let offset = op.offset() as usize;
                    let bufsize = op.size() as usize;
                    let content = CONTENT.as_bytes().get(offset..).unwrap_or(&[]);
                    reply.reply(&content[..std::cmp::min(content.len(), bufsize)])?;
                }

                Operation::Poll(op) => {
                    let mut out = PollOut::default();

                    if self.is_ready.is_set() {
                        tracing::info!("the background task is completed and ready to read");
                        out.revents(op.events() & libc::POLLIN as u32);
                        return reply.reply(out).map_err(Into::into);
                    }

                    if let Some(kh) = op.kh() {
                        tracing::info!("register the poll handle for notification: kh={}", kh);
                        let mut handle = self.handle.lock().await;
                        let handle = handle.as_mut().expect("open handle is empty");
                        handle.kh.replace(kh);
                    }

                    reply.reply(out)?;
                }

                Operation::Release(_op) => {
                    let mut handle = self.handle.lock().await;
                    handle.take();
                    reply.reply(&[])?;
                }

                _ => reply.error(libc::ENOSYS)?,
            }

            Ok(())
        })
        .instrument(span)
        .await
    }
}

struct OpenHandle {
    is_nonblock: bool,
    kh: Option<u64>,
}

struct Notifier {
    session: Arc<Session>,
    writer: Writer,
}
