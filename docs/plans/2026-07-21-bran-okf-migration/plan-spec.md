---
type: plan-spec
name: bran-okf-migration
status: draft
date: 2026-07-21
applies_to: bran
parent_baseline: legacy use-okf skill and tools/okf/okf
---

## Problem

Internal repository validation, retrieval, and repair currently flow through legacy
`use-okf` and `tools/okf/okf` in mixed ownership and shared command posture, while
BRAN has an independent parser/validator/repair stack already in-tree.
Owner constraints are now explicit, and migration proceeds under evidence-grounded
staging and coordinated inventory tracking.

## Goal

- Migrate to BRAN-core/CLI-owned validation and maintenance behavior while preserving
  compatibility-only support for legacy `use-okf` and `tools/okf/okf` adapter
  entrypoints until parity is proven.
- Preserve evidence-only/read-only default semantics and stronger-source precedence
  outcomes during migration.
- Make self-healing repair authority explicit and bounded BRAN-owned derived
  state.

## Scope

In scope:

- Retrieval, validation, and maintenance command/receipt behavior in BRAN core and CLI
  surfaces.
- Compatibility posture for legacy command names and per-repository OKF configurations.
- Evidence on proposal/apply/revalidate repair semantics, typed success/failure states,
  rollback and receipt attribution, and boundary conditions.
- Compatibility surface in neighboring BRAN consumers is treated as migration target
  inventory only (not implementation authorization).
- Retirement evidence for active consumers is in-scope, with repository-by-repository
  migration status preserved until all active consumers are eligible for adapter retirement.

Out of scope:

- Schema/serialization details for new BRAN-native policy formats not yet finalized.
- Hard-coded legacy-adapter retirement window or fixed cutover date.
- New owner decisions for compatibility removal criteria beyond confirmed parity.

## Current behavior

- In `Alphazedehq`, `/home/spectre/alphazede/Alphazedehq/.grok/hooks/use-okf.sh` and
  `/home/spectre/alphazede/Alphazedehq/.grok/hooks/use-okf.json` are the current
  `use-okf` compatibility surfaces; both are evidence-only and read-only by policy
  (`/home/spectre/alphazede/Alphazedehq/.grok/hooks/README.md:24-53`).
- In `Alphazedehq`, `/home/spectre/alphazede/Alphazedehq/tools/okf/okf` exposes legacy
  command forms (`check`, `doctor`, `bran-verify-pin`, `bran-shadow`) and uses legacy policy
  inputs in its adapter layer.
- BRAN CLI currently exposes `maintain <propose|apply|revalidate>` and typed exits
  (`Validation`, `Operation`, `Usage`, `Success`) in command help (`crates/bran-cli/src/main.rs:74-77,109-117`).
- BRAN repair core currently models explicit `RepairTerminal` states, proposal
  digests, unsafe-path checks, and rollback-on-failed-validation behavior
  (`crates/bran-core/src/repair/mod.rs:1-17,89-117,189-295`).
- Canonical retrieval indexes in BRAN core include canonical/status/freshness/authority
  keys and path/title/tag matching for deterministic source selection
  (`crates/bran-core/src/graph/query.rs:1596-1620`).
- Metadata frontmatter handling and parser fallback remain implemented in BRAN core
  (`crates/bran-core/src/metadata/mod.rs:248,737`).

## Target behavior

- Staged replacement: BRAN becomes canonical internal repository-knowledge and
  validation implementation; legacy `use-okf` and `tools/okf/okf` remain compatibility
  adapters only until parity is proven.
- Strict repository validation is BRAN-native behavior (including required/allowed
  frontmatter validation, metadata/status coverage, source-link integrity, source tag
  policy, public/private boundaries, deterministic reports, and typed unavailable/conflict
  behavior), with adapters only translating invocation/result shape during migration.
- Self-healing authority is split by ownership:
  BRAN may auto-rebuild only BRAN-derived state such as indexes, snapshots, caches,
  reports, and generated validator artifacts.
- Repository source, metadata, classifications, configurations, and source links are never
  rewritten silently.
- BRAN can propose bounded repair operations, then require explicit authority with a digest,
  then revalidate; validator failure restores original bytes and emits a restoration receipt path.

## Use cases

### UC-1 Validation parity use case
BRAN-native strict validation handles repository checks and returns deterministic pass/fail
exit plus structured output for repository boundary violations.

### UC-2 Legacy-compatibility check
Legacy `use-okf` and `tools/okf/okf` commands remain callable as adapters and map into
BRAN-native behavior without changing core outcomes during migration.

### UC-3 Retrieval use case
Queries continue to rank and select canonical evidence deterministically using canonical,
authority, status, and path/title/tag fields.

### UC-4 Self-heal proposal use case
`maintain propose` returns digest and target metadata without mutating repository source.

