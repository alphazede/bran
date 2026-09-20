---
type: implementation
name: bran-okf-final-cutover
status: complete
date: 2026-07-23
applies_to: bran
plan_spec: ./plan-spec.md
design: ./design.md
seit: ./seit.md
planning_route: codex gpt-5.6-sol
planning_reasoning: high
---

## Dependencies

- Wave 1: Slice 1.1, then Slice 1.2.
- Wave 2: Slice 2.1 and Slice 2.2 after Wave 1.
- Wave 3: Slice 3.1 after Wave 2.
- Wave 4: Slice 4.1, Slice 4.2, Slice 4.3, Slice 4.4, Slice 4.5, and Slice 4.6 after Wave 3; their writers are disjoint.
- Wave 5: Slice 5.1 after every Wave 4 consumer reaches a terminal receipt.
- Wave 6: Slice 6.1 only after Slice 5.1 proves eligibility and the owner separately approves retirement.

## Phase 1 — Shared BRAN gates

### Slice 1.1 — Portable reserved-document conformance

**Goal.** Close the portable reserved `index.md` and `log.md` coverage gap while keeping strict results independent.

**Requirement IDs.** AC-002, AC-008; REQ-GATE-001, REQ-GATE-005.

**Design IDs.** DES-001, DES-002, DES-003, CONTRACT-001, CONTRACT-002.

**SEIT proof rows.** SEIT-001, SEIT-002, SEIT-003.

**Type.** code

**Design lenses.** CDD, SecDD, RDD, ODD, OOPDSA.

**Implementation role.** Crewmate

**Agent model route.** codex gpt-5.6-terra

**Agent reasoning level.** medium

**Ponytail mode.** full

**Review path.** Harness-native read-only reviewer; Surveyor fallback only when unavailable.

### 1.1 execution manifest

**Write set.** Write only `crates/bran-core/src/profile.rs`, `fixtures/conformance/okf-v0.1-index-valid.fixture`, `fixtures/conformance/okf-v0.1-index-invalid.fixture`, `fixtures/conformance/okf-v0.1-log-valid.fixture`, `fixtures/conformance/okf-v0.1-log-invalid.fixture`, `tools/ci/test-budget.json`.

**Command IDs.** CMD-OKF-PORTABLE, CMD-PROFILES.

**Stop condition.** Stop if the normative portable structure cannot be cited, a strict-only rule enters the portable result, or the write set must expand.

**Human decision.** None after the normative source and exact write set are verified; otherwise stop for owner scope direction.

### Slice 1.2 — Public-root repair and fast-gate regression

**Goal.** Remove the doubled-path failure and update `tools/ci/test-budget.json` and `tools/ci/test_budget_check.py` as a deterministic inventory of named CI journeys, direct CI commands, and owned fixtures—not one registry row per Rust unit test. Preserve `CMD-BUDGET` as the first fast-gate check, proving deterministic negative failure on missing/duplicate journeys, direct commands, or fixture ownership, while all Rust unit tests remain mandatory through `CMD-FAST` workspace test commands.

**Requirement IDs.** AC-002; REQ-GATE-002, REQ-GATE-003, REQ-GATE-004, REQ-GATE-005.

**Design IDs.** DES-004, DES-005, CONTRACT-003.

**SEIT proof rows.** SEIT-004, SEIT-005, SEIT-006.

**Type.** code

**Design lenses.** CDD, SecDD, RDD, ODD, OOPDSA.

**Implementation role.** Crewmate

**Agent model route.** codex gpt-5.6-terra

**Agent reasoning level.** medium

**Ponytail mode.** full

**Review path.** Harness-native read-only reviewer; Surveyor fallback only when unavailable.

### 1.2 execution manifest

**Write set.** Write only `tools/ci/public_boundary_check.py`, `tools/ci/test-budget.json`, `tools/ci/test_budget_check.py`, `crates/bran-core/src/migration.rs`, `crates/bran-core/src/packet/mod.rs`, `crates/bran-core/src/policy.rs`, `crates/bran-core/src/profile.rs`, `crates/bran-core/src/scan/mod.rs`, `crates/bran-cli/src/main.rs`.

**Command IDs.** CMD-BUDGET, CMD-PUBLIC, CMD-FAST.

**Stop condition.** Stop if the checker must scan outside the BRAN/public export surface, depends on caller CWD, or the budget check no longer fails first on missing or duplicate inventory entries. Stop if the clippy repair requires semantic behavior change or lint suppression.

