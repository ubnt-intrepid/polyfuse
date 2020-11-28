use crate::env::Env;
use anyhow::{Context as _, Result};
use std::{
    env,
    ffi::OsStr,
    io,
    process::{Command, Stdio},
    time::Duration,
};
use wait_timeout::ChildExt as _;

pub fn command(env: &Env, program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.current_dir(&env.project_root);
    command.stdin(Stdio::null());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());
    command
}

pub fn cargo(env: &Env) -> Command {
    let cargo = env::var_os("CARGO")
        .or_else(|| option_env!("CARGO").map(Into::into))
        .unwrap_or_else(|| "cargo".into());
    let mut command = command(env, cargo);
    command.env("CARGO_INCREMENTAL", "0");
    command.env("CARGO_NET_OFFLINE", "true");
    command.env("RUST_BACKTRACE", "full");
    command
}

pub trait CommandExt {
    fn with<F>(&mut self, f: F) -> &mut Self
    where
        F: FnOnce(&mut Self) -> &mut Self;

    fn silent(&mut self) -> &mut Self;

    fn run(&mut self) -> Result<()>;
    fn run_timeout(&mut self, timeout: Duration) -> Result<()>;
}

impl CommandExt for Command {
    fn silent(&mut self) -> &mut Self {
        self.stdout(Stdio::null());
        self.stderr(Stdio::null());
        self
    }

    fn with<F>(&mut self, f: F) -> &mut Self
    where
        F: FnOnce(&mut Self) -> &mut Self,
    {
        f(self)
    }

    fn run(&mut self) -> Result<()> {
        run_impl(self, None)
    }

    fn run_timeout(&mut self, timeout: Duration) -> Result<()> {
        run_impl(self, Some(timeout))
    }
}

fn run_impl(cmd: &mut Command, timeout: Option<Duration>) -> Result<()> {
    let mut child = cmd.spawn().context("failed to spawn the subprocess")?;

    let st = match timeout {
        Some(timeout) => match child.wait_timeout(timeout)? {
            Some(st) => st,
            None => {
                if let Err(err) = child.kill() {
                    match err.kind() {
                        io::ErrorKind::InvalidInput => (),
                        _ => anyhow::bail!(err),
                    }
                }
                child.wait()?
            }
        },
        None => child.wait()?,
    };

    anyhow::ensure!(st.success(), "Subprocess failed: {}", st);

    Ok(())
}
