---
type: seit
name: bran-okf-final-cutover
status: complete
date: 2026-07-23
applies_to: bran
plan_spec: ./plan-spec.md
design: ./design.md
planning_route: codex gpt-5.6-sol
planning_reasoning: high
---

## Purpose

This SEIT plan defines prospective proof for the BRAN final OKF cutover. It does
not claim that planned commands, consumer repositories, a stable release, or
retirement evidence exist. Current observations are labeled separately from
future acceptance.

## Evidence state

### Verified current

- `CMD-BUDGET` fails (exits 1) because the existing checker requires per-unit-test
  registration against the obsolete fixed ceiling.
- `CMD-FAST` stops at `CMD-BUDGET` first due to the test budget failure.
- `CMD-PUBLIC` fails on the doubled path
  `/home/spectre/alphazede/bran/bran/fixtures/...`.
- BRAN distinguishes `index.md` and `log.md`, but current portable profile code
  does not validate their upstream structures.

### Planned

- Test-budget no-ceiling inventory update and budget gate repair (`CMD-BUDGET`).
- Reserved-document portable conformance.
- Portable root behavior for `CMD-PUBLIC`.
- Release build/seal/publication/install verification.
- Six consumer migrations and parity receipts.
- Compatibility retirement and both upstream proposal drafts.

### Unavailable

- Retrieval parity evidence.
- HGTS checkout and current revision.
- Owner-approved stable public release identity.
- Owner approval for consumer writes, external proposal submission, or
  retirement.

## Test strategy

### Pre-lens stance

Use offline deterministic unit, contract, fixture, and procedure tests. Freeze
all repository/corpus identities before comparison. Retain raw evidence before
normalization. Use exact digests rather than timestamps as identity.

### Lens additions

- **CDD:** schema, version, profile, command, adapter, manifest, parity, and
  receipt contract cases.
- **SecDD:** path traversal, symlink, boundary conflict, signature, checksum,
  wrong revision, private-data, and unauthorized-side-effect cases.
- **RDD:** missing repository/runner, timeout, partial build/install, retry,
  rollback, and restoration-failure cases.
- **ODD:** exact command/revision/input evidence, first failure, unavailable
  state, deterministic ordering, and claim-state cases.
- **OOPDSA:** state transition, value-object validation, boundary precedence,
  DAG cycle, and write-set overlap cases.

### Evidence retention

Each required row retains:

- command/procedure ID and exact invocation;
- repository identity and exact revision;
- relevant config/corpus/artifact digests;
- stdout/stderr or structured raw output;
- exit code and first failure;
- normalized semantic receipt when applicable;
- before/after mutation inventory for write-capable procedures; and
- rollback receipt or explicit non-applicability.

Private corpus bodies, credentials, raw authentication state, hidden grader
truth, and unsanitized provider traces are forbidden evidence.

## Command context

Commands run from the BRAN repository root unless a procedure explicitly
changes into an owner-approved consumer checkout.

Execution binds these variables to exact values before a release or consumer
procedure:

```sh
BRAN_TAG=bran-vX.Y.Z
BRAN_SOURCE_SHA=<lowercase-40-hex>
BRAN_DIST=<absolute-empty-dist-directory>
BRAN_FINGERPRINT=<approved-lowercase-40-or-64-hex>
BRAN_MANIFEST=<absolute-path-to-bran-release-manifest.json>
BRAN_ASSET=<absolute-path-to-platform-archive>
CONSUMER_ROOT=<absolute-owner-approved-checkout>
CONSUMER_REVISION=<exact-consumer-commit>
CONSUMER_MANIFEST=<absolute-path-to-consumer-cutover-manifest>
EVIDENCE_DIR=<absolute-empty-consumer-evidence-directory>
```

Placeholders are planning bindings, not permission to choose a tag, key,
checkout, or destination. The implementation slice must record their resolved
values before any side effect.

## Required Commands

### Existing BRAN commands

- **CMD-OKF-PORTABLE** —
  `cargo test --manifest-path Cargo.toml -p bran-core p1_conformance`
- **CMD-PROFILES** —
  `cargo test --manifest-path Cargo.toml -p bran-core profile::tests::p1_profiles`
- **CMD-BUDGET** —
  `python3 tools/ci/test_budget_check.py tools/ci/test-budget.json`
- **CMD-FAST** — `./tools/ci/check.sh --fast`
- **CMD-PUBLIC** — `python3 tools/ci/public_boundary_check.py`
- **CMD-RELEASE-CONTRACT** —
  `python3 tools/ci/release_contract_check.py`
