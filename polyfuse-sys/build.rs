const LIBFUSE_ATLEAST_VERSION: &str = "3.0.0";
const LIBFUSE_PKG_NAME: &str = "fuse3";

fn main() {
    let libfuse = pkg_config::Config::new()
        .atleast_version(LIBFUSE_ATLEAST_VERSION)
        .probe(LIBFUSE_PKG_NAME)
        .unwrap_or_else(|e| panic!("pkg-config error: {}", e));

    let mut build = cc::Build::new();
    build.warnings_into_errors(true);
    build.file("src/libfuse.c");
    for incpath in &libfuse.include_paths {
        build.include(incpath);
    }
    build.compile("libfuse");
}
