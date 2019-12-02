# Changelog
All notable changes to this project will be documented in this file.

This format is based on [Keep a Changelog], and this project adheres to [Semantic Versioning].

## [Unreleased]

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

[Unreleased]: https://github.com/ubnt-intrepid/polyfuse/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/ubnt-intrepid/polyfuse/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/ubnt-intrepid/polyfuse/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/ubnt-intrepid/polyfuse/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ubnt-intrepid/polyfuse/tree/v0.1.0

[Keep a Changelog]: https://keepachangelog.com/en/1.0.0/
[Semantic Versioning]: https://semver.org/spec/v2.0.0.html
