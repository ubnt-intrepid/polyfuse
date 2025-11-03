#![forbid(unsafe_code)]

use polyfuse::{
    mount::MountOptions,
    op::{self, Operation},
    reply::ReplySender as _,
    session::Request,
    types::{FileAttr, FileMode, FilePermissions, FileType, NodeID},
    Device, KernelConfig,
};

use anyhow::{ensure, Context as _, Result};
use libc::{SIGHUP, SIGINT, SIGTERM};
use rustix::{
    io::Errno,
    process::{getgid, getuid},
};
use signal_hook::iterator::Signals;
use std::{io, path::PathBuf, thread, time::Duration};

const CONTENT: &[u8] = b"Hello from FUSE!\n";

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    // Establish connection to FUSE kernel driver mounted on the specified path.
    let (session, device, mount) =
        polyfuse::connect(mountpoint, MountOptions::new(), KernelConfig::new())?;

    thread::scope(|scope| -> Result<()> {
        // Spawn the background thread for waiting a termination signal.
        let mut signals = Signals::new([SIGHUP, SIGTERM, SIGINT])?;
        let signals_handle = signals.handle();
        scope.spawn(move || {
            if let Some(sig) = signals.wait().next() {
                tracing::debug!(
                    "caught the termination signal: {}",
                    match sig {
                        SIGHUP => "SIGHUP",
                        SIGTERM => "SIGTERM",
                        SIGINT => "SIGINT",
                        _ => "<unknown>",
                    }
                );
                let _ = mount.unmount();
            }
        });

        // Receive an incoming FUSE request from the kernel.
        let mut buf = session.new_fallback_buffer();
        while session.recv_request(&device, &mut buf)? {
            let (req, op, _remains) = session.decode(&device, &mut buf)?;
            match op {
                // Dispatch your callbacks to the supported operations...
                Operation::Getattr(op) => getattr(req, op)?,
                Operation::Read(op) => read(req, op)?,

                // Or annotate that the operation is not supported.
                _ => req.reply_error(Errno::NOSYS)?,
            };
        }

        signals_handle.close();

        Ok(())
    })?;

    Ok(())
}

fn getattr(req: Request<'_, &Device>, op: op::Getattr<'_>) -> io::Result<()> {
    if op.ino != NodeID::ROOT {
        return req.reply_error(Errno::NOENT);
    }

    req.reply_attr(
        FileAttr {
            ino: NodeID::ROOT,
            size: CONTENT.len() as u64,
            mode: FileMode::new(FileType::Regular, FilePermissions::READ),
            nlink: 1,
            uid: getuid(),
            gid: getgid(),
            ..FileAttr::new()
        },
        Some(Duration::from_secs(1)),
    )
}

fn read(req: Request<'_, &Device>, op: op::Read<'_>) -> io::Result<()> {
    if op.ino != NodeID::ROOT {
        return req.reply_error(Errno::NOENT);
    }

    let mut data: &[u8] = &[];

    let offset = op.offset as usize;
    if offset < CONTENT.len() {
        let size = op.size as usize;
        data = &CONTENT[offset..];
        data = &data[..std::cmp::min(data.len(), size)];
    }

    req.reply_bytes(data)
}
