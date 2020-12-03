use crate::{
    env::Env,
    process::{cargo, CommandExt as _},
};
use anyhow::Result;
use std::{fs, time::Duration};

// TODOs:
// * restrict network access during building docs.
// * restrict all write access expect target/

// ref: https://blog.rust-lang.org/2019/09/18/upcoming-docsrs-changes.html#what-will-change
const CARGO_DOC_TIMEOUT: Duration = Duration::from_secs(60 * 15);

const DOC_PACKAGES: &[&str] = &[
    "polyfuse", //
    "polyfuse-kernel",
];

pub struct DocBuilder<'env> {
    pub env: &'env Env,
}

impl DocBuilder<'_> {
    pub fn build_docs(&self) -> Result<()> {
        let doc_dir = self.env.target_dir.join("doc");
        if doc_dir.exists() {
            fs::remove_dir_all(&doc_dir)?;
        }

        for package in DOC_PACKAGES {
            self.build_doc(package)?;
        }

        let lockfile = doc_dir.join(".lock");
        if lockfile.exists() {
            fs::remove_file(lockfile)?;
        }

        let indexfile = doc_dir.join("index.html");
        fs::write(
            indexfile,
            "<meta http-equiv=\"refresh\" content=\"0;url=polyfuse\">\n",
        )?;

        Ok(())
    }

    fn build_doc(&self, package: &str) -> Result<()> {
        cargo(self.env)
            .arg("doc")
            .arg("--no-deps")
            .arg(format!("--package={}", package))
            .with(|cmd| {
                println!("[cargo-xtask] Run {:?}", cmd);
                cmd
            })
            .run_timeout(CARGO_DOC_TIMEOUT)?;
        Ok(())
    }
}
