---
type: design-contract
proposal_id: BRAN-ENTERPRISE-DOCUMENT-EVIDENCE
kind: evidence-envelope-v1
title: "Enterprise Document Evidence Envelope"
okf_status: draft
status: draft
tags:
  - internal
  - bran
freshness: "2026-08-18"
resource: https://github.com/alphazede/bran-dev/issues/5
public_boundary: private
---

# Enterprise Document Evidence Envelope

V1 design contract for [GitHub issue #5](https://github.com/alphazede/bran-dev/issues/5).
Google Document AI or another owner-approved managed parser owns native
DOCX, XLSX, PPTX, PDF, OCR, and layout extraction. BRAN owns only the
deterministic evidence envelope, fidelity and attestation receipts, policy
checks, and packet admission. BRAN stays offline and dependency-free by
default. This contract does not add a parser, archive, provider adapter, or
live integration.

## Product boundary

Managed tooling is responsible for reading the original file, extracting
text and structure, and reporting parser-specific limits. BRAN is
responsible for:

- preserving original media type, claimed byte length, and SHA-256
- recording parser identity, processor, version, and whether attestation is
  available
- recording a native locator plus exact revision state when attested
- assigning deterministic evidence and anchor identities
- hashing normalized content and recording content-addressed assets
- recording per-feature fidelity, truncation, malformed-input, and
  unavailable-evidence receipts
- applying DLP, classification, and public-boundary outcomes
- admitting only validated envelopes into queries and bounded packets

An unavailable provider claim stays unavailable. BRAN does not invent
revision, permission, fidelity, or completeness evidence.

## Canonical JSON

The envelope is canonical JSON. There is no custom archive or container.

Canonical bytes are UTF-8
`json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)`.
Equivalent objects with permuted in-memory key order serialize to the same
bytes.

- `envelope_digest` is SHA-256 of the canonical envelope after omitting
  `envelope_digest`
- `normalized.digest` is SHA-256 of the canonical `normalized.content`
  object
- `anchors[].text_digest` is SHA-256 of the UTF-8 text bytes
- `original.sha256` and `assets[].sha256` are attested content addresses;
  V1 does not recompute them from absent native bytes

The JSON Schema at
`schemas/enterprise-document-evidence-envelope.schema.json` is the portable
shape. `tools/ci/enterprise_contract_check.py` is the semantic oracle for
digest equality, path safety, family pairing, and cycle freedom.

## Envelope

Required fields:

| Field | Role |
| --- | --- |
| `schema_version` | `1.0.0` |
| `evidence_id` | stable envelope identity |
| `envelope_digest` | canonical envelope digest |
| `original` | media type, byte length, SHA-256 |
| `parser` | identity, processor, version, attestation |
| `source` | native locator and revision state |
| `normalized` | family, text, language, content digest |
| `anchors` | typed locators for one family |
| `assets` | content-addressed references |
| `relations` | derivation and citation edges |
| `fidelity` | per-feature `exact`, `normalized`, `approximated`, or `unsupported` |
| `receipts` | truncation, malformed-input, unavailable |
| `hazards` | active content and external references |
| `policy` | classification, DLP, public-boundary |
| `admission` | packet and query eligibility |

V1 original media types are `application/pdf`, the three OOXML
word/presentation/sheet types, and no others. Claimed original size is at
most 20,971,520 bytes, matching the Document AI online input cap as a
recorded bound, not a live parser call. Canonical envelope bytes are at
most 1,048,576, matching BRAN packet byte limits.

## Typed anchors

The four families do not share semantics. A PDF table is still
fixed-layout; an XLSX cell is still grid.

| Family | Media | Locator |
| --- | --- | --- |
| `fixed-layout` | PDF | `page`, `block`, integer `bbox` |
| `flow` | DOCX | `section`, `ordinal` |
| `presentation` | PPTX | `slide`, `shape`, `z_index` |
| `grid` | XLSX | `sheet`, `row`, `column` |

`normalized.content.family` and every anchor family must match the original
media type. Mixed-family envelopes are unsupported evidence. Coordinates
are integers so canonical JSON has no float drift.

Required fidelity features also differ:

- fixed-layout: `text`, `reading_order`, `bounding_boxes`, `tables`,
  `figures`, `ocr`, `javascript`
- flow: `text`, `paragraphs`, `headings`, `lists`, `tables`,
  `headers_footers`, `macros`
- presentation: `text`, `slides`, `shapes`, `speaker_notes`, `z_order`,
  `macros`, `animations`
- grid: `text`, `sheets`, `cells`, `formulas`, `charts`, `macros`

`macros`, `formulas`, `javascript`, `animations`, `embedded_objects`,
`external_relationships`, and `round_trip` must never be `exact`.
Unsupported features are listed in `receipts.unavailable.features`.

## Security and receipts

The contract never executes macros, formulas, scripts, embedded objects, or
external relationships, and never fetches a relationship or contacts a
provider.

- `source.revision.state=unavailable` forces `value=null` and
  `receipts.unavailable.revision=true`
- `parser.attestation=unavailable` forces
  `receipts.unavailable.parser_attestation=true`
- active content or a positive external-reference count is rejected before
  admission
- asset paths are repository-relative POSIX paths with no `.`, `..`,
  absolute root, or backslash
- digest mismatch, malformed structure, cycles, and claimed original size
  above the bound are rejected
- credentials, OAuth tokens, signed URLs, cookies, and raw authentication
  state are not envelope fields

## Policy and packet admission

Policy outcomes reuse BRAN vocabulary. Classification values are `public`,
`public-compatible`, `private`, and `internal`. DLP status is
`not-evaluated`, `passed`, or `findings`. Public-boundary outcomes are
`admit-export`, `reject-export`, or `unavailable`. `admit-export` is
allowed only when classification is `public`. Internal evidence may still
enter an internal packet.

Validated anchors do not become graph nodes automatically. After the
contract admits an envelope:

1. Each admitted anchor may be supplied to `PacketAssembler` as
   `EvidenceContent`.
2. `id` is the anchor id, which already matches the packet `NodeId`
   pattern `^[A-Za-z0-9./:_-]+$`.
3. `content` is the bounded anchor text.
4. Typed locator facts become `PreservationAnchor` values (`id` ≤ 64 bytes,
   `[A-Za-z0-9._-]+`; `value` ≤ 512 bytes).
5. `priority`, `authority`, and `freshness` come from the compiled view or
   caller. The envelope does not invent ranking.
6. Existing item, byte, and runtime-token bounds still apply. Required
   evidence that does not fit remains a packet error; other items may be
   omitted and recorded on the packet receipt.
7. Query ranking may consume the same admitted evidence as external
   content. This contract does not add scanner ingestion or new
   `EvidenceClass` values.

`admission.status=admitted` is necessary and not sufficient. DLP findings
force `rejected` / `ineligible`. Rejected envelopes never become packet
evidence.

## Fixtures and check

`fixtures/enterprise-documents/positive/` holds four synthetic normalized
envelopes, one per V1 media type. They are not native binaries and do not
claim round-trip fidelity. `fixtures/enterprise-documents/negative/` holds
named rejections: `digest-mismatch`, `unsafe-asset-path`, `active-content`,
`external-reference`, `malformed-structure`, `oversized`, and
`unsupported-evidence`.

`python3 tools/ci/enterprise_contract_check.py` uses the Python standard
library only. It validates structural invariants, recomputes canonical
bytes and digests, accepts the four positives, rejects each negative for
its typed reason, and checks that a permuted in-memory object matches the
golden canonical bytes.

## Non-goals

No DOCX, XLSX, PPTX, or PDF parser. No Document AI, Office, or layout
engine reimplementation. No conversion platform. No lossless or unsupported
fidelity claims. No provider SDK, credentials, or live Google dependency.
Issues #6–#10 remain conditional adapter work. Issue #13 remains the
Google attestation profile.