- **CMD-RELEASE-PLAN** —
  `./tools/ci/build-release.sh --plan --tag "$BRAN_TAG" --dist "$BRAN_DIST"`
- **CMD-RELEASE-BUILD-LINUX-X86** —
  `./tools/ci/build-release.sh --target x86_64-unknown-linux-gnu --tag "$BRAN_TAG" --dist "$BRAN_DIST"`
- **CMD-RELEASE-BUILD-LINUX-ARM** —
  `./tools/ci/build-release.sh --target aarch64-unknown-linux-gnu --tag "$BRAN_TAG" --dist "$BRAN_DIST"`
- **CMD-RELEASE-BUILD-MAC-X86** —
  `./tools/ci/build-release.sh --target x86_64-apple-darwin --tag "$BRAN_TAG" --dist "$BRAN_DIST"`
- **CMD-RELEASE-BUILD-MAC-ARM** —
  `./tools/ci/build-release.sh --target aarch64-apple-darwin --tag "$BRAN_TAG" --dist "$BRAN_DIST"`
- **CMD-RELEASE-BUILD-WINDOWS-X86** —
  `./tools/ci/build-release.sh --target x86_64-pc-windows-msvc --tag "$BRAN_TAG" --dist "$BRAN_DIST"`
- **CMD-RELEASE-DRY-SEAL** —
  `./tools/ci/release-check.sh --tag "$BRAN_TAG" --dist "$BRAN_DIST" --dry-run-unsigned`
- **CMD-RELEASE-SEAL** —
  `./tools/ci/release-check.sh --tag "$BRAN_TAG" --dist "$BRAN_DIST" --fingerprint "$BRAN_FINGERPRINT"`

### Planned bounded verification commands

These interfaces are prospective. Their owning implementation slices must
create them before any dependent consumer slice:

- **CMD-EXACT-SHA** —
  `python3 tools/cutover/verify_release.py --manifest "$BRAN_MANIFEST" --asset "$BRAN_ASSET" --source-sha "$BRAN_SOURCE_SHA" --fingerprint "$BRAN_FINGERPRINT"`
- **CMD-INSTALL-VERIFY** —
  `python3 tools/cutover/consumer_gate.py install-verify --consumer "$CONSUMER_ROOT" --revision "$CONSUMER_REVISION" --manifest "$CONSUMER_MANIFEST" --evidence "$EVIDENCE_DIR"`
- **CMD-CONSUMER-PARITY** —
  `python3 tools/cutover/consumer_gate.py parity --consumer "$CONSUMER_ROOT" --revision "$CONSUMER_REVISION" --manifest "$CONSUMER_MANIFEST" --evidence "$EVIDENCE_DIR"`
- **CMD-REFERENCE-AUDIT** —
  `python3 tools/cutover/consumer_gate.py reference-audit --consumer "$CONSUMER_ROOT" --revision "$CONSUMER_REVISION" --manifest "$CONSUMER_MANIFEST" --evidence "$EVIDENCE_DIR"`
- **CMD-ROLLBACK** —
  `python3 tools/cutover/consumer_gate.py rollback --consumer "$CONSUMER_ROOT" --revision "$CONSUMER_REVISION" --manifest "$CONSUMER_MANIFEST" --evidence "$EVIDENCE_DIR"`
- **CMD-RETIREMENT-PROOF** —
  `python3 tools/cutover/retirement_gate.py verify --manifest "$RETIREMENT_MANIFEST" --evidence "$EVIDENCE_DIR"`
- **CMD-ROUTE-TRACE** —
  `python3 tools/cutover/validate_route.py docs/plans/2026-07-22-bran-okf-final-cutover`

The planned tools are standard-library-only, offline, and read-only unless
invoked in the explicitly owner-approved install, rollback, or retirement
mode. Read-only modes reject a changed consumer tree.

### Stable consumer procedure IDs

Each procedure binds the shared commands to one exact consumer manifest and
checkout. It includes discovery, owner gate, install verification, validation
and retrieval parity, hook/skill check, active-reference audit, rollback proof,
and final `ConsumerGateReceipt`.

- **PROC-PARITY-ALPHAZEDEHQ**
- **PROC-PARITY-ALPHAZEDE-SPORTS**
- **PROC-PARITY-BETBOT**
- **PROC-PARITY-DEVELOPERS**
- **PROC-PARITY-HGTS**
- **PROC-PARITY-ALPHAZEDE-MARKETS**

Additional procedures:

