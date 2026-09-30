---
type: Concept
title: Enterprise-document parser dependency and license review
okf_status: active
tags:
  - public
  - developer
freshness: "2026-09-30"
resource: https://github.com/alphazede/bran
public_boundary: public
---

# Enterprise-document parser dependency and license review

This review covers [issue #25](https://github.com/alphazede/bran/issues/25).
The issue requires a dependency and license review before BRAN selects
document parsers or renderers. The owner has approved native local adapters
([#20](https://github.com/alphazede/bran/issues/20) no longer gates them).
The review selects the approach for DOCX ([#21](https://github.com/alphazede/bran/issues/21)),
PPTX ([#22](https://github.com/alphazede/bran/issues/22)),
PDF ([#23](https://github.com/alphazede/bran/issues/23)), and
XLSX ([#26](https://github.com/alphazede/bran/issues/26)).

## Constraints

- `deny.toml` allows only `MIT` and `Apache-2.0` licences, crates.io only.
  An SPDX `OR` expression passes when one branch is allowed.
- Import runs offline, executes nothing, and must be deterministic.
- Parsers must refuse, not repair silently. A failure is typed and bounded.
- `bran-core` has no dependencies today. Keep it that way. Parser
  dependencies live in a separate crate, `bran-document`.
- Every new crate gets a supply-chain scan before merge.

## Method

Candidates were resolved with default features into a scratch manifest on
2026-09-29 and inspected from the downloaded sources. The two XML finalists
were also run against a nesting-depth probe, a DTD with an internal entity,
an undeclared entity, a mismatched end tag, and a truncated document. "Transitive" counts
normal dependencies with default features. "Unsafe" counts `unsafe` tokens in
`src/`, including comments. "Blocked licences" lists transitive licences that
`deny.toml` would reject with default features. Release dates come from each
crate's changelog where one ships in the package.

## ZIP container (DOCX, PPTX, XLSX)

| Candidate | Version | Licence | Finding |
|---|---|---|---|
| `zip` | 8.6.0 | MIT | Already in the workspace (`xtask`). `SharedBuilder::build` inserts central-directory entries into an `IndexMap` keyed by raw name, so a duplicate entry silently replaces the earlier one. The duplicate-part fixture row cannot be enforced through it. Default features add AES, bzip2, deflate64, LZMA, PPMd, XZ, and zstd. `unsafe` in `spec.rs` block reads. |
| BRAN-owned central-directory reader | n/a | MIT OR Apache-2.0 | About 210 lines, plus a 60-line deterministic writer (`crates/bran-document/src/zip.rs`). Stored and deflate only. Inflate uses `flate2` 1.1.9 with `rust_backend` (`miniz_oxide`, already in the workspace). CRC through `crc32fast` 1.5.0 (already in the workspace). |

Decision: the BRAN-owned reader. It sees every central-directory record, so
it can refuse duplicate names, overlapping entry data, local/central name
mismatches, declared-size lies, encryption, ZIP64, multi-disk archives, and
unknown compression methods. Each refusal is typed. `zip` stays in `xtask`
for release packaging.

## XML (OOXML parts)

| Candidate | Version | Licence | Transitive | Unsafe | Released | Finding |
|---|---|---|---|---|---|---|
| `roxmltree` | 0.21.1 | MIT OR Apache-2.0 | 1 (`memchr`, already in the workspace) | 0, `forbid(unsafe_code)` | 2025-10-09 | Read-only tree. `allow_dtd` defaults to `false`: any DTD fails with `DtdDetected`. Entity reference depth is capped at 10. `nodes_limit` bounds node count. About 5,400 lines. Recursive descent: deep nesting aborts the process (measured below). Rejected. |
| `quick-xml` | 0.42.0 | MIT | 1 (`memchr`) | `forbid(unsafe_code)` | no changelog shipped | Streaming reader and writer. Does not expand custom entities; the caller must refuse `DocType` itself. About 35,000 lines including serde support. |
| `xml-rs` | 1.0.0 | MIT | 1 (`xml`) | 0 | no changelog shipped | 1.0.0 is a re-export shim over the `xml` crate. Slower; entity handling must be audited through the second crate. |

Measured on 2026-09-29 with the probe described under Method: `roxmltree`
0.21.1 recurses once per nested element. A document of `<a>` repeated
5,000 times inside itself parsed in a release build; 20,000 levels aborted
the process with a stack overflow, and a debug build aborted at 1,000. A
stack overflow is an abort, not a typed error, so no BRAN limit applied
after parsing can catch it. `quick-xml` 0.42.0 read 1,000,000 nested levels
without recursion, reported `<!DOCTYPE ...>` as a `DocType` event, reported
an undeclared `&a;` as a `GeneralRef` event, rejected mismatched end tags,
and ended a truncated document at `Eof` with open elements still counted.

Decision: `quick-xml` 0.42.0 with default features off (no serde, no
`encoding_rs`). BRAN's shared reader (`bran-document::xml`) refuses any
`DocType` event, so neither internal-entity expansion (billion-laughs) nor
external entities are reachable. It refuses every entity reference except
the five predefined ones and character references, counts depth and nodes
itself, and treats `Eof` with open elements as malformed. OOXML never needs
a DTD, so refusing one loses no supported content. The same crate's writer
is available to adapters for export; no second XML dependency is needed.

## Format-level OOXML crates (rejected)

| Candidate | Version | Licence | Transitive | Blocked licences | Reason rejected |
|---|---|---|---|---|---|
| `calamine` | 0.36.1 | MIT | 35 | `zlib-rs` (Zlib) | XLSX read-only. Opens packages through `zip`, so duplicate parts collapse before BRAN sees them. Its own package policy bypasses the shared intake. |
| `docx-rs` | 0.4.22 | MIT | 60 | `zlib-rs` (Zlib) | Pulls `wasm-bindgen`, `ts-rs`, `image`, and `serde_json`. Opens packages through `zip`. |
| `rust_xlsxwriter` | 0.99.1 | MIT OR Apache-2.0 | 16 | `zlib-rs` (Zlib) | Write-only, about 112,000 lines. Useful export model but duplicates the shared canonical writer the adapters need anyway. |

No maintained PPTX crate was found worth evaluating. Decision: DOCX, PPTX,
and XLSX adapters map OOXML parts themselves on top of the shared intake
(`bran-document::opc`) and the shared XML reader (`bran-document::xml`). Each adapter owns only its
content model, not package safety.

## PDF import ([#23](https://github.com/alphazede/bran/issues/23))

| Candidate | Version | Licence | Transitive | Unsafe | Blocked licences | Finding |
|---|---|---|---|---|---|---|
| `hayro-syntax` | 0.7.2 | Apache-2.0 OR MIT | 7 | only behind the optional `unsafe` feature (plus `smallvec`) | none | Lazy object resolution and cross-reference repair for damaged files. About 14,000 lines. |
| `lopdf` | 0.45.0 | MIT | 89 | 0, `forbid(unsafe_code)` | `alloc-no-stdlib`, `alloc-stdlib` (BSD-3-Clause), `zlib-rs` (Zlib) | Loads the whole document eagerly. Released 2026-07-10 (0.44.0) and later. Needs feature trimming to pass `deny.toml`. |
| `pdf` (pdf-rs) | 0.10.0 | MIT | 76 | 6 tokens, `memmap2` | `adler32`, `foldhash` (Zlib) | Memory-mapped input; blocked licences. |
| `pdfium-render` | not fetched | MIT OR Apache-2.0 | n/a | FFI | n/a | Needs the PDFium shared library, which is not installed and is C++. Rejected. |
| MuPDF, Poppler bindings | not fetched | AGPL / GPL | n/a | FFI | licence | Rejected by `deny.toml`. |

Decision: `hayro-syntax` without the `unsafe` feature is the selected PDF
import parser, subject to two checks #23 must record before adding it: the
dependency closure with the features #23 enables passes `cargo deny check
licenses bans sources`, and the nesting probe that rejected `roxmltree`
(deeply nested arrays and dictionaries) ends in a typed error, not an
abort. BRAN wraps it with its own object-count, recursion,
stream-expansion, and page limits; the parser's repair of damaged
cross-reference tables must surface as a fidelity diagnostic, never as
silent success.

### Checks recorded by #23 (2026-09-30)

`hayro-syntax` 0.7.2 with `default-features = false, features = ["std"]`
resolves to two more crates, `rustc-hash` 2.1.3 and `smallvec` 1.16.2, both
MIT OR Apache-2.0, so the licence check would pass. The nesting and
expansion checks do not:

- A page whose content object is `[` nested N times, read through
  `Pdf::new` and `XRef::get` in a debug build on an 8 MiB main-thread stack:
  100, 1,000, and 10,000 levels parsed; 100,000 levels aborted the process
  with `fatal runtime error: stack overflow`. `Array::skip` recurses once
  per level and has no depth limit. Test threads have 2 MiB stacks, so the
  threshold there is lower.
- A Flate content stream of 194,266 bytes decoded through `Stream::decoded`
  to 200,000,000 bytes. The decoder takes no output limit, and `Pdf::new`
  and `XRef::get` decode cross-reference and object streams internally
  before BRAN sees them, so BRAN cannot bound that expansion by wrapping the
  crate.

An abort is not a typed error, and the expansion happens inside the crate,
so the condition above is not met and `hayro-syntax` is not added. As with
the ZIP reader, BRAN reads PDF syntax itself (`crates/bran-document/src/pdf_syntax.rs`):
a lexer, objects with a depth and node limit, cross-reference tables and
streams, object streams, header-scan repair reported as `xref-repaired`, and
Flate through the workspace's existing `flate2` with ratio, part, and total
limits. The full tier parses 1,000,000 nested levels to a typed
`pdf-depth-limit` refusal. `lopdf` and `pdf` (pdf-rs) stay rejected for the
reasons in the table.

## PDF export ([#23](https://github.com/alphazede/bran/issues/23))

| Candidate | Version | Licence | Transitive | Unsafe | Blocked licences | Finding |
|---|---|---|---|---|---|---|
| `pdf-writer` | 0.15.0 | MIT OR Apache-2.0 | 4 | 0, `forbid(unsafe_code)` | none | Low-level writer. Tagged structure trees are possible but BRAN must build them. |
| `krilla` | 0.8.2 | MIT OR Apache-2.0 | 68 | 0, `forbid(unsafe_code)` | `arrayref` (BSD-2-Clause), `tiny-skia-path` (BSD-3-Clause), `yoke`, `zerofrom` (Unicode-3.0), `zlib-rs` (Zlib) | High-level tagged PDF with PDF/UA and PDF/A validation. |

Decision: `pdf-writer` 0.15.0. `krilla` gives more accessibility machinery,
but its closure needs BSD, Unicode-3.0, and Zlib licences that `deny.toml`
rejects. Widening the licence policy is an owner decision, not an adapter
detail. If #23 cannot meet the accessible-export acceptance with
`pdf-writer`, it stops and asks for that decision. Any PDF/A claim needs an
independent validator either way.

#23 did not need `pdf-writer` either. The export is one font, one content
stream per page, and a structure tree with one element per block; the
adapter writes those objects directly in about 200 lines, deterministically,
and poppler, Ghostscript, and LibreOffice open the result
([`enterprise-document-pdf-adapter.md`](enterprise-document-pdf-adapter.md)).
Add `pdf-writer` when an export needs embedded fonts, images, or annotations.

## Selected approach

| Format | Container | Parser | Export | New crates |
|---|---|---|---|---|
| DOCX | BRAN-owned ZIP reader | `quick-xml` | BRAN-ordered XML + shared ZIP writer | `quick-xml` |
| PPTX | BRAN-owned ZIP reader | `quick-xml` | BRAN-ordered XML + shared ZIP writer | `quick-xml` |
| XLSX | BRAN-owned ZIP reader | `quick-xml` | BRAN-ordered XML + shared ZIP writer | `quick-xml` |
| PDF | n/a | BRAN-owned reader (`hayro-syntax` failed the nesting and expansion checks) | BRAN-written tagged PDF | none |

This issue adds only `quick-xml` 0.42.0. `flate2`, `crc32fast`, and `memchr`
are already locked in the workspace. #23 adds no crate.

## Re-review triggers

Repeat this review when a selected crate changes major version, when a new
format or renderer is proposed, when `cargo deny` reports a new licence in
the closure, or when an adapter needs a feature this review rejected.
