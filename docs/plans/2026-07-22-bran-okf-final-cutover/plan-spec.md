---
type: plan-spec
name: bran-okf-final-cutover
status: gathered
date: 2026-07-22
applies_to: bran
parent_plan: ../2026-07-21-bran-okf-migration/plan-spec.md
planning_provider: codex
planning_model: gpt-5.6-sol
planning_reasoning: high
---

## Problem

The staged BRAN/OKF migration established BRAN-native validation, retrieval,
repair, compatibility, and evidence-based retirement contracts, but the final
cutover is not yet eligible to execute.

Shared BRAN gates still have known blockers:

- portable OKF-v0.1 conformance does not yet prove the reserved `index.md` and
  `log.md` structures;
- `CMD-FAST` is blocked by test-budget-registry drift; and
- `CMD-PUBLIC` is blocked by a stale repository path.

Publication, installation, per-consumer parity, and global retirement also lack
one complete reproducible route. Consumer evidence is incomplete in material
ways: AlphaZede Sports needs granular public-boundary policy, BetBot has an
oversized document, retrieval parity is unavailable, and HGTS is absent.

The final plan must close those gaps without treating unavailable evidence as a
pass, weakening either validation profile, overlapping consumer writers, or
removing compatibility before every consumer is ready.

## Goal

Produce an executable, evidence-backed final-cutover route that:

1. repairs and proves the shared BRAN gates;
2. creates a reproducible, attributable BRAN release and exact-SHA consumer
   installation procedure with rollback;
3. migrates and validates each active consumer in one bounded, independently
   gated slice;
4. retains every compatibility surface until all consumer gates pass;
5. makes global compatibility retirement a separately approved final slice;
   and
6. drafts two bounded upstream OKF proposals without conflating local
   `bran-strict` results with portable `okf-v0.1` conformance.

The planning journey is complete only when `plan-spec.md`, `design.md`,
`seit.md`, `implementation.md`, and `review.html` exist in the canonical plan
directory, validate together, and are returned for owner route selection.
Planning completion does not authorize execution.

## Current truth

### Established

- The completed staged-migration plan is
  `docs/plans/2026-07-21-bran-okf-migration/`.
- That plan fixes the active consumer inventory as:
  `Alphazedehq`, `alphazede-sports`, `betbot`, `developers`, `hgts`, and
  `alphazede-markets`.
- BRAN already distinguishes reserved `index.md` and `log.md` document kinds in
  `crates/bran-core/src/bundle.rs`.
- `okf-v0.1` and `bran-strict` are separate profiles; strict-only failures must
  not be reported as portable OKF failures.
- The established shared command IDs include:
  `CMD-FAST` = `./tools/ci/check.sh --fast` and
  `CMD-PUBLIC` = `python3 tools/ci/public_boundary_check.py`, resolved from the
  BRAN repository root.
- Legacy retirement is evidence-based rather than date-based.

### Known failing or blocked

- **BLK-GATE-001:** portable OKF conformance lacks complete reserved
  `index.md`/`log.md` proof.
- **BLK-GATE-002:** `CMD-FAST` has a test-budget-registry blocker.
- **BLK-GATE-003:** `CMD-PUBLIC` has a stale-path blocker.
- **BLK-AZS-001:** AlphaZede Sports lacks accepted granular public-boundary
  policy evidence.
- **BLK-BETBOT-001:** BetBot contains an oversized document that prevents a
  clean parity claim under the current bounded-input behavior.
- **BLK-PARITY-001:** retrieval parity evidence is unavailable.
- **BLK-HGTS-001:** the HGTS consumer checkout is absent, so its current state
  and parity cannot be attested.
- **BLK-PLAN-001:** the current Bearing run state still identifies
  `docs/plans/2026-07-23-create-or-resume-the-complete-planning-journey-for-bran-s-final-okf-cuto`
  as its validated `planDirectory`, while the owner selected this
  `2026-07-22-bran-okf-final-cutover` directory as canonical. The runtime path
  must be retargeted or treated only as a temporary receipt pointer before Map
  the Route writes artifacts; it is not authority to continue the duplicate.

These blockers are pending work, not evidence that the affected capability
passed.

### Not performed or authorized