- **PROC-PUBLICATION** — resolve release variables, run release build/seal
  commands, stop for approval, publish later, then verify exact public assets.
- **PROC-PROPOSAL-DRAFTS** — write both local drafts and prove their claim
  boundaries; no external write.
- **PROC-RETIREMENT** — evaluate the barrier, prove restoration, stop for
  approval, apply exact manifest later, and revalidate or restore.
- **PROC-PLAN-CANONICALIZE** — verify the five artifacts in the canonical
  directory, ensure review embeds exact sources, and remove the stale
  auto-slugged alias only after Bearing no longer depends on it.

## Traceability Matrix

| SEIT row ID | Acceptance/Risk ID | Design/Contract ID | Boundary/Test Layer | Positive Case | Negative/Failure Case | Command/Procedure ID | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| SEIT-001 | AC-002 | DES-001, DES-002, DES-003, CONTRACT-001, CONTRACT-002 | portable index conformance | frozen upstream-valid index passes `okf-v0.1` | each cited structural violation yields a stable portable code | CMD-OKF-PORTABLE, CMD-PROFILES | normative locator, fixtures, both profile outcomes |
| SEIT-002 | AC-002 | DES-001, DES-002, DES-003, CONTRACT-001, CONTRACT-002 | portable log conformance | frozen upstream-valid log passes `okf-v0.1` | each cited structural violation yields a stable portable code | CMD-OKF-PORTABLE, CMD-PROFILES | normative locator, fixtures, ordered diagnostics |
| SEIT-003 | AC-002, AC-008 | DES-001, DES-003, CONTRACT-001 | profile contract | portable pass/strict fail and portable fail/strict result remain separate | strict-only code changes portable status or selected exit | CMD-PROFILES | four outcome combinations and CLI envelope |
| SEIT-004 | AC-002 | DES-004, CONTRACT-003 | test-budget gate | named CI journeys, direct CI commands, and owned fixtures are deterministically inventoried; all Rust unit tests run in CMD-FAST | missing/duplicate journey, direct command, or fixture ownership fails CMD-BUDGET before expensive steps | CMD-BUDGET, CMD-FAST | budget inventory diff and first-gate output |
| SEIT-005 | AC-002 | DES-005, CONTRACT-003 | public-root security | root and relocated checkout find the exact public surface | doubled prefix, caller CWD, traversal, symlink, or broad scope passes | CMD-PUBLIC, CMD-FAST | resolved root, enumerated relative paths, exits |
| SEIT-006 | AC-002 | DES-001, DES-002, DES-003, DES-004, DES-005, CONTRACT-003 | offline shared gates | shared commands pass without provider or owner-local configuration | a shared command attempts network/provider lookup | CMD-OKF-PORTABLE, CMD-BUDGET, CMD-PUBLIC, CMD-FAST | sanitized environment and receipts |
| SEIT-007 | AC-003 | DES-006, DES-007, DES-008, CONTRACT-004 | deterministic release build | two same-input builds produce byte-identical target archives | dirty tree, missing lock/target, unknown target, or altered metadata passes | CMD-RELEASE-PLAN, CMD-RELEASE-BUILD-LINUX-X86, CMD-RELEASE-BUILD-LINUX-ARM, CMD-RELEASE-BUILD-MAC-X86, CMD-RELEASE-BUILD-MAC-ARM, CMD-RELEASE-BUILD-WINDOWS-X86 | source, tag, lock, and archive digests |
| SEIT-008 | AC-003 | DES-006, DES-007, DES-008, DES-009, CONTRACT-004 | release seal and provenance | tag, commit, lock, checksums, signature, manifest, and provenance agree | wrong tag/SHA/fingerprint, floating URL, tampered or extra asset, or symlink passes | CMD-RELEASE-CONTRACT, CMD-RELEASE-DRY-SEAL, CMD-RELEASE-SEAL, CMD-EXACT-SHA | ReleaseIdentity and seal output |
| SEIT-009 | AC-003 | DES-009, CONTRACT-004 | publication authority | approved exact packet is the only publishable identity | dry-run, recommendation, or stale approval authorizes publication | PROC-PUBLICATION | exact owner decision and immutable public readback |
| SEIT-010 | AC-003, AC-004 | DES-010, CONTRACT-005 | consumer installation | staged asset verifies before one pin changes | overwrite, digest/member/version mismatch, or latest URL selects | CMD-EXACT-SHA, CMD-INSTALL-VERIFY | InstallSnapshot and pin readback |
| SEIT-011 | AC-003, AC-004 | DES-011, CONTRACT-006 | consumer rollback | prior pin and bytes restore and focused commands pass | partial restore or failed verification reports success | CMD-ROLLBACK | RollbackReceipt and byte inventory |
| SEIT-012 | AC-004 | DES-013, DES-014, CONTRACT-007 | semantic parity | identical frozen input produces equivalent validation/retrieval semantics | normalizer hides locator, precedence, conflict, diagnostic, or unavailable state | CMD-CONSUMER-PARITY | raw pair, normalizer version, semantic diff |
| SEIT-013 | AC-004, AC-006 | DES-012, DES-013, DES-014, DES-019, DES-020, CONTRACT-010 | AlphazedeHQ consumer gate | install, parity, hook/skill, audit, and rollback pass | unexpected legacy reference or fail-open hook drift is ignored | PROC-PARITY-ALPHAZEDEHQ | ConsumerGateReceipt |
| SEIT-014 | AC-004, AC-005 | DES-012, DES-013, DES-014, DES-015, DES-019, DES-020, CONTRACT-008, CONTRACT-010 | AlphaZede Sports gate | granular allow/deny/inheritance rules and complete gate pass | ambiguous, escaping, unmatched, or weakened boundary policy passes | PROC-PARITY-ALPHAZEDE-SPORTS | rule set, boundary matrix, ConsumerGateReceipt |
| SEIT-015 | AC-004, AC-005 | DES-012, DES-013, DES-014, DES-016, DES-019, DES-020, CONTRACT-009, CONTRACT-010 | BetBot gate | approved split preserves locators, parity, bounded size, and rollback | global cap rises, content is lost, or rollback differs | PROC-PARITY-BETBOT | OversizedDocumentPlan, retrieval diff, byte rollback |
| SEIT-016 | AC-004 | DES-012, DES-013, DES-014, DES-019, DES-020, CONTRACT-010 | developers consumer gate | public source, install, parity, audit, and rollback agree | exported source differs or floating release is used | PROC-PARITY-DEVELOPERS | ConsumerGateReceipt and export/source identity |
| SEIT-017 | AC-004, AC-005 | DES-012, DES-014, DES-017, DES-018, DES-019, DES-020, CONTRACT-010 | HGTS consumer gate | verified checkout identity then complete gate passes | absent or wrong checkout is substituted or inferred passing | PROC-PARITY-HGTS | discovery or typed unavailable receipt |
| SEIT-018 | AC-004 | DES-012, DES-013, DES-014, DES-019, DES-020, CONTRACT-010 | alphazede-markets gate | install, parity, audit, and rollback pass | repository identity or receipt mismatch is ignored | PROC-PARITY-ALPHAZEDE-MARKETS | ConsumerGateReceipt |
| SEIT-019 | AC-004, AC-005 | DES-014, DES-017, DES-018, CONTRACT-007, CONTRACT-010 | unavailable parity | missing runner/corpus/capability yields typed unavailable | unavailable is normalized to success or silently skipped | PROC-PARITY-ALPHAZEDEHQ, PROC-PARITY-ALPHAZEDE-SPORTS, PROC-PARITY-BETBOT, PROC-PARITY-DEVELOPERS, PROC-PARITY-HGTS, PROC-PARITY-ALPHAZEDE-MARKETS | raw error, typed row, blocked gate |
| SEIT-020 | AC-001, AC-004 | DES-019, DES-025, CONTRACT-013 | route DAG and write sets | consumer write sets are disjoint and shared work precedes them | overlap or dependency cycle is accepted | CMD-ROUTE-TRACE | normalized write sets and DAG report |
| SEIT-021 | AC-006 | DES-020, DES-021, CONTRACT-010 | compatibility retention | legacy surfaces and prior binaries stay recoverable | a consumer slice deletes or disables compatibility | CMD-REFERENCE-AUDIT, PROC-PARITY-ALPHAZEDEHQ, PROC-PARITY-ALPHAZEDE-SPORTS, PROC-PARITY-BETBOT, PROC-PARITY-DEVELOPERS, PROC-PARITY-HGTS, PROC-PARITY-ALPHAZEDE-MARKETS | before/after path and reference inventory |
| SEIT-022 | AC-004, AC-006 | DES-012, DES-020, CONTRACT-010 | active reference audit | active compatibility and historical references classify deterministically | unexpected active reference is ignored or history triggers deletion | CMD-REFERENCE-AUDIT | raw matches, classification, digest |
| SEIT-023 | AC-006 | DES-021, CONTRACT-011 | retirement barrier | six passing receipts and fresh reference audit yield eligible | failed, unavailable, stale, or missing evidence yields eligible | CMD-RETIREMENT-PROOF | barrier identities and decision |
| SEIT-024 | AC-006 | DES-011, DES-022, CONTRACT-006, CONTRACT-011 | retirement restoration | recovery archive restores every proposed removal before approval | missing path, digest mismatch, or uncertain restore passes | CMD-RETIREMENT-PROOF, PROC-RETIREMENT | archive digest and restoration rehearsal |
| SEIT-025 | AC-006 | DES-022, CONTRACT-011 | retirement apply | separately approved exact manifest applies and gates pass | manifest drift, extra deletion, or post-gate failure lacks restore | PROC-RETIREMENT, CMD-FAST | approval, applied inventory, gate or restore receipt |
| SEIT-026 | AC-007 | DES-023, CONTRACT-012 | bundle-scope proposal | local draft covers root, coexistence, inclusion, exclusion, symlink, references, and reserved docs | draft invents repository-wide or resource-limit portable rules | PROC-PROPOSAL-DRAFTS | UpstreamProposalDraft and examples |
| SEIT-027 | AC-007, AC-008 | DES-001, DES-023, CONTRACT-001, CONTRACT-012 | layered-profile proposal | portable floor and additive results remain separate | strict failure is called portable nonconformance | PROC-PROPOSAL-DRAFTS, CMD-PROFILES | proposal and outcome examples |
| SEIT-028 | AC-007 | DES-009, DES-023, CONTRACT-012 | proposal submission gate | recommendation follows portable proof and stable release | plan, local build, or draft authorizes external write | PROC-PROPOSAL-DRAFTS, PROC-PUBLICATION | current evidence and separate owner decision |
| SEIT-029 | AC-001, AC-005, AC-008 | DES-025, CONTRACT-003, CONTRACT-007, CONTRACT-010, CONTRACT-013 | claim-state integrity | state labels match current receipts | plan text or prior run is surfaced as current pass | CMD-ROUTE-TRACE | RouteTrace and claim-lint result |
| SEIT-030 | AC-001, AC-009 | DES-024, DES-025, CONTRACT-013 | canonical artifacts and review | five canonical sources validate and review embeds exact sources | divergent duplicate, missing source, stale review, or prompt dependency remains | CMD-ROUTE-TRACE, PROC-PLAN-CANONICALIZE | artifact digests, links, directory inventory |
| SEIT-031 | AC-001, AC-004 | DES-019, DES-025, CONTRACT-013 | slice schema and assignments | every slice uses supported assignment and required manifest fields | unsupported route, reasoning, overlap, or missing field passes | CMD-ROUTE-TRACE | parsed assignments and slice manifests |
| SEIT-032 | AC-009, AC-010 | DES-024, DES-025, CONTRACT-013 | planning stop | route returns for owner selection without execution | Explorer, Expedition, external write, or removal starts | CMD-ROUTE-TRACE, PROC-PLAN-CANONICALIZE | journey stage and unchanged product/consumer inventory |