**Human decision.** None unless the public export boundary itself must change.

## Phase 2 — Cutover contracts and proposal drafts

### Slice 2.1 — Cutover verification tools

**Goal.** Add the bounded offline verifiers required for exact release, consumer parity, reference, rollback, retirement, and route receipts.

**Requirement IDs.** AC-001, AC-003, AC-004, AC-005, AC-006; REQ-REL-003, REQ-REL-004, REQ-REL-005, REQ-CONS-002, REQ-CONS-003, REQ-CONS-004, REQ-CONS-005, REQ-CONS-006, REQ-PARITY-001, REQ-COMPAT-001, REQ-RETIRE-001, REQ-PLAN-002, REQ-PLAN-003, REQ-PLAN-004.

**Design IDs.** DES-010, DES-011, DES-013, DES-014, DES-017, DES-019, DES-021, DES-025, CONTRACT-005, CONTRACT-006, CONTRACT-007, CONTRACT-010, CONTRACT-011, CONTRACT-013.

**SEIT proof rows.** SEIT-010, SEIT-011, SEIT-012, SEIT-019, SEIT-020, SEIT-021, SEIT-023, SEIT-029, SEIT-031.

**Type.** code

**Design lenses.** CDD, SecDD, RDD, ODD, OOPDSA.

**Implementation role.** Crewmate

**Agent model route.** codex gpt-5.6-terra

**Agent reasoning level.** medium

**Ponytail mode.** full

**Review path.** Harness-native read-only reviewer; Surveyor fallback only when unavailable.

### 2.1 execution manifest

**Write set.** Write only `tools/cutover/verify_release.py`, `tools/cutover/consumer_gate.py`, `tools/cutover/retirement_gate.py`, `tools/cutover/validate_route.py`, `tools/ci/test-budget.json`.

**Command IDs.** CMD-EXACT-SHA, CMD-INSTALL-VERIFY, CMD-CONSUMER-PARITY, CMD-REFERENCE-AUDIT, CMD-ROLLBACK, CMD-RETIREMENT-PROOF, CMD-ROUTE-TRACE.

**Stop condition.** Stop on a dependency, network requirement, hidden consumer rule, write-capable default, semantic-normalization loss, or path outside the exact write set.

**Human decision.** None; every later side-effect mode remains separately gated.

### Slice 2.2 — Upstream OKF proposal drafts

**Goal.** Draft the bundle-scope clarification and layered-profile issue without external submission.

**Requirement IDs.** AC-007, AC-008; REQ-UPSTREAM-001, REQ-UPSTREAM-002, REQ-UPSTREAM-003.

**Design IDs.** DES-001, DES-009, DES-023, CONTRACT-001, CONTRACT-012.

**SEIT proof rows.** SEIT-026, SEIT-027, SEIT-028.

**Type.** documentation

**Design lenses.** CDD, SecDD, RDD, ODD.

**Implementation role.** Crewmate

**Agent model route.** agy agent default

**Agent reasoning level.** medium

**Ponytail mode.** off

**Review path.** Harness-native read-only reviewer; Surveyor fallback only when unavailable.

### 2.2 execution manifest

**Write set.** Write only `docs/integrations/proposals/okf-bundle-scan-scope.md`, `docs/integrations/proposals/okf-layered-profile-separation.md`.

**Command IDs.** PROC-PROPOSAL-DRAFTS, CMD-PROFILES.

**Stop condition.** Stop if either draft claims uncited normative behavior, combines the proposals, or implies submission/publication authority.

**Human decision.** Separate owner approval is required later for each exact external text and destination.

## Phase 3 — Stable release

### Slice 3.1 — Reproducible seal, publication gate, and exact public verification

**Goal.** Build and seal the exact multi-platform release, stop for publication approval, then verify immutable public assets.

**Requirement IDs.** AC-003; REQ-REL-001, REQ-REL-002, REQ-REL-003, REQ-REL-006, REQ-UPSTREAM-003.

**Design IDs.** DES-006, DES-007, DES-008, DES-009, CONTRACT-004.

**SEIT proof rows.** SEIT-007, SEIT-008, SEIT-009.

**Type.** operational

**Design lenses.** CDD, SecDD, RDD, ODD.

**Implementation role.** Crewmate

**Agent model route.** agy agent default

**Agent reasoning level.** medium

