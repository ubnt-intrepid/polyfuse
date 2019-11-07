use ctest::TestGenerator;

fn main() {
    generate_kernel_tests();
    generate_libfuse_tests();
}

fn generate_kernel_tests() {
    let mut cfg = TestGenerator::new();
    cfg.header("fuse_kernel.h");
    cfg.header("sys/ioctl.h");
    cfg.include("libfuse/include");

    cfg.field_name(|_s, field| field.replace("typ", "type"));
    cfg.skip_field(|s, field| s == "fuse_dirent" && field == "name");

    cfg.skip_struct(|s| s == "UnknownOpcode" || s == "InvalidFileLock");

    cfg.generate("../polyfuse-sys/src/kernel.rs", "kernel.rs");
}

fn generate_libfuse_tests() {
    let mut cfg = TestGenerator::new();
    cfg.define("FUSE_USE_VERSION", Some("30"));
    cfg.header("fuse_lowlevel.h");
    cfg.include("libfuse/include");
    cfg.skip_fn(|name| name == "fuse_session_new_empty");
    cfg.generate("../polyfuse-sys/src/libfuse.rs", "libfuse.rs");
}
