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
  <a href="https://docs.rs/polyfuse">
    <img src="https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square"
         alt="docs.rs" />
  </a>
</div>

<br />

`polyfuse` is a framework for implementing filesystems based on [Filesystem in Userspace (FUSE)](https://en.wikipedia.org/wiki/Filesystem_in_Userspace) in Rust.

The goal of this project is to provide a Rust FUSE library that has a high affinity with the `async`/`.await` syntax stabilized in Rust 1.39.

## Installation

Add a dependency to your package using [`cargo-edit`](https://github.com/killercup/cargo-edit) as follows:

```shell-session
$ cargo add polyfuse@0.1 async-trait@0.1
```

To run FUSE daemon, it is necessary to choose appropriate supporting package according to the asynchronous runtime to be used.
Currently, `polyfuse` only provides a support for the [`tokio`](https://github.com/tokio-rs/tokio) and adding support for [`async-std`](https://github.com/async-rs/async-std) is a future work).

```
$ cargo add tokio --git https://github.com/tokio-rs/tokio.git
$ cargo add polyfuse-tokio --git https://github.com/ubnt-intrepid/polyfuse.git
```

## Platform Requirements

Currently, `polyfuse` only supports the Linux platforms with the FUSE ABI version is 7.23 or higher.
The required kernel version is Linux 3.15 or later.

> Adding support for other Unix platform running FUSE (FreeBSD, macOS, and so on) is a future work.

In order to establish the connection with the FUSE kernel driver, the command
`fusermount` must be installed on the platform where the filesystem runs.
This binary is typically including in the fuse package provided by the distribution's package system.

On Debian/Ubuntu or other APT based distributions:

```shell-session
$ sudo apt-get install fuse
```

On Fedora/RHEL or other RPM based distributions:

```shell-session
$ sudo dnf install fuse
```

On Arch Linux or other Pacman based distributions:

```shell-session
$ sudo pacman -S fuse2
```

## License

This library is licensed under either of

* MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)

at your option.
