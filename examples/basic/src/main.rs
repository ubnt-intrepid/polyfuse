use polyfuse::{
    bytes::{write_bytes, Bytes},
    op,
    reply::{AttrOut, Reply},
    Config, MountOptions, Operation, Request, Session,
};
use polyfuse_example_async_std_support::AsyncConnection;

use anyhow::{ensure, Context as _, Result};
use std::{io, path::PathBuf, time::Duration};

const CONTENT: &[u8] = b"Hello from FUSE!\n";

#[async_std::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    // Establish connection to FUSE kernel driver mounted on the specified path.
    let conn = AsyncConnection::open(mountpoint, MountOptions::default()).await?;

    // Start FUSE session.
    let session = Session::start(&conn, &conn, Config::default()).await?;

    // Receive an incoming FUSE request from the kernel.
    while let Some(req) = session.next_request(&conn).await? {
        let reply = ReplyWriter {
            req: &req,
            conn: &conn,
        };

        match req.operation()? {
            // Dispatch your callbacks to the supported operations...
            Operation::Getattr(op) => getattr(op, reply)?,
            Operation::Read(op) => read(op, reply)?,

            // Or annotate that the operation is not supported.
            _ => reply.error(libc::ENOSYS)?,
        };
    }

    Ok(())
}

fn getattr(op: op::Getattr<'_>, reply: ReplyWriter<'_>) -> io::Result<()> {
    if op.ino() != 1 {
        return reply.error(libc::ENOENT);
    }

    let mut out = AttrOut::default();
    out.attr().ino(1);
    out.attr().mode(libc::S_IFREG as u32 | 0o444);
    out.attr().size(CONTENT.len() as u64);
    out.attr().nlink(1);
    out.attr().uid(unsafe { libc::getuid() });
    out.attr().gid(unsafe { libc::getgid() });
    out.ttl(Duration::from_secs(1));

    reply.ok(out)
}

fn read(op: op::Read<'_>, reply: ReplyWriter<'_>) -> io::Result<()> {
    if op.ino() != 1 {
        return reply.error(libc::ENOENT);
    }

    let mut data: &[u8] = &[];

    let offset = op.offset() as usize;
    if offset < CONTENT.len() {
        let size = op.size() as usize;
        data = &CONTENT[offset..];
        data = &data[..std::cmp::min(data.len(), size)];
    }

    reply.ok(data)
}

struct ReplyWriter<'req> {
    req: &'req Request,
    conn: &'req AsyncConnection,
}

impl ReplyWriter<'_> {
    fn ok<T>(self, arg: T) -> io::Result<()>
    where
        T: Bytes,
    {
        write_bytes(self.conn, Reply::new(self.req.unique(), 0, arg))
    }

    fn error(self, code: i32) -> io::Result<()> {
        write_bytes(self.conn, Reply::new(self.req.unique(), code, ()))
    }
}