**Ponytail mode.** off

**Review path.** Harness-native read-only reviewer; Surveyor fallback only when unavailable.

### 3.1 execution manifest

**Write set.** No writes required in the repository; release archives, seal evidence, and public readback receipts remain outside the source checkout.

**Command IDs.** CMD-RELEASE-PLAN, CMD-RELEASE-BUILD-LINUX-X86, CMD-RELEASE-BUILD-LINUX-ARM, CMD-RELEASE-BUILD-MAC-X86, CMD-RELEASE-BUILD-MAC-ARM, CMD-RELEASE-BUILD-WINDOWS-X86, CMD-RELEASE-CONTRACT, CMD-RELEASE-DRY-SEAL, CMD-RELEASE-SEAL, CMD-EXACT-SHA, PROC-PUBLICATION.

**Stop condition.** Stop on dirty source, missing target, non-reproducible archive, identity/signature/public-boundary mismatch, floating URL, or approval drift.

**Human decision.** Owner approval of the exact tag, source SHA, digest set, fingerprint, destination, and boundary receipt is required before signing, uploading, publishing, or public installation verification.

## Phase 4 — Disjoint consumer migrations

### Slice 4.1 — AlphazedeHQ migration and parity

**Goal.** Install the exact release, switch the BRAN-backed advisory path, prove parity and rollback, and retain every compatibility surface.

**Requirement IDs.** AC-004, AC-005, AC-006; REQ-REL-004, REQ-REL-005, REQ-CONS-001, REQ-CONS-003, REQ-CONS-004, REQ-CONS-005, REQ-CONS-006, REQ-CONS-007, REQ-PARITY-001, REQ-COMPAT-001, REQ-COMPAT-002.

**Design IDs.** DES-010, DES-011, DES-012, DES-013, DES-014, DES-017, DES-019, DES-020, CONTRACT-005, CONTRACT-006, CONTRACT-007, CONTRACT-010, CONTRACT-013.

**SEIT proof rows.** SEIT-010, SEIT-011, SEIT-012, SEIT-013, SEIT-019, SEIT-020, SEIT-021, SEIT-022.

**Type.** consumer

**Design lenses.** CDD, SecDD, RDD, ODD, OOPDSA.

**Implementation role.** Crewmate

**Agent model route.** codex gpt-5.6-terra

**Agent reasoning level.** medium

**Ponytail mode.** full

**Review path.** Harness-native read-only reviewer; Surveyor fallback only when unavailable.

### 4.1 execution manifest

**Write set.** Within the AlphazedeHQ checkout write only `.bran/policy.yaml`, `.bran/evidence/bran-cutover.json`, `.grok/hooks/use-okf.sh`, `.grok/hooks/use-okf.json`, `tools/okf/runtime/bran-release-pin.json`.

**Command IDs.** CMD-EXACT-SHA, CMD-INSTALL-VERIFY, CMD-CONSUMER-PARITY, CMD-REFERENCE-AUDIT, CMD-ROLLBACK, CMD-ROUTE-TRACE, PROC-PARITY-ALPHAZEDEHQ.

**Stop condition.** Stop on repository/revision mismatch, unavailable retrieval, hook contract drift, unexpected active reference, parity failure, or rollback mismatch.

**Human decision.** Owner approval of this checkout, revision, artifact digest, exact write set, commands, and rollback is required before mutation.

### Slice 4.2 — AlphaZede Sports migration and granular boundary parity

**Goal.** Install the exact release and prove the repository's granular public-boundary policy, parity, references, and rollback.

**Requirement IDs.** AC-004, AC-005, AC-006; REQ-REL-004, REQ-REL-005, REQ-CONS-001, REQ-CONS-003, REQ-CONS-004, REQ-CONS-005, REQ-CONS-006, REQ-CONS-007, REQ-AZS-001, REQ-PARITY-001, REQ-COMPAT-001, REQ-COMPAT-002.

**Design IDs.** DES-010, DES-011, DES-012, DES-013, DES-014, DES-015, DES-017, DES-019, DES-020, CONTRACT-005, CONTRACT-006, CONTRACT-007, CONTRACT-008, CONTRACT-010, CONTRACT-013.

**SEIT proof rows.** SEIT-010, SEIT-011, SEIT-012, SEIT-014, SEIT-019, SEIT-020, SEIT-021, SEIT-022.

**Type.** consumer

