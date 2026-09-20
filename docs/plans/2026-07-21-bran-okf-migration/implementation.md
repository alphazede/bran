---
type: implementation
name: bran-okf-migration
status: draft
date: 2026-07-21
plan_spec: ./plan-spec.md
design: ./design.md
seit: ./seit.md
---

# Implementation - BRAN OKF Migration

This is a pipeline plan. Waves are sequential because later compatibility work
consumes the native BRAN policy, validation, repair, and receipt contracts. All
slices use the existing Pi route for `deepseek-v4-pro`. Publication, release,
consumer-repository mutation, and legacy removal are outside this execution authority.

The integrated closeout runs `CMD-FAST` once after all code-bearing slices and uses
the repository's native read-only review on the integrated diff. `CMD-FAST` remains a
cross-cutting repository gate rather than a slice-owned semantic proof.

## Wave 1 - Native BRAN ownership

Wave 1 establishes the native policy and deterministic core behavior. Its slices are
sequential because they share the core module registry and normalized policy contract.

### Slice 1.1 — Native repository policy

**Goal.** Add the versioned BRAN repository-policy model, loader, schema, and frozen fixtures.

**Requirement IDs.** AC-2, RISK-4

**Design IDs.** DES-1, DES-8, CONTRACT-3

**SEIT proof rows.** SEIT-2, SEIT-12

**Type.** /tdd

**Design lenses.** CDD, SecDD

**Implementation role.** Rust policy and schema maintainer

**Agent model route.** Pi (deepseek-v4-pro)

**Agent reasoning level.** high

**Ponytail mode.** full

**Review path.** Focused tests followed by the BRAN native read-only review on the integrated diff.

### 1.1 execution manifest

**Write set.** Only `crates/bran-core/src/policy.rs`, `crates/bran-core/src/lib.rs`, `schemas/bran-repository-policy.schema.json`, `fixtures/policy/valid-v1.yaml`, `fixtures/policy/invalid-version.yaml`, and `fixtures/policy/unsafe-path.yaml`.

**Command IDs.** CMD-POLICY

**Stop condition.** Stop on a schema decision that contradicts DES-8 or requires consumer-source mutation.

**Human decision.** None; ask before changing the selected policy path or serialization.

### Slice 1.2 — Strict validation and retrieval parity

**Goal.** Make native policy drive strict validation, preserve deterministic source precedence,
reject active packet references to superseded prompts, and verify migration body preservation.

**Requirement IDs.** AC-2, AC-5

**Design IDs.** DES-2, DES-4, DES-10, CONTRACT-7

**SEIT proof rows.** SEIT-3, SEIT-6, SEIT-17

**Type.** /tdd

**Design lenses.** CDD, SecDD, RDD

**Implementation role.** Rust validation and retrieval maintainer

**Agent model route.** Pi (deepseek-v4-pro)

**Agent reasoning level.** high

**Ponytail mode.** full

**Review path.** Focused tests followed by the BRAN native read-only review on the integrated diff.

### 1.2 execution manifest

**Write set.** Only `crates/bran-core/src/profile.rs`, `crates/bran-core/src/graph/query.rs`,
`crates/bran-core/src/packet/mod.rs`, `crates/bran-core/src/migration.rs`,
`crates/bran-core/src/lib.rs`, and `fixtures/conformance/bran-policy-parity.fixture`.

**Command IDs.** CMD-PROFILE, CMD-QUERY, CMD-PACKET, CMD-MIGRATION, PROC-PARITY

**Stop condition.** Stop if compatibility requires duplicating native rules in an adapter or changing established retrieval precedence.

**Human decision.** None; ask before weakening a public/private or source-precedence rule.

### Slice 1.3 — Derived-state self-healing

**Goal.** Rebuild only BRAN-owned derived artifacts while refusing automatic source or policy repair.

**Requirement IDs.** AC-6

**Design IDs.** DES-3, DES-5, CONTRACT-9

**SEIT proof rows.** SEIT-7

**Type.** /tdd

**Design lenses.** SecDD, RDD

**Implementation role.** Rust maintenance-state maintainer

**Agent model route.** Pi (deepseek-v4-pro)

**Agent reasoning level.** high

**Ponytail mode.** full

**Review path.** Focused tests followed by the BRAN native read-only review on the integrated diff.

### 1.3 execution manifest

**Write set.** Only `crates/bran-core/src/derived_state.rs`, `crates/bran-core/src/lib.rs`, and `fixtures/derived-state/rebuild-v1.json`.

**Command IDs.** CMD-DERIVED, CMD-REPAIR

**Stop condition.** Stop if a proposed automatic action targets source, metadata, classification, configuration, or source links.

**Human decision.** None; explicit authority is required for any non-derived repair proposal.

## Wave 2 - Authorized repair and CLI contracts

Wave 2 consumes the native validator from Wave 1 and closes the source-mutation
boundary before any compatibility surface can invoke maintenance behavior.