## Requirement Coverage Matrix

| Requirement | Design/contract | SEIT proof | Command/procedure | Prospective implementation owner | Rollback or N/A |
| --- | --- | --- | --- | --- | --- |
| REQ-GATE-001 | DES-001..003, CONTRACT-001..002 | SEIT-001..003 | CMD-OKF-PORTABLE, CMD-PROFILES | shared conformance slice | revert exact core/fixture write set |
| REQ-GATE-002 | DES-004, CONTRACT-003 | SEIT-004 | CMD-BUDGET, CMD-FAST | shared gate slice | revert registry/test paths together |
| REQ-GATE-003 | DES-005, CONTRACT-003 | SEIT-005 | CMD-PUBLIC, CMD-FAST | public-root slice | restore prior checker bytes |
| REQ-GATE-004 | DES-019, CONTRACT-013 | SEIT-020, SEIT-032 | CMD-ROUTE-TRACE | route/integration gate | N/A, read-only validation |
| REQ-GATE-005 | DES-001..005, CONTRACT-003 | SEIT-006 | shared commands | shared gate slices | revert owning write set |
| REQ-REL-001 | DES-006..008, CONTRACT-004 | SEIT-007..008 | release build/seal commands | release readiness slice | discard unpromoted dist |
| REQ-REL-002 | DES-005, DES-008..009 | SEIT-005, SEIT-008..009 | CMD-PUBLIC, PROC-PUBLICATION | release readiness slice | stop before publication |
| REQ-REL-003 | DES-006..008, CONTRACT-004 | SEIT-008 | CMD-RELEASE-SEAL, CMD-EXACT-SHA | release seal slice | reject identity |
| REQ-REL-004 | DES-010, CONTRACT-005 | SEIT-010 | CMD-INSTALL-VERIFY | each consumer slice | restore prior pin |
| REQ-REL-005 | DES-011, CONTRACT-006 | SEIT-011 | CMD-ROLLBACK | each consumer slice | is the rollback proof |
| REQ-REL-006 | DES-009 | SEIT-009 | PROC-PUBLICATION | publication gate | no side effect before approval |
| REQ-CONS-001 | DES-012, DES-019, CONTRACT-010 | SEIT-013..018 | six PROC-PARITY-* | six consumer slices | per-consumer |
| REQ-CONS-002 | DES-019, CONTRACT-013 | SEIT-020 | CMD-ROUTE-TRACE | route validator | N/A |
| REQ-CONS-003 | DES-013, CONTRACT-007 | SEIT-012 | CMD-CONSUMER-PARITY | parity harness + consumer | read-only corpus |
| REQ-CONS-004 | DES-013..014, CONTRACT-010 | SEIT-012..019 | six PROC-PARITY-* | each consumer | CMD-ROLLBACK |
| REQ-CONS-005 | DES-012, DES-020, CONTRACT-010 | SEIT-021..022 | CMD-REFERENCE-AUDIT | each consumer | restore changed references |
| REQ-CONS-006 | DES-014, DES-021 | SEIT-013..019, SEIT-023 | six procedures, CMD-RETIREMENT-PROOF | gate aggregator | N/A |
| REQ-CONS-007 | DES-012 | SEIT-010, SEIT-013..018 | six PROC-PARITY-* | owner gate per consumer | no write before approval |
| REQ-AZS-001 | DES-015, CONTRACT-008 | SEIT-014 | PROC-PARITY-ALPHAZEDE-SPORTS | Sports slice | restore policy/pin |
| REQ-BETBOT-001 | DES-016, CONTRACT-009 | SEIT-015 | PROC-PARITY-BETBOT | BetBot slice | exact source-byte restore |
| REQ-PARITY-001 | DES-013, DES-017, CONTRACT-007 | SEIT-012, SEIT-019 | CMD-CONSUMER-PARITY | parity harness | read-only; unavailable blocks |
| REQ-HGTS-001 | DES-018, CONTRACT-010 | SEIT-017 | PROC-PARITY-HGTS | HGTS slice | no write while absent |
| REQ-COMPAT-001 | DES-020..021 | SEIT-021, SEIT-023 | CMD-REFERENCE-AUDIT, CMD-RETIREMENT-PROOF | consumers + retirement gate | retained compatibility |
| REQ-COMPAT-002 | DES-010..011, DES-020 | SEIT-010..011, SEIT-021 | install/rollback commands | each consumer | restore prior pin/fallback |
| REQ-RETIRE-001 | DES-021..022, CONTRACT-011 | SEIT-023..025 | PROC-RETIREMENT | final separate slice | restoration archive |
| REQ-RETIRE-002 | DES-022, CONTRACT-011 | SEIT-024..025 | PROC-RETIREMENT | owner-gated retirement | stop before approval |
| REQ-RETIRE-003 | DES-011, DES-022 | SEIT-024 | CMD-RETIREMENT-PROOF | retirement rehearsal | is the proof |
| REQ-UPSTREAM-001 | DES-023, CONTRACT-012 | SEIT-026 | PROC-PROPOSAL-DRAFTS | local proposal slice | N/A, local draft |
| REQ-UPSTREAM-002 | DES-001, DES-023, CONTRACT-012 | SEIT-027 | PROC-PROPOSAL-DRAFTS, CMD-PROFILES | local proposal slice | N/A |
| REQ-UPSTREAM-003 | DES-009, DES-023 | SEIT-028 | proposal/publication procedures | later owner gate | no external write |
| REQ-PLAN-001 | DES-024, CONTRACT-013 | SEIT-030 | CMD-ROUTE-TRACE, PROC-PLAN-CANONICALIZE | Bearing planning route | remove stale alias only after validation |
| REQ-PLAN-002 | DES-025, CONTRACT-013 | SEIT-029..031 | CMD-ROUTE-TRACE | route validator | N/A |
| REQ-PLAN-003 | CONTRACT-003, CONTRACT-013 | SEIT-001..032 | command registry | SEIT/route owner | N/A |
| REQ-PLAN-004 | DES-019, OOPDSA DAG | SEIT-020, SEIT-031 | CMD-ROUTE-TRACE | implementation drafting | N/A |
| REQ-PLAN-005 | DES-024..025 | SEIT-030 | Bearing review generator, CMD-ROUTE-TRACE | Bearing | regenerate, never hand-edit |
| REQ-PLAN-006 | DES-024 | SEIT-030 | PROC-PLAN-CANONICALIZE | planning closeout | preserve canonical; remove stale alias |
| REQ-PLAN-007 | DES-025 | SEIT-032 | CMD-ROUTE-TRACE | planning checkpoint | N/A |

