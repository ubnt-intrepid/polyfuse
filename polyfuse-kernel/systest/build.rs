use ctest::TestGenerator;

fn main() {
    generate_abi_tests();
}

fn generate_abi_tests() {
    let mut cfg = TestGenerator::new();
    cfg.header("fuse_kernel.h");
    cfg.header("sys/ioctl.h");
    cfg.include("libfuse/include");

    cfg.field_name(|_s, field| field.replace("typ", "type"));
    cfg.skip_field(|s, field| s == "fuse_dirent" && field == "name");

    cfg.skip_struct(|s| s == "UnknownOpcode" || s == "InvalidFileLock");

    // FUSE_FSYNC_FDATASYNC is defined since libfuse 3.7.0.
    cfg.skip_const(|name| name == "FUSE_FSYNC_FDATASYNC");

    cfg.generate("../src/lib.rs", "kernel.rs");
}
