# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `bran query <root> --brief [--top N] <request>` returns a compact JSON
  response (`<= 2,000` Unicode code points target for 8 results) with ranked
  exact paths (including paths above 1,024 bytes, and `"bundle"` per item in
  multi-root queries), one-line reasons shortened first when over budget,
  bounded `query_coverage` and `warnings` (summarizing omitted repository scan
  diagnostics in a single `scan_warnings` line), `coverage_basis: "sources"`,
  and a content-addressed `result_id` in `FilesystemResultStore` (`.bran/results/`). ([#63](https://github.com/alphazede/bran/pull/63))
- `bran get <result-id> --full` re-runs the full default query for the stored
  root and question and adds a `"stale: true"` warning when the source revision
  changed. ([#63](https://github.com/alphazede/bran/pull/63))
- `tools/recall/recall_check.py` and the synthetic recall fixture in
  `fixtures/recall/`, plus opt-in `BRAN_TIMINGS=1` per-step stderr timings in
  single-root and multi-root `bran query`. ([#63](https://github.com/alphazede/bran/pull/63))
- Optional `.bran/ranking.yaml` path-class tiers (`classes: [{name, tier, globs}]`,
  tier `0..=9`, first matching class wins) for `bran query --brief`, keeping
  default `bran query` output unchanged. ([#65](https://github.com/alphazede/bran/pull/65))

### Changed

- `bran query --brief` now scores candidates with a single integer score that
  prioritizes rarity-scaled term coverage, path and filename hits over capped
  body repeats, and deterministic path tie-breaking, using copy-free ASCII
  case-insensitive matching across candidate scoring. ([#65](https://github.com/alphazede/bran/pull/65))

- Documented that default `bran query` `selected_locators` remains alphabetical
  while `source_rankings` and `query --brief` `results` are relevance-ordered,
  and updated `README.md`, `docs/integrations/agent-setup.md`, and
  `skill/use-bran/SKILL.md` to recommend `--brief` first with full follow-up. ([#63](https://github.com/alphazede/bran/pull/63))

- `cargo deny` now allows the `Unicode-3.0` license, which `unicode-ident`
  uses. Newer `zip` releases depend on it.
  ([#62](https://github.com/alphazede/bran/pull/62))

- Bumped `zip` from 8.6.0 to 9.0.5 in `xtask`. The lockfile adds `nix` 0.31.3
  and `cfg_aliases` 0.2.2. ([#57](https://github.com/alphazede/bran/pull/57))
- Bumped GitHub Actions: `actions/checkout` to v7.0.1 (pinned by SHA in the
  fast gate), `actions/upload-artifact` to v7, `actions/download-artifact` to
  v8 and `sigstore/cosign-installer` to v4.1.2.
  ([#54](https://github.com/alphazede/bran/pull/54))

## [0.2.2] - 2026-11-06

### Fixed

- The release workflow builds the macOS archives on the `macos-15` runner,
  because GitHub no longer supports the `macos-14` runner.
  ([#59](https://github.com/alphazede/bran/pull/59))

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
