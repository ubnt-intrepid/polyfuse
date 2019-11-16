use clap::{App, AppSettings, ArgMatches, SubCommand};
use std::path::{Path, PathBuf};

fn main() -> anyhow::Result<()> {
    let app = App::new("automation scripts")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .setting(AppSettings::VersionlessSubcommands)
        .subcommand(SubCommand::with_name("install-hooks"))
        .subcommand(SubCommand::with_name("run").args_from_usage(
            "<command> 'The name of script command'\n\
             [args]..  'Argument of script'",
        ));

    let matches = app.get_matches();
    match matches.subcommand() {
        ("install-hooks", _) => do_install_hooks(),
        ("run", Some(arg)) => do_run(arg),
        (_, _) => unreachable!(),
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

fn do_run(arg: &ArgMatches<'_>) -> anyhow::Result<()> {
    let command = arg.value_of_os("command").unwrap();
    let args = arg.values_of_os("args");

    let bin_dir = project_root()?.join("bin").canonicalize()?;
    anyhow::ensure!(bin_dir.is_dir(), "bin/ is not a directory");

    use std::process::{Command, Stdio};

    let mut script = Command::new(command);
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
    if let Some(args) = args {
        script.args(args);
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
