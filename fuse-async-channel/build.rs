const LIBFUSE_ATLEAST_VERSION: &str = "2.6.0";

#[cfg(not(target_os = "macos"))]
const LIBFUSE_PKG_NAME: &str = "fuse";

#[cfg(target_os = "macos")]
const LIBFUSE_PKG_NAME: &str = "osxfuse";

fn main() {
    pkg_config::Config::new()
        .atleast_version(LIBFUSE_ATLEAST_VERSION)
        .probe(LIBFUSE_PKG_NAME)
        .unwrap_or_else(|e| panic!("pkg-config error: {}", e));
}
