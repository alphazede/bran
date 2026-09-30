---
type: Concept
title: Enterprise-document evidence envelope
okf_status: active
tags:
  - public
  - developer
freshness: "2026-09-15"
resource: https://github.com/alphazede/bran
public_boundary: public
---

# Enterprise-document evidence envelope

This is the V1 contract for [issue #20](https://github.com/alphazede/bran/issues/20).
A managed parser (Google Document AI or another owner-approved tool) reads
native DOCX, XLSX, PPTX, or PDF. BRAN does not. BRAN validates a JSON envelope
around that output so the evidence can enter provenance, DLP, public-boundary,
query, and packet workflows without executing the original file.

The schema is
[`schemas/enterprise-document-evidence-envelope.schema.json`](../schemas/enterprise-document-evidence-envelope.schema.json).
JSON Schema cannot bind digest equality, asset-path safety, family/media
pairing, or relation acyclicity. Those rules live in
[`tools/ci/enterprise_contract_check.py`](../tools/ci/enterprise_contract_check.py),
the schema's `x-semantic-oracle`. Default tests and the checker make no
provider or network calls.

## Product boundary

The parser owns native extraction and parser-specific limits. BRAN owns:

- original media type, byte length, and SHA-256
- parser identity, processor, version, and attestation state
- source locator plus revision state (`attested` with a value, or
  `unavailable` with a null value)
- deterministic evidence and anchor identities
- hashes of normalized content and referenced assets
- fidelity, truncation, malformed-input, and unavailable receipts
- containment, byte budgets, DLP, classification, and public-boundary outcomes
- admission into query and packet paths

An unavailable parser, revision, classification, or public-boundary claim
stays unavailable. BRAN does not invent completeness, permission, fidelity,
or revision evidence.

Never store or return credentials, OAuth tokens, signed URLs, cookies, or
raw authentication state in any envelope field — including `source.locator`,
parser strings, asset paths/roles, receipts, reasons, and relation ids. The
oracle rejects envelopes whose strings contain a secret marker
(`X-Goog-Signature=`, `X-Goog-Credential=`, `access_token=`,
`refresh_token=`, `ya29.`, `AIza`, `private_key`, or `-----BEGIN `) as
`secret-reflection`, before digest checks. That list is substring matching,
not a DLP product.

## Envelope

Required keys, no extras: `schema_version` (`1.0.0`), `evidence_id`,
`envelope_digest`, `original`, `parser`, `source`, `normalized`, `anchors`,
`assets`, `relations`, `fidelity`, `receipts`, `hazards`, `policy`,
`admission`.

Fail-closed budgets: canonical envelope 1 MiB, original 20 MiB, 4096
anchors, 256 assets, 8192 relations, 8192 UTF-8 bytes per text field.
Overrun is `oversized`.

`original.media_type` is one of PDF, DOCX, PPTX, or XLSX. That media type
selects exactly one content family:

| Media type | Family |
|---|---|
| `application/pdf` | `fixed-layout` |
| WordprocessingML DOCX | `flow` |
| PresentationML PPTX | `presentation` |
| SpreadsheetML XLSX | `grid` |

`normalized.content.family` and every anchor family must match that pairing.
A mismatch is `unsupported-evidence`.

## Canonical bytes and digests

Canonical bytes are UTF-8
`json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)`.

- `envelope_digest` is SHA-256 of those bytes after omitting `envelope_digest`.
- `normalized.digest` is SHA-256 of the canonical `normalized.content` object.
- Each `anchors[].text_digest` is SHA-256 of the UTF-8 anchor text.

Equivalent logical input therefore serializes byte-identically. Object key
order must not change a digest. The oracle proves this on the first admitted
positive fixture by reversing object keys; it does not reorder arrays or
repeat the check on the other three positives.

## Anchors, assets, and relations

Anchors are the citable units. Each has a stable `id`, family-specific
`role`, text, `text_digest`, and a locator:

- `fixed-layout`: page, block, bbox
- `flow`: section, ordinal
- `presentation`: slide, shape, z_index
- `grid`: sheet, row, column

Allowed roles by family:

| Family | Roles |
|---|---|
| `fixed-layout` | heading, paragraph, table, figure, header, footer, title, list |
| `flow` | heading, paragraph, table, list, header, footer, title |
| `presentation` | heading, paragraph, table, list, title, notes, shape |
| `grid` | sheet, cell, range, table, header, chart |

The schema `role` enum is the union of those sets. The oracle requires the
family subset.

Anchor ids are unique and sorted. Asset ids share that namespace, are unique
against anchors, and must themselves be sorted. Assets are content-addressed.
Asset `path` is a relative POSIX path of `[A-Za-z0-9._-]` segments separated
by `/`, at most 256 bytes, with no `.` or `..` segment, no leading slash, no
backslash, and no NUL. Spaces fail as `unsafe-asset-path`.

Relations (`derived-from`, `cites`, `extracts`, `summarizes`) must name
existing ids, must not be self-loops, and must be unique sorted tuples.
`derived-from` and `extracts` share one acyclicity graph: a cycle that
alternates those kinds still fails.

## Fidelity, receipts, hazards, and policy

`fidelity` keys must be exactly the family set. Values are `exact`,
`normalized`, `approximated`, or `unsupported`.

| Family | Features |
|---|---|
| `fixed-layout` | text, reading_order, bounding_boxes, tables, figures, ocr, javascript |
| `flow` | text, paragraphs, headings, lists, tables, headers_footers, macros |
| `presentation` | text, slides, shapes, speaker_notes, z_order, macros, animations |
| `grid` | text, sheets, cells, formulas, charts, macros |

Of those keys, `macros`, `formulas`, `javascript`, and `animations` must
never be `exact`. `embedded_objects`, `external_relationships`, and
`round_trip` are never-exact in the oracle but are not family features;
using them as fidelity keys is `malformed-structure`.

`receipts.unavailable.features` is exactly the features marked
`unsupported`. `receipts.unavailable.revision` / `parser_attestation` must
match `source.revision.state` and `parser.attestation`. Truncation and
malformed-input receipts carry a reason only when the condition is present.
When `truncated` is false, `omitted_bytes` and `omitted_anchor_count` must
be 0.

Active content or external references fail closed
(`active-content`, `external-reference`). They never become a valid envelope.

`policy.dlp.status` is `not-evaluated`, `passed`, or `findings`.
`public_boundary.outcome` `admit-export` is allowed only when the boundary
value is `public`.

## Admission into query and packet

`admission` is the DLP/refusal gate on this envelope. It is not an export
verdict and not a trust verdict. The oracle blocks `admitted` only when
`policy.dlp.status` is `findings`. Classification, public-boundary outcome,
parser attestation, truncation, and malformed-input do not flip admission.
Shipped positives may be `admitted` while `classification` is `internal`
and `public_boundary.outcome` is `reject-export`.

| Condition | `status` | `packet` / `query` | `reasons` |
|---|---|---|---|
| DLP `findings` | `rejected` | `ineligible` | includes `dlp-findings` |
| Otherwise accepted | `admitted` | `eligible` | empty |
| Otherwise refused | `rejected` | `ineligible` | non-empty |

A later ingest path may consider only `admitted` envelopes, and only through
their anchors (id, family, role, locator, text digest) plus original digest,
parser identity/version, source locator/revision, derivation relations, and
fidelity/truncation receipts. `rejected` envelopes stay out of query and
packet. `admitted` does not authorize export: ingest must still honor
`policy.classification` and `policy.public_boundary` separately, and must
not treat `unavailable` parser or revision claims as attested.

Issue #46 implements the ingest. `bran document inspect <root> <path>`
prints this envelope for one supported document without writing anything.
`bran query` and `bran packet` append a `document_evidence` member (sources,
ranked anchor matches, typed refusals, unsupported formats) when document
files exist, and byte-identical output otherwise.

Admission design (why, for the record):

- Discovery is by filename extension (`.docx`, `.xlsx`, `.pptx`, `.pdf`,
  ASCII case-insensitive) under the requested root, not by policy
  `document_coverage` roots: coverage classifies Markdown knowledge
  documents, while enterprise files are found where the author left them.
  The walk mirrors the repository scanner (sorted entries, `.branignore` /
  `.okfignore` / `.gitignore`, no symlinks, depth 64, root containment) and
  caps at 64 files with an `omitted_files` count.
- Routing is by registered adapter, not by file: an extension whose adapter
  has not landed (DOCX and PPTX until #21 and #22 merge) is reported under
  `unsupported`, never refused and never ranked. Each format adds only a
  projection reader; envelope assembly is shared.
- Document anchors never rank as Markdown sources. `score_source_candidates`
  skips document paths, so a refused file cannot be selected by a path
  match; admitted anchors rank in the appended member only, by matched-term
  count then bundle, path, and anchor id, capped at 32 matches. Markdown
  rankings, selection, and byte accounting are untouched.
- Derivation is per anchor. Native parse output is `embedded`; a projection
  block marked OCR-derived is `ocr`, sorts after every embedded match, and
  carries no byte rank. No OCR engine ships, so `ocr` anchors cannot occur
  yet; the branch is pinned by unit tests.
- The envelope's `language` is the adapter-declared default `en`, not a
  detected language: the schema requires a two-letter tag and the adapters
  are byte decoders, not language detectors. Consumers must not treat it as
  detected. Classification and public-boundary stay `unavailable` (never
  invented); only DLP gates admission.
- Fixed-layout bounding boxes are clamped to the envelope's non-negative
  range, matching the adapter's `approximated` fidelity for geometry, rather
  than refusing citable text over coordinates.

The owner has approved native local Office/PDF adapters (#21, #22, #23,
#26). They build on the shared conformance suite from #25, described in
[`enterprise-document-conformance.md`](enterprise-document-conformance.md).

## Fixtures

Synthetic normalized outputs, not native round-trips, under
`fixtures/enterprise-documents/`:

- positive: `pdf-fixed-layout.json`, `docx-flow.json`,
  `pptx-presentation.json`, `xlsx-grid.json`
- negative: `digest-mismatch`, `unsafe-asset-path`, `active-content`,
  `external-reference`, `malformed-structure`, `oversized`,
  `secret-reflection`, `unsupported-evidence`

The Google attestation profile records a Document AI `output.digest` as this
envelope's `envelope_digest`. See
[`schemas/google-source-attestation.schema.json`](../schemas/google-source-attestation.schema.json).
