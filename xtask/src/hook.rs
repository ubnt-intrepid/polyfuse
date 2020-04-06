use std::{
    fs::OpenOptions, //
    io::Write as _,
    os::unix::fs::PermissionsExt as _,
    path::PathBuf,
};

fn resolve_git_dir() -> anyhow::Result<PathBuf> {
    let mut project_root = crate::project_root();
    if !project_root.has_root() {
        project_root = project_root.canonicalize()?;
    }

    for dir in project_root.ancestors() {
        let git_dir = dir.join(".git");
        if git_dir.is_dir() {
            return Ok(git_dir);
        }
    }

    anyhow::bail!("Git directory is not found");
}

pub fn install(force: bool) -> anyhow::Result<()> {
    let hooks_dir = resolve_git_dir()?.join("hooks");

    let install = |name: &str| -> anyhow::Result<()> {
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

pub fn pre_commit() -> anyhow::Result<()> {
    eprintln!("[cargo-xtask] run pre-commit hook");
    crate::do_lint()?;
    Ok(())
}
