---
type: Concept
title: Enterprise-document PDF adapter
okf_status: active
tags:
  - public
  - developer
freshness: "2026-09-30"
resource: https://github.com/alphazede/bran
public_boundary: public
---

# Enterprise-document PDF adapter

This is the PDF adapter for [issue #23](https://github.com/alphazede/bran/issues/23).
It registers with the shared harness from
[issue #25](https://github.com/alphazede/bran/issues/25)
([`enterprise-document-conformance.md`](enterprise-document-conformance.md)),
so the same determinism, fidelity, DLP, and parser-safety checks apply. PDF is
the fixed-layout family of the
[evidence envelope](enterprise-document-evidence-envelope.md). It stays
distinct from flow documents, spreadsheets, and presentations: BRAN never
treats a PDF as editable Word content.

## Layout

| Module | Owns |
|---|---|
| `bran-document::pdf_syntax` | Bounded PDF object reader: lexer, objects, cross-reference tables and streams, object streams, repair, Flate with PNG predictors. |
| `bran-document::pdf_text` | Content-stream interpretation: text runs, fonts, ToUnicode maps, encodings, marked content, images, form XObjects. |
| `bran-document::pdf` | The adapter: page tree, hazards, annotations, outline, forms, signatures, attachments, DLP, projection, anchors, export. |

The reader is BRAN-owned. `hayro-syntax` was selected by #25 subject to a
nesting check; it failed that check, so no PDF crate is added. The
[dependency review](enterprise-document-dependency-review.md) records the
measurements. The adapter adds no dependency: it uses `flate2`, which the
workspace already locks.

## Projection and locators

Import returns canonical JSON (`bran-pdf-projection-v1`, the envelope's
canonical byte rule), the sorted receipt codes, and one anchor per text block.
The projection does not contain the file's own digest: re-encodings of one
document must project identically, and the caller already holds the bytes.

| Locator | Form | Stable across |
|---|---|---|
| Text block | `pdf:p{page}:b{block}` | object order, object streams, cross-reference form, compression, incremental updates, producer and dates |
| Image | `pdf:p{page}:i{image}` | same |
| Annotation | `pdf:p{page}:a{annotation}` | same |

Each block records its bounding box in PDF user space before `/Rotate`
(rounded to whole points), its role (`heading` when its marked-content tag
is `H`, `H1`...`H6`, or `Title`, otherwise `paragraph`), its text, the text
digest, and `derivation`. Each page records its media box, rotation, the
digest of its decoded content streams (the source-page digest an OCR engine
would cite), `text_source`, and reading order with provenance and
confidence (`content-stream-order`, `low`).

Blocks follow marked-content sequences (`MCID`) when present, otherwise text
objects (`BT`...`ET`); a gap of more than one and a half lines also starts a
block. Line and word breaks are measured along each run's baseline, so rotated
text joins like upright text. `/ActualText` replaces the glyphs it covers, as
readers do.

## Capability matrix

Status values follow the compatibility table in the conformance document:
exact, normalized, approximated, unsupported, refused.

| Input | Status | What BRAN records | Evidence |
|---|---|---|---|
| Text PDFs | normalized | page order, box, rotation; blocks with text, role, bounding box (approximated), derivation; images as figure locators | `pdf-ordinary-projection`, `ordinary_projection_has_stable_locators` |
| Scanned PDFs (image-only pages) | unsupported text | `text_source: none`, image locators, receipt `ocr-not-run`; no text is invented | `pdf-scanned-page`, `ocr_is_opt_in_and_never_faked` |
| Forms (AcroForm) | normalized | full field name, type, value; never submitted | `pdf-form` |
| XFA forms | refused | `active-content` (XFA can carry scripts) | `pdf-xfa-form` |
| Annotations | normalized | subtype, rectangle, contents, internal target page or named target | `pdf-ordinary-projection` |
| Links to URIs | normalized | URI with `fetched: false`, receipt `hyperlink-not-fetched`; never dereferenced | `pdf-uri-link` |
| Bookmarks (outline) | normalized | title, level, target page | `pdf-ordinary-projection` |
| Metadata | normalized | title, author, subject, keywords, language; producer and dates are excluded so they cannot change the projection | `pdf-ordinary-projection`, reordered re-encoding |
| Attachments (embedded files) | unsupported content | name, declared size, `extracted: false`, receipt `embedded-file-not-extracted`; bytes are never extracted or opened | `pdf-embedded-file` |
| Encryption, password protection | refused | `encrypted-or-legacy-container` for any `/Encrypt`, including an empty user password | `pdf-encrypted` |
| Digital signatures | evidence only | field, filter, subfilter, byte range, contents digest, whether the byte range covers the file, `trust: not-verified`, receipt `signature-not-verified`; no trust claim | `pdf-signed` |
| Damaged cross-reference tables | normalized with receipt | table rebuilt from object headers, receipt `xref-repaired`; never silent | `pdf-damaged-xref` |
| Wrong stream lengths | normalized with receipt | end found from `endstream`, receipt `stream-length-repaired` | `pdf-stream-length-mismatch` |
| Missing referenced objects | normalized with receipt | treated as null (PDF rule), receipt `dangling-reference` | `pdf-dangling-reference` |
| Malformed page tree or object graph | refused | `malformed-pdf` | `pdf-malformed-object-graph`, `pdf-recursive-structure`, `pdf-not-a-pdf` |
| JavaScript, launch, submit, import, media, 3D, embedded-document actions and annotations | refused | `active-content`, wherever the object sits in the file, reachable or not | `pdf-active-action`, `pdf-launch-action` |
| Remote go-to actions, URL file specifications, external streams, reference XObjects | refused | `external-reference` | `pdf-remote-goto` |
| Fonts without a Unicode mapping | approximated | U+FFFD for each unmapped glyph, receipt `text-unmapped-glyphs`; glyphs are not guessed | `pdf-unmapped-glyphs` |
| Unsupported stream filters (LZW, ASCII85, image codecs) | unsupported | receipt `stream-not-decoded`; images are never decoded | none (no fixture uses them) |
| Tables | unsupported | text only | none |
| OCR text | unsupported | see OCR below | `ocr_is_opt_in_and_never_faked` |

## Safety limits

The adapter reuses the shared `Limits`. Every limit counts bytes, objects, or
nesting, never time.

| Limit | PDF meaning | Refusal |
|---|---|---|
| `max_package_bytes` | input file size | `oversized` |
| `max_parts` | page count | `pdf-page-limit` |
| `max_part_bytes` | one stream, raw or decoded | `oversized` / `decompression-limit` |
| `max_ratio` | decoded/raw ratio per Flate stream | `decompression-limit` |
| `max_total_bytes` | all decoded bytes plus all executed content bytes, so repeated form XObjects cannot amplify work | `decompression-limit` |
| `max_xml_depth` | nesting of arrays and dictionaries, page tree, outline, form fields, `q`, marked content, and form XObjects | `pdf-depth-limit` |
| `max_xml_nodes` | parsed objects and cross-reference entries | `pdf-object-limit` |

Cancellation is checked per object, page, and content stream and returns
`cancelled` with no partial output. Nothing is executed, fetched, decrypted,
or rendered, and the static audit in the conformance suite confirms the crate
has no network or process access.

## OCR

OCR is opt-in (`pdf::Options { ocr: true }`). No OCR engine ships with BRAN
and none is installed in the reference environment, so a request is recorded
as `ocr: {requested: true, effective: unavailable}` with receipt
`ocr-unavailable`, and nothing is recognised. Without a request the projection
says `ocr: {requested: false, effective: not-run}`. Every block carries
`derivation: embedded-text`, and every export receipt repeats each block's
derivation, so OCR text can never be mistaken for byte-derived text. An OCR
engine, when one is approved, must add blocks with `derivation: ocr` plus
engine identity, settings, confidence, and the page's `content_digest`.

## Export

`pdf::export` writes a new PDF from the projection alone, so actions,
JavaScript, annotations, attachments, forms, and signatures cannot survive
it. It refuses first, before building anything, when the import carries
`dlp-findings` or `public-boundary-violation`, or when any emitted text fails
the shared DLP and public-boundary check. Writing goes through the shared
gate (`export::write_new`): explicit format and destination, DLP again, and
never overwrite.

The export is tagged: `/MarkInfo << /Marked true >>`, a structure tree
(`Document` with an `H1` or `P` element per block, marked content with
`MCID`s, a parent tree), `/Tabs /S`, `/Lang` when the source has one, and the
title with `/DisplayDocTitle`. Text uses Helvetica with WinAnsiEncoding; text
with no WinAnsi code is refused with `export-unsupported`, never replaced.

The export receipt (canonical JSON) records each block's id, role, and
derivation, the features removed (annotations, attachments, forms, images,
outline, signatures), fidelity, the projection digest, and
`claims: {pdf_a: not-claimed, pdf_ua: not-claimed}`. No PDF/A or PDF/UA
validator (veraPDF) is available here, so no conformance level is claimed.
Helvetica is not embedded, which PDF/UA would also require.

Re-importing an export yields the same anchors, and a second export is
byte-identical (`pdf-round-trip-anchors`, harness check step 5).

## Corpus rows

Rows are built in memory from the synthetic
`fixtures/enterprise-documents/conformance/pdf-base.objects` fixture. Every
admitted row also runs three logically equal re-encodings: reversed object
order with another producer and date, a PDF 1.5 layout with an object stream
and a predictor-encoded cross-reference stream, and an incremental update.

| Row | Expected outcome |
|---|---|
| `pdf-ordinary-projection` | admitted; projection digest equals the recorded digest |
| `pdf-round-trip-anchors` | admitted; export, re-import, and anchors unchanged |
| `pdf-damaged-xref` | admitted with `xref-repaired` |
| `pdf-stream-length-mismatch` | admitted with `stream-length-repaired` |
| `pdf-malformed-object-graph` | `malformed-pdf` (a font where a page belongs) |
| `pdf-recursive-structure` | `malformed-pdf` (page tree contains itself) |
| `pdf-dangling-reference` | admitted with `dangling-reference` |
| `pdf-deep-nesting` | `pdf-depth-limit` (1,000,000 levels in the full tier) |
| `pdf-excessive-objects` | `pdf-object-limit` |
| `pdf-too-many-pages` | `pdf-page-limit` |
| `pdf-active-action` | `active-content` (JavaScript open action) |
| `pdf-launch-action` | `active-content` |
| `pdf-xfa-form` | `active-content` |
| `pdf-remote-goto` | `external-reference` |
| `pdf-uri-link` | admitted with `hyperlink-not-fetched` |
| `pdf-embedded-file` | admitted with `embedded-file-not-extracted` |
| `pdf-encrypted` | `encrypted-or-legacy-container` |
| `pdf-signed` | admitted with `signature-not-verified` |
| `pdf-form` | admitted |
| `pdf-dlp-canary` | admitted with `dlp-findings`; export refused |
| `pdf-public-boundary-marker` | admitted with `public-boundary-violation`; export refused |
| `pdf-oversized-stream` | `decompression-limit` (Flate bomb) |
| `pdf-oversized-input` | `oversized` |
| `pdf-scanned-page` | admitted with `ocr-not-run` |
| `pdf-unmapped-glyphs` | admitted with `text-unmapped-glyphs` |
| `pdf-not-a-pdf` | `malformed-pdf` |
| `pdf-cancelled` | `cancelled` |

The mutation property mutates the ordinary row in its classic and packed
layouts with a fixed seed. Import and export must not panic, and an admitted
result must be canonical JSON with one anchor per block. These are seeded
property targets, not coverage-guided fuzzing, as in the conformance suite.

| Budget | Fast | Full |
|---|---|---|
| Runner | `pdf_conformance_fast` (workspace tests) | `pdf_conformance_full` (ignored; `check.sh --full` security gate) |
| Limits | the conformance suite's fast limits | `Limits::default()` |
| Largest generated input | 2 MiB | 24 MiB |
| Seeded mutations (bit flip, truncation, insertion) | 500 | 20,000 |
| Runtime ceiling (debug build) | 10 s | 120 s |
| Measured on 2026-09-30 (debug build) | 1.0 s, 27 rows | 41.8 s, 27 rows |
| Fixture file size | 8 KiB | same file |

## Independent readers

`pdf_opens_in_independent_readers` is opt-in (`--ignored`). It writes the
ordinary fixture and its export, then runs `pdfinfo`, `pdftotext`, and
Ghostscript when installed; a missing reader prints `unavailable`, never a
pass. Results on 2026-09-30:

| Reader | Fixture | Export |
|---|---|---|
| poppler 24.02 `pdfinfo` | exit 0, 2 pages, `Tagged: no`, `JavaScript: no` | exit 0, 2 pages, `Tagged: yes`, `JavaScript: no` |
| poppler 24.02 `pdfinfo -struct-text` | n/a | `Document` > `H1`, `P`, `P` with the block text |
| poppler 24.02 `pdftotext` | exit 0, all block text | exit 0, same text |
| Ghostscript 10.02.1 (`nullpage`) | exit 0, no error | exit 0, no error |
| LibreOffice 26 Draw (`--convert-to pdf`) | opened and re-exported, text intact | opened and re-exported, text intact |

The adapter also imported the PDFs those tools wrote (LibreOffice, Ghostscript
`pdfwrite`, and cairo via `pdftocairo`) without refusal and with the same
block text. Those files embed font subsets, so they are not committed.

## Acceptance status for #23

| Item | Status | Evidence or reason |
|---|---|---|
| Capability matrix: text, scanned, forms, annotations, attachments, encryption, signatures, damaged cross-reference tables, active content | done | capability matrix above; one row per input class |
| Deterministic fixtures prove stable page and citation locators | done | `pdf-ordinary-projection` recorded digest; three re-encodings per admitted row; `ordinary_projection_has_stable_locators` |
| OCR and non-OCR paths distinguishable in every packet, citation, and export receipt | partial | citations (blocks) and export receipts carry `derivation`; the projection records OCR requested and effective. No OCR engine is installed or approved, so the OCR path itself does not exist; `bran query` and `bran packet` do not ingest document evidence yet (#25) |
| Adversarial fixtures: active content, embedded files, oversized streams, recursive objects, malformed structures, password-protected input | done | rows above |
| Accessible tagged PDF export or typed refusal; PDF/A claims independently validated | partial | tagged export with structure tree, language, title, and reading order, verified by `pdfinfo -struct-text`; unencodable text refused. No PDF/A or PDF/UA claim is made because no validator is installed, and the standard font is not embedded |
| Export removes or refuses active content; DLP and public boundary first | done | `export_is_tagged_inert_and_carries_a_fidelity_receipt`, `export_runs_dlp_first_and_never_overwrites` |
| No Adobe software or cloud conversion service | done | no dependency added; `importers_have_no_network_or_process_access` |
