---
type: Concept
title: XLSX adapter
okf_status: active
tags:
  - public
  - developer
freshness: "2026-09-30"
resource: https://github.com/alphazede/bran
public_boundary: public
---

# XLSX adapter

This is the native XLSX adapter for
[issue #26](https://github.com/alphazede/bran/issues/26). It lives in
`crates/bran-document/src/xlsx.rs`, registers with the shared conformance
harness from [#25](https://github.com/alphazede/bran/issues/25)
([`enterprise-document-conformance.md`](enterprise-document-conformance.md)),
and uses the parser approach recorded in
[`enterprise-document-dependency-review.md`](enterprise-document-dependency-review.md):
the shared ZIP reader, OPC intake, and `quick-xml` reader. It adds no crate.
It needs no spreadsheet application and no cloud converter.

## What import does

`xlsx::import` opens the package through `opc::open`, so every shared
package rule applies first. It then maps SpreadsheetML into the canonical
grid projection `bran-xlsx-grid/1`. The projection is canonical JSON under
the envelope's byte rule, so equal workbooks give equal bytes.

| Key | Holds |
|---|---|
| `sheets` | Worksheets in workbook order: `name`, `sheet_id`, `state` (`visible`, `hidden`, `veryHidden`), declared `dimension`, `cells`, `merged_cells`, `hyperlinks`, `data_validations`, `comments`, `tables` |
| `cells[]` | `ref`, `row`, `column`, and either `value` or `formula` plus `cached`; `number_format` (built-in id or custom code); `labels` |
| `value`, `cached` | `{"type": ..., "value": ...}` with type `number`, `string`, `boolean`, `error`, or `date`. Numbers keep their lexical form; nothing is parsed into a float. |
| `formula` | `text`, and `kind`, `ref`, `si` for shared and array formulas; `quarantined` when active (below) |
| `defined_names` | `name`, `value`, `scope` (sheet name for a local name), `hidden` |
| `anchors` | Citation anchors in the envelope's grid shape: `id`, `family: grid`, `role`, `text`, `text_digest`, `locator {sheet, row, column}` |
| `unsupported` | Every relationship or element the adapter does not model, typed and content-addressed |
| `fidelity` | The envelope's grid features: `text`, `sheets`, `cells`, `formulas`, `charts`, `macros` |
| `receipt` | The versioned receipt codes (also returned as `Imported::receipt`) |

A cached formula result is never presented as a calculated value. It is
stored under `cached`, apart from `formula`, and a formula cell's anchor text
reads `=SUM(Table1[Value]) [cached: 42]`. A formula without a cached value
gets no value. BRAN never recalculates.

Relationship ids (`r:id` on sheets and hyperlinks) are resolved through the
part's namespace binding for the relationships namespace, transitional or
strict, whatever prefix the producer chose. The shared XML reader keeps
attribute prefixes as written, so an `id` in a foreign namespace never stands
in for a relationship id. SpreadsheetML's own attributes are unprefixed.

Anchor ids are stable across export and re-import because they use
workbook identities, not part names: `anc:xlsx:s<sheetId>` (role `sheet`),
`anc:xlsx:s<sheetId>:r<row>c<column>` (role `cell`, or `header` in a table's
header row), `anc:xlsx:s<sheetId>:table-<id>` (role `table`), and
`anc:xlsx:name-<name>` or `anc:xlsx:s<sheetId>:name-<name>` (role `range`)
for a defined name that is a plain `Sheet!$A$1[:$B$2]` reference. Name bytes
outside `[A-Za-z0-9.-]` are written as `_HH`.

Labels help interpret a cell. The column label is the table header above a
cell inside a table, otherwise the text in row 1 of that column. The row
label is the text in column A of that row.

Determinism: ZIP entry order, compression, timestamps, shared-string table
order, and document-property timestamps do not change the projection. Shared
and inline strings both become `string` values. `docProps` are not imported.

## Receipt codes

The shared intake codes (`hyperlink-not-fetched`, `signature-not-verified`,
`dlp-findings`, `public-boundary-violation`, `missing-relationship-target`)
pass through. The adapter adds:

| Code | Meaning |
|---|---|
| `formula-cached-result-not-recalculated` | At least one formula carries a cached result, kept apart and labelled |
| `formula-quarantined` | A formula, defined name, or validation formula is active (below) |
| `hyperlink-quarantined` | An external hyperlink uses a scheme other than `http`, `https`, or `mailto` |
| `rich-text-flattened` | Rich-text runs were joined; their formatting is dropped |
| `styles-reduced-to-number-formats` | Only number formats survive; fonts, fills, borders, and themes do not |
| `layout-not-imported` | Views, column widths, page setup, and similar layout |
| `workbook-settings-not-imported` | Workbook properties, views, and calculation settings |
| `document-properties-not-imported` | `docProps` core, app, and custom properties |
| `calculation-chain-not-imported` | The calculation chain cache |
| `table-details-not-imported` | Table filters, sort state, styles, and column formulas |
| `unsupported-<kind>` | One per `unsupported` entry kind, for example `unsupported-chart`, `unsupported-image`, `unsupported-drawing`, `unsupported-conditional-formatting`, `unsupported-pivot-table` |

## Safety

The package is untrusted. The shared intake bounds bytes, parts, ratio,
depth, and nodes, and refuses VBA, XLM macro sheets, ActiveX, OLE,
macro-enabled workbooks, data connections, and external relationships. The
adapter adds:

| Input | Outcome |
|---|---|
| External workbook link part (`externalLink`), query table, data model | `external-reference` |
| Power Query mashup (`DataMashup` in a `customXml` part, UTF-8 or UTF-16) | `active-content` |
| Shared string out of range, cell outside the grid or its row, duplicate cell, non-numeric number, unknown style index, duplicate sheet name or id, missing worksheet, non-workbook main part | `malformed-container` |
| More label work than 64 times the XML node budget (cells times tables) | `xml-node-limit` |

Nothing is calculated, fetched, or executed. Formulas that would reach
outside the workbook when a spreadsheet application calculates them are
quarantined: DDE (`|`), external workbook references (`[1]Sheet!A1`,
`'[Book.xlsx]Sheet'!A1`, paths), and `WEBSERVICE`, `FILTERXML`, `IMAGE`,
`RTD`, `CALL`, `REGISTER`, `REGISTER.ID`, `EXEC`. String literals are
ignored. A quarantined formula is kept as inert evidence and flagged; export
refuses it with `active-content`.

The adapter re-runs `bran-core`'s DLP and public-boundary check on the text
it extracts, so a canary split across rich-text runs, which the shared byte
scan cannot see, still yields `dlp-findings`.

## Export

`xlsx::export` writes the projection as a deterministic XLSX package and
returns it with its fidelity receipt. The adapter's `Adapter::export` returns
the same bytes.

1. The import receipt must not carry `dlp-findings` or
   `public-boundary-violation`.
2. The projection is re-read strictly and must be `bran-xlsx-grid/1`.
3. DLP and the public boundary run on every string in the projection
   before any bytes are built, so an edited projection is re-checked.
4. Active formulas and links are refused with `active-content`. A text cell
   is always written as an inline string, never as a formula, so text such as
   `=cmd|'/C calc'!A0` cannot become live content.
5. Parts are written in sorted order with one fixed DOS timestamp.

The package contains the workbook, worksheets, a minimal style sheet with
the number formats, comments with a legacy VML drawing, tables, and
`bran/fidelity-receipt.json`, which the package relationships reference. The
receipt records the source projection digest, the import receipt, the
fidelity, what was normalized, every `unsupported` entry as `omitted`, and
`"formulas_recalculated": false`. Writing to disk goes through
`export::write_new`: explicit format and destination, DLP first, never
overwrite.

Round trip: import, export, and import again give the same `sheets`,
`defined_names`, and `anchors`, and exporting the re-import gives the same
workbook parts byte for byte. Only the receipt differs, because it names a
different source. Data-table formulas (`t="dataTable"`) are listed as
unsupported, and export refuses them rather than write an invalid formula.

## Compatibility

| Feature | Status |
|---|---|
| Workbook and worksheet order, names, sheet ids, visibility | exact |
| Declared dimensions | exact |
| Cell values (number, string, boolean, error, date) | exact |
| Shared versus inline strings | normalized (both are `string`) |
| Rich text | normalized (runs joined) |
| Number formats | exact; other styles are dropped |
| Formulas (normal, shared, array) and cached results | text and cached value exact, kept apart; never recalculated |
| Data-table formulas | unsupported |
| Tables (name, range, columns, header and totals rows) | exact; filters, sort, and styles dropped |
| Defined names (global, local, hidden) | exact |
| Merged cells | exact |
| Comments (legacy) | author and text exact; box layout dropped |
| Threaded comments | unsupported |
| Hyperlinks (external URL, internal location, display, tooltip) | exact; never fetched |
| Data validation | exact for the ECMA attributes and formulas |
| Conditional formatting, pivot tables, slicers, sparklines, extension lists | unsupported |
| Charts, drawings, images | unsupported, listed as typed relationships with digests |
| Chart sheets and dialog sheets | unsupported |
| Layout, views, page setup, document properties | not imported |
| VBA, XLM, ActiveX, OLE, macro-enabled workbooks | refused |
| External workbook links, data connections, query tables, data models | refused |
| Power Query | refused |

## Independent readers

On 2026-09-30 the exports of the base and feature fixtures were opened in
two independent readers, and again after the adapter moved to the stricter
shared intake (prefixed attribute keys, XML 1.0 character checks, exact
deflate and local-header checks). `xlsx_reader_samples` writes them when
`BRAN_XLSX_READER_DIR` is set; it is ignored by default, and the readers are
not dependencies of any crate or gate.

| Reader | Command | Result |
|---|---|---|
| LibreOffice 26.2.5.2 (headless) | `libreoffice --headless --convert-to pdf <file>` | Both converted (exit 0). Rendered values match the projection: `25.0%`, the date `1/1/2026`, `TRUE`, `#DIV/0!`, cached `42` and `Alpha!`; the hidden sheet is not printed. |
| LibreOffice 26.2.5.2 (headless) | `libreoffice --headless --convert-to xlsx <file>`, then openpyxl on the result | LibreOffice kept both formulas, the table, the comment, both hyperlinks, the validation, both defined names, the merge, and the hidden sheet. The adapter imports LibreOffice's file and round-trips its anchors. |
| openpyxl 3.1.5 | `load_workbook(file)` and `load_workbook(file, data_only=True)`, warnings as errors | No warnings. Sheet order and states, every value and type, number formats, formulas, cached values, merge, both hyperlinks, comment, validation, table, and both defined names read back as imported. `=1+1 stays text` reads as a string. |

## Acceptance status for #26

| Item | Status | Evidence |
|---|---|---|
| Deterministic regardless of ZIP entry order, shared-string ordering, timestamps | done | `xlsx_import_is_deterministic`; conformance re-encoding step on every admitted row |
| Fixtures cover formulas and cached values, tables, named ranges, merged cells, comments, links, charts, images | done | `fixtures/enterprise-documents/conformance/xlsx-features.parts`; `xlsx_features_import`; `xlsx_cached_results_are_never_presented_as_calculated` |
| Adversarial fixtures: external links, macro-enabled workbooks, malformed XML, path escape, decompression limits | done | `xlsx_hostile_workbooks_are_refused`, `xlsx_relationship_ids_resolve_through_namespace_bindings`, and the 39 shared package rows run through the adapter |
| Exported workbooks open in two independent readers and include a fidelity receipt | done | Reader table above; receipt part checked in `xlsx_round_trip_preserves_supported_content` |
| Import-export-import preserves cell types, formulas, sheet identities, relationships, citation locators | done | `xlsx_round_trip_preserves_supported_content`; conformance row `xlsx-round-trip-anchors` |
| DLP and public-boundary checks run before export | done | `xlsx_export_runs_dlp_and_public_boundary_first` |
| No installed Excel or cloud converter | done | pure Rust on the shared crate; `importers_have_no_network_or_process_access` |
| Formula-injection prevention on export | done | `xlsx_export_never_turns_text_into_formulas`; `xlsx_active_formulas_are_quarantined` |

Not covered: Excel itself was not available, so the reader evidence is
LibreOffice and openpyxl. The projection is not yet wrapped in a full
evidence envelope (parser identity, source locator, policy block); its
anchors already use the envelope's grid shape, and envelope assembly belongs
to the ingest follow-up described in
[`enterprise-document-evidence-envelope.md`](enterprise-document-evidence-envelope.md).