## Acceptance traceability

| Acceptance | Required SEIT proof |
| --- | --- |
| AC-001 | SEIT-029..031 plus complete requirement matrix |
| AC-002 | SEIT-001..006 |
| AC-003 | SEIT-007..011 |
| AC-004 | SEIT-012..020 |
| AC-005 | SEIT-014..019 |
| AC-006 | SEIT-021..025 |
| AC-007 | SEIT-026..028 |
| AC-008 | SEIT-003, SEIT-027 |
| AC-009 | SEIT-030..032 |
| AC-010 | SEIT-032 |

## Cross-cutting Checks

- **Determinism:** repeat tests with permuted discovery order and compare
  semantic identities.
- **Mutation containment:** snapshot every read-only fixture, consumer, and
  evidence directory before and after.
- **Path safety:** normalize and root-bind every enumerated or written path;
  reject symlink aliases for release assets and escaping consumer targets.
- **Claims:** lint every receipt and review for planned/passed/failed/
  unavailable/rolled_back accuracy.
- **Privacy:** scan source, fixtures, logs, receipts, proposal drafts, and
  release assets for forbidden private material.
- **Compatibility:** prove legacy surfaces remain callable and recoverable
  until the final barrier.
- **Recovery:** byte/configuration/digest uncertainty is a failed rollback.
- **Concurrency:** compare normalized write sets and dependency DAG before
  activating parallel consumer lanes.

