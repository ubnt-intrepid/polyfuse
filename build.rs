use bindgen::EnumVariation;
use std::{
    env,
    path::{Path, PathBuf},
};

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").map(PathBuf::from).unwrap();
    let out_dir = env::var("OUT_DIR").map(PathBuf::from).unwrap();

    if cfg!(all(feature = "libfuse2", feature = "libfuse3")) {
        eprintln!("The feature `libfuse2` and `libfuse3` cannot be specified at the same time.");
        std::process::exit(1);
    }

    if cfg!(feature = "libfuse2") {
        const LIBFUSE_PKG_NAME: &str = "fuse";

        // link libfuse.
        pkg_config::probe_library(LIBFUSE_PKG_NAME).unwrap();
    }

    if cfg!(feature = "libfuse3") {
        build_libfuse3_helpers();
    }

    // generage kernel interface.
    generate_abi_bindings(manifest_dir.join("src/abi.h"), out_dir.join("bindings.rs"));
}

fn build_libfuse3_helpers() {
    const FUSE_USE_VERSION: &str = "34";
    const LIBFUSE_PKG_NAME: &str = "fuse3";

    // link libfuse.
    let libfuse = pkg_config::Config::new().probe(LIBFUSE_PKG_NAME).unwrap();

    // build helper C functions.
    let mut helpers = cc::Build::new();
    helpers.warnings_into_errors(true);
    helpers.file("src/channel/libfuse3_helper.c");
    helpers.define("FUSE_USE_VERSION", FUSE_USE_VERSION);
    for incpath in &libfuse.include_paths {
        helpers.include(incpath);
    }
    helpers.compile("tokio-fuse-helpers");
}

fn generate_abi_bindings(src_path: impl AsRef<Path>, out_path: impl AsRef<Path>) {
    let bindings = bindgen::builder()
        .ctypes_prefix("libc")
        .derive_debug(true)
        .header(format!("{}", src_path.as_ref().display()))
        .default_enum_style(EnumVariation::Rust {
            non_exhaustive: false,
        })
        .blacklist_type("__(.*)|(u)?int(_(.*)|max)_t")
        .generate()
        .unwrap();
    bindings.write_to_file(out_path).unwrap();
}
