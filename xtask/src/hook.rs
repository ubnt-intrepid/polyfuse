use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use crate::env::Env;
use crate::lint::Linter;
use crate::TaskResult;

fn resolve_git_dir(env: &Env) -> TaskResult<PathBuf> {
    let mut project_root = env.project_root.clone();

    if !project_root.has_root() {
        project_root = project_root.canonicalize()?;
    }

    for dir in project_root.ancestors() {
        let git_dir = dir.join(".git");
        if git_dir.is_dir() {
            return Ok(git_dir);
        }
    }

    Err("Git directory is not found".into())
}

pub fn install(env: &Env, force: bool) -> TaskResult<()> {
    let hooks_dir = resolve_git_dir(env)?.join("hooks");

    let install = |name: &str| -> TaskResult<()> {
        let hook_path = hooks_dir.join(name);

        if hook_path.exists() && !force {
            eprintln!("[cargo-xtask] hook is already installed: {}", name);
            return Ok(());
        }

        eprintln!(
            "[cargo-xtask] install hook shim script to {}",
            hook_path.display()
        );

        let mut file = OpenOptions::new() //
            .write(true)
            .create(true)
            .truncate(true)
            .open(&hook_path)?;

        writeln!(
            file,
            "#!/bin/sh\n\
             cargo xtask {}",
            name
        )?;

        let mut perms = file.metadata()?.permissions();
        perms.set_mode(0o755);
        file.set_permissions(perms)?;

        Ok(())
    };

    install("pre-commit")?;

    Ok(())
}

pub fn pre_commit(env: &Env) -> TaskResult<()> {
    eprintln!("[cargo-xtask] run pre-commit hook");
    Linter { env }.run_rustfmt()?;
    Ok(())
}
