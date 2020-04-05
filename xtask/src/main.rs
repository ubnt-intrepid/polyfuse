use pico_args::Arguments;
use std::{ffi::OsString, path::PathBuf};

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
        "script" => {
            let show_help = || {
                eprintln!(
                    "\
cargo-xtask script
Run external script

Usage:
    cargo xtask script <COMMAND> <ARGS>...

Flags:
    -h, --help  Show this message
    <COMMAND>   Script name
    <ARGS>...   Arguments passed to the script
"
                );
            };

            if args.contains(["-h", "--help"]) {
                show_help();
                return Ok(());
            }

            let command = args
                .free_from_os_str(|arg| Ok::<_, std::convert::Infallible>(arg.to_owned()))?
                .ok_or_else(|| {
                    show_help();
                    anyhow::anyhow!("missing script name")
                })?;

            let args = args.free_os()?;

            do_script(command, args)
        }
        _ => {
            show_help();
            anyhow::bail!("invalid CLI arguments");
        }
    }
}

fn do_script(command: OsString, args: Vec<OsString>) -> anyhow::Result<()> {
    let bin_dir = project_root()?.join("bin").canonicalize()?;
    anyhow::ensure!(bin_dir.is_dir(), "bin/ is not a directory");

    use std::process::{Command, Stdio};

    let mut script = Command::new(command);
    script.args(args);
    script.stdin(Stdio::null());
    script.stdout(Stdio::inherit());
    script.stderr(Stdio::inherit());
    if let Some(orig_path) = std::env::var_os("PATH") {
        let paths: Vec<_> = Some(bin_dir)
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
