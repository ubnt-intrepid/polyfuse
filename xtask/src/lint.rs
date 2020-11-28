use crate::{
    env::Env,
    process::{cargo, CommandExt as _},
};
use anyhow::Result;

pub struct Linter<'env> {
    pub env: &'env Env,
}

impl Linter<'_> {
    pub fn run_rustfmt(&self) -> Result<()> {
        let has_rustfmt = cargo(self.env)
            .args(&["fmt", "--version"])
            .silent()
            .run()
            .is_ok();

        if has_rustfmt {
            cargo(self.env)
                .args(&["fmt", "--", "--check"])
                .with(|cmd| {
                    println!("[cargo-xtask] Run {:?}", cmd);
                    cmd
                })
                .run()?;
        }

        Ok(())
    }

    pub fn run_clippy(&self) -> Result<()> {
        let has_clippy = cargo(self.env)
            .args(&["clippy", "--version"])
            .silent()
            .run()
            .is_ok();

        if has_clippy {
            cargo(self.env)
                .args(&["clippy", "--all-targets"])
                .with(|cmd| {
                    println!("[cargo-xtask] Run {:?}", cmd);
                    cmd
                })
                .run()?;
        }

        Ok(())
    }
}