### UC-5 Apply with explicit authority
`maintain apply` requires a replacement plan and non-empty authority reason, verifies digest
and staleness, performs mutation, and only reports success after revalidation.

### UC-6 Failure and rollback use case
Validation failure after apply restores the exact prior target state and returns failure receipts
with explicit failure terminal state.

### UC-7 Boundary safety use case
Repository/classification/source-link checks enforce boundaries, with explicit diagnostics for
unsafe paths, stale source, missing authority marker, and unavailable checks.

## Components

- `crates/bran-cli/src/main.rs`: CLI command surface (`maintain`) and envelope
  exits.
- `crates/bran-core/src/repair/mod.rs`: proposal/apply/state machine and restoration receipts.
- `crates/bran-core/src/metadata/mod.rs`: metadata parsing and header extraction behavior.
- `crates/bran-core/src/graph/query.rs`: deterministic retrieval path/title/tag/canonical
  match behavior.
- `Alphazedehq` compatibility tooling (`/home/spectre/alphazede/Alphazedehq/.grok/hooks/use-okf.sh`,
  `/home/spectre/alphazede/Alphazedehq/tools/okf/okf`).

## Acceptance criteria

- **AC-1**: Staged replacement keeps BRAN as canonical behavior for repository validation
  while legacy compatibility remains explicitly adapter-only until explicit parity sign-off.
- **AC-2**: BRAN strict validation behavior remains behaviorally explicit in core/CLI and
  continues to enforce frontmatter parsing, source-link handling, and boundary policy.
- **AC-3**: Repository maintenance flow enforces read-only proposal, explicit authority on apply,
  digest matching, and revalidation before success.
- **AC-4**: Validation failures in apply path restore original target content and emit an
  attributable failure lifecycle value indicating restoration.
- **AC-5**: Retrieval and compatibility flows preserve stronger-source precedence outcomes and
  do not require unverified source-policy claims.
- **AC-6**: No repository rewrite happens without explicit, bounded BRAN-owned authority and
  compatible revalidation result.
- **AC-7**: Legacy `use-okf`, `tools/okf/okf`, legacy hooks, and legacy configurations are
  retired only after every active consumer has migrated to BRAN-native policy and BRAN-backed hook/skill.
  Required evidence remains BRAN-vs-legacy retrieval and validation parity, plus audit that no active code,
  skill, hook, or CI reference uses legacy entrypoints/configuration outside explicitly historical documents.

## Risks and open questions

- **RISK-1**: [Non-blocking] Active consumers can progress on different schedules; one consumer delay can
  delay global adapter removal but does not invalidate completed migration evidence from completed consumers.
- **RISK-2**: [Non-blocking] Hook behavior migration requires careful implementation of fail-open
  fallback and compatibility-only semantics, but owner-confirmed posture is now fixed.
- **RISK-3**: [Non-blocking] Publication sequencing is owner-authorized and constrained by public-boundary
  review, with no remaining migration-blocking ambiguity.
- **RISK-4**: [Non-blocking] Native schema filename/serialization details remain design concerns and are
  intentionally deferred until implementation design review.
- **RISK-5**: [Non-blocking] Compatibility parity evidence cadence will remain a planning and QA scheduling
  pressure rather than a hard migration blocker.

## Owner decisions

1. Staged replacement. BRAN becomes the canonical internal repository-knowledge and
   validation implementation. The legacy `use-okf` skill and `tools/okf/okf` command remain
   deprecated compatibility adapters only until consumer parity is proven and owner resolves retirement
   criteria. There is no hard same-change cutover.
2. Strict repository validation is native BRAN Core/CLI behavior, not a call through to the legacy
   validator. It must preserve required/allowed frontmatter validation, canonical/legacy/excluded/unclassified
   coverage, source-link integrity, repository tag/status policy, public/private boundary enforcement,
   deterministic reports, and compatible typed exit behavior. The compatibility adapter may translate
   old invocations/results during migration.
3. Self-healing authority is split by ownership. BRAN may automatically rebuild only BRAN-owned derived
  state such as indexes, snapshots, caches, reports, and generated validator artifacts. Repository source,
  metadata, classifications, configuration, and source links are never silently rewritten. BRAN may propose a
  bounded repair; apply requires explicit authority and the exact proposal digest, then revalidation.
  Validation failure restores the exact original bytes and emits an attributable receipt.

4. Configuration relationship. BRAN owns a new native repository-policy schema. Existing
   `tools/okf/config.yaml` inputs are migration-only and accepted read-only through the
   compatibility adapter. They are never automatically rewritten. Repository conversion is deliberate
   and occurs only after parity evidence. Exact native filename and serialization remain a design
   concern unless owner input is genuinely required.
