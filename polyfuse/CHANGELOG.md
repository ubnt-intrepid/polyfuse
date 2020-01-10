# Changelog
All notable changes to this project will be documented in this file.

This format is based on [Keep a Changelog], and this project adheres to [Semantic Versioning].

## [Unreleased]

### Added

* `Reply` trait (introduced in [#70](https://github.com/ubnt-intrepid/polyfuse/pull/70) as `ScatteredBytes`, and renamed in [6682a82](https://github.com/ubnt-intrepid/polyfuse/commit/6682a828934ffc257d9bc7b690215e02014df08a)))
* `TryFrom<Metadata>` for `FileAttr` ([deb093c](https://github.com/ubnt-intrepid/polyfuse/commit/deb093ce195fcbd4bb675c12155c8a05d58211c6))
* `LockOwner` for protection the raw owner identifier ([#73](https://github.com/ubnt-intrepid/polyfuse/pull/73))

### Fixed

* improve the `fmt::Debug` implementation in public types ([ca4546c](https://github.com/ubnt-intrepid/polyfuse/commit/ca4546c3c0cf1ddd5e66c81771857a559959e9d3))

### Deprecated

* `Context::reply_raw` (#70)
* Replying methods such as `Lookup::reply` and so on ([#71](https://github.com/ubnt-intrepid/polyfuse/pull/71))

## [0.3.2] (2020-01-03)

### Fixed

* avoid clearing readonly init flags ([2674812](https://github.com/ubnt-intrepid/polyfuse/commit/267481224d77860c9221e6964662620918374757))

## [0.3.1] (2020-01-01)

### Deprecated

* `ReplyEntry::new` ([8e73cb6](https://github.com/ubnt-intrepid/polyfuse/commit/8e73cb6c6a3a698498fd2578ea5c8e669ffa8e74))
* `op::CopyFileRange::{input,output}` (in [#64](https://github.com/ubnt-intrepid/polyfuse/pull/64))

### Fixed

* fix some wrong cases when parsing requests containing string(s) ([#66](https://github.com/ubnt-intrepid/polyfuse/pull/66)).

## [0.3.0] (2019-12-28)

Only the characteristic changes are listed here.
See the commit log for a detailed change history.

### Added

* Add `Reader` and `Writer` for representing the FUSE-specific I/O abstraction.
* Add `NotifyReply` and `Interrupt` variants to `Operation`.  In the previous version,
  these requests are automatically handled by `Session::receive`.

### Changed

* Reform the definition of `Filesystem` based on `Reader` and `Writer`.
* `Notifier` is integrated into `Session`.
* `notify_retrieve` returns the unique ID of the reply message, instead of a channel for
   awaiting the received cache data. The filesystem needs to handle the corresponding
   `NotifyReply` operation and forward the cache data returned from the kernel to the
   appropriate end.
* The representation of time / time instant are changed to use the `std::time` types.
  In order to avoid the conversion overhead, some methods using the *raw* time values
  are still retained.

### Removed

* The module `request` and the older `Buffer` trait are removed. These features are replaced
  with the new `io` module.

## [0.2.1] (2019-12-02)

### Fixed

* set the field `nodeid` correctly when `ReplyEntry` is created ([1cb39b3](https://github.com/ubnt-intrepid/polyfuse/commit/1cb39b3077d62210f3d7b91ce7be57719b2fb73f))
* add a null terminator to the notification payload ([9af5ab4](https://github.com/ubnt-intrepid/polyfuse/commit/9af5ab4daf0f59a898c0fed08098a57fe2664243))

## [0.2.0] (2019-11-30)

### Added

* re-export `async_trait` from the crate root ([1160953](https://github.com/ubnt-intrepid/polyfuse/commit/1160953f8e74c8888c7d4270eff926a8112ea256))
* enable `readdirplus` operation ([b36f60b](https://github.com/ubnt-intrepid/polyfuse/commit/b36f60b5055bd723ba9a7406252917f0b4829f0f))

### Changed

* bump bytes to 0.5.0 ([5f57c2e](https://github.com/ubnt-intrepid/polyfuse/commit/5f57c2e471df5ac83ca2776c0bd18a653d6f9360))
* refine `Operation` and reply types ([7b48d7b](https://github.com/ubnt-intrepid/polyfuse/commit/7b48d7bc408d753033a4b21df927b52fd8420a7c) and [3c41327](https://github.com/ubnt-intrepid/polyfuse/commit/3c41327856b317c14a84e06093c30bd5139aff2c))

### Removed

* The package `polyfuse-sys` is integrated into `polyfuse` and private to users ([90d54eb](https://github.com/ubnt-intrepid/polyfuse/commit/90d54eb9a5d13a9f06cd3e1922cd01c087c7a416))

## [0.1.1] (2019-11-19)

* hide `Request::new` ([ca3a7ba](https://github.com/ubnt-intrepid/polyfuse/commit/ca3a7baf3650304d16b270c556704f7c631b0888))

## [0.1.0] (2019-11-16)

* initial release

<!-- links -->

[Unreleased]: https://github.com/ubnt-intrepid/polyfuse/compare/v0.3.2...HEAD
[0.3.2]: https://github.com/ubnt-intrepid/polyfuse/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/ubnt-intrepid/polyfuse/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/ubnt-intrepid/polyfuse/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/ubnt-intrepid/polyfuse/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/ubnt-intrepid/polyfuse/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/ubnt-intrepid/polyfuse/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ubnt-intrepid/polyfuse/tree/v0.1.0

[Keep a Changelog]: https://keepachangelog.com/en/1.0.0/
[Semantic Versioning]: https://semver.org/spec/v2.0.0.html
