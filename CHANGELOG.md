# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0](https://github.com/alphazede/bran/releases/tag/bran-v0.4.0) - 2026-11-20

### Changed

- Pin `az-lint` to `v0.1.2`, which stops pull-request lint from reporting `EXE001` on executable scripts. ([#74](https://github.com/alphazede/bran/pull/74))
- Harden the hosted lint step: hash-pinned Python tools, pull requests linted in untrusted mode with the base revision's configuration, and `az-lint` pinned to `v0.1.1`. ([#73](https://github.com/alphazede/bran/pull/73))
- The local fast gate finds `az-lint` through `AZ_LINT` or `PATH` and prints install instructions when the runner is missing. ([#71](https://github.com/alphazede/bran/pull/71))
- Pin `az-lint` to its re-authored `v0.1.0` commit; the files are unchanged. ([#70](https://github.com/alphazede/bran/pull/70))
- `--brief` treats `.jsx`, `.mjs`, `.cjs`, `.tsx`, `.mts` and `.cts` files with `test`, `tests`, `spec` or `specs` in their path as test files, so a non-test question demotes them like `.ts` tests. ([#72](https://github.com/alphazede/bran/pull/72))
- `--brief` matches a filename word through its agent-noun verb form in both directions: `parser` in a question matches a file named `parse`, and the reverse. Only the filename field uses this form. ([#72](https://github.com/alphazede/bran/pull/72))
- `--brief` adds the directory name to the filename field of an `index` or `README` document, so the page that describes a directory matches that directory's name. ([#72](https://github.com/alphazede/bran/pull/72))
- `--brief` applies the value-source rule to set questions (`which owners may ...`), so the configuration file that lists the members can rank above the script that reads it. `which <noun>` and `which <plural> <verb>` questions are unchanged. ([#72](https://github.com/alphazede/bran/pull/72))

- `tools/ci/check.sh` waits for a shared gate slot and caps its CPU cores when `AZ_GATE_SLOTS_SH` names a helper; without it the script runs as before. ([#69](https://github.com/alphazede/bran/pull/69))
- Lint changed files with [`az-lint`](https://github.com/alphazede/az-lint) as a blocking step before tests, locally and in the hosted gate. ([#68](https://github.com/alphazede/bran/pull/68))
- `--brief` ranks with a fielded lexical index: filename, enclosing path,
  body and a short description field, Snowball stemming on both sides, and
  BM25-style per-field scoring with documented generic weights. A rare
  defining word now competes with several incidental mentions, so coverage
  alone no longer fixes the order. Test files gain weight for test questions
  and lose it otherwise; quantity and frequency questions can raise a
  configuration file that a relevant source names. Tiers from
  `.bran/ranking.yaml` still sort first. The index is rebuilt from each
  query's scan, so results are never stale. `reason` values are now
  `exact|partial:` followed by `filename`, `path`, `body`, `description` or
  `reference`. Details in `docs/brief-ranking.md`. ([#72](https://github.com/alphazede/bran/pull/72))

### Added

- Optional `.bran/descriptions.jsonl` sidecar: one
  `{"path", "sha256", "description"}` object per line adds a description to
  a file's description field while the hash matches the file. Size limits,
  in-tree paths only; bad lines are skipped with one
  `descriptions_ignored` warning and never fail the query. ([#72](https://github.com/alphazede/bran/pull/72))

- Brief-mode advisor: optional `send_fields` value `excerpt` sends a bounded
  (900-byte, UTF-8) excerpt of each candidate file, and the advisor may answer
  `{"scores": [{"id", "score"}]}` with a score from 0 to 1 per candidate
  instead of a path list. Every candidate now carries its `id`. Protocol in
  `docs/brief-advisor.md`. ([#72](https://github.com/alphazede/bran/pull/72))

### Fixed

- `--brief` selection keeps results by the same relevance measure the ranking
  sorts by, so an exact field on the leader no longer hides close runners-up. ([#72](https://github.com/alphazede/bran/pull/72))
- `--brief` no longer admits a document matched only by an ignored word when
  the query has a content word. ([#72](https://github.com/alphazede/bran/pull/72))
- `--brief` ranking takes the strongest location signal per query word; a
  filename substring no longer lowers a whole directory-token match. ([#72](https://github.com/alphazede/bran/pull/72))
- `--brief` morphology normalizes query words and document tokens the same
  way, credits filename stems independently of body hits, scores a stem below
  the exact token, and matches a stem inside an identifier only at a sub-word
  start (`IndexBuilder`), never mid-word (`Install`). ([#72](https://github.com/alphazede/bran/pull/72))
- `--brief` score components no longer saturate into ties on long queries. ([#72](https://github.com/alphazede/bran/pull/72))
- `.bran/ranking.yaml` rejects a misaligned sibling key and accepts an
  apostrophe inside a plain scalar. ([#72](https://github.com/alphazede/bran/pull/72))
- `--brief` output now stays within its budget: 2,000 characters for up to 8
  results plus 250 per result an explicit `--top` adds. When exact paths
  alone do not fit, trailing results are dropped and a `brief_truncated`
  warning reports how many. ([#72](https://github.com/alphazede/bran/pull/72))

## [0.3.0](https://github.com/alphazede/bran/releases/tag/bran-v0.3.0) - 2026-11-19

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
- Brief mode selection step (`crates/bran-cli/src/select.rs`): deterministic
  score-ratio ordered-subsequence rule (`selection.source: "rule"`, applied to
  the bounded shortlist before `--top` truncates the output) and optional
  caller-scoped external advisor configured only via `BRAN_ADVISOR_CONFIG`
  (`sha256`-verified, cleared environment with `pass_env`, `requires_env`,
  bounded stdout, deadline kill and reap, silent fallback to the rule).
  ([#66](https://github.com/alphazede/bran/pull/66))

### Changed

- `bran query --brief` now scores candidates with a single integer score:
  coverage of distinct query terms and path or filename hits come first, term
  rarity orders candidates only within equal coverage, then capped body
  repeats and a deterministic path tie-break, using copy-free ASCII
  case-insensitive matching. ([#65](https://github.com/alphazede/bran/pull/65))

- Parallelized repository file reads and metadata parsing in `RepositoryScanner`
  using bounded `std::thread` workers while preserving deterministic traversal
  order, scan limits, and output equivalence.
  ([#64](https://github.com/alphazede/bran/pull/64))
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
