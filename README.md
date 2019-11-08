<h1 align="center">
  <code>polyfuse</code>
</h1>
<div align="center">
  <strong>
    A FUSE (Filesystem in Userspace) framework for Rust.
  </strong>
</div>

<br />

<div align="center">
  <a href="https://crates.io/crates/polyfuse">
    <img src="https://img.shields.io/crates/v/polyfuse.svg?style=flat-square"
         alt="crates.io"
    />
  </a>
  <a href="https://blog.rust-lang.org/2019/11/07/Rust-1.39.0.html">
    <img src="https://img.shields.io/badge/rust%20toolchain-1.39.0%2B-gray.svg?style=flat-square"
         alt="rust toolchain"
    />
  </a>
  <a href="https://ubnt-intrepid.github.io/polyfuse/">
    <img src="https://img.shields.io/badge/doc-master-informational?style=flat-square"
         alt="master doc"
    />
  </a>
</div>

<br />

`polyfuse` is a framework for implementing filesystems based on [Filesystem in Userspace (FUSE)](https://en.wikipedia.org/wiki/Filesystem_in_Userspace) in Rust.

The goal of this project is to provide a Rust FUSE library that has a high affinity with the `async`/`.await` syntax stabilized in Rust 1.39.

## Status

Under development

## Platforms

Currently, `polyfuse` only supports the Linux platforms with the FUSE ABI version is 7.23 or higher.
The required kernel version is Linux 3.15 or later.

Adding support for other Unix platform running FUSE (FreeBSD, macOS, and so on) is a future work.

## Installation

Add a dependency to your package using [`cargo-edit`](https://github.com/killercup/cargo-edit) as follows:

```shell-session
$ cargo add polyfuse --git https://github.com/ubnt-intrepid/polyfuse.git

$ cargo add async-trait@0.1
$ cargo add tokio --git https://github.com/tokio-rs/tokio.git
```

Furthermore, the dependency for `libfuse` is required to establish the connection with the FUSE kernel driver.

On Debian/Ubuntu or other APT based distributions:

```shell-session
$ sudo apt-get install fuse
$ sudo apt-get install pkg-config libfuse-dev  # for building
```

On Fedora/RHEL or other RPM based distributions:

```shell-session
$ sudo dnf install fuse
$ sudo dnf install fuse-devel pkg-config  # for building
```

On Arch Linux or other Pacman based distributions:

```shell-session
$ sudo pacman -S fuse2
$ sudo pacman -S pkgconf  # for building, included in base-devel
```

## License

This library is licensed under either of

* MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)

at your option.