**Design lenses.** CDD, SecDD, RDD, ODD, OOPDSA.

**Implementation role.** Crewmate

**Agent model route.** codex gpt-5.6-terra

**Agent reasoning level.** medium

**Ponytail mode.** full

**Review path.** Harness-native read-only reviewer; Surveyor fallback only when unavailable.

### 4.2 execution manifest

**Write set.** Within the alphazede-sports checkout write only `.bran/policy.yaml`, `.bran/evidence/bran-cutover.json`, `tools/okf/runtime/bran-release-pin.json`.

**Command IDs.** CMD-EXACT-SHA, CMD-INSTALL-VERIFY, CMD-CONSUMER-PARITY, CMD-REFERENCE-AUDIT, CMD-ROLLBACK, CMD-ROUTE-TRACE, PROC-PARITY-ALPHAZEDE-SPORTS.

**Stop condition.** Stop on repository/revision mismatch, ambiguous boundary precedence, unmatched required path, unavailable retrieval, parity/reference failure, or rollback mismatch.

**Human decision.** Owner approval of this checkout, revision, boundary rules, artifact digest, exact write set, commands, and rollback is required before mutation.

### Slice 4.3 — BetBot migration and oversized-document resolution

**Goal.** Install the exact release and resolve the oversized knowledge document without raising BRAN's global limit, then prove parity and rollback.

**Requirement IDs.** AC-004, AC-005, AC-006; REQ-REL-004, REQ-REL-005, REQ-CONS-001, REQ-CONS-003, REQ-CONS-004, REQ-CONS-005, REQ-CONS-006, REQ-CONS-007, REQ-BETBOT-001, REQ-PARITY-001, REQ-COMPAT-001, REQ-COMPAT-002.

**Design IDs.** DES-010, DES-011, DES-012, DES-013, DES-014, DES-016, DES-017, DES-019, DES-020, CONTRACT-005, CONTRACT-006, CONTRACT-007, CONTRACT-009, CONTRACT-010, CONTRACT-013.

**SEIT proof rows.** SEIT-010, SEIT-011, SEIT-012, SEIT-015, SEIT-019, SEIT-020, SEIT-021, SEIT-022.

**Type.** consumer

**Design lenses.** CDD, SecDD, RDD, ODD, OOPDSA.

**Implementation role.** Crewmate

**Agent model route.** codex gpt-5.6-terra

**Agent reasoning level.** medium

**Ponytail mode.** full

**Review path.** Harness-native read-only reviewer; Surveyor fallback only when unavailable.

### 4.3 execution manifest

**Write set.** Within the BetBot checkout write only `.bran/policy.yaml`, `.bran/evidence/bran-cutover.json`, `.bran/migrations/oversized-document-plan.json`, `docs/okf`, `tools/okf/runtime/bran-release-pin.json`.

**Command IDs.** CMD-EXACT-SHA, CMD-INSTALL-VERIFY, CMD-CONSUMER-PARITY, CMD-REFERENCE-AUDIT, CMD-ROLLBACK, CMD-ROUTE-TRACE, PROC-PARITY-BETBOT.

**Stop condition.** Stop if the exact oversized source is outside `docs/okf`, the semantic split lacks owner approval, locators/relationships change without mapping, retrieval is unavailable, parity fails, or rollback differs.

**Human decision.** Owner approval of the exact source and split paths, checkout revision, artifact digest, write set, commands, and rollback is required before mutation.

### Slice 4.4 — developers public consumer migration and parity

**Goal.** Install the exact public release and prove exported-source identity, parity, references, and rollback.

**Requirement IDs.** AC-004, AC-005, AC-006; REQ-REL-004, REQ-REL-005, REQ-CONS-001, REQ-CONS-003, REQ-CONS-004, REQ-CONS-005, REQ-CONS-006, REQ-CONS-007, REQ-PARITY-001, REQ-COMPAT-001, REQ-COMPAT-002.

**Design IDs.** DES-010, DES-011, DES-012, DES-013, DES-014, DES-017, DES-019, DES-020, CONTRACT-005, CONTRACT-006, CONTRACT-007, CONTRACT-010, CONTRACT-013.

**SEIT proof rows.** SEIT-010, SEIT-011, SEIT-012, SEIT-016, SEIT-019, SEIT-020, SEIT-021, SEIT-022.

**Type.** consumer

**Design lenses.** CDD, SecDD, RDD, ODD, OOPDSA.

