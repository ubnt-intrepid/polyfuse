use anyhow::Result;
use std::{env, path::PathBuf};

pub struct Env {
    pub project_root: PathBuf,
    pub target_dir: PathBuf,
}

impl Env {
    pub fn init() -> Result<Self> {
        let manifest_dir = env::var_os("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .or_else(|| option_env!("CARGO_MANIFEST_DIR").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("./xtask"));
        let project_root = manifest_dir.parent().unwrap().to_owned();

        let target_dir = env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| project_root.join("target"));

        Ok(Self {
            project_root,
            target_dir,
        })
    }
}