### Slice 2.1 — Repair and maintenance lifecycle

**Goal.** Complete production-safe proposal, authority, digest, staged apply, revalidation,
rollback, receipt, typed-exit behavior, and bounded native policy input from stdin.

**Requirement IDs.** AC-3, AC-4, AC-6

**Design IDs.** DES-3, DES-5, DES-6, DES-9, CONTRACT-2, CONTRACT-3, CONTRACT-9

**SEIT proof rows.** SEIT-4, SEIT-5, SEIT-7, SEIT-14, SEIT-16

**Type.** /tdd

**Design lenses.** CDD, SecDD, RDD, ODD

**Implementation role.** Rust repair and CLI security maintainer

**Agent model route.** Pi (deepseek-v4-pro)

**Agent reasoning level.** high

**Ponytail mode.** full

**Review path.** Focused fault tests followed by the BRAN native read-only review on the integrated diff.

### 2.1 execution manifest

**Write set.** Only `crates/bran-core/src/repair/mod.rs`, `crates/bran-cli/src/main.rs`, and `fixtures/repair/rollback-v1.json`.

**Command IDs.** CMD-REPAIR, CMD-CLI, CMD-DERIVED, CMD-POLICY, PROC-ROLLBACK

**Stop condition.** Stop on any false-success state, uncertain rollback without explicit terminal evidence, or inferred mutation authority.

**Human decision.** None; ask before introducing a new authority source or widening mutation targets.

## Wave 3 - AlphaZedeHQ staged adoption

Wave 3 updates the canonical internal skill, shared compatibility tool, and hooks.
These slices are sequential because they share the adapter invocation and pinned binary.
No consumer repository is migrated or rewritten in this wave.

### Slice 3.1 — BRAN skill and shared compatibility adapter

**Goal.** Make BRAN the canonical internal knowledge workflow while retaining deprecated OKF entrypoints as translation-only adapters.

**Requirement IDs.** AC-1, AC-7

**Design IDs.** DES-4, DES-7, DES-10, DES-11, DES-12, DES-13, CONTRACT-1, CONTRACT-5, CONTRACT-10, CONTRACT-11, CONTRACT-12, CONTRACT-13

**SEIT proof rows.** SEIT-1, SEIT-8, SEIT-17, SEIT-18, SEIT-19, SEIT-20

**Type.** /tdd

**Design lenses.** CDD, RDD, ODD

**Implementation role.** Rust/Python policy interoperability and agent-skill maintainer

**Agent model route.** Pi (deepseek-v4-pro)

**Agent reasoning level.** high

**Ponytail mode.** full

**Review path.** AlphaZedeHQ focused tests plus native read-only review of the cross-repository integrated diff.

### 3.1 execution manifest

**Write set.** Only `/home/spectre/alphazede/bran/crates/bran-core/src/policy.rs`, `/home/spectre/alphazede/bran/crates/bran-core/src/scan/mod.rs`, `/home/spectre/alphazede/bran/crates/bran-cli/src/main.rs`, `/home/spectre/alphazede/Alphazedehq/skills/use-bran/SKILL.md`, `/home/spectre/alphazede/Alphazedehq/skills/use-okf/SKILL.md`, `/home/spectre/alphazede/Alphazedehq/tools/okf/okf`, `/home/spectre/alphazede/Alphazedehq/tools/okf/bran_runtime.py`, and `/home/spectre/alphazede/Alphazedehq/tools/okf/test_bran_runtime.py`. Keep the bounded BRAN parser/scanner packets and dependent AlphaZedeHQ adapter packet sequential in their respective task worktrees.

**Command IDs.** CMD-POLICY, CMD-SCAN, CMD-CLI, CMD-ADAPTER, PROC-PARITY, PROC-REFERENCES

**Stop condition.** Stop if the adapter becomes a semantic rule owner, rewrites legacy configuration, or retires an entrypoint without AC-7 evidence.

**Human decision.** None; publication and final legacy removal remain owner decisions.

### Slice 3.2 — BRAN-backed fail-open hooks

**Goal.** Preserve the existing trigger and timeout behavior through a native BRAN hook with deprecated OKF forwarding compatibility.

**Requirement IDs.** AC-1, RISK-2

**Design IDs.** DES-4, DES-6, CONTRACT-1, CONTRACT-4

**SEIT proof rows.** SEIT-1, SEIT-10

**Type.** /tdd

**Design lenses.** CDD, SecDD, RDD, ODD

**Implementation role.** Shell hook and compatibility maintainer

**Agent model route.** Pi (deepseek-v4-pro)

**Agent reasoning level.** high

**Ponytail mode.** full

**Review path.** AlphaZedeHQ shell fixtures plus native read-only review of the cross-repository integrated diff.

### 3.2 execution manifest

