---
type: Concept
title: Enterprise-document conformance suite
okf_status: active
tags:
  - public
  - developer
freshness: "2026-09-30"
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
test until `adapter_row` in `crates/bran-document/tests/conformance.rs`
implements every row of that format, so a registered format cannot keep rows
in the unavailable state. The XLSX adapter (#26,
[`enterprise-document-xlsx.md`](enterprise-document-xlsx.md)) is registered.

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
issue; it never counts them as passing.

| Row | Needs |
|---|---|
| `docx-ordinary-projection`, `docx-unsupported-benign-fidelity`, `docx-round-trip-anchors` | #21 |
| `pptx-ordinary-projection`, `pptx-unsupported-benign-fidelity`, `pptx-round-trip-anchors` | #22 |

XLSX adapter rows run now against the registered adapter:

| Row | Checks |
|---|---|
| `xlsx-ordinary-projection` | the ordinary workbook's grid projection equals the recorded digest and has anchors |
| `xlsx-unsupported-benign-fidelity` | `xlsx-features.parts` is admitted, re-encoding invariant, with `unsupported-chart`, `unsupported-image`, `unsupported-drawing`, `unsupported-conditional-formatting`, `rich-text-flattened`, `formula-cached-result-not-recalculated`, and `hyperlink-not-fetched` |
| `xlsx-round-trip-anchors` | `xlsx-features.parts` exports, re-imports with identical anchors, and exports the same bytes twice (`round_trip: true`) |

The PDF adapter (#23) is registered, so its rows are executable: 27 rows,
including `pdf-ordinary-projection`, `pdf-malformed-object-graph`,
`pdf-recursive-structure`, `pdf-active-action`, `pdf-embedded-file`,
`pdf-encrypted`, `pdf-signed`, `pdf-dlp-canary`, `pdf-oversized-stream`, and
`pdf-round-trip-anchors`. They run in `crates/bran-document/tests/pdf.rs`
through `conformance::check`, from the synthetic `pdf-base.objects` fixture,
with their own fast and full budgets. See
[`enterprise-document-pdf-adapter.md`](enterprise-document-pdf-adapter.md).

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
  so the XLSX adapter (#26) detects and refuses them.
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
| Same logical input, same canonical bytes and digest | check step 2; `ordinary` recorded digests; `canonical_json_matches_envelope_rule`; `xlsx-ordinary-projection` | done for packages and XLSX; other adapters when they register |
| Archive order, timestamps, producer differences do not change the result | check step 4 on every admitted row | done for packages |
| Every normalization, approximation, omission, or refusal in a versioned receipt | typed refusals and receipt codes (`RECEIPT_VERSION` 1); check step 3 | done for package outcomes, XLSX and PDF content; DOCX and PPTX content fidelity needs #21, #22 |
| Citation anchors survive deterministic round trips | check step 5; `harness_rejects_round_trip_anchor_drift`; `xlsx-round-trip-anchors`; `pdf-round-trip-anchors` | done for XLSX and PDF; DOCX, PPTX when their adapters export |
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
| Measured on 2026-09-30 (debug build, shared intake only) | 0.8 s, 120 checks | 23.7 s, 120 checks |
| Measured on 2026-09-30 (debug build, XLSX adapter registered) | 0.8 s, 162 checks | 28.6 s, 162 checks |
| Fixture file size | 8 KiB each | same files |

The budgets are constants at the top of the test file. A tier that runs
longer than its ceiling fails. The ceilings gate the suite, never an import
outcome.

## Compatibility table

Status values: exact, normalized, approximated, unsupported, refused.
`pending #N` means no adapter exists yet and nothing is claimed.

| Feature | DOCX | XLSX | PPTX | PDF |
|---|---|---|---|---|
| Package container | normalized (canonical inventory) | normalized | normalized | normalized (object order, object streams, cross-reference form, compression, incremental updates) |
| ZIP entry order, timestamps, compression | normalized away | normalized away | normalized away | n/a |
| Encrypted or legacy binary container | refused | refused | refused | refused (any `/Encrypt`) |
| Digital signature parts | preserved as evidence, trust not verified | same | same | same |
| Macros, VBA, XLM, ActiveX, OLE objects | refused | refused | refused | n/a |
| JavaScript, launch and other PDF actions | n/a | n/a | n/a | refused |
| External relationships, remote media, external workbooks | refused | refused | refused | refused (remote go-to, URL file specifications, external streams) |
| External hyperlinks | recorded, never fetched | recorded, never fetched | recorded, never fetched | recorded, never fetched |
| DTDs and custom entities | refused | refused | refused | n/a |
| Text, structure, tables, lists | pending #21 | exact values and tables; rich text and styles normalized | pending #22 | text normalized; tables unsupported |
| Comments, tracked changes, notes | pending #21 | legacy comments exact; threaded comments unsupported | pending #22 | annotations normalized |
| Formulas and cached values | n/a | text exact, kept apart, never recalculated; active formulas quarantined | n/a | n/a |
| Images and media | pending #21 | unsupported, typed relationship with digest | pending #22 | image locators approximated; pixels not decoded |
| Charts, SmartArt, animations | pending #21 | unsupported, typed relationship with digest | pending #22 | n/a |
| Power Query, external workbook links, query tables | n/a | refused | n/a | n/a |
| OCR text | n/a | n/a | n/a | unsupported (no engine; requests reported unavailable) |
| Export | pending #21 | deterministic, with fidelity receipt; DLP first | pending #22 | tagged PDF; active content removed; no PDF/A or PDF/UA claim |

## Acceptance status for #25

| Item | Status | Evidence or owner |
|---|---|---|
| Fixture matrix, package classes | done | package rows above |
| Fixture matrix, PDF classes | done | PDF rows in `tests/pdf.rs` (#23) |
| Fixture matrix, unsupported-benign fidelity | not done | adapter rows; #21, #22, #26 |
| Required properties | see table above | |
| CLI read-only inspection before export | not done | needs adapter output to inspect; #21, #22, #23, #26 |
| Query and packet select document evidence | not done | needs admitted envelopes from adapters, then an ingest path |
| Export needs explicit format and destination, never overwrites | done for the shared gate (`export::write_new`) | CLI wiring with the first adapter export |
| Adapter tests and shared corpus in the fast/full split with budgets | done | budget table above |
| Fuzz/property targets | done as seeded property targets | no coverage-guided fuzzing |
| Compatibility table | done for package features | content rows pending the adapters |