No final-cutover implementation, publication, push, deployment, merge,
consumer-repository mutation, public proposal submission, compatibility
removal, release promotion, or global retirement has been performed or
authorized by this plan.

## Scope

### In scope for the complete plan

- BRAN source, conformance fixtures, tests, and CI metadata needed to clear the
  shared gates.
- Reproducible build, seal, provenance, publication, installation,
  exact-revision verification, and rollback procedures.
- One migration and parity-validation slice for each of the six consumers.
- Consumer-local policy/configuration, hooks, and installation changes only
  inside that consumer's later owner-approved slice.
- Retention and eventual separately approved retirement of compatibility
  adapters, legacy hooks, `use-okf`, legacy entrypoints, and legacy
  configuration.
- Drafts for the upstream OKF bundle-scan-scope and layered-profile-separation
  proposals.
- Stable requirement IDs, design decisions, SEIT proof cases, command IDs,
  exact write sets, dependency waves, stop conditions, owner approvals, and
  rollback criteria across the final artifact set.

### Out of scope for this planning journey

- Implementing any planned change.
- Mutating a consumer repository.
- Building or publishing a live release.
- Pushing, deploying, merging, promoting, or installing an artifact.
- Submitting either upstream proposal.
- Removing or disabling compatibility software.
- Starting Explorer or Expedition.
- Creating plan-local prompt artifacts.

## Authority

- Gather Supplies may update only this canonical `plan-spec.md`.
- Map the Route may create or update only `design.md`, `seit.md`,
  `implementation.md`, and `review.html` in this canonical directory, plus
  traceability corrections to this specification if validation requires them.
- The selected top-level planning and review route is
  `codex gpt-5.6-sol` with `high` reasoning.
- `implementation.md` must assign the owner-selected execution routes by work
  type: `codex gpt-5.6-terra` with `medium` reasoning for coding and
  consumer-mutation slices, and `agy agent default` with `medium`
  reasoning for documentation, operational, and destructive slices. These are
  execution assignments; the planning/review route remains
  `codex gpt-5.6-sol` with `high` reasoning. Silent fallback is prohibited.
- No planning artifact grants implementation or external-write authority.
- Each consumer migration, public publication, upstream submission, and global
  retirement requires its own explicit owner approval at the gate identified
  below.

## Owner decisions and assumptions

### Fixed owner decisions

1. **Canonical directory.** The only plan directory is
   `docs/plans/2026-07-22-bran-okf-final-cutover`. Preserve its existing
   `plan-spec.md` history, do not continue the auto-slugged duplicate, and leave
   no duplicate plan directory when the planning journey completes.
2. **Staged replacement remains binding.** BRAN is canonical, while legacy
   surfaces remain compatibility-only until consumer parity and final approval.
3. **No hard retirement date.** Evidence, not elapsed time, controls consumer
   completion and global retirement.
4. **Profile separation.** Report `okf-v0.1` and `bran-strict` independently.
5. **Proposal sequencing.** Draft both upstream proposals before publication,
   but recommend submission only after portable conformance evidence and a
   stable public BRAN release exist.
6. **Execution routing.** Use Terra Medium for coding and consumer-mutation
   slices. Use Agy Medium for documentation, operational, and destructive
   slices. Record requested and effective identity for every packet and stop
   rather than silently substituting a model.

### Safe assumptions

- The six-consumer inventory from the staged-migration plan remains
  authoritative.
- An unavailable, absent, skipped, oversized, or policy-incomplete check is a
  blocked gate, never a pass or implicit waiver.
- HGTS remains in the inventory while absent. Its absence blocks its own gate
  and global retirement without invalidating completed evidence for other
  consumers.
- AlphaZede Sports boundary granularity and BetBot's oversized document must be
  resolved through explicit policy/content handling and proof. The route may
  not add an undocumented exemption or silently raise a safety limit.
- Consumer slices may proceed independently after shared gates and publication
  prerequisites pass, but no two slices may write the same repository or shared
  release surface concurrently.
- The duplicate auto-slugged plan directory is cleanup work for the planning
  workflow. It must not be used as a source of authority and must be removed
  only after canonical artifacts are present and preservation checks pass.

## Requirements

### Shared BRAN gates

- **REQ-GATE-001 — Reserved-document conformance.** Add portable
  `okf-v0.1` valid and invalid proof cases for reserved `index.md` and `log.md`
  structures. The proof must run without a provider account or owner-local
  configuration and must report `okf-v0.1` independently from `bran-strict`.