**Write set.** Only `/home/spectre/alphazede/Alphazedehq/.grok/hooks/use-bran.sh`, `/home/spectre/alphazede/Alphazedehq/.grok/hooks/use-bran.json`, `/home/spectre/alphazede/Alphazedehq/.grok/hooks/test-use-bran.sh`, `/home/spectre/alphazede/Alphazedehq/.grok/hooks/use-okf.sh`, `/home/spectre/alphazede/Alphazedehq/.grok/hooks/use-okf.json`, `/home/spectre/alphazede/Alphazedehq/.grok/hooks/test-use-okf.sh`, and `/home/spectre/alphazede/Alphazedehq/.grok/hooks/README.md`.

**Command IDs.** CMD-HOOK, CMD-ADAPTER

**Stop condition.** Stop if any hook path blocks agent completion, performs source mutation, retries maintenance, or trusts a target-repository executable.

**Human decision.** None; strict behavior remains limited to explicit validation and configured CI.

### Slice 3.3 — Pinned local adoption and boundary check

**Goal.** Verify the exact local BRAN artifact used by the adapter and hooks while preserving the publication boundary.

**Requirement IDs.** AC-1, AC-7, RISK-3

**Design IDs.** DES-5, DES-7

**SEIT proof rows.** SEIT-11, SEIT-15

**Type.** manual

**Design lenses.** SecDD, ODD

**Implementation role.** Local release provenance operator

**Agent model route.** Pi (deepseek-v4-pro)

**Agent reasoning level.** high

**Ponytail mode.** off

**Review path.** Checksum readback, public-boundary evidence, and native read-only review; no publication review is implied.

### 3.3 execution manifest

**Write set.** No committed repository writes. Owner-approved local runtime artifacts under `/home/spectre/alphazede/Alphazedehq/tools/okf/runtime/` may be created by the adoption procedure. On 2026-07-22 the owner explicitly authorized an uncommitted local `tools/okf/runtime/bran-release-pin.json` even though that path is trackable rather than ignored. The exception permits local pin verification only; the pin must remain uncommitted and does not authorize promotion, publication, push, or deployment.

**Command IDs.** CMD-PUBLIC, PROC-PIN, PROC-PUBLICATION

**Stop condition.** Stop on checksum mismatch, path aliasing, private-boundary leakage, or any action that would publish or promote an artifact.

**Human decision.** Explicit owner approval is required before publication, release, promotion, or public install verification.

## Wave 4 - Consumer evidence and closeout

Wave 4 is read-only. It measures the six active consumers independently and preserves
incomplete evidence without mutating their source, configuration, hooks, or CI.

### Slice 4.1 — Six-consumer parity and retirement inventory

**Goal.** Produce independent parity and active-reference evidence without prematurely removing compatibility.

**Requirement IDs.** AC-7, RISK-1, RISK-5

**Design IDs.** DES-7, CONTRACT-5, CONTRACT-8

**SEIT proof rows.** SEIT-8, SEIT-9, SEIT-13

**Type.** manual

**Design lenses.** CDD, RDD, ODD

**Implementation role.** Repository migration evidence auditor

**Agent model route.** Pi (deepseek-v4-pro)

**Agent reasoning level.** high

**Ponytail mode.** off

**Review path.** Read-only evidence review followed by the BRAN native integrated-diff review.

### 4.1 execution manifest

**Write set.** No writes required; evidence is retained in the execution transcript until a separately authorized evidence path is approved.

**Command IDs.** PROC-PARITY, PROC-REFERENCES

**Stop condition.** Stop on corpus mismatch, attempted consumer mutation, unsupported parity claim, or loss of completed per-consumer evidence.

**Human decision.** Owner approval is required for every consumer migration and for final global adapter removal.

## Execution closeout

After Wave 4, run `CMD-FAST` once against the integrated BRAN diff and the focused
AlphaZedeHQ commands referenced by Wave 3. Run one native read-only review across the
integrated BRAN and AlphaZedeHQ diffs. Do not run a provider evaluation, publish BRAN,
promote a release, modify the six consumer repositories, or remove legacy adapters.

If implementation exposes only missing proof coverage, append a SEIT-only amendment
through `design-driven-build`. If it changes policy, authority, compatibility,
security, or acceptance, stop for the appropriate design amendment.

### Authorization-gated upstream follow-up

After local migration closeout, preserve draft candidates `UPSTREAM-1` (bundle scan
scope) and `UPSTREAM-2` (strict-profile separation) from `design.md`. Before any GitHub
issue, pull request, comment, branch push, or other external publication, present the
exact proposed text and destination to the owner and obtain explicit authorization.
Upstream contribution work is not part of this eight-slice execution and cannot delay,
weaken, or reclassify local migration evidence.

Separately plan and authorize complete reserved `index.md`/`log.md` structural validation
before describing `okf-v0.1` as full upstream conformance certification. Until then,
describe it as the OKF v0.1 concept-document interoperability floor.