## Optional and unavailable tools

- Provider or live model evaluation is not required.
- Network access is not required for shared gates or local release sealing.
- Public download verification remains unavailable until owner-approved
  publication.
- Consumer CI or retrieval runners that are unavailable remain separately
  typed; fixture success cannot replace them.
- HGTS work remains discovery-only while its checkout is absent.

## Design-and-SEIT checkpoint

This checkpoint is ready only when:

1. `design.md` and `seit.md` parse from the canonical directory;
2. all DES, CONTRACT, SEIT, requirement, and acceptance IDs are unique and
   traceable;
3. current failures are not presented as passing;
4. no `implementation.md` or hand-edited `review.html` was created; and
5. the receipt does not grant publication, consumer mutation, proposal
   submission, compatibility removal, Explorer, or Expedition authority.

Bearing owns baseline `review.html` generation after this checkpoint. The next
agent must reuse these exact sources, draft only `implementation.md`, and stop
again for deterministic review and owner route selection.

## 2026-07-23 Slice 2.1 wire-contract clarification

This section appends the verification procedures and fixture test matrix binding the existing Slice 2.1 command IDs (`CMD-EXACT-SHA`, `CMD-INSTALL-VERIFY`, `CMD-CONSUMER-PARITY`, `CMD-REFERENCE-AUDIT`, `CMD-ROLLBACK`, `CMD-RETIREMENT-PROOF`) to temporary standard-library-only offline fixtures without modifying existing requirements, contracts, slices, paths, or command flags.

