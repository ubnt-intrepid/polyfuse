// Ref: https://doc.rust-lang.org/nightly/unstable-book/compiler-flags/source-based-code-coverage.html

use crate::{
    env::Env,
    process::{cargo, command, CommandExt as _},
};
use anyhow::{Context as _, Result};
use json::JsonValue;
use once_cell::sync::Lazy;
use regex::Regex;
use std::{
    fs,
    io::{BufRead as _, BufReader},
    path::{Path, PathBuf},
    process::Stdio,
};

pub fn do_coverage(env: &Env) -> Result<()> {
    let cov_dir = env.target_dir.join("cov");
    assert!(cov_dir.is_absolute());

    let report_dir = cov_dir.join("report");

    eprintln!(
        "[cargo-xtask] Clean old coverage results in {}",
        cov_dir.display()
    );
    if cov_dir.is_dir() {
        fs::remove_dir_all(&cov_dir)?;
    } else if cov_dir.is_file() {
        fs::remove_file(&cov_dir)?;
    }

    eprintln!("[cargo-xtask] Build instrumented test artifacts");
    let tests = build_instrumented_tests(env)?;

    for test in &tests {
        let test_name = extract_test_name(&test).context("failed to extract test name")?;

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

        cargo(env)
            .arg("profdata")
            .arg("--")
            .arg("merge")
            .arg("-sparse")
            .arg(&profraw_path)
            .with(|cmd| cmd.arg("-o").arg(&profdata_path))
            .with(|cmd| {
                println!("[cargo-xtask] Create coverage map for {}", test_name);
                cmd
            })
            .run()?;

        cargo(env)
            .arg("cov")
            .arg("--")
            .arg("show")
            .arg("-Xdemangler=rustfilt")
            .arg(test)
            .arg(format!("-instr-profile={}", profdata_path.display()))
            .arg("-show-line-counts-or-regions")
            .arg("-show-instantiations")
            .arg("--ignore-filename-regex=/.cargo/registry/")
            .arg("--ignore-filename-regex=/target/")
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

fn extract_test_name(test: &Path) -> Option<&str> {
    static TEST_NAME_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(.*)-([a-z0-9]+)").unwrap());
    let test_name = test.file_stem()?.to_str()?;
    let test_name = TEST_NAME_PATTERN.captures(test_name)?.get(1)?.as_str();
    Some(test_name)
}
