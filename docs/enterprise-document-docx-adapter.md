---
type: Concept
title: DOCX adapter
okf_status: active
tags:
  - public
  - developer
freshness: "2026-09-30"
resource: https://github.com/alphazede/bran
public_boundary: public
---

# DOCX adapter

This is the native DOCX adapter for
[issue #21](https://github.com/alphazede/bran/issues/21). It lives in
`crates/bran-document/src/docx.rs`, is registered with the shared
conformance harness from
[`enterprise-document-conformance.md`](enterprise-document-conformance.md),
and uses only the parser selected in
[`enterprise-document-dependency-review.md`](enterprise-document-dependency-review.md)
(`quick-xml` through the shared `xml` reader). It adds no dependency and does
not need Microsoft Word, LibreOffice, or a network service.

## Boundary

- Input is an untrusted OPC package. `opc::open` owns package safety: path
  escape, duplicate parts, bombs, budgets, macro and OLE content types,
  external relationships other than hyperlinks, DTDs, and entities. The
  adapter adds its own refusals only for the parts it reads.
- The shared XML reader validates the part before `quick-xml` resolves each
  element and attribute through its in-scope namespace bindings. Transitional
  and Strict WordprocessingML accept arbitrary prefixes and default element
  namespaces; unprefixed attributes have no namespace. Foreign elements never
  become Word content, and foreign roots are refused.
- Cumulative run payload and metadata copies (including transient templates)
  are bounded by `Limits::max_total_bytes`. The importer returns `oversized`
  before a copy would exceed that budget, including repeated tracked-change
  authors and hyperlink targets.
- Markup compatibility alternatives (`mc:AlternateContent`) in a read part
  are explicitly refused. The adapter does not claim to select an eligible
  `Choice` or `Fallback`.
- Nothing is fetched or executed. Fields keep their cached result and their
  code is dropped. Hyperlink targets are recorded, never dereferenced.
- Content the model does not carry is skipped and recorded in the fidelity
  map. It is never silently kept or silently dropped.

| Input | Refusal |
|---|---|
| Main part is not WordprocessingML (for example a workbook) | `unsupported-container` |
| A read part has a foreign root namespace or `mc:AlternateContent` | `unsupported-container` |
| Cumulative run payload and metadata copies exceed `Limits::max_total_bytes` | `oversized` |
| Undeclared prefixes or duplicate attributes with the same expanded name | `malformed-xml` |
| No `officeDocument` relationship, missing main part, or no `body` | `malformed-container` |
| Malformed XML or an undeclared entity in any part the adapter reads | `malformed-xml` |
| A DTD in any part the adapter reads | `xml-dtd-refused` |
| XML nodes or depth over the caller's `Limits` | `xml-node-limit`, `xml-depth-limit` |
| Tables or block content controls nested more than 32 deep | `xml-depth-limit` |

## Content model

`docx::read` returns a `Document`: the `Content` model, a `fidelity` map, and
the package `diagnostics`. The model holds body blocks, footnotes, endnotes,
comments, and image assets keyed by SHA-256.

- Blocks are paragraphs and tables, in body order. A paragraph whose
  properties hold a `sectPr` ends a section (`section_break`).
- A paragraph kind is `body`, `title`, `caption`, `heading` (level 1 to 9,
  from the style name or outline level through `basedOn`), or `list`
  (numbering id, level 0 to 8, and number format, from direct or style
  numbering).
- Table cells keep their column span, vertical merge state, and blocks,
  including nested tables.
- Inlines are runs, comment range markers, and bookmarks. A run holds text,
  an image, or a footnote, endnote, or comment reference, plus bold, italic,
  underline, an optional hyperlink (external target or internal bookmark),
  and an optional tracked change (insert, delete, move from, move to, with id,
  author, and date).

Tracked changes stay as tracked changes. Import accepts and rejects nothing.

The canonical bytes (`Document::canonical`) are the envelope's canonical JSON
of the model, the citation anchors, the fidelity map, and the diagnostics.
Image bytes are carried base64-encoded, so an export needs nothing but these
bytes. `Document::from_canonical` refuses input that does not re-serialize to
the same bytes or whose assets do not match their digests.

## Citation anchors

| Unit | Id | Locator (`section`, `ordinal`) | Role |
|---|---|---|---|
| Body block | `docx:s{section}:b{ordinal}` | `s1`, `s2`, ...; 1-based within the section | `title`, `heading`, `list`, `paragraph`, or `table` |
| Footnote, endnote | `docx:footnote:{id}`, `docx:endnote:{id}` | `footnotes` or `endnotes`; note order | `paragraph` |
| Comment | `docx:comment:{id}` | `comments`; comment order | `paragraph` |

Anchor text is the citable view: inserted and moved-in text is included,
deleted and moved-away text is not. Table text joins cells with tabs and rows
with newlines. Blocks with no text have no anchor but keep their ordinal. The
roles and locators are the envelope's `flow` family. The adapter returns each
anchor's id and SHA-256 text digest to the harness.

## Fidelity receipt

The fidelity map records every observed feature with the worst status seen.
The adapter's receipt codes are the package diagnostics plus one
`feature:status` code per entry. `MODEL_VERSION` (`1`) versions the model,
this vocabulary, and the export receipt.

| Feature | Status | Meaning |
|---|---|---|
| `tracked-changes` | exact | run insertions, deletions, and moves kept with id, author, date; `approximated` when changes nest |
| `hyperlinks`, `bookmarks`, `images` | exact | targets, names, image bytes and extents kept; `normalized` when a target, bookmark end, or extent was missing or duplicated |
| `headings`, `lists`, `captions`, `styles` | normalized | kind, level, numbering id, and format kept; style formatting, bullet glyphs, and start values dropped |
| `tables`, `sections` | normalized | cells, spans, merges, and section breaks kept; widths, borders, and page setup dropped |
| `footnotes`, `endnotes`, `comments` | normalized | ids, text, author, date, initials kept; note and comment formatting dropped |
| `fields`, `content-controls` | normalized | cached field result kept, field code dropped and not evaluated; content controls unwrapped |
| `run-formatting`, `paragraph-formatting`, `breaks`, `text` | normalized | only bold, italic, underline kept; page and column breaks become line breaks; XML-invalid characters dropped |
| `floating-images`, `svg-images`, `references` | normalized | anchored images inlined; SVG kept as its raster fallback; references to missing notes or comments dropped |
| `charts`, `smartart`, `shapes`, `vml`, `math`, `symbols`, `embedded-objects`, `embedded-documents` | unsupported | skipped |
| `headers-footers`, `document-properties`, `comment-threads`, `digital-signatures` | unsupported | parts not read |
| `formatting-changes`, `paragraph-mark-changes`, `table-row-changes`, `table-cell-changes` | unsupported | tracked formatting and structure changes not kept |
| `unrecognized-content`, `unrecognized-parts` | unsupported | anything else, skipped |

The adapter re-runs the shared DLP and public-boundary check on the text it
extracts, joined per paragraph. A marker split across runs, which the package
byte scan cannot see, adds `dlp-findings` or `public-boundary-violation`.

## Export

`docx::export` writes the model as a DOCX and returns the bytes with a
fidelity receipt. `docx::export_file` writes through the shared gate
(`export::write_new`): an explicit contained `.docx` destination, never
overwritten. Both refuse with `dlp-findings` or `public-boundary-violation`
before any bytes are built when the import receipt, the canonical
diagnostics, or any emitted string carries a finding, so no file is written.

The export is deterministic: parts are sorted, every entry has the same
timestamp, and styles and numbering are regenerated from the model. Importing
an export gives the same model, the same anchors, and, exported again, the
same bytes. Entries whose deflate representation would exceed the default
intake compression ratio are stored instead. Attribute tabs, newlines, and
carriage returns are escaped as character references, preserving metadata
through XML attribute normalization.

The receipt is canonical JSON with `schema_version`, `format`,
`source_canonical_sha256`, `output_sha256`, `output_byte_length`,
`import_fidelity` (the fidelity map), `export_fidelity` (`model` exact;
`styles`, `numbering`, `sections`, `tables`, and `run-formatting`
normalized), and the exported anchor ids.

## Independent readers

`docx_export_opens_in_independent_readers` is an ignored test. It exports the
representative fixture and opens it with python-docx and with LibreOffice
(converted to PDF, read back with `pdftotext`). A missing reader is reported
as unavailable, and fewer than two readers fails the test. Set
`BRAN_READER_DIR` to a directory the readers may use and
`BRAN_READER_PYTHON` to a Python with python-docx, then run
`cargo test -p bran-document --test docx -- --ignored`. The normal gate never
runs it, and nothing shipped depends on these readers.

Recorded on 2026-09-30: python-docx 1.2.0 read 11 paragraphs with their
styles, the table, one inline image, and two sections. LibreOffice 26.2.5.2
converted it to a two-page PDF with the lists, table, tracked changes,
footnote, and endnote. Microsoft Word was not available and was not tested.

## Tests

- `crates/bran-document/tests/docx.rs`: structure, review state, anchors,
  re-encoding invariance, unsupported features, hostile packages, round trip,
  export receipt, canonical tampering, DLP before write, and no overwrite.
- `crates/bran-document/tests/conformance.rs`: every package row through the
  adapter, plus the executable `docx-ordinary-projection` (recorded digest),
  `docx-unsupported-benign-fidelity`, and `docx-round-trip-anchors` rows.
- Fixtures: `fixtures/enterprise-documents/docx/representative.parts` and
  `unsupported-benign.parts`, synthetic, with the one-pixel PNG inline in the
  tests. The `docx/review/` text fixtures hold the original synthetic reviewer
  packages and memory-ceiling controls as deflate bytes in hex, with original
  lengths and SHA-256 digests checked after decoding. The shape fixture
  uses a direct drawing; compatibility wrappers have a separate refusal test.

## Not done here

- Microsoft Word itself was not run.
- Headers, footers, charts, SmartArt, shapes, text boxes, and math are
  reported unsupported, not imported.
- Emitting the V1 evidence envelope and the read-only CLI inspection are
  shared follow-up work from #25.
