---
type: design-contract
proposal_id: BRAN-GOOGLE-ENTERPRISE-ATTESTATION
kind: google-attestation-profile-v1
title: "Google Enterprise Attestation Profile"
okf_status: draft
status: draft
tags:
  - internal
  - bran
freshness: "2026-08-18"
resource: https://github.com/alphazede/bran-dev/issues/13
public_boundary: private
---

# Google Enterprise Attestation Profile

V1 design contract for [GitHub issue #13](https://github.com/alphazede/bran-dev/issues/13).
Gemini Enterprise and Agent Search own connectors and enterprise search.
Document AI owns native PDF, DOCX, PPTX, XLSX, OCR, and layout extraction.
Knowledge Catalog owns BigQuery metadata, governance, and lineage. BRAN owns
only deterministic attestation, provenance, replay, DLP, public-boundary, and
packet admission. This contract does not add a connector, parser, catalog,
provider adapter, or live Google dependency. A bounded offline CLI can
replay a saved processor create/get response and a saved process response
into issue #5 and #13 artifacts; it does not authenticate or call Google.

Issue #5 remains the canonical enterprise-document evidence envelope. Document
AI output in this profile references that envelope and reuses its canonical
JSON and digest rules. Direct Cloud Storage, Drive, BigQuery, or repository
adapters stay out of V1.

## Product boundary

Managed Google products are named capabilities, not reimplemented adapters.

| Product | Owns | BRAN records |
| --- | --- | --- |
| Gemini Enterprise | connectors, engines, retrieval | engine/connector identity, filters, access mode, result digest, revision when attested |
| Agent Search | data stores and enterprise search | data-store identity, selected filters, federated/indexed/imported state |
| Document AI | native parse, OCR, layout, chunks | processor identity, location, #5 envelope digest, page/item limits |
| Knowledge Catalog | BigQuery metadata and lineage | entry identity, asset type, metadata revision, lineage references |
| Existing BRAN Git/OKF | repository and bundle evidence | unchanged Git/OKF path; no Google call |

Google-managed products own connectivity, search, parsing, catalog metadata,
IAM, and end-user access enforcement. BRAN owns offline capability
declarations, owner-approved source and tenant allowlists, opaque account
references, source/revision/generation/entry evidence, normalized output
digests, checkpoint receipts, deterministic replay fixtures, DLP,
classification, public-boundary, byte-budget, and packet admission.

An unavailable provider claim stays unavailable. BRAN does not invent
revision, permission, fidelity, completeness, cost, or perimeter evidence.

## Capability states

Requested, effective, and attested capability are recorded separately.

| Product | Allowed attested states |
| --- | --- |
| `gemini-enterprise` | `federated`, `indexed`, `imported`, `unavailable` |
| `agent-search` | `federated`, `indexed`, `imported`, `unavailable` |
| `document-ai` | `parsed`, `unavailable` |
| `knowledge-catalog` | `metadata-only`, `unavailable` |
| `bran-git`, `bran-okf` | `git-okf`, `unavailable` |

`federated`, `indexed`, and `imported` are different search states. Indexed
or imported results require attested source revision; otherwise the typed
failure is `history-incomplete`. Federated search may record
`revision.state=unavailable` without claiming history completeness. Catalog
metadata and lineage are never an authorization proof.
`permission.authorization_proof` is always false in V1.

The default local profile performs no provider or network call.
`runtime.network=disabled` forces attested and effective `unavailable` and
the typed failure `network-disabled` for Google-managed products. Synthetic
positives use `runtime.network=not-invoked`: they are recorded outputs, not
live calls.

Existing Git and OKF snapshots remain the repository/bundle path. They do
not require a Google product. `bran-git` and `bran-okf` may stay attested
when the network is disabled.

## Canonical JSON

The attestation is canonical JSON. There is no provider SDK and no custom
archive.

Canonical bytes reuse the issue #5 rule: UTF-8
`json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)`.
Equivalent objects with permuted in-memory key order serialize to the same
bytes. `tools/ci/google_attestation_contract_check.py` imports those helpers
from `tools/ci/enterprise_contract_check.py` and does not fork
canonicalization.

- `attestation_digest` is SHA-256 of the canonical attestation after omitting
  `attestation_digest`
- `source.filter_digest` is SHA-256 of the canonical `source.filters` object
- `source.configured_digest` is SHA-256 of the canonical tenant, project,
  account reference, locator, and filters
- Document AI `output.digest` is the issue #5 `envelope_digest` of the
  referenced envelope, recomputed with the integrated #5 oracle
- Search, catalog, and `git-okf` `output.digest` is SHA-256 of canonical
  `output.normalized`
- Current checkpoints with an attested revision hash locator, revision
  value, and output digest
- Same attested provider output plus BRAN policy produces the same
  canonical bytes and digest

The JSON Schema at `schemas/google-source-attestation.schema.json` is the
portable shape. `x-product-capabilities` is the capability table. The
semantic oracle binds digest equality, #5 envelope linkage, product/state
pairing, and typed fail-closed outcomes.

## Attestation

Required fields:

| Field | Role |
| --- | --- |
| `schema_version` | `1.0.0` |
| `attestation_id` | stable attestation identity |
| `attestation_digest` | canonical attestation digest |
| `profile` | `bran-google-enterprise-v1` / `1.0.0` |
| `product` | named managed product, identity, component, version |
| `capability` | requested, effective, and attested states |
| `tenancy` | tenant, project, location, opaque `acct:opaque:…` reference, perimeter |
| `source` | native locator, filters, configured/filter digests, revision |
| `permission` | `attested`, `partial`, or `unavailable`; never an authorization proof |
| `output` | kind, digest, optional #5 envelope path, normalized result identity |
| `checkpoint` | replay identity, digest, `current` / `stale` / `conflict` / `unavailable` |
| `truncation` | omitted bytes and items |
| `cost` | recorded or unavailable quota evidence |
| `runtime` | `not-invoked` or `disabled`; never an action path |
| `policy` | classification, DLP, public-boundary |
| `failures` | sorted typed failures computed from the fields |
| `admission` | packet and query eligibility |

Revision kind is product-specific: search uses `revision`, Document AI uses
`generation`, Knowledge Catalog uses `entry`, and `bran-git` / `bran-okf`
use `revision`. `state=unavailable` forces `kind=unavailable` and
`value=null`. Credentials, OAuth tokens, signed URLs, cookies, and raw
authentication state are not attestation fields.

Every product has an exact locator grammar and required scope fields. The
oracle parses locators as resource identities, not local filesystem paths,
and never treats object names as pathnames.

| Product | Locator grammar | Required scope |
| --- | --- | --- |
| `gemini-enterprise` | `projects/{project}/locations/{location}/collections/{collection}/engines/{engine}` | project, location |
| `agent-search` | `projects/{project}/locations/{location}/collections/{collection}/dataStores/{data_store}` | project, location |
| `document-ai` | `projects/{project}/locations/{location}/processors/{processor}/processorVersions/{processor_version}` | project, location |
| `knowledge-catalog` | `projects/{project}/locations/{location}/entryGroups/{entry_group}/entries/{entry}` | project, location |
| `bran-git` | `git:repository/{repository}/commit/{snapshot}` or `.../tree/{snapshot}` | repository, commit or tree, exact snapshot |
| `bran-okf` | `okf:bundle/{bundle}/snapshot/{snapshot}` | bundle root, exact source snapshot |

`bran-git` and `bran-okf` record existing BRAN Git and OKF evidence. They
are not Google connectors and do not require a provider call.
`output.kind` is `git-okf`. An attested `bran-git` or `bran-okf` revision
value must equal the locator snapshot identity.

Unknown, opaque, malformed, cross-project, cross-location, cross-tenant, or
cross-bundle locator shapes fail closed. They emit `tenant-escape` and
`location-mismatch` when the product-specific grammar cannot supply the
required scope, and they are never query or packet eligible.

Document AI `output.kind` is `enterprise-document-evidence-envelope`.
`output.envelope_path` must be a repository-relative issue #5 positive
envelope. The oracle loads that envelope, runs the integrated #5 classifier,
and requires `output.digest` to equal the envelope digest. BRAN still does
not parse native DOCX, XLSX, PPTX, or PDF. Search and catalog outputs keep
`search-result` and `catalog-entry`. `bran-git` and `bran-okf` use
`git-okf` with the same normalized result-identity contract.

## Security and typed failures

The contract never calls a provider, never follows an action path, and never
executes macros, formulas, scripts, or external relationships.

Typed failures are a closed vocabulary:

| Code | Meaning |
| --- | --- |
| `permission-unavailable` | permission is partial or unavailable |
| `history-incomplete` | indexed/imported state lacks attested revision |
| `stale` | checkpoint state is stale |
| `conflict` | checkpoint state is conflict |
| `mixed-revision` | source revision and checkpoint revision disagree |
| `dlp-findings` | DLP status is `findings` |
| `quota-exhausted` | quota evidence is exhausted |
| `location-mismatch` | locator location is missing or is not the recorded location |
| `perimeter-denied` | recorded perimeter is denied |
| `tenant-escape` | locator project, repository, or bundle is missing or is not the recorded scope |
| `secret-reflection` | a field contains a credential or signed-URL marker |
| `unauthorized-action` | a field requests a provider mutation |
| `completeness-overclaim` | authorization proof, or recorded failures do not match computed failures |
| `network-disabled` | default profile denied a Google product because the network is disabled |

Computed failures must equal the recorded `failures` array. Any non-empty
failure set forces `admission.status=rejected` and packet/query
`ineligible`. Rejected attestations never become packet evidence.

Account references stay opaque. Locators must match the product grammar and
stay under the recorded project, location, repository, or bundle. Search-only
profiles remain read-only.

## Policy and packet admission

Policy outcomes reuse the issue #5 vocabulary. Classification values are
`public`, `public-compatible`, `private`, and `internal`. DLP status is
`not-evaluated`, `passed`, or `findings`. Public-boundary outcomes are
`admit-export`, `reject-export`, or `unavailable`. `admit-export` is
allowed only when classification is `public`. Internal evidence may still
enter an internal packet.

`admission.status=admitted` is necessary and not sufficient. After the
contract admits an attestation:

1. Native locator, product identity, permission status, truncation, and
   output digest are the derivation path.
2. Document AI anchors enter `PacketAssembler` only through the admitted
   issue #5 envelope.
3. Search and catalog `result_ids` may be supplied as `EvidenceContent`
   identities that already match `NodeId`.
4. Locator facts become `PreservationAnchor` values under the existing
   packet byte limits.
5. `priority`, `authority`, and `freshness` come from the compiled view or
   caller. The attestation does not invent ranking.
6. Existing item, byte, and runtime-token bounds still apply.
7. This contract does not add scanner ingestion or new `EvidenceClass`
   values.

## Fixtures and check

`fixtures/google-attestation/positive/` holds six synthetic attestations:
Gemini Enterprise indexed search, Agent Search federated search, Document AI
wrapping the issue #5 PDF envelope, Knowledge Catalog metadata-only entry
evidence, existing BRAN Git commit evidence, and existing BRAN OKF bundle
evidence. They are not live project output.

`fixtures/google-attestation/negative/` combines typed failures instead of
creating one file per bullet: incomplete permission plus incomplete
revision plus stale plus truncation; conflict plus mixed revision; DLP
rejection; quota plus location plus perimeter; cross-tenant escape plus
secret reflection plus unauthorized action; completeness overclaim; network
disabled; and unknown opaque locator plus missing scope.

`python3 tools/ci/google_attestation_contract_check.py` uses the Python
standard library only. It validates structural invariants, recomputes
canonical bytes and digests, accepts the six positives, checks the
Document AI envelope with the issue #5 oracle, rejects each negative for
its typed failures, and checks that a permuted in-memory object matches the
golden canonical bytes. The check also executes
`tools/ci/document_ai_smoke_adapter.py` against synthetic recorded
responses, validates both outputs with the same #5 and #13 classifiers,
requires the recorded #5 envelope to stay packet and query ineligible
because hazard evidence, DLP, and response/input binding are unavailable,
requires the #5 source identity to be the input SHA-256, requires the #13
record to stay rejected because permission is unavailable, requires
repeated offline replay to be byte-identical, proves an oversized-text
probe reports the same nonzero truncation on #5 and #13, refuses to
overwrite existing destination files, and proves the fail-closed
negatives including an explicit non-PDF MIME mismatch. The check imports
no socket, TLS, or HTTP client and performs no provider call. Every
schema-declared product has an output contract and a locator grammar.
Unrecognized locators never become packet or query eligible.

## Recorded-response Document AI adapter

`tools/ci/document_ai_smoke_adapter.py` is an offline stdlib CLI. It
accepts a saved processor create or get JSON, a saved process JSON, the
original PDF, an output directory, an opaque account reference, and a
tenant. It never authenticates, opens a network client, or calls Google.
`runtime.network=not-invoked` means the adapter did not invoke a runtime
network path.

Current Layout Parser process responses expose
`document.documentLayout.blocks` with recursive `textBlock` and
`tableBlock` content. The adapter consumes that tree only. Legacy
`document.text` / `document.pages` output is not a usable layout. A
bounded live process response may omit `document.mimeType` while still
returning `documentLayout`. Omission is accepted because `--input-pdf`
bytes are independently required to begin `%PDF`. If `mimeType` is
present, it must be `application/pdf`; an explicit non-PDF type is
rejected. The processor create or get response must supply the full
`projects/{project}/locations/{location}/processors/{processor}/processorVersions/{processor_version}`
identity. A smoke that does not persist that response cannot emit an
honest #13 locator. That processor-version locator belongs in
`parser.processor` and the #13 `source.locator`. The #5 `source.locator`
is the supplied original input digest `sha256:{input-sha256}`, not the
parser identity.

The adapter has no PDF parser, hazard scanner, DLP evaluation, or
cryptographic binding from process response to the supplied input bytes.
It cannot independently prove that the recorded response came from that
PDF, and it cannot prove original-file hazard absence. Hazard fields with
`present=false` mean none observed in the normalized recorded response,
not proof of absence in the original PDF. The deterministic #5 envelope
stays structurally valid and fail-closed: packet and query are ineligible,
with sorted reasons that hazard evidence is unavailable, DLP is not
evaluated, and response/input binding is unavailable. Those reasons do
not mean the checks were performed. Raw adapter output is not packet or
query eligible until separate owner-approved evidence supplies those
checks. This path is offline recorded-response replay, not a live
provider success.

The #13 truncation receipt is the #5 truncation receipt.
`omitted_item_count` is the envelope `omitted_anchor_count`. If those
exact totals exceed the #13 numeric bounds, the adapter fails closed
instead of clamping or underreporting.

The adapter fails closed when:

- full processor identity or the default processor version cannot be derived
- locator, project, and location disagree
- the input is not a PDF
- process `document.mimeType` is present and is not `application/pdf`
- `documentLayout` blocks contain no usable text
- either destination file already exists
- the #5 omitted-byte or omitted-item total cannot be represented exactly on #13
- an output path would escape the output directory
- required #5 or #13 evidence cannot be truthfully attested

It does not invent revision, permission, cost, perimeter, hazard-absence,
DLP-pass, or response/input-binding evidence. Requested, effective, and
attested capability stay separate fields. Committed files under
`fixtures/google-attestation/recorded/` are synthetic models of the
observed response shape. They are not a live provider test and contain no
live or private identifiers.

## Non-goals

No Gemini Enterprise or Agent Search connector or search engine. No
Document AI parser. No Knowledge Catalog metadata or lineage
reimplementation. No direct GCS, BigQuery, Drive, or repository adapter.
No mirroring of customer estates, arbitrary BigQuery rows, or a general
cloud browser. No provider actions, IAM or sharing changes, or combined
security domains. No live Google integration, billed query, credential, or
SDK. No provider client, connector abstraction, or Cargo/dependency
change. Those remain a separately approved adapter issue after an owner
selects one concrete GCP MVP.