5. Hook behavior. Preserve the existing use-okf hook logic while replacing its implementation
   with BRAN: SessionStart lightweight availability, relevant-keyword UserPromptSubmit, and
   relevant-file PostToolUse; dynamic managed-repository discovery; trusted executable resolved from
   the hook's physical checkout or owner-approved stable path; bounded timeouts; compact output;
   graceful unavailable fallback; always exit success/fail-open; no hook-driven source mutation.
   Strict failures remain exclusive to explicit BRAN validation or configured CI.
6. Adoption/publication order. Adopt an exact locally built BRAN artifact first using a checksum pin
  and stable local path. Run compatibility/shadow and consumer parity before public publication. A public
  download/install is later release-install verification, not a prerequisite for internal adoption.
  Publication remains owner-authorized and occurs only after migration parity and public-boundary review.

7. Legacy retirement is evidence-based, not date-based. There is no arbitrary migration window or adapter
   retirement date. Retirement of legacy `use-okf`, `tools/okf/okf`, legacy hooks, and legacy
   configurations occurs only when every active consumer repository has deliberately migrated to BRAN-native
   policy and BRAN-backed hook/skill. Required evidence includes:
   (a) BRAN-vs-legacy validation and retrieval compatibility parity passes; and
   (b) confirmed absence of active code, skill, hook, or CI references to legacy entrypoints/configuration
       outside explicitly historical documents. A migration problem in one repository can delay only that repository
   and final global adapter removal, without invalidating completed migration evidence from other repositories.
   Active migration inventory confirmed by focused discovery: Alphazedehq, alphazede-sports, betbot,
   developers, hgts, and alphazede-markets.

## Sequencing

- Run compatibility inventory and parity evidence collection for retrieval and validation.
- Confirm repair lifecycle and rollback invariants with explicit failure/restore traces.
- Run per-repository BRAN-native migration and retire legacy entrypoints only when owner decision 7
  conditions and evidence are met; unresolved repositories remain on adapter pathways until complete.

## Evidence consulted

- `/home/spectre/alphazede/Alphazedehq/.grok/hooks/use-okf.sh:7-20,26-57,81-99,125-187`
- `/home/spectre/alphazede/Alphazedehq/.grok/hooks/use-okf.json:1-38`
- `/home/spectre/alphazede/Alphazedehq/.grok/hooks/README.md:24-53`
- `/home/spectre/alphazede/Alphazedehq/tools/okf/okf:56-85,88-174`
- `/home/spectre/alphazede/Alphazedehq/tools/okf/config.yaml:1`
- `/home/spectre/alphazede/alphazede-sports/tools/okf/config.yaml:1`
- `/home/spectre/alphazede/betbot/tools/okf/config.yaml:1`
- `/home/spectre/alphazede/developers/tools/okf/config.yaml:1`
- `/home/spectre/alphazede/hgts/tools/okf/config.yaml:1`
- `/home/spectre/alphazede/alphazede-markets/tools/okf/config.yaml:1`
- `/home/spectre/alphazede/Alphazedehq/AGENTS.md:106-123`
- `/home/spectre/alphazede/bran/AGENTS.md:32-44`
- `/home/spectre/alphazede/bran/docs/plans/AGENTS.md:1-24`
- `/home/spectre/alphazede/bran/crates/bran-core/src/metadata/mod.rs:248,737`
- `/home/spectre/alphazede/bran/crates/bran-core/src/graph/query.rs:1596,1600,1617-1620`
- `/home/spectre/alphazede/bran/crates/bran-core/src/repair/mod.rs:4,19,123,180-188,189-297`
- `/home/spectre/alphazede/bran/crates/bran-cli/src/main.rs:76,1994,2032,2124,2381`

## Handoff to design-driven-build

- **CDD pressure**: preserve staged replacement versus hard cutover, and boundary between proven parity
  and completed owner decisions.
- **SecDD pressure**: typed repair failures, authority validation, stale-digest/conflict handling, and
  rollback-receipt requirements.
- **RDD pressure**: retrieval precedence and compatibility adapter behavior (`legacy` vs `canonical`),
  with parity invariants preserved under owner decision 7 and active-consumer sequencing.
- **ODD pressure**: command surface/API posture and exit-state contracts (`Validation`, `Operation`,
  `Usage`, `Success`) across maintenance and adapter boundaries.
- **OOPDSA focus**: ownership boundaries for repair state, adapter layer split, and explicit authority
  transitions from proposal to revalidation/receipt.
- **SEIT obligations**: include positive and negative parity cases, legacy compatibility cases,
  validation failures, stale-source and stale-digest cases, and rollback assertions with evidence
  retention.

## See also

- `/home/spectre/alphazede/Alphazedehq/.grok/hooks/use-okf.sh`
- `/home/spectre/alphazede/Alphazedehq/.grok/hooks/use-okf.json`
- `/home/spectre/alphazede/Alphazedehq/tools/okf/okf`
- `/home/spectre/alphazede/bran/crates/bran-cli/src/main.rs`
- `/home/spectre/alphazede/bran/crates/bran-core/src/repair/mod.rs`
