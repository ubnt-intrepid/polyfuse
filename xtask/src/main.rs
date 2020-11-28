mod doc;
mod env;
mod hook;
mod lint;
mod process;

use anyhow::{Context as _, Result};
use pico_args::Arguments;
use std::path::PathBuf;

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

    const BLACKLISTED_TESTS: &[&str] = &[
        "polyfuse_example_basic", //
        "polyfuse_example_hello",
    ];

    let tests = build_instrumented_tests(env)?;
    let cov_dir = env.target_dir.join("cov");
    let report_dir = cov_dir.join("report");

    for test in &tests {
        let test_name = test
            .file_stem()
            .expect("missing file name in test artifact path")
            .to_str()
            .expect("test artifact name is not UTF-8");

        if BLACKLISTED_TESTS
            .iter()
            .any(|prefix| test_name.starts_with(prefix))
        {
            println!("[cargo-xtask] Skip instrumented test {}", test_name);
            continue;
        }

        let profraw_path = cov_dir.join(format!("{}.profraw", test_name));
        let profdata_path = cov_dir.join(format!("{}.profdata", test_name));
        let report_path = report_dir.join(test_name);

        // NOTE: The current directory for running test is restricted to `project_root`.
        command(env, test)
            .env("LLVM_PROFILE_FILE", &profraw_path)
            .with(|cmd| {
                println!("[cargo-xtask] Run instrumented test: {:?}", cmd);
                cmd
            })
            .run()?;

        command(env, "llvm-profdata")
            .arg("merge")
            .arg("-sparse")
            .arg(&profraw_path)
            .with(|cmd| cmd.arg("-o").arg(&profdata_path))
            .with(|cmd| {
                println!("[cargo-xtask] Create coverage map for {}", test_name);
                cmd
            })
            .run()?;

        command(env, "llvm-cov")
            .arg("show")
            .arg("-Xdemangler=rustfilt")
            .arg(test)
            .arg(format!("-instr-profile={}", profdata_path.display()))
            .arg("-show-line-counts-or-regions")
            .arg("-show-instantiations")
            .arg("-format=html")
            .arg(format!("-output-dir={}", report_path.display()))
            .with(|cmd| {
                println!("[cargo-xtask] Create coverage report for {}", test_name);
                cmd
            })
            .run()?;
    }

    Ok(())
}

fn build_instrumented_tests(env: &Env) -> Result<Vec<PathBuf>> {
    use json::JsonValue;
    use std::io::{BufRead as _, BufReader};
    use std::process::Stdio;

    let mut child = cargo(env)
        .arg("test")
        .arg("--workspace")
        .arg("--no-run")
        .arg("--message-format=json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .env("CARGO_BUILD_RUSTFLAGS", "-Zinstrument-coverage")
        .env("LLVM_PROFILE_FILE", "/dev/null")
        .with(|cmd| {
            println!("[cargo-xtask] Build instrumented tests...");
            println!("[cargo-xtask] $ {:?}", cmd);
            cmd
        })
        .spawn()?;

    // Ref: https://doc.rust-lang.org/cargo/reference/external-tools.html#json-messages
    let stdout = child.stdout.as_mut().context("missing stdout pipe")?;
    let stdout = BufReader::new(stdout);

    let mut executables = vec![];

    for line in stdout.lines().filter_map(|line| line.ok()) {
        let msg = match json::parse(line.trim()) {
            Ok(JsonValue::Object(msg)) => msg,
            _ => continue,
        };

        match msg.get("reason") {
            Some(reason) if reason == "compiler-artifact" => (),
            _ => continue,
        }

        let is_test_artifact = msg
            .get("profile")
            .and_then(|profile| match profile {
                JsonValue::Object(ref profile) => profile.get("test")?.as_bool(),
                _ => None,
            })
            .unwrap_or(false);
        if !is_test_artifact {
            continue;
        }

        if let Some(executable) = msg
            .get("executable")
            .and_then(|executable| executable.as_str())
        {
            executables.push(PathBuf::from(executable));
        }
    }

    child.wait()?;

    Ok(executables)
}