- **REQ-GATE-002 — Fast-gate registry.** Define `tools/ci/test-budget.json` as
  a deterministic inventory of named CI journeys, direct CI commands, and owned
  fixtures—not one registry row per Rust unit test. `CMD-BUDGET` remains the first
  fast-gate check in `CMD-FAST`, failing deterministically on any missing or
  duplicate journey, direct command, or fixture ownership. All Rust unit tests
  remain mandatory through the existing `CMD-FAST` workspace test command, ensuring
  that removing per-test registry accounting does not skip or weaken test execution.
- **REQ-GATE-003 — Public-boundary path.** Resolve the `CMD-PUBLIC` stale path
  using repository-root-relative, portable path handling. Acceptance requires
  the command to pass from a clean checkout without an AlphaZede parent layout
  assumption.
- **REQ-GATE-004 — Gate isolation.** Shared gate repairs may write only BRAN
  source, fixtures, tests, schemas, and CI metadata explicitly listed in their
  implementation slices. They may not mutate consumers or remove compatibility.
- **REQ-GATE-005 — Offline determinism.** All shared conformance, fast-gate,
  and public-boundary proofs must remain deterministic and runnable without a
  provider account.

### Publication and provenance

- **REQ-REL-001 — Reproducible artifact.** Define one source revision, clean
  build procedure, toolchain inputs, artifact identity, checksum, and retained
  build receipt sufficient for an independent rebuild comparison.
- **REQ-REL-002 — Public-boundary seal.** Publication eligibility requires
  clean conformance, `CMD-FAST`, `CMD-PUBLIC`, release-contract, credential,
  private-corpus, hidden-truth, and raw-provider-trace checks.
- **REQ-REL-003 — Exact revision.** Bind the release tag or release identifier,
  source commit SHA, artifact SHA-256, manifest, and installed binary version
  into one verification chain. A tag name alone is insufficient.
- **REQ-REL-004 — Installation.** Define a repeatable per-consumer install or
  pin procedure that uses the sealed artifact, verifies its digest before use,
  records the prior installation, and produces an installation receipt.
- **REQ-REL-005 — Rollback.** Define and prove restoration to the exact prior
  consumer pin/binary/configuration without deleting the new artifact or legacy
  fallback until post-rollback validation passes.
- **REQ-REL-006 — Publication approval.** Stop before any public release,
  upload, promotion, or public install test until the owner approves the exact
  revision, artifact digest, destination, manifest, and public-boundary receipt.

### Consumer migration and parity

- **REQ-CONS-001 — One slice per consumer.** `implementation.md` must contain
  exactly one bounded migration-and-parity slice for each of:
  `Alphazedehq`, `alphazede-sports`, `betbot`, `developers`, `hgts`, and
  `alphazede-markets`.
- **REQ-CONS-002 — Non-overlapping writers.** Each consumer slice owns only
  that consumer's checkout and consumer-local evidence directory. Shared BRAN
  release/proposal surfaces must be completed in earlier exclusive slices.
- **REQ-CONS-003 — Frozen comparison.** Each consumer gate must compare pinned
  BRAN-native validation and retrieval with the legacy adapter over identical
  frozen inputs, normalize only representation differences, and retain raw and
  normalized receipts.
- **REQ-CONS-004 — Required parity.** A consumer passes only when validation
  outcomes, retrieval selection/precedence, typed unavailable/conflict states,
  hook behavior, installation identity, and rollback proof meet the designed
  parity contract.
- **REQ-CONS-005 — Reference audit.** Each passing consumer must retain an
  audit of active code, skill, hook, CI, and configuration references. Legacy
  references may remain during migration only when identified as active
  compatibility surfaces or historical documentation.
- **REQ-CONS-006 — Independent progress.** A blocked consumer does not erase
  another consumer's passing evidence, but it blocks global retirement.
- **REQ-CONS-007 — Consumer approval.** Stop before mutating each consumer
  until the owner approves that slice's exact repository, revision, write set,
  artifact digest, verification commands, and rollback procedure.

### Explicit evidence gaps