**Implementation role.** Crewmate

**Agent model route.** codex gpt-5.6-terra

**Agent reasoning level.** medium

**Ponytail mode.** full

**Review path.** Harness-native read-only reviewer; Surveyor fallback only when unavailable.

### 4.4 execution manifest

**Write set.** Within the developers checkout write only `.bran/policy.yaml`, `.bran/evidence/bran-cutover.json`, `tools/okf/runtime/bran-release-pin.json`.

**Command IDs.** CMD-EXACT-SHA, CMD-INSTALL-VERIFY, CMD-CONSUMER-PARITY, CMD-REFERENCE-AUDIT, CMD-ROLLBACK, CMD-ROUTE-TRACE, PROC-PARITY-DEVELOPERS.

**Stop condition.** Stop on repository/revision mismatch, export/source identity mismatch, floating asset, unavailable retrieval, parity/reference failure, or rollback mismatch.

**Human decision.** Owner approval of this checkout, revision, artifact digest, exact write set, commands, and rollback is required before mutation.

### Slice 4.5 — HGTS discovery, migration, and parity

**Goal.** Verify HGTS identity when available, then install and prove parity and rollback without substituting another checkout.

**Requirement IDs.** AC-004, AC-005, AC-006; REQ-REL-004, REQ-REL-005, REQ-CONS-001, REQ-CONS-003, REQ-CONS-004, REQ-CONS-005, REQ-CONS-006, REQ-CONS-007, REQ-HGTS-001, REQ-PARITY-001, REQ-COMPAT-001, REQ-COMPAT-002.

**Design IDs.** DES-010, DES-011, DES-012, DES-013, DES-014, DES-017, DES-018, DES-019, DES-020, CONTRACT-005, CONTRACT-006, CONTRACT-007, CONTRACT-010, CONTRACT-013.

**SEIT proof rows.** SEIT-010, SEIT-011, SEIT-012, SEIT-017, SEIT-019, SEIT-020, SEIT-021, SEIT-022.

**Type.** consumer

**Design lenses.** CDD, SecDD, RDD, ODD, OOPDSA.

**Implementation role.** Crewmate

**Agent model route.** codex gpt-5.6-terra

**Agent reasoning level.** medium

**Ponytail mode.** full

**Review path.** Harness-native read-only reviewer; Surveyor fallback only when unavailable.

### 4.5 execution manifest

**Write set.** When HGTS is absent, write nothing; after identity and owner approval, within the HGTS checkout write only `.bran/policy.yaml`, `.bran/evidence/bran-cutover.json`, `tools/okf/runtime/bran-release-pin.json`.

**Command IDs.** CMD-EXACT-SHA, CMD-INSTALL-VERIFY, CMD-CONSUMER-PARITY, CMD-REFERENCE-AUDIT, CMD-ROLLBACK, CMD-ROUTE-TRACE, PROC-PARITY-HGTS.

**Stop condition.** Stop with `unavailable` while HGTS is absent, or on identity/revision mismatch, unavailable retrieval, parity/reference failure, or rollback mismatch.

**Human decision.** Owner approval of the verified checkout, revision, artifact digest, exact write set, commands, and rollback is required before any HGTS mutation.

### Slice 4.6 — alphazede-markets migration and parity

**Goal.** Install the exact release and prove policy, validation/retrieval parity, references, and rollback.

**Requirement IDs.** AC-004, AC-005, AC-006; REQ-REL-004, REQ-REL-005, REQ-CONS-001, REQ-CONS-003, REQ-CONS-004, REQ-CONS-005, REQ-CONS-006, REQ-CONS-007, REQ-PARITY-001, REQ-COMPAT-001, REQ-COMPAT-002.

**Design IDs.** DES-010, DES-011, DES-012, DES-013, DES-014, DES-017, DES-019, DES-020, CONTRACT-005, CONTRACT-006, CONTRACT-007, CONTRACT-010, CONTRACT-013.

**SEIT proof rows.** SEIT-010, SEIT-011, SEIT-012, SEIT-018, SEIT-019, SEIT-020, SEIT-021, SEIT-022.

**Type.** consumer

**Design lenses.** CDD, SecDD, RDD, ODD, OOPDSA.

**Implementation role.** Crewmate

**Agent model route.** codex gpt-5.6-terra

**Agent reasoning level.** medium

**Ponytail mode.** full

