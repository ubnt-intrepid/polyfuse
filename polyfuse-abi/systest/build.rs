use ctest::TestGenerator;
use std::{env, path::PathBuf};

fn main() {
    let mut cfg = TestGenerator::new();
    cfg.header("fuse_kernel.h");
    cfg.include(PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("libfuse/include"));

    cfg.field_name(|_s, field| field.replace("typ", "type"));
    cfg.skip_field(|s, field| s == "fuse_dirent" && field == "name");

    cfg.skip_struct(|s| s == "UnknownOpcode" || s == "InvalidFileLock");

    cfg.generate("../src/lib.rs", "all.rs");
}