- **REQ-AZS-001 — Granular boundary policy.** The AlphaZede Sports slice must
  define and prove the minimum native BRAN policy granularity needed to express
  its public/private path distinctions. It must include positive, negative,
  inheritance/precedence, and ambiguous-policy cases and may not weaken the
  shared public-boundary gate.
- **REQ-BETBOT-001 — Oversized document.** The BetBot slice must preserve the
  current bounded-input safety contract while resolving the oversized document
  through an explicitly designed content, policy, or bounded streaming route.
  It must prove deterministic failure before the change, bounded success after
  it, unchanged content identity where required, and rollback.
- **REQ-PARITY-001 — Retrieval availability.** Retrieval parity must have a
  runnable deterministic corpus and retained BRAN/legacy comparison receipt.
  If the legacy or native side is unavailable, the consumer remains blocked
  with a typed unavailable result.
- **REQ-HGTS-001 — Absent consumer.** The HGTS slice must begin with checkout
  identity, revision, and ownership verification. If HGTS remains absent, the
  slice returns `unavailable`, performs no substitution or inferred pass, and
  keeps global retirement blocked.

### Compatibility and retirement

- **REQ-COMPAT-001 — Retention.** Compatibility adapters, legacy hooks,
  `use-okf`, legacy entrypoints, and legacy configuration remain present and
  recoverable until all six consumer gates pass.
- **REQ-COMPAT-002 — No early disabling.** A migrated consumer may switch its
  active pin only after its gate and rollback proof pass; it may not delete the
  legacy fallback during its migration slice.
- **REQ-RETIRE-001 — Separate final slice.** Global retirement must be its own
  final implementation slice after all consumer receipts and reference audits
  pass. It may not be folded into a consumer or release slice.
- **REQ-RETIRE-002 — Explicit approval.** The retirement slice requires
  separate owner approval of exact write sets, deletion targets, retained
  historical references, rollback archive, verification commands, and the
  integrated evidence manifest.
- **REQ-RETIRE-003 — Rollback before removal.** Prove restoration from the
  proposed retirement state before deleting or disabling any compatibility
  surface. Failed restoration or any active reference stops retirement.

### Upstream proposals

- **REQ-UPSTREAM-001 — Bundle scan scope.** Draft a proposal that specifies
  whether portable OKF scanning applies to one bundle root, nested bundle roots,
  or a repository-wide discovery set, including deterministic inclusion,
  exclusion, symlink, and reserved-document behavior.
- **REQ-UPSTREAM-002 — Layered profiles.** Draft a proposal that separates the
  portable `okf-v0.1` floor from additive implementation profiles such as
  `bran-strict`, with independent results and no reclassification of strict-only
  failures as portable failures.
- **REQ-UPSTREAM-003 — Submission gate.** Proposal drafts are local planning
  artifacts. Recommend submission only after REQ-GATE-001 through
  REQ-GATE-005 pass and an owner-approved stable public release satisfies
  REQ-REL-001 through REQ-REL-006. Submission still requires separate owner
  approval.

### Route and artifact quality

- **REQ-PLAN-001 — Complete artifact set.** Map the Route must produce
  `design.md`, `seit.md`, `implementation.md`, and `review.html` beside this
  specification; no prompt artifacts are required.
- **REQ-PLAN-002 — Traceability.** Every requirement must map to at least one
  design decision, SEIT proof case, implementation slice, command/procedure ID,
  retained evidence path, stop condition, and rollback or explicit
  non-applicability.
- **REQ-PLAN-003 — Command registry.** SEIT must bind exact repository-root
  invocations to stable IDs, including at minimum:
  `CMD-OKF-PORTABLE`, `CMD-FAST`, `CMD-PUBLIC`, `CMD-RELEASE-BUILD`,
  `CMD-RELEASE-SEAL`, `CMD-INSTALL-VERIFY`, `CMD-EXACT-SHA`,
  `CMD-ROLLBACK`, six consumer parity procedure IDs, `CMD-REFERENCE-AUDIT`,
  and `CMD-RETIREMENT-PROOF`.
- **REQ-PLAN-004 — Waves and write sets.** Implementation must give every
  slice an exact write set, dependencies, route/model/reasoning assignment,
  semantic verification, cross-cutting commands, retained evidence, stop
  condition, owner gate, and rollback. Parallel slices must have disjoint
  writers.