### Fixture-based verification suite for wire contracts

Each command ID will be validated against temporary standard-library test fixtures to verify positive wire shapes, all required failure modes, and strict mutation containment.

#### 1. Positive wire shape verification

- **Consumer gate verification (`CMD-INSTALL-VERIFY`, `CMD-CONSUMER-PARITY`, `CMD-REFERENCE-AUDIT`, `CMD-ROLLBACK`):** Will be verified using complete, fully valid `ConsumerGateReceipt` fixtures across generic consumer test cases (including the six consumers `Alphazedehq`, `alphazede-sports`, `betbot`, `developers`, `hgts`, `alphazede-markets`), proving that valid `ReleaseIdentity`, `InstallSnapshot` (with evidence-backed `staged_path` and `prior_pin`), `ParityReceipt` (nested under authoritative `ReleaseIdentity`, with exact `semantic_rows[].native` and `.legacy` keys), `hook_check`, `reference_audit` (with `expected_compatibility`), and `RollbackReceipt` (with exact `restored_paths` items) structures pass with status `passed` and exit code 0.
- **Six-consumer retirement verification (`CMD-RETIREMENT-PROOF`):** Will be verified using a complete, valid `RetirementManifest` referencing all six passing consumer receipts, valid evidence locator/digest objects for `active_reference_audit` (referencing strict JSON containing matching per-consumer status), `recovery_archive` (referencing a regular non-symlink archive), and `restoration_proof` (referencing strict JSON byte checks and commands), sorted normalized write/deletion path lists, and an `owner_approval_reference`; the fixture must prove successful evaluation without executing apply/post-apply commands and must prove identity-bound freshness.

