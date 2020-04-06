use anyhow::Context as _;
use pico_args::Arguments;
use std::{
    env, fs, io,
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};
use wait_timeout::ChildExt as _;

fn main() -> anyhow::Result<()> {
    let show_help = || {
        eprintln!(
            "\
cargo-xtask
Free-form automation tool

Usage:
    cargo xtask <SUBCOMMAND>

Subcommands:
    script  Run external script

Flags:
    -h, --help  Show this message
"
        );
    };

    let mut args = Arguments::from_env();

    if args.contains(["-h", "--help"]) {
        show_help();
        return Ok(());
    }

    let subcommand = args.subcommand()?.unwrap_or_default();
    match &*subcommand {
        "test" => {
            let show_help = || {
                eprintln!(
                    "\
cargo-xtask test
Run test suites

Usage:
    cargo xtask test

Flags:
    -h, --help  Show this message
"
                );
            };
            if args.contains(["-h", "--help"]) {
                show_help();
                return Ok(());
            }
            args.finish().map_err(|err| {
                show_help();
                err
            })?;
            do_test()
        }
        "docs" => {
            let show_help = || {
                eprintln!(
                    "\
cargo-xtask docs
Build API docs

Usage:
    cargo xtask docs

Flags:
    -h, --help  Show this message
"
                );
            };
            if args.contains(["-h", "--help"]) {
                show_help();
                return Ok(());
            }
            args.finish().map_err(|err| {
                show_help();
                err
            })?;
            do_docs()
        }
        "coverage" => {
            let show_help = || {
                eprintln!(
                    "\
cargo-xtask coverage
Run coverage test

Usage:
    cargo xtask coverage

Flags:
    -h, --help  Show this message
"
                );
            };
            if args.contains(["-h", "--help"]) {
                show_help();
                return Ok(());
            }
            args.finish().map_err(|err| {
                show_help();
                err
            })?;
            do_coverage()
        }
        _ => {
            show_help();
            anyhow::bail!("invalid CLI arguments");
        }
    }
}

fn do_test() -> anyhow::Result<()> {
    if cargo().arg("fmt").arg("--version").run_silent().is_ok() {
        cargo().arg("fmt").arg("--").arg("--check").run()?;
    }

    if cargo().args(&["clippy", "--version"]).run_silent().is_ok() {
        cargo()
            .arg("clippy")
            .arg("--workspace")
            .arg("--all-targets")
            .run()?;
    }

    cargo().arg("test").arg("--workspace").run()?;

    Ok(())
}

fn do_docs() -> anyhow::Result<()> {
    // ref: https://blog.rust-lang.org/2019/09/18/upcoming-docsrs-changes.html#what-will-change
    const CARGO_DOC_TIMEOUT: Duration = Duration::from_secs(60 * 15);

    let doc_dir = target_dir().join("doc");
    if doc_dir.exists() {
        fs::remove_dir_all(&doc_dir)?;
    }

    cargo()
        .arg("doc")
        .arg("--no-deps")
        .arg("--package=polyfuse")
        .run_timeout(CARGO_DOC_TIMEOUT)?;

    cargo()
        .arg("doc")
        .arg("--no-deps")
        .arg("--package=polyfuse-tokio")
        .run_timeout(CARGO_DOC_TIMEOUT)?;

    let lockfile = doc_dir.join(".lock");
    if lockfile.exists() {
        fs::remove_file(lockfile)?;
    }

    let indexfile = doc_dir.join("index.html");
    fs::write(
        indexfile,
        "<meta http-equiv=\"refresh\" content=\"0;url=polyfuse\">\n",
    )?;

    Ok(())
}

fn do_coverage() -> anyhow::Result<()> {
    if cargo()
        .args(&["tarpaulin", "--version"])
        .run_silent()
        .is_err()
    {
        eprintln!("[cargo-xtask] cargo-tarpaulin is not installed");
        return Ok(());
    }

    let cov_dir = target_dir().join("cov");
    if cov_dir.exists() {
        fs::remove_dir_all(&cov_dir)?;
    }
    fs::create_dir_all(&cov_dir)?;

    cargo()
        .arg("tarpaulin")
        .arg("-v")
        .arg("--workspace")
        .arg("--out")
        .arg("Xml")
        .arg("--output-dir")
        .arg(&cov_dir)
        .arg("--target-dir")
        .arg(&cov_dir)
        .run()?;

    Ok(())
}

fn cargo() -> Command {
    let cargo = env::var_os("CARGO")
        .or_else(|| option_env!("CARGO").map(Into::into))
        .unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command.current_dir(project_root());
    command.stdin(Stdio::null());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());
    command.env("CARGO_INCREMENTAL", "0");
    command.env("CARGO_NET_OFFLINE", "true");
    command.env("RUST_BACKTRACE", "full");
    command
}

trait CommandExt {
    fn run(&mut self) -> anyhow::Result<()>;
    fn run_timeout(&mut self, timeout: Duration) -> anyhow::Result<()>;
    fn run_silent(&mut self) -> anyhow::Result<()>;
}

impl CommandExt for Command {
    fn run(&mut self) -> anyhow::Result<()> {
        run_impl(self, None)
    }

    fn run_timeout(&mut self, timeout: Duration) -> anyhow::Result<()> {
        run_impl(self, Some(timeout))
    }

    fn run_silent(&mut self) -> anyhow::Result<()> {
        self.stdout(Stdio::null());
        self.stderr(Stdio::null());
        self.run()
    }
}

fn run_impl(cmd: &mut Command, timeout: Option<Duration>) -> anyhow::Result<()> {
    eprintln!("[cargo-xtask] run {:?}", cmd);

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

fn project_root() -> PathBuf {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .or_else(|| option_env!("CARGO_MANIFEST_DIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("./xtask"));
    manifest_dir.parent().unwrap().to_owned()
}

fn target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root().join("target"))
}
