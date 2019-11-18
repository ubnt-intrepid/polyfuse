use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
enum Arg {
    /// Install Git hooks
    InstallHooks,

    /// Run a script.
    Script {
        /// The script name
        #[structopt(name = "command", parse(from_os_str))]
        command: OsString,

        /// The arguments passed to script
        #[structopt(name = "args", parse(from_os_str))]
        args: Vec<OsString>,
    },
}

fn main() -> anyhow::Result<()> {
    match Arg::from_args() {
        Arg::InstallHooks => do_install_hooks(),
        Arg::Script { command, args } => do_run(command, args),
    }
}

fn do_install_hooks() -> anyhow::Result<()> {
    let src_dir = project_root()?.join(".cargo/hooks").canonicalize()?;
    anyhow::ensure!(src_dir.is_dir(), "Cargo hooks directory");

    let dst_dir = project_root()?.join(".git/hooks").canonicalize()?;
    anyhow::ensure!(dst_dir.is_dir(), "Git hooks directory");

    install_hook(src_dir.join("pre-commit"), dst_dir.join("pre-commit"))?;
    install_hook(src_dir.join("pre-push"), dst_dir.join("pre-push"))?;

    Ok(())
}

fn install_hook(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> anyhow::Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();

    if src.is_file() {
        println!("install {:?} to {:?}", src, dst);

        std::fs::create_dir_all(
            dst.parent()
                .ok_or_else(|| anyhow::anyhow!("missing dst parent"))?,
        )?;
        std::fs::remove_file(dst)?;
        std::os::unix::fs::symlink(src, dst)?;
    }

    Ok(())
}

fn do_run(command: OsString, args: Vec<OsString>) -> anyhow::Result<()> {
    let bin_dir = project_root()?.join("bin").canonicalize()?;
    anyhow::ensure!(bin_dir.is_dir(), "bin/ is not a directory");

    use std::process::{Command, Stdio};

    let mut script = Command::new(command);
    script.args(args);
    script.stdin(Stdio::null());
    script.stdout(Stdio::inherit());
    script.stderr(Stdio::inherit());
    if let Some(orig_path) = std::env::var_os("PATH") {
        let paths: Vec<_> = Some(bin_dir.clone())
            .into_iter()
            .chain(std::env::split_paths(&orig_path))
            .collect();
        let new_path = std::env::join_paths(paths)?;
        script.env("PATH", new_path);
    }

    let status = script.status()?;
    anyhow::ensure!(status.success(), format!("Script failed: {}", status));

    Ok(())
}

fn project_root() -> anyhow::Result<PathBuf> {
    Ok(std::path::Path::new(&env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("missing ancestor"))?
        .to_owned())
}
