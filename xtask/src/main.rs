mod doc;
mod env;
mod hook;
mod lint;
mod process;

use anyhow::Result;
use pico_args::Arguments;

use crate::{
    doc::DocBuilder,
    env::Env,
    lint::Linter,
    process::{cargo, command, CommandExt as _},
};

fn show_help() {
    eprintln!(
        "\
cargo-xtask
Free-form automation tool

Usage:
    cargo xtask <SUBCOMMAND>

Subcommands:
    lint            Run lints
    doc             Build API docs
    coverage        Run coverage test
    install-hooks   Install Git hooks
    pre-commit      Run pre-commit hook

Flags:
    -h, --help  Show this message
"
    );
}

fn main() -> Result<()> {
    let mut args = Arguments::from_env();
    if args.contains(["-h", "--help"]) {
        show_help();
        return Ok(());
    }

    let env = Env::init()?;

    let subcommand = args.subcommand()?;
    match subcommand.as_deref() {
        Some("lint") => {
            args.finish()?;

            let linter = Linter { env: &env };
            linter.run_rustfmt()?;
            linter.run_clippy()?;
        }

        Some("doc") => {
            args.finish()?;

            let doc_builder = DocBuilder { env: &env };
            doc_builder.build_docs()?;
        }

        Some("coverage") => {
            args.finish()?;
            do_coverage(&env)?;
        }

        Some("install-hooks") => {
            let force = args.contains(["-f", "--force"]);
            args.finish()?;
            hook::install(&env, force)?;
        }

        Some("pre-commit") => {
            args.finish()?;
            hook::pre_commit(&env)?;
        }

        Some(subcommand) => {
            show_help();
            anyhow::bail!("unknown subcommand: {}", subcommand);
        }
        None => {
            show_help();
            anyhow::bail!("missing subcommand");
        }
    }

    Ok(())
}

fn do_coverage(env: &Env) -> Result<()> {
    // Refs:
    // * https://doc.rust-lang.org/nightly/unstable-book/compiler-flags/source-based-code-coverage.html
    // * https://marco-c.github.io/2020/11/24/rust-source-based-code-coverage.html

    if command(env, "grcov")
        .arg("--version")
        .silent()
        .run()
        .is_err()
    {
        eprintln!("[cargo-xtask] grcov is not installed");
        return Ok(());
    }

    let cov_dir = env.target_dir.join("cov");
    let profraw_file = cov_dir.join("polyfuse-%p-%m.profraw");

    cargo(env)
        .arg("test")
        .arg("--workspace")
        .env("CARGO_BUILD_RUSTFLAGS", "-Zinstrument-coverage")
        .env("LLVM_PROFILE_FILE", &profraw_file)
        .with(|cmd| {
            println!("[cargo-xtask] Run instrumented tests...");
            println!("[cargo-xtask] $ {:?}", cmd);
            cmd
        })
        .run()?;

    command(env, "grcov")
        .arg(&env.project_root)
        .with(|cmd| cmd.arg("--binary-path").arg(env.target_dir.join("debug")))
        .with(|cmd| cmd.arg("--source-dir").arg(&env.project_root))
        .with(|cmd| cmd.arg("--output-type").arg("lcov"))
        .arg("--branch")
        .arg("--ignore-not-existing")
        .with(|cmd| cmd.arg("--ignore").arg("/*"))
        .with(|cmd| cmd.arg("--output-path").arg(cov_dir.join("lcov.info")))
        .with(|cmd| {
            println!("[cargo-xtask] Run {:?}", cmd);
            cmd
        })
        .run()?;

    Ok(())
}