- **REQ-PLAN-005 — Review.** Generate a baseline `review.html` after design and
  SEIT, regenerate it after implementation, and validate the final artifact set
  before returning for owner selection.
- **REQ-PLAN-006 — Canonical cleanup.** Before planning completion, verify all
  canonical artifacts are present in
  `docs/plans/2026-07-22-bran-okf-final-cutover`, preserve unrelated owner
  changes, and remove the duplicate auto-slugged plan directory without
  force-removing any unrelated evidence.
- **REQ-PLAN-007 — Planning stop.** Do not start Explorer or Expedition.
  Return the validated plan and final review for explicit owner route selection.

## Acceptance criteria

- **AC-001:** All requirements have complete bidirectional traceability across
  the final five artifacts.
- **AC-002:** The shared gate route proves portable reserved-document
  conformance and clears `CMD-FAST` and `CMD-PUBLIC` without parent-layout or
  provider-account dependencies.
- **AC-003:** The publication route binds source SHA, artifact digest, manifest,
  installation identity, and rollback receipt and stops at an explicit owner
  approval.
- **AC-004:** Exactly six consumer slices exist, their write sets do not
  overlap, and every slice has validation, retrieval, installation, reference
  audit, and rollback proof or a typed blocking result.
- **AC-005:** AlphaZede Sports, BetBot, retrieval parity, and HGTS gaps each map
  to explicit proof cases and stop conditions; none is represented as passed
  while evidence is unavailable.
- **AC-006:** Compatibility remains until every consumer passes; global
  retirement is a separate owner-approved slice with successful rollback proof.
- **AC-007:** Both upstream proposal drafts exist before publication planning
  completes, while submission remains gated on portable conformance, stable
  public release, and separate owner approval.
- **AC-008:** `okf-v0.1` and `bran-strict` results remain separately visible in
  commands, receipts, reviews, and release claims.
- **AC-009:** The final planning artifact set validates in the canonical
  directory, the duplicate auto-slugged directory is absent, and no product or
  consumer code was changed by planning.
- **AC-010:** The journey stops after owner-facing review and does not begin an
  execution route.

## Required proof classes for SEIT

Map the Route must include at least:

- valid and invalid reserved `index.md` and `log.md` portable fixtures;
- registry-complete and intentionally unregistered-test cases;
- repository-root and relocated-checkout public-boundary cases;
- reproducible-build, tampered-artifact, wrong-SHA, wrong-tag, interrupted
  install, and exact rollback cases;
- one positive and one negative validation/retrieval parity case per consumer;
- typed unavailable cases for missing retrieval capability and absent HGTS;
- granular boundary allow/deny/ambiguous cases for AlphaZede Sports;
- oversized/rejected, bounded-success, identity, and rollback cases for BetBot;
- legacy reference present/allowed, active/unexpected, and absent cases;
- early-retirement rejection and retirement-rollback cases; and
- independent `okf-v0.1` pass/fail and `bran-strict` pass/fail combinations.

## Stop conditions

Stop the affected route or slice when:

- a required repository, revision, artifact, policy, or parity runner is
  unavailable or cannot be identified;
- a command depends on credentials, provider access, owner-local memory, or an
  undeclared parent-directory layout;
- the proposed write set overlaps another active writer or exceeds its
  repository boundary;
- checksum, exact-SHA, manifest, public-boundary, validation, retrieval,
  installation, reference-audit, or rollback evidence fails;
- a plan attempts to classify unavailable evidence as passing;
- an action would publish, push, deploy, merge, mutate a consumer, submit an
  upstream proposal, or remove compatibility without its explicit owner gate;
- `bran-strict` evidence is used to claim `okf-v0.1` conformance; or
- the active planning runtime requires new route artifacts to be written in the
  rejected auto-slugged directory instead of this canonical directory; or
- preserving unrelated owner changes cannot be proven.

## Owner approval gates

The route must stop for explicit owner approval before:

1. publishing or promoting the exact sealed BRAN release;
2. mutating each of the six consumers;
3. submitting either upstream proposal; and
4. executing the final global retirement slice.

A recommendation or passing plan review is advice, not approval.

## Rollback criteria

- Shared BRAN repairs must be revertible within their exact write sets and leave
  existing compatibility behavior callable.
- Release rollback must restore the prior artifact pin and verify the restored
  digest and behavior before declaring success.
