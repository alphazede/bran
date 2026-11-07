# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.2] - 2026-11-06

### Fixed

- The release workflow builds the macOS archives on the `macos-15` runner,
  because GitHub no longer supports the `macos-14` runner.

## [0.2.1] - 2026-11-06

Tagged but not published, because its macOS builds could not start. Its
changes ship in 0.2.2.

### Changed

- Raised the default total scan limit from 64 MiB to 256 MiB. Repositories
  with more than 64 MiB of scannable text previously failed with
  `scan_error: LimitExceeded`. The 1 MiB per-file and 10,000-file limits are
  unchanged. ([#58](https://github.com/alphazede/bran/pull/58))
- The repository scan snapshot schema now allows `total_bytes` up to 256 MiB to
  match the new default.
- The scan configuration feeds the scan input fingerprint, so existing derived
  indexes are rebuilt once after upgrading.

## [0.2.0] - 2026-09-30

See the [0.2.0 release notes](https://github.com/alphazede/bran/releases/tag/bran-v0.2.0).

[Unreleased]: https://github.com/alphazede/bran/compare/bran-v0.2.2...HEAD
[0.2.2]: https://github.com/alphazede/bran/compare/bran-v0.2.1...bran-v0.2.2
[0.2.1]: https://github.com/alphazede/bran/compare/bran-v0.2.0...bran-v0.2.1
[0.2.0]: https://github.com/alphazede/bran/releases/tag/bran-v0.2.0