#### 2. Negative wire shape and failure mode matrix

Verification commands will be executed against temporary fixtures containing single structural or semantic defects to confirm deterministic failure (non-zero exit code, typed blocker/diagnostic emission, and no side effects):

- **Duplicate key failure:** JSON fixture containing duplicate keys within an object (e.g. duplicate top-level keys or duplicate `archives` keys) is rejected.
- **Unknown behavioral key failure:** Objects containing unrecognized behavioral keys fail verification.
- **Bad digest failure:** SHA-256 strings containing invalid length, non-hex characters, uppercase hex, or failing bounded recomputation against actual evidence files cause verification failure.
- **Symlink / path traversal failure:** Evidence locators using absolute paths, empty strings, `.`, `..`, backslashes, or pointing to symlinks or files outside the evidence directory are rejected.
- **Wrong / dirty revision failure:** Receipt or audit revision failing to match the CLI revision argument or current clean git HEAD causes immediate failure.
- **InstallSnapshot evidence locator failure:** `InstallSnapshot` where `staged_path` or `prior_pin` evidence locator bytes do not hash to `selected_digest` or `prior_digest` fails verification.
- **Missing raw parity failure:** `ParityReceipt` missing either `native_raw` or `legacy_raw` evidence locator/digest objects, or where raw evidence is not a UTF-8 JSON array corresponding one-to-one with semantic rows, fails verification.
- **Missing semantic row key failure:** `ParityReceipt` where any `semantic_rows[].native` or `.legacy` object drops required keys (`locator`, `precedence`, `diagnostic_code`, `conflict`, `unavailable`, `outcome`) fails verification.
- **Unavailable parity failure:** `ParityReceipt` containing `unavailable` validation/retrieval status blocks consumer completion and exits with failure status.
- **Reference audit expected compatibility mismatch failure:** `reference_audit` missing a `compatibility-active` match for any object in `expected_compatibility`, or containing a `compatibility-active` match absent from `expected_compatibility`, fails verification.
- **Unexpected / unclassified reference failure:** `reference_audit` containing `unexpected-active` or `unclassified` classifications fails gate verification.
- **RollbackReceipt restored paths failure:** `RollbackReceipt` with `restored_paths` item missing exact keys (`path`, `sha256`, `source`), with invalid `source`, escaping path, or mismatching SHA-256 fails verification.
- **Partial rollback failure:** `RollbackReceipt` with unpassed byte checks, unpassed command execution records, or missing restored paths/digests reports failure.
- **Stale identity failure:** `ReleaseIdentity` mismatch between consumer gate receipt, install snapshot, and selected release manifest causes failure.
- **Retirement referenced evidence failure:** `RetirementManifest` where `active_reference_audit` or `restoration_proof` referenced file is not strict JSON with exact required keys, or where `recovery_archive` is a symlink, fails verification.
- **Missing / five / seven / duplicate consumer failure:** `RetirementManifest` containing fewer than six consumers (e.g. 5), more than six (e.g. 7), missing consumers, or duplicate consumer entries is rejected.
- **Absent approval reference failure:** `RetirementManifest` with an empty or missing `owner_approval_reference` fails verification.

#### 3. Output determinism and mutation containment

- **Deterministic stdout:** Command stdout will be verified to be byte-for-byte identical across repeated runs on identical fixture inputs, with output sorted lexically by consumer, path, and semantic key, and with compact canonical JSON digests computed via UTF-8 `json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)`.
- **Before/after directory snapshots:** Pre-run and post-run file system tree and hash snapshots will verify that read-only commands (`install-verify`, `parity`, `reference-audit`, `rollback`, and retirement `verify`) perform zero writes, zero file creations, zero deletions, and zero mutations on the target repository or evidence directory.