- Consumer rollback must restore exact prior bytes/configuration/pin and rerun
  that consumer's legacy and BRAN gate; retained compatibility is the fallback.
- Retirement rollback must be proven before removal and must restore all active
  entrypoints, hooks, skills, configurations, pins, and reference integrity.
- Any byte, digest, behavior, or reference mismatch is rollback failure and
  blocks forward progress.

## Evidence consulted

- `docs/plans/AGENTS.md`
- `docs/plans/2026-07-21-bran-okf-migration/plan-spec.md`
- `docs/plans/2026-07-21-bran-okf-migration/design.md`
- `docs/plans/2026-07-21-bran-okf-migration/seit.md`
- `docs/plans/2026-07-21-bran-okf-migration/implementation.md`
- `crates/bran-core/src/bundle.rs`
- `crates/bran-core/src/profile.rs`
- `fixtures/conformance/`
- `tools/ci/check.sh`
- `tools/ci/test-budget.json`
- `tools/ci/test_budget_check.py`
- `tools/ci/public_boundary_check.py`

## Handoff to Map the Route

### Role and outcome

Act as the bounded Bearing Map-the-Route planning agent. Produce and validate
the remaining four artifacts in this canonical directory, then stop with an
owner-reviewable route and no execution.

### Execute now

1. Read this specification and the completed staged-migration artifacts.
2. Write `design.md` with the minimum decisions needed to satisfy every
   requirement and preserve the authority boundaries.
3. Write `seit.md` with exact command/procedure IDs, proof cases, evidence
   paths, and complete traceability.
4. Generate the baseline `review.html`.
5. Write `implementation.md` with bounded write sets, dependency waves,
   disjoint consumer writers, supported route assignments, owner gates, stop
   conditions, and rollback.
6. Regenerate `review.html`, validate all five artifacts, consolidate only
   canonical artifacts, and remove the duplicate plan directory.
7. Return the validated artifacts for owner selection. Do not start Explorer or
   Expedition.

### Verification and evidence

Retain the final traceability result, artifact validation result, review
generation result, and canonical-directory inventory. Clearly separate planned,
currently failing, unavailable, and proven states.

### Return or stop conditions

Return only when the five canonical artifacts validate and the duplicate plan
directory is absent. Stop earlier on any authority expansion, unresolved
material owner decision, unavailable required evidence that changes the route,
inability to preserve unrelated owner changes, or a runtime plan-directory
constraint that would force route artifacts into the rejected duplicate.

## Owner review amendment — 2026-07-23

This append-only amendment records the owner's requested planning-package
correction. It supersedes earlier handoff instructions only where they imply
that completed design, SEIT, or implementation work should be repeated.

### Current truth

- The canonical directory
  `docs/plans/2026-07-22-bran-okf-final-cutover` contains the current
  `plan-spec.md`, `design.md`, `seit.md`, and `implementation.md`, but does not
  contain `review.html`.
- The temporary Bearing runtime directory
  `docs/plans/2026-07-23-create-or-resume-the-complete-planning-journey-for-bran-s-final-okf-cuto`
  contains those four Markdown sources plus a generated `review.html`.
- At inspection, each corresponding Markdown source in the two directories was
  the same regular-file inode and therefore byte-identical. The observed
  digests before this amendment were:
  `plan-spec.md` =
  `1f84e28b3b9be9b973fb2b2e969ac1722c220e81469bfb392e3f4082a79f37b0`,
  `design.md` =
  `db9f3ac640d6ccb6428ec30599edcb3385f664121f1d5a8dd892460d9af031a3`,
  `seit.md` =
  `59026ab4da817496dd97cddd5bf529e73803c8c2e414ff858f56e3545a71b0d4`,
  and `implementation.md` =
  `a4906dea4afd3c40e5cdc89038c13d81db5f9ee0cf14ddcde78e91e17eb04a7f`.
- The runtime `review.html` digest observed before this amendment was
  `87ea2cb4531640015cfcbe9cb6998d9e0ea36aaec23de3ae354f3f37014cb35f`.
  This plan-spec amendment makes that review stale until Bearing regenerates
  it from the four current canonical sources.
- No implementation slice has been executed. No product code, consumer
  repository, publication target, compatibility surface, or upstream
  submission was changed by this planning correction.