**Review path.** Harness-native read-only reviewer; Surveyor fallback only when unavailable.

### 4.6 execution manifest

**Write set.** Within the alphazede-markets checkout write only `.bran/policy.yaml`, `.bran/evidence/bran-cutover.json`, `tools/okf/runtime/bran-release-pin.json`.

**Command IDs.** CMD-EXACT-SHA, CMD-INSTALL-VERIFY, CMD-CONSUMER-PARITY, CMD-REFERENCE-AUDIT, CMD-ROLLBACK, CMD-ROUTE-TRACE, PROC-PARITY-ALPHAZEDE-MARKETS.

**Stop condition.** Stop on repository/revision mismatch, unavailable retrieval, policy/parity/reference failure, or rollback mismatch.

**Human decision.** Owner approval of this checkout, revision, artifact digest, exact write set, commands, and rollback is required before mutation.

## Phase 5 — Global evidence barrier

### Slice 5.1 — Retirement eligibility and restoration rehearsal

**Goal.** Aggregate six immutable consumer receipts, audit active references, and prove restoration before retirement can be recommended.

**Requirement IDs.** AC-004, AC-005, AC-006; REQ-CONS-006, REQ-COMPAT-001, REQ-RETIRE-001, REQ-RETIRE-002, REQ-RETIRE-003.

**Design IDs.** DES-011, DES-020, DES-021, DES-022, CONTRACT-006, CONTRACT-010, CONTRACT-011.

**SEIT proof rows.** SEIT-021, SEIT-022, SEIT-023, SEIT-024, SEIT-029.

**Type.** operational

**Design lenses.** CDD, SecDD, RDD, ODD, OOPDSA.

**Implementation role.** Crewmate

**Agent model route.** agy agent default

**Agent reasoning level.** medium

**Ponytail mode.** off

**Review path.** Harness-native read-only reviewer; Surveyor fallback only when unavailable.

### 5.1 execution manifest

**Write set.** No writes required in consumer repositories; the retirement manifest, reference audit, and restoration rehearsal remain owner-reviewable runtime evidence.

**Command IDs.** CMD-REFERENCE-AUDIT, CMD-RETIREMENT-PROOF, PROC-RETIREMENT.

**Stop condition.** Stop on any non-passing, stale, missing, or unavailable consumer receipt, unexpected active reference, recovery archive mismatch, or failed restoration rehearsal.

**Human decision.** No removal is authorized; return the exact retirement packet for a separate owner decision.

## Phase 6 — Separately approved global retirement

### Slice 6.1 — Compatibility retirement transaction

**Goal.** After separate approval, apply the exact global retirement manifest, run integrated gates, and restore on any failure.

**Requirement IDs.** AC-006; REQ-COMPAT-001, REQ-COMPAT-002, REQ-RETIRE-001, REQ-RETIRE-002, REQ-RETIRE-003.

**Design IDs.** DES-011, DES-021, DES-022, CONTRACT-006, CONTRACT-011.

**SEIT proof rows.** SEIT-023, SEIT-024, SEIT-025.

**Type.** destructive

**Design lenses.** CDD, SecDD, RDD, ODD, OOPDSA.

**Implementation role.** Crewmate

**Agent model route.** agy agent default

**Agent reasoning level.** medium

**Ponytail mode.** off

**Review path.** Harness-native read-only reviewer; Surveyor fallback only when unavailable.

### 6.1 execution manifest

**Write set.** Across the owner-approved Alphazede workspace write only `Alphazedehq/.grok/hooks/use-okf.sh`, `Alphazedehq/.grok/hooks/use-okf.json`, `Alphazedehq/skills/use-okf`, `Alphazedehq/tools/okf/okf`, `Alphazedehq/tools/okf/config.yaml`, `alphazede-sports/tools/okf/config.yaml`, `betbot/tools/okf/config.yaml`, `developers/tools/okf/config.yaml`, `hgts/tools/okf/config.yaml`, `alphazede-markets/tools/okf/config.yaml`.

**Command IDs.** CMD-RETIREMENT-PROOF, PROC-RETIREMENT, CMD-FAST.

**Stop condition.** Stop before writes without six passing receipts and exact approval; after writes, stop and restore on manifest drift, extra deletion, active reference, or any integrated-gate failure.

**Human decision.** Separate explicit owner approval of the exact retirement manifest, deletion targets, recovery archive, restoration proof, commands, and evidence is mandatory.
