use polyfuse::{op, reply::AttrOut, KernelConfig, MountOptions, Operation, Request, Session};

use anyhow::{ensure, Context as _, Result};
use std::{io, path::PathBuf, time::Duration};

const CONTENT: &[u8] = b"Hello from FUSE!\n";

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    // Establish connection to FUSE kernel driver mounted on the specified path.
    let session = Session::mount(MountOptions::new(mountpoint), KernelConfig::default())?;

    // Receive an incoming FUSE request from the kernel.
    while let Some(req) = session.next_request()? {
        match req.operation()? {
            // Dispatch your callbacks to the supported operations...
            Operation::Getattr(op) => getattr(&req, op)?,
            Operation::Read(op) => read(&req, op)?,

            // Or annotate that the operation is not supported.
            _ => req.reply_error(libc::ENOSYS)?,
        };
    }

    Ok(())
}

fn getattr(req: &Request, op: op::Getattr<'_>) -> io::Result<()> {
    if op.ino() != 1 {
        return req.reply_error(libc::ENOENT);
    }

    let mut out = AttrOut::default();
    out.attr().ino(1);
    out.attr().mode(libc::S_IFREG as u32 | 0o444);
    out.attr().size(CONTENT.len() as u64);
    out.attr().nlink(1);
    out.attr().uid(unsafe { libc::getuid() });
    out.attr().gid(unsafe { libc::getgid() });
    out.ttl(Duration::from_secs(1));

    req.reply(out)
}

fn read(req: &Request, op: op::Read<'_>) -> io::Result<()> {
    if op.ino() != 1 {
        return req.reply_error(libc::ENOENT);
    }

    let mut data: &[u8] = &[];

    let offset = op.offset() as usize;
    if offset < CONTENT.len() {
        let size = op.size() as usize;
        data = &CONTENT[offset..];
        data = &data[..std::cmp::min(data.len(), size)];
    }

    req.reply(data)
}
