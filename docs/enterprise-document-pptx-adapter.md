---
type: Concept
title: Enterprise-document PPTX adapter
okf_status: active
tags:
  - public
  - developer
freshness: "2026-09-30"
resource: https://github.com/alphazede/bran
public_boundary: public
---

# Enterprise-document PPTX adapter

This is the native PPTX adapter for
[issue #22](https://github.com/alphazede/bran/issues/22). It is
`bran_document::pptx` and is registered with the shared conformance suite
described in [`enterprise-document-conformance.md`](enterprise-document-conformance.md).
It adds no dependency: it reads packages through the shared OPC intake and
the shared `quick-xml` reader selected in
[`enterprise-document-dependency-review.md`](enterprise-document-dependency-review.md).
No PowerPoint installation, cloud service, or network access is needed.

## Boundary

Import calls `opc::open` first, so ZIP safety, part names, duplicate parts,
budgets, DTD refusal, macro and ActiveX parts, external references, and the
package DLP scan are decided once for every OOXML format. The adapter then
adds PPTX rules:

| Input | Outcome |
|---|---|
| Any relationship of type `package`, `oleObject`, or `control`; any `oleObj` or `control` element | `active-content` |
| Click or hover action `ppaction://program`, `ppaction://macro`, or `ppaction://ole` | `active-content` |
| No main `officeDocument` part | `malformed-container` |
| Main part that is not a presentation, slide show, or template | `unsupported-container` |
| Slide list entry whose relationship is missing, external, not a slide, points at a missing part, or repeats a part or slide id | `malformed-container` |
| Slide id outside 256 to 2147483647, or a non-numeric shape id | `malformed-xml` |
| Run or shape link whose relationship id does not exist | `malformed-container` |
| Groups nested more than 64 deep | `xml-depth-limit` |
| Relationship expansion, projected text copies, or anchors exhaust the cumulative `max_total_bytes` budget | `oversized` |
| Repeated XML traversal or projection exhausts the cumulative `max_xml_nodes` budget | `xml-node-limit` |
| Unbound element or attribute prefix, including an empty prefix binding | `malformed-xml` |

Nothing is fetched, executed, or played. External hyperlinks are recorded as
text and receipted as `hyperlink-not-fetched` by the intake.

The shared reader keeps attribute prefixes as written. The adapter resolves
relationship attributes (`id`, `embed`, `link`) by namespace, not by prefix:
any prefix bound to the Transitional or Strict relationship namespace reads as
`r:`, and `r` bound to another namespace is not a relationship attribute. A
producer's prefix choice therefore cannot hide a link or an image.
The shared reader rejects unbound prefixes in every XML part. Projection
budgets count each traversal of a shared target and reserve text and anchor
copies before materializing them; ZIP size alone does not bound this work.

## Canonical content

`import` returns the harness `Imported` value. Its canonical bytes are
canonical JSON (the envelope rule) tagged `bran.pptx.content/1`:

| Key | Content |
|---|---|
| `slides` | In `sldIdLst` order: slide id, cSld name, layout name, hidden flag, shapes, notes, comments |
| `sections` | Section name, section id, and slide ids |
| `slide_size` | `cx`, `cy` |
| shapes | In shape-tree order (reading and z-order), groups nested: kind (`shape`, `picture`, `group`, `table`, `connector`), id, name, alt text (`descr`), alt title, hidden, placeholder type and index, click link, preset geometry, position and size |
| text | Paragraphs with level and runs; a line break is a run of `\n`; a field becomes its text |
| links | External URL, target slide id, or action; never fetched |
| tables | Column widths, row heights, and cell paragraphs |
| notes | Notes body placeholder id and paragraphs |
| comments | Author, initials, and text; legacy and modern comments, threads flattened |
| `assets` | Images by SHA-256, with media type, length, and bytes (hex); one image used twice is one asset |
| `anchors` | Envelope-shaped citation anchors, sorted by id |
| `fidelity` | The seven presentation features of the envelope |

The canonical content excludes ZIP order, timestamps, compression, and the
package inventory, so re-encodings of one deck give identical bytes.

## Anchors

Each anchor has the envelope shape: `id`, `family` `presentation`, `role`,
`text`, `text_digest` (SHA-256 of the text), and a locator with `slide`
(position), `shape`, and `z_index`.

| Anchor id | Role | Text |
|---|---|---|
| `anc:pptx:slide-<slide id>:shape-<shape id>` | `title` for title placeholders, `heading` for subtitles, `table` for tables, otherwise `paragraph` | shape paragraphs joined by line breaks; table cells joined by tabs and rows |
| `…:shape-<shape id>:alt` | `shape` | alt text |
| `anc:pptx:slide-<slide id>:notes` | `notes` | speaker notes |
| `anc:pptx:slide-<slide id>:comment-<n>` | `notes` | comment text (the envelope has no comment role) |

Slide ids and shape ids come from the deck, so anchors survive slide
reordering and re-export. A repeated shape id on one slide gets a `-z<n>`
suffix and the `duplicate-shape-id` receipt code.

## Receipt codes

The receipt is the intake's codes plus these. A code appears only when the
deck contained that feature.

| Code | Meaning |
|---|---|
| `text-formatting-normalized` | run formatting (font, size, colour, language) is not kept |
| `shape-styling-normalized` | fills, lines, effects, rotation, and flips are not kept |
| `layout-design-normalized` | only layout names are kept; masters, layouts, and themes are not |
| `table-formatting-normalized` | table styles and cell merges are not kept |
| `comment-metadata-normalized` | comment dates and positions are not kept |
| `comment-threads-flattened` | modern comment replies become separate comments |
| `field-as-text` | a field (slide number, date) is kept as its current text |
| `unsupported-transition`, `unsupported-animation` | transitions and animations are not kept |
| `unsupported-chart`, `unsupported-smartart`, `unsupported-graphic-frame` | the frame is left out of the projection |
| `unsupported-media` | audio, video, or sounds are not kept; a video's poster image is |
| `unsupported-content-part` | ink or other content parts are not kept |
| `alternate-content-fallback` | the standard fallback of alternate content was read |
| `alternate-content-omitted` | alternate content without a fallback was left out |
| `hover-action-omitted` | hover actions are not kept |
| `link-target-omitted` | an internal link to something other than a slide was dropped |
| `shape-identity-missing` | a shape had no `cNvPr` id; export refuses such a deck |
| `duplicate-shape-id` | see Anchors |
| `unmapped-parts-omitted` | parts outside the content model (masters, themes, document properties) were not carried |
| `dlp-findings`, `public-boundary-violation` | the joined text failed the shared DLP or boundary check; this catches a canary split across runs, which the intake's byte scan cannot see |

## Export

`export` projects canonical content into a new deck. It never copies source
parts. The deck uses one generic master, one layout per distinct layout name,
a generic theme, a notes master when there are notes, and legacy comments.
The package carries `bran/fidelity-receipt.json` (relationship type
`https://schemas.alphazede.dev/bran/relationships/fidelity-receipt`): the
content digest, the import receipt, the export codes
(`generic-master-layout-theme`, `run-properties-defaulted`,
`comments-written-legacy`), and the fidelity map.

Before returning bytes, export refuses when the import receipt has
`dlp-findings` or `public-boundary-violation`, then runs the shared check on
every paragraph's joined text, every name, alt text, URL, and comment, and
every generated part's name and bytes, including binary image metadata, using
the intake's byte scan. It also refuses content that is not
`bran.pptx.content/1` (`export-unsupported`), malformed content
(`malformed-container`), and shapes without an id (`export-unsupported`).
Writing to disk goes through `export::write_new`: explicit format, contained
destination, no overwrite.

The output is deterministic: sorted part names, one fixed timestamp, deflate
unless a part would exceed the default intake compression ratio, in which
case it is stored. Part count, part bytes, total expanded bytes, and package
bytes are checked before ZIP buffers grow; the generated package then passes
the shared OPC intake, including XML node and depth limits.
Import, export, and import again give byte-identical canonical content for the
synthetic decks.

## Evidence

Tests are in `crates/bran-document/tests/pptx.rs`; the corpus rows are in
`tests/conformance.rs`. All run in `check.sh --fast` except the reader check.

| Test | Shows |
|---|---|
| `pptx_import_maps_presentation_structure` | slide order, ids, sections, layouts, hidden slides, reading order, groups, text boxes, runs, breaks, fields, links, tables, notes, comments, alt text, assets, anchors, receipt |
| `pptx_import_ignores_archive_order_timestamps_and_compression` | determinism across re-encodings |
| `pptx_relationship_attributes_resolve_by_namespace` | relationship attributes read by namespace, not prefix |
| `pptx_ordinary_projection_is_recorded` | recorded canonical digest |
| `pptx_unsupported_features_are_receipted` | transitions, animations, charts, SmartArt, video, alternate content |
| `pptx_adversarial_decks_are_refused` | 25 hostile decks through `conformance::check` |
| `pptx_round_trip_preserves_structure` | import, export, import keeps canonical content and anchors |
| `pptx_export_carries_fidelity_receipt` | receipt part and relationship, written through the shared gate |
| `pptx_export_refuses_dlp_before_writing` | split canary and boundary marker refused before any bytes |
| `pptx_export_refuses_malformed_or_foreign_content` | typed export refusals |
| `pptx_mutated_decks_never_panic` | 240 seeded container and content mutations; admitted decks round-trip |
| `pptx_review_binary_dlp_rescan` | exact reviewer PNG metadata canary refused even with the import receipt cleared |
| `pptx_review_shared_notes_budget` | exact reviewer shared-notes deck returns `oversized` under a 512 MiB address-space limit on Linux; ordinary deck succeeds under the same limit |
| `pptx_review_compressible_text_round_trip` | exact reviewer 100,000-character run exports deterministically and re-imports with the same content and anchors |
| `pptx_review_unbound_relationship_prefix` | exact reviewer undeclared `r` deck and related empty-binding/unbound-element inputs return `malformed-xml` |
| `pptx_export_opens_in_independent_readers` | opt-in, see below |

The corpus also runs every package row (macro-enabled decks, external media,
path escape, decompression bombs, and the rest) through the adapter.
The four reviewer ZIPs are preserved in `fixtures/enterprise-documents/pptx-review/`
as raw-deflate hex text. Tests decode them and verify SHA-256 against the
original input before exercising the adapter. They are separate from the
8 KiB conformance fixture directory; its existing budget is unchanged.

### Independent readers

`pptx_export_opens_in_independent_readers` is ignored by default. It writes
the exported and the source synthetic decks, opens them with python-pptx, and
converts them with LibreOffice. A missing reader is printed as
`unavailable`. Run it with:

```sh
BRAN_PPTX_READER_DIR=<non-hidden directory> BRAN_PPTX_PYTHON=<python with python-pptx> \
  cargo test -p bran-document --test pptx pptx_export_opens_in_independent_readers -- --ignored --nocapture
```

Recorded on 2026-09-30 with python-pptx 1.0.2 and LibreOffice 26.2.5.2: both
readers opened all four decks. python-pptx listed the same slide ids, layout
names, shape ids in reading order, text, notes, alt text, and image digests
for each export as for its source. LibreOffice converted each deck to a
two-page PDF; the hidden third slide is skipped in both source and export.

## Acceptance status for #22

| Item | Status | Evidence |
|---|---|---|
| Import is deterministic regardless of ZIP ordering and timestamps | done | `pptx-ordinary-projection` row; `pptx_import_ignores_archive_order_timestamps_and_compression`; `pptx_ordinary_projection_is_recorded` |
| Fixtures cover layouts, text boxes, grouped shapes, tables, notes, comments, alt text, links, images, reading order | done | `pptx-*.parts`; `pptx_import_maps_presentation_structure` |
| Adversarial fixtures: external media, macro-enabled decks, malformed relationships, path escape, decompression limits | done | `pptx_adversarial_decks_are_refused`; package rows through the adapter |
| Exported decks open in two independent readers and include a fidelity receipt | done | reader check above; `pptx_export_carries_fidelity_receipt` |
| Import-export-import keeps slide order, structural identities, notes, accessibility metadata, citation anchors | done | `pptx-round-trip-anchors` row; `pptx_round_trip_preserves_structure` |
| DLP and public-boundary checks complete before export | done | `pptx_export_refuses_dlp_before_writing` |
| No PowerPoint installation or cloud service required | done | every test above runs offline in-process; `importers_have_no_network_or_process_access` |

## Not covered

- The canonical content is not yet wrapped in a full evidence envelope
  (`original`, `parser`, `policy`, `admission`). The envelope needs a
  `language` and a single normalized text of at most 8 KiB, which a native
  import cannot always supply without inventing values. The CLI inspection
  and query ingest paths in #25 are also not built.
- The reader check has not been run against PowerPoint, which is not
  installed and not required.
- Audio and video bytes are not carried as assets.
