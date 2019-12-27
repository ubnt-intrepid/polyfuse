# Changelog
All notable changes to this project will be documented in this file.

This format is based on [Keep a Changelog], and this project adheres to [Semantic Versioning].

## [Unreleased]

## [0.2.0] (2019-12-28)

### Added

* Add `Builder` for building `Server`.
* Add `Server::run_single` for the single-threaded use.

### Changed

* Bump `polyfuse` to 0.3
* The main loop of server now uses a context pool for concurrently
  processing requests. The loop will create the context value for
  processing the incoming requests by need, and will reuse the created
  contexts after reaching the specified limit.

### Removed

* `Notifier` is integrated into `Server`.

## [0.1.0] (2019-12-04)

* initial release

<!-- links -->

[Unreleased]: https://github.com/ubnt-intrepid/polyfuse/compare/polyfuse-tokio-v0.2.0...HEAD
[0.2.0]: https://github.com/ubnt-intrepid/polyfuse/compare/polyfuse-tokio-v0.1.0...polyfuse-tokio-v0.2.0
[0.1.0]: https://github.com/ubnt-intrepid/polyfuse/tree/polyfuse-tokio-v0.1.0

[Keep a Changelog]: https://keepachangelog.com/en/1.0.0/
[Semantic Versioning]: https://semver.org/spec/v2.0.0.html
