use bindgen::EnumVariation;
use std::{
    env,
    path::{Path, PathBuf},
};

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").map(PathBuf::from).unwrap();
    let out_dir = env::var("OUT_DIR").map(PathBuf::from).unwrap();

    // generage kernel interface.
    generate_abi_bindings(manifest_dir.join("src/abi.h"), out_dir.join("bindings.rs"));
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
