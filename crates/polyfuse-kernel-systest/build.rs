use ctest::TestGenerator;

fn main() {
    generate_abi_tests();
}

fn generate_abi_tests() {
    let mut cfg = TestGenerator::new();
    cfg.header("fuse_kernel.h");
    cfg.header("fuse_kernel_compat.h");
    cfg.header("sys/ioctl.h");
    cfg.include("./include");

    cfg.field_name(|_s, field| field.replace("typ", "type"));
    cfg.skip_field(|s, field| {
        matches!(
            (s, field),
            ("fuse_dirent", "name") | ("fuse_supp_groups", "groups")
        )
    });

    cfg.skip_struct(|s| s == "UnknownOpcode" || s == "InvalidFileLock");

    cfg.generate("../polyfuse-kernel/src/lib.rs", "kernel.rs");
}