### Fixed owner decision

The owner has not approved execution. Bearing must deterministically regenerate
the complete `review.html` from the current canonical `plan-spec.md`,
`design.md`, `seit.md`, and `implementation.md`, place the resulting regular
file in the canonical directory, and keep any runtime receipt-mirror copy
byte-identical. The four canonical Markdown sources must remain unchanged after
this amendment unless the exact validator reproduces a defect that requires a
minimal traceability correction.

### Additional requirements

- **REQ-PLAN-008 — Canonical deterministic review.** Bearing's deterministic
  review generator must render `review.html` from the exact current bytes of
  the four canonical Markdown sources. The canonical review must embed all four
  complete sources, expose working relative artifact links, and remain
  unedited by hand.
- **REQ-PLAN-009 — Source preservation.** This append-only owner amendment is
  the only authorized Markdown-source change during the review repair.
  `design.md`, `seit.md`, and `implementation.md` must retain the digests
  recorded above. Any further source change requires a reproduced validator
  defect, the narrowest traceability correction, an explicit retained diff,
  and another deterministic review generation.
- **REQ-PLAN-010 — Canonical package validation.** Validate the five canonical
  artifacts together after review generation and retain the exact validator
  output, canonical inventory, source/review digests, embedded-source equality
  result, relative-link result, and runtime-mirror equality result. A runtime
  review alone is not canonical completion.

### Additional acceptance criteria

- **AC-011:** Canonical `review.html` exists as a regular file and is
  byte-for-byte the deterministic render of the current canonical
  `plan-spec.md`, `design.md`, `seit.md`, and `implementation.md`.
- **AC-012:** The exact five-artifact validator passes in the canonical
  directory; `design.md`, `seit.md`, and `implementation.md` retain their
  recorded digests; any still-required runtime mirror is byte-identical; and no
  implementation or external action begins.

## Superseding handoff to Map the Route

### Role and outcome

Act as the bounded Bearing review-repair agent. Produce the missing canonical
deterministic review, validate the five canonical artifacts together, reconcile
the temporary runtime receipt mirror, and stop without execution.

### Current truth

Design, SEIT, and implementation drafting are complete and must not be
repeated. The four Markdown sources are available at both paths as shared
regular files. Canonical `review.html` is missing, and the runtime review is
stale because this amendment changed `plan-spec.md`.

### Scope and authority

- Use the selected planning/review route `codex gpt-5.6-sol` with `high`
  reasoning.
- Bearing may generate `review.html` and place byte-identical regular-file
  instances at the canonical and temporarily required runtime paths.
- Do not hand-edit `review.html`.
- Do not change `design.md`, `seit.md`, or `implementation.md` unless the exact
  validator first reproduces a defect. If it does, stop and report the defect
  before expanding beyond the narrowest traceability repair.
- Do not execute Explorer, Expedition, any implementation slice, publication,
  push, deploy, merge, consumer mutation, proposal submission, or compatibility
  retirement.

### Execute now

1. Capture the current four canonical source digests and confirm the
   corresponding runtime sources are byte-identical regular files.
2. Run Bearing's deterministic review generator from those current source
   bytes.
3. Materialize the generated `review.html` as a regular file in the canonical
   directory and keep the runtime receipt-mirror review byte-identical while
   Bearing still requires that path.
4. Run the exact five-artifact validator against the canonical directory and
   retain its complete output.
5. Verify embedded-source equality, working relative links, source-preservation
   digests, review equality, and a planning-only scoped status.
6. Remove the temporary runtime directory only when Bearing no longer requires
   it and preservation can be proven; otherwise return it as the sole remaining
   cleanup blocker rather than deleting required state.

### Verification and evidence

Return the canonical five-artifact inventory, exact validator output, all five
digests, source-preservation comparison, canonical/runtime review equality,
relative-link result, and scoped repository status. Label the generated review
and validation as planning evidence, not implementation evidence.

### Return or stop conditions

Return for owner route selection only after AC-011 and AC-012 pass. Stop on a
deterministic-render mismatch, missing or stale embedded source, broken
artifact link, non-identical required mirror, unexpected Markdown-source
change, validator failure requiring non-traceability work, authority expansion,
or any attempt to begin execution. A recommendation or passing review is not
owner approval.
