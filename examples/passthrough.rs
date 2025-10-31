use polyfuse::{
    mount::MountOptions,
    op::Operation,
    reply::{OpenOutFlags, ReplySender as _},
    types::FileID,
    KernelConfig, KernelFlags,
};

use anyhow::{ensure, Context as _, Result};
use rustix::{
    fs::{Mode, OFlags},
    io::Errno,
    thread::CapabilitySet,
};
use std::{os::unix::prelude::*, path::PathBuf};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let caps = rustix::thread::capabilities(None)?;
    ensure!(
        caps.effective.contains(CapabilitySet::SYS_ADMIN),
        "Passthrough feature requires CAP_SYS_ADMIN."
    );

    let mut args = pico_args::Arguments::from_env();

    let source: PathBuf = args.value_from_str("--source")?;
    ensure!(source.is_file(), "source path must be a regular file");

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_file(), "mountpoint must be a regular file");

    let mut config = KernelConfig::new();
    config.flags |= KernelFlags::PASSTHROUGH;
    config.max_stack_depth = 2;
    let (session, device, mount) = polyfuse::connect(mountpoint, MountOptions::new(), config)?;

    ensure!(
        session.config().minor >= 40,
        "FUSE_PASSTHROUGH is not supported by the kernel"
    );

    let source_file = rustix::fs::open(&source, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())?;
    let backing_id = unsafe { device.backing_open(source_file.as_fd())? };

    let mut buf = session.new_splice_buffer()?;
    while session.recv_request(&device, &mut buf)? {
        let (req, op, _remains) = session.decode(&device, &mut buf)?;
        match op {
            Operation::Open(_op) => req.reply_open(
                FileID::from_raw(0),
                OpenOutFlags::DIRECT_IO | OpenOutFlags::PASSTHROUGH,
                Some(backing_id),
            )?,
            Operation::Release(_op) => req.reply_bytes(())?,
            _ => req.reply_error(Errno::NOSYS)?,
        };
    }

    unsafe {
        device.backing_close(backing_id)?;
    }

    mount.unmount()?;

    Ok(())
}
