//! test script:
//!
//! ```shell-session
//! $ for i in {1..1000}; do clear; ls -al /path/to/heartbeat_entry; usleep 500000; done
//! ```

#![allow(clippy::unnecessary_mut_passed)]
#![deny(clippy::unimplemented, clippy::todo)]

use polyfuse::{
    fs::{self, Filesystem},
    mount::MountOptions,
    op,
    reply::{AttrOut, EntryOut, FileAttr, ReaddirOut},
    types::{NodeID, GID, UID},
    KernelConfig,
};

use anyhow::{ensure, Context as _, Result};
use chrono::Local;
use libc::{EISDIR, ENOENT, ENOTDIR, S_IFDIR, S_IFREG};
use std::{io, mem, os::unix::prelude::*, path::PathBuf, sync::Mutex, time::Duration};

const FILE_INO: NodeID = NodeID::from_raw(2);

const DEFAULT_TTL: Duration = Duration::from_secs(0);
const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mut args = pico_args::Arguments::from_env();

    let no_notify = args.contains("--no-notify");

    let ttl = args
        .opt_value_from_str("--ttl")?
        .map_or(DEFAULT_TTL, Duration::from_secs);
    tracing::info!(?ttl);

    let update_interval = args
        .opt_value_from_str("--update-interval")?
        .map_or(DEFAULT_INTERVAL, Duration::from_secs);
    tracing::info!(?update_interval);

    let mountpoint: PathBuf = args.opt_free_from_str()?.context("missing mountpoint")?;
    ensure!(mountpoint.is_dir(), "mountpoint must be a directory");

    polyfuse::fs::run(
        Heartbeat::new(ttl, update_interval, no_notify),
        mountpoint,
        MountOptions::default(),
        KernelConfig::default(),
    )?;

    Ok(())
}

fn generate_filename() -> String {
    Local::now().format("Time_is_%Hh_%Mm_%Ss").to_string()
}

struct Heartbeat {
    root_attr: libc::stat,
    file_attr: libc::stat,
    ttl: Duration,
    update_interval: Duration,
    current: Mutex<CurrentFile>,
    no_notify: bool,
}

#[derive(Debug)]
struct CurrentFile {
    filename: String,
    nlookup: u64,
}

impl Heartbeat {
    fn new(ttl: Duration, update_interval: Duration, no_notify: bool) -> Self {
        let mut root_attr = unsafe { mem::zeroed::<libc::stat>() };
        root_attr.st_ino = NodeID::ROOT.into_raw();
        root_attr.st_mode = S_IFDIR | 0o555;
        root_attr.st_nlink = 1;

        let mut file_attr = unsafe { mem::zeroed::<libc::stat>() };
        file_attr.st_ino = FILE_INO.into_raw();
        file_attr.st_mode = S_IFREG | 0o444;
        file_attr.st_nlink = 1;

        Self {
            root_attr,
            file_attr,
            ttl,
            update_interval,
            current: Mutex::new(CurrentFile {
                filename: generate_filename(),
                nlookup: 0,
            }),
            no_notify,
        }
    }

    fn heartbeat(&self, notifier: &fs::Notifier<'_>) -> Result<()> {
        let span = tracing::debug_span!("heartbeat", notify = !self.no_notify);
        let _enter = span.enter();

        loop {
            tracing::info!("heartbeat");

            let new_filename = generate_filename();
            let mut current = self.current.lock().unwrap();
            tracing::debug!(filename = ?current.filename, nlookup = ?current.nlookup);
            tracing::debug!(?new_filename);
            let old_filename = mem::replace(&mut current.filename, new_filename);

            if !self.no_notify && current.nlookup > 0 {
                tracing::info!("send notify_inval_entry");
                notifier.inval_entry(NodeID::ROOT, old_filename.as_ref())?;
            }

            drop(current);

            std::thread::sleep(self.update_interval);
        }
    }
}

impl Filesystem for Heartbeat {
    fn init<'env>(&'env self, cx: fs::Context<'_, 'env>) -> io::Result<()> {
        cx.spawner.spawn(move || -> Result<()> {
            self.heartbeat(&cx.notifier)?;
            Ok(())
        });
        Ok(())
    }

    fn lookup(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Lookup<'_>>) -> fs::Result {
        if req.arg().parent() != NodeID::ROOT {
            Err(ENOTDIR)?;
        }

        let mut current = self.current.lock().unwrap();

        if req.arg().name().as_bytes() == current.filename.as_bytes() {
            let mut out = EntryOut::default();
            out.ino(NodeID::from_raw(self.file_attr.st_ino));
            fill_attr(out.attr(), &self.file_attr);
            out.ttl_entry(self.ttl);
            out.ttl_attr(self.ttl);

            let res = req.reply(out)?;

            current.nlookup += 1;

            Ok(res)
        } else {
            Err(ENOENT)?
        }
    }

    fn forget(&self, _: fs::Context<'_, '_>, forgets: &[op::Forget]) {
        let mut current = self.current.lock().unwrap();
        for forget in forgets {
            if forget.ino() == FILE_INO {
                current.nlookup -= forget.nlookup();
            }
        }
    }

    fn getattr(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Getattr<'_>>) -> fs::Result {
        let attr = match req.arg().ino() {
            NodeID::ROOT => &self.root_attr,
            FILE_INO => &self.file_attr,
            _ => Err(ENOENT)?,
        };

        let mut out = AttrOut::default();
        fill_attr(out.attr(), attr);
        out.ttl(self.ttl);

        req.reply(out)
    }

    fn read(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Read<'_>>) -> fs::Result {
        match req.arg().ino() {
            NodeID::ROOT => Err(EISDIR)?,
            FILE_INO => req.reply(()),
            _ => Err(ENOENT)?,
        }
    }

    fn readdir(&self, _: fs::Context<'_, '_>, req: fs::Request<'_, op::Readdir<'_>>) -> fs::Result {
        if req.arg().ino() != NodeID::ROOT {
            Err(ENOTDIR)?;
        }
        if req.arg().offset() > 0 {
            return req.reply(());
        }

        let current = self.current.lock().unwrap();

        let mut out = ReaddirOut::new(req.arg().size() as usize);
        out.entry(current.filename.as_ref(), FILE_INO, 0, 1);
        req.reply(out)
    }
}

fn fill_attr(attr: &mut FileAttr, st: &libc::stat) {
    attr.ino(NodeID::from_raw(st.st_ino));
    attr.size(st.st_size as u64);
    attr.mode(st.st_mode);
    attr.nlink(st.st_nlink as u32);
    attr.uid(UID::from_raw(st.st_uid));
    attr.gid(GID::from_raw(st.st_gid));
    attr.rdev(st.st_rdev as u32);
    attr.blksize(st.st_blksize as u32);
    attr.blocks(st.st_blocks as u64);
    attr.atime(Duration::new(st.st_atime as u64, st.st_atime_nsec as u32));
    attr.mtime(Duration::new(st.st_mtime as u64, st.st_mtime_nsec as u32));
    attr.ctime(Duration::new(st.st_ctime as u64, st.st_ctime_nsec as u32));
}
