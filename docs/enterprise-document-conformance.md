---
type: Concept
title: Enterprise-document conformance suite
okf_status: active
tags:
  - public
  - developer
freshness: "2026-09-29"
resource: https://github.com/alphazede/bran
public_boundary: public
---

# Enterprise-document conformance suite

This is the shared suite for [issue #25](https://github.com/alphazede/bran/issues/25).
It owns the cross-format guarantees, so the DOCX
([#21](https://github.com/alphazede/bran/issues/21)), XLSX
([#26](https://github.com/alphazede/bran/issues/26)), PPTX
([#22](https://github.com/alphazede/bran/issues/22)), and PDF
([#23](https://github.com/alphazede/bran/issues/23)) adapters do not each
invent a safety or fidelity policy. Parser selection is recorded in
[`enterprise-document-dependency-review.md`](enterprise-document-dependency-review.md).
The envelope the adapters emit is
[`enterprise-document-evidence-envelope.md`](enterprise-document-evidence-envelope.md).
The DOCX adapter is described in
[`enterprise-document-docx-adapter.md`](enterprise-document-docx-adapter.md).

## Layout

The crate `crates/bran-document` holds everything shared:

| Module | Owns |
|---|---|
| `zip` | Bounded ZIP reader and deterministic writer. Stored and deflate only. |
| `xml` | Shared XML reader. Refuses DTDs, custom entities, and characters or names outside XML 1.0; bounds depth and nodes. Element names are local names; attribute keys keep their prefix (`r:id`, `ContentType`). |
| `opc` | OPC package intake: part names, content types, relationships, active content, external references, signatures, DLP markers, canonical inventory. |
| `canonical` | Canonical JSON bytes and SHA-256, matching the envelope rule. |
| `export` | Export gate: explicit format and destination, DLP first, never overwrite. |
| `conformance` | The `Adapter` trait, the adapter registry, and the checks every row runs. |

`bran-core` stays dependency-free. The only change there is that its DLP
and public-boundary string check (`export::validate_emitted_string`) is now
public, so import and export share one policy.

## Registering an adapter

An adapter implements `conformance::Adapter` (`format`, `import`, and
optionally `export`) and adds itself to `conformance::registered()`. From
then on the corpus runs every package row of its format through the adapter
as well as through the shared intake. The adapter must give the same outcome
the row expects. An adapter that registers for a format also fails the corpus
test until each of that format's adapter rows has an executable arm in
`adapter_row` in `crates/bran-document/tests/conformance.rs`, so a registered
format cannot keep rows in the unavailable state.

For each row, `conformance::check` verifies:

1. The adapter does not panic (`catch_unwind`).
2. Two imports of the same bytes are identical (canonical bytes, receipt,
   anchors, or refusal).
3. A refusal row returns exactly the expected typed refusal. An admitted row
   carries every expected receipt code.
4. Every re-encoding of an admitted package (reversed entry order, other
   timestamps, the other compression method) gives identical canonical
   bytes. The fast tier supplies the re-encodings.
5. When `export` succeeds, re-importing the export yields the same anchors
   and a second export gives the same bytes. When `export` refuses, the
   outcome records `round_trip: false`; nothing is claimed.

## Fixture matrix

Package rows are built in memory from the synthetic `.parts` fixtures in
`fixtures/enterprise-documents/conformance/`. The fixture text is small and
reviewable; the test builds each ZIP, including the hostile ones. Every
package row runs for DOCX, XLSX, and PPTX. No fixture contains customer or
private enterprise data.

| Issue #25 fixture class | Package rows (run now) | Expected outcome |
|---|---|---|
| Ordinary documents with stable expected projections | `ordinary` | admitted; canonical package digest equals the recorded digest per format |
| Unsupported-but-benign features | none at package level | adapter rows below |
| Macro, script, active content | `macro-part`, `macro-enabled-main`, `ole-object` | `active-content` |
| External relationships and remote media | `external-media` | `external-reference` |
| | `external-hyperlink` | admitted with `hyperlink-not-fetched` |
| ZIP path escape | `zip-path-escape`, `zip-absolute-path`, `zip-backslash-path`, `relationship-escape` | `unsafe-part-path` |
| Duplicate parts | `duplicate-part`, `duplicate-part-case` | `duplicate-part` |
| Malformed XML | `malformed-xml`, `undeclared-entity` | `malformed-xml` |
| Entity expansion | `entity-expansion` | `xml-dtd-refused` |
| Decompression bombs | `decompression-bomb` | `decompression-limit` |
| | `bomb-declared-size-lie` | `malformed-container` after inflating at most the declared size plus one byte |
| Encrypted inputs | `encrypted-container`, `encrypted-entry` | `encrypted-or-legacy-container` |
| Signed inputs | `signed-package` | admitted with `signature-not-verified` |
| DLP canaries | `dlp-canary` | admitted with `dlp-findings` (the envelope then rejects admission) |
| Private/public-boundary violations | `public-boundary-marker` | admitted with `public-boundary-violation` |
| Oversized assets | `oversized-part`, `oversized-package` | `oversized` |
| Excessive parts or nodes | `too-many-parts`, `excessive-nodes` | `too-many-parts`, `xml-node-limit` |
| Deep nesting | `deep-nesting` | `xml-depth-limit` |
| Timeout and cancellation | `cancelled` | `cancelled` |
| Not an OPC package | `missing-content-types`, `not-a-zip` | `malformed-container` |
| Foreign-namespace attribute shadowing a policy attribute | `foreign-attribute-macro` | `active-content` |
| DLP canary in a part name or the content-types part | `canary-part-name`, `canary-content-types` | admitted with `dlp-findings` |
| Truncated or empty deflate stream | `truncated-deflate`, `empty-deflate-stream` | `malformed-container` |
| Local header that contradicts the central directory | `local-header-method` | `malformed-container` |
| | `local-header-encryption` | `encrypted-or-legacy-container` |
| XML characters and names outside XML 1.0 | `xml-invalid-character`, `xml-invalid-name` | `malformed-xml` |

Adapter rows need a content model, so they stay unavailable until the owning
adapter registers. The corpus test prints each one as `unavailable` with its
issue; it never counts them as passing. The DOCX rows are executable:
`docx-ordinary-projection` checks the representative fixture and its recorded
canonical digest, `docx-unsupported-benign-fidelity` requires the
`feature:unsupported` and `feature:normalized` receipt codes, and
`docx-round-trip-anchors` requires an export whose re-import keeps every
anchor.

| Row | Needs |
|---|---|
| `docx-ordinary-projection`, `docx-unsupported-benign-fidelity`, `docx-round-trip-anchors` | #21 (executable) |
| `xlsx-ordinary-projection`, `xlsx-unsupported-benign-fidelity`, `xlsx-round-trip-anchors` | #26 |
| `pptx-ordinary-projection`, `pptx-unsupported-benign-fidelity`, `pptx-round-trip-anchors` | #22 |
| `pdf-ordinary-projection`, `pdf-malformed-object-graph`, `pdf-recursive-structure`, `pdf-active-action`, `pdf-embedded-file`, `pdf-encrypted`, `pdf-signed`, `pdf-dlp-canary`, `pdf-oversized-stream`, `pdf-round-trip-anchors` | #23 |

PDF fixtures are not written yet. A hand-written PDF with no parser to check
it would be an untested fixture; #23 adds them with the parser.

## Timeouts are deterministic budgets

BRAN results must not depend on timing. No import outcome uses wall-clock
time. The work an import may do is bounded by byte, part, ratio, depth, and
node budgets (`Limits`), so a hostile input stops at the same point on every
machine. A caller that needs a time limit cancels through `Cancel`; the
result is the typed `cancelled` refusal with no partial output, and a
cancelled result is never admitted as evidence.

## Policy the intake applies

- ZIP: stored and deflate only; ZIP64, multi-disk, encryption flags in the
  central or local header, duplicate raw names, a local header whose name,
  method, data-descriptor flag, CRC, or sizes contradict the central record,
  overlapping entry data, declared-size mismatch, CRC mismatch, and trailing
  bytes are refused. A deflate stream must reach its end marker exactly at
  its last compressed byte; truncated, empty, or over-long streams are
  `malformed-container`.
- XML: characters and names must be XML 1.0 (`Char`, `Name`, at most one
  colon), comments must not contain `--`, text must not contain `]]>`, and
  attribute values must not contain `<`. Policy attributes are read by their
  exact unprefixed name, so `x:ContentType` in a foreign namespace cannot
  shadow `ContentType`.
- Part names: relative, no `.` or `..` segment, no empty segment, no
  backslash, no control character, no encoded `/` or `\`. Duplicates are
  compared ASCII case-insensitively, as OPC requires.
- Relationships: internal targets resolve against the source part and must
  stay inside the package; a target that does not exist is recorded as
  `missing-relationship-target`. External hyperlinks are recorded as
  `hyperlink-not-fetched` and never dereferenced. Every other external
  target is refused.
- Active content: VBA projects and data, ActiveX, OLE objects, XLM macro
  sheets, and macro-enabled main parts are refused by content type. XLSX
  data connections are refused as external references by content type.
  Power Query mashups sit in custom XML parts with a generic content type,
  so #26 must detect and refuse them.
- Budgets: `Limits::default()` allows a 20 MiB package, 16 MiB per part,
  128 MiB inflated in total, and a 100:1 compression ratio per part. Large
  spreadsheets can exceed the part budget; the refusal is typed and the
  caller can pass larger limits.
- Signatures: signature parts are kept as evidence with
  `signature-not-verified`; BRAN makes no trust claim.
- DLP: every original ZIP entry name and byte (the content-types part and
  directory entries included) and the canonical package bytes, which hold
  every emitted metadata string after entity normalization, are scanned for
  the synthetic canaries and the `important_boundary` marker with
  `bran-core`'s shared check. The raw byte scan cannot see a canary split
  across XML runs; each adapter must re-run the same check on its extracted
  text.

## Required properties

| Property | Evidence | Status |
|---|---|---|
| Same logical input, same canonical bytes and digest | check step 2; `ordinary` recorded digests; `canonical_json_matches_envelope_rule`; `docx-ordinary-projection` recorded digest | done for packages and DOCX; other adapters when they register |
| Archive order, timestamps, producer differences do not change the result | check step 4 on every admitted row | done for packages and DOCX |
| Every normalization, approximation, omission, or refusal in a versioned receipt | typed refusals and receipt codes (`RECEIPT_VERSION` 1); check step 3; DOCX `feature:status` codes (`docx::MODEL_VERSION` 1) | done for package outcomes and DOCX content; #22, #23, #26 pending |
| Citation anchors survive deterministic round trips | check step 5; `harness_rejects_round_trip_anchor_drift`; `docx-round-trip-anchors` | done for DOCX; other formats when their adapters export |
| No network requests, no active content executed | `importers_have_no_network_or_process_access`; active-content and external rows | done |
| Containment, DLP, classification, byte budgets on import and export | budget rows; `dlp-canary`; `export_gate_*` tests | done for the shared gates; classification is the envelope's `policy` block |
| Typed, bounded failures; no panic; no partial output | check step 1; `parser_limit_property`; refusal rows; export gate tests | done |
| Dependency and licence review before parser selection | `enterprise-document-dependency-review.md` | done |

## Property targets

`properties()` in `crates/bran-document/tests/conformance.rs` runs seeded,
deterministic property targets:

- package paths: random names never panic, and accepted names satisfy the
  part-name rules
- relationship resolution: every resolved target is itself a valid part name
- canonical serialization: output is UTF-8, has no raw control characters,
  and does not depend on key insertion order
- parser limits: mutated packages (bit flips, truncation, insertion) and
  random XML never panic, and admitted packages stay inside the budgets;
  `inflation_stops_one_byte_past_the_declared_size` shows a lying ZIP entry
  stops inflating one byte past its declared size

These are property targets, not coverage-guided fuzzing. A `cargo fuzz`
target needs a nightly toolchain and libFuzzer, which this repository does
not use.

## Fast and full split with budgets

| Budget | Fast | Full |
|---|---|---|
| Runner | `enterprise_conformance_fast` (workspace tests in `check.sh --fast`) | `enterprise_conformance_full` (ignored by default; `check.sh --full` runs it in the security gate) |
| Limits | small test limits (1 MiB package, 256 KiB part, depth 64, 10,000 nodes) | `Limits::default()` (20 MiB package, 16 MiB part, 128 MiB total, ratio 100, depth 256, 1,000,000 nodes) |
| Property iterations | 2,000 | 50,000 |
| Largest generated package | 2 MiB | 24 MiB |
| Runtime ceiling (debug build, whole tier) | 10 s | 120 s |
| Measured on 2026-09-30 (debug build, with the DOCX adapter) | 0.9 s, 162 checks | 29.8 s, 162 checks |
| Fixture file size | 8 KiB each | same files |

The budgets are constants at the top of the test file. A tier that runs
longer than its ceiling fails. The ceilings gate the suite, never an import
outcome.

## Compatibility table

Status values: exact, normalized, approximated, unsupported, refused.
`pending #N` means no adapter exists yet and nothing is claimed.

| Feature | DOCX | XLSX | PPTX | PDF |
|---|---|---|---|---|
| Package container | normalized (canonical inventory) | normalized | normalized | pending #23 |
| ZIP entry order, timestamps, compression | normalized away | normalized away | normalized away | n/a |
| Encrypted or legacy binary container | refused | refused | refused | pending #23 |
| Digital signature parts | preserved as evidence, trust not verified | same | same | pending #23 |
| Macros, VBA, XLM, ActiveX, OLE objects | refused | refused | refused | n/a |
| JavaScript, launch and other PDF actions | n/a | n/a | n/a | pending #23 |
| External relationships, remote media, external workbooks | refused | refused | refused | pending #23 |
| External hyperlinks | recorded, never fetched | recorded, never fetched | recorded, never fetched | pending #23 |
| DTDs and custom entities | refused | refused | refused | n/a |
| Text, structure, tables, lists | exact text; normalized headings, lists, tables, sections | pending #26 | pending #22 | pending #23 |
| Comments, tracked changes, notes | exact tracked run changes; normalized comments and notes; unsupported formatting changes | pending #26 | pending #22 | pending #23 |
| Hyperlinks and bookmarks | exact (external targets recorded, never fetched) | pending #26 | pending #22 | pending #23 |
| Fields and content controls | normalized (cached result kept, code dropped, never evaluated) | pending #26 | pending #22 | pending #23 |
| Headers and footers | unsupported | pending #26 | pending #22 | pending #23 |
| Formulas and cached values | n/a | pending #26 | n/a | n/a |
| Images and media | exact bytes, content-addressed; floating images normalized to inline | pending #26 | pending #22 | pending #23 |
| Charts, SmartArt, animations | unsupported | pending #26 | pending #22 | n/a |
| OCR text | n/a | n/a | n/a | pending #23 |
| Export | deterministic DOCX with fidelity receipt | pending #26 | pending #22 | pending #23 |

## Acceptance status for #25

| Item | Status | Evidence or owner |
|---|---|---|
| Fixture matrix, package classes | done | package rows above |
| Fixture matrix, PDF classes and unsupported-benign fidelity | DOCX done; others not done | adapter rows; #22, #23, #26 |
| Required properties | see table above | |
| CLI read-only inspection before export | not done | needs adapter output to inspect; #21, #22, #23, #26 |
| Query and packet select document evidence | not done | needs admitted envelopes from adapters, then an ingest path |
| Export needs explicit format and destination, never overwrites | done for the shared gate (`export::write_new`) | CLI wiring with the first adapter export |
| Adapter tests and shared corpus in the fast/full split with budgets | done | budget table above |
| Fuzz/property targets | done as seeded property targets | no coverage-guided fuzzing |
| Compatibility table | done for package features and DOCX | content rows pending #22, #23, #26 |
