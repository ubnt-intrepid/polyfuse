use std::{env, path::PathBuf};

const FUSE_USE_VERSION: &str = "34";
const LIBFUSE_PKG_NAME: &str = "fuse3";

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").map(PathBuf::from).unwrap();

    // link libfuse.
    let libfuse = pkg_config::Config::new().probe(LIBFUSE_PKG_NAME).unwrap();

    // build helper C functions.
    let mut helpers = cc::Build::new();
    helpers.warnings_into_errors(true);
    helpers.file(manifest_dir.join("src/backend.c"));
    helpers.define("FUSE_USE_VERSION", FUSE_USE_VERSION);
    for incpath in &libfuse.include_paths {
        helpers.include(incpath);
    }
    helpers.compile("tokio-fuse-helpers");
}
