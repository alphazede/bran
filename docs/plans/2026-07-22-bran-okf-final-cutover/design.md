---
type: design
name: bran-okf-final-cutover
status: complete
date: 2026-07-23
applies_to: bran
plan_spec: ./plan-spec.md
lenses_applied: [CDD, SecDD, RDD, ODD]
lenses_skipped: [BizDD, DDD, EDD, GDD, PDD]
oopdsa: mandatory
planning_route: codex gpt-5.6-sol
planning_reasoning: high
---

## Synthesis

The final cutover is a gated evidence pipeline, not a flag flip. BRAN first
closes its portable conformance and repository gate gaps, then produces an exact
sealed release, then migrates six consumers independently, and only then becomes
eligible for separately approved global compatibility retirement.

Four rules govern the route:

1. `okf-v0.1` is the portable result and `bran-strict` is an additive result.
   They are computed and reported independently.
2. Release identity is the tuple of source commit, tag, lockfile digest,
   platform artifact digests, signer fingerprint, and immutable manifest.
3. A consumer is complete only after exact installation, validation and
   retrieval parity, reference audit, and byte/configuration rollback proof.
4. Missing or unavailable evidence is a typed blocked state. It is never
   normalized into success.

The route reuses the staged-migration ownership model: `bran-core` owns policy,
profiles, deterministic scanning, retrieval, and semantic outcomes; `bran-cli`
owns command envelopes; checked-in release tools own build/seal verification;
consumer-local adapters and hooks translate or invoke but do not redefine BRAN
semantics.

## Approved lens record

The owner approved CDD, SecDD, RDD, and ODD with mandatory OOPDSA hardening.

- **CDD** governs profile, receipt, manifest, policy, adapter, and command
  contracts.
- **SecDD** governs root containment, public/private policy, signatures,
  checksums, symlinks, credentials, publication authority, and rollback.
- **RDD** governs unavailable evidence, interrupted installation, partial
  consumer progress, idempotency, recovery, and retirement barriers.
- **ODD** governs deterministic receipts, exact command evidence, claim state,
  first-failure diagnostics, and retained provenance.
- **OOPDSA** fixes ownership, state transitions, deterministic collections,
  rule precedence, and wave scheduling without adding a framework.

BizDD, DDD, EDD, GDD, and PDD are skipped because this route adds no business
model, new domain language, event platform, game mechanics, or performance
target. Consumer and policy boundaries are already explicit in the approved
specification.

## Current evidence and claim state

Evidence was verified from `/home/spectre/alphazede/bran` on 2026-07-23:

- `python3 tools/ci/test_budget_check.py tools/ci/test-budget.json` (`CMD-BUDGET`)
  currently fails (exits 1) because the existing checker requires per-unit-test
  registration against the obsolete fixed ceiling, contradicting the unbuilt target inventory.
- `./tools/ci/check.sh --fast` (`CMD-FAST`) stops at `CMD-BUDGET` first due to this
  test budget check failure.
- `python3 tools/ci/public_boundary_check.py` (`CMD-PUBLIC`) failed when run directly
  because it resolved `bran/fixtures/...` beneath the BRAN checkout, producing
  `/home/spectre/alphazede/bran/bran/fixtures/...`.
- `DocKind` already distinguishes `index.md` and `log.md`, while the portable
  profile explicitly applies its current frontmatter/type requirement only to
  concept documents.
- Release build, release contract, release seal, schema, and exact-tag fixtures
  already exist under `tools/ci/`, `schemas/`, and `fixtures/release/`.

Therefore:

- `CMD-BUDGET` and `CMD-FAST` are **currently failing** (test-budget no-ceiling inventory remains **planned and unproven**);
- `CMD-PUBLIC` is **currently failing**;
- reserved-document conformance, publication, consumer parity, and retirement
  remain **planned and unproven**; and
- retrieval parity and HGTS remain **unavailable**.

## Design decisions

### Shared gates and profile semantics

- **DES-001 — Independent profile outcomes.** `ProfileValidator` returns a
  separate ordered diagnostic set for `okf-v0.1` and `bran-strict`. Selecting
  one profile controls the command exit; it does not erase or reclassify the
  other result.
- **DES-002 — Normative reserved-document rules.** Portable `index.md` and
  `log.md` validation is implemented as an OKF-specific reserved-document
  validator invoked only by the `okf-v0.1` path. Its rules and fixtures must
  cite the frozen upstream v0.1 source used by the implementation. If the
  normative source is unavailable or ambiguous, implementation stops rather
  than guessing a portable rule.
- **DES-003 — Deterministic reserved diagnostics.** Reserved-document
  diagnostics use stable codes, repository-relative paths, and ordering by
  path, code, then message. Strict-only readiness fields remain outside the
  portable validator.
- **DES-004 — Budget inventory as fast regression gate.** `tools/ci/test-budget.json`
  defines a deterministic inventory of named CI journeys, direct CI commands, and
  owned fixtures rather than one registry row per Rust unit test, eliminating fixed
  plan, phase, and slice test-count ceilings. `CMD-BUDGET` runs as the first check
  in `CMD-FAST`, enforcing deterministic negative evidence (failing on missing or
  duplicate journeys, direct commands, or fixture ownership). All Rust unit tests
  remain mandatory and fully executed through `CMD-FAST` workspace test commands;
  removing per-test registry accounting does not skip or weaken Rust unit test execution.
- **DES-005 — Physical BRAN root for public checks.** `CMD-PUBLIC` derives the
  BRAN checkout root from the physical checker path, not the caller's current
  directory or an assumed `bran/` parent layout. All enumerated paths and
  fixture constants are BRAN-root-relative. Git enumeration is scoped from
  that root and rejects absolute, parent-traversal, and symlink escapes.

### Release, publication, installation, and rollback

- **DES-006 — Existing exact-release contract is canonical.** Reuse
  `build-release.sh`, `release-check.sh`, `release_seal.py`,
  `release_contract_check.py`, and
  `schemas/bran-release-manifest.schema.json`. Do not create a parallel
  manifest or packaging format.
- **DES-007 — Reproducible package inputs.** Each platform archive is built
  from a clean exact tagged commit with `Cargo.lock`, `--locked`, a declared
  target triple, normalized archive metadata, and the existing five-platform
  asset naming contract.
- **DES-008 — Sealed provenance chain.** Publication eligibility requires the
  five archive digests, exact `SHA256SUMS`, verified OpenPGP signature and
  fingerprint, source commit, lockfile digest, tag-to-commit equality, clean
  worktree, immutable direct asset URLs, and SLSA-v1-shaped provenance already
  represented by the release manifest.
- **DES-009 — Publication is an owner-gated side effect.** Local build and
  unsigned dry-run evidence may be produced inside an approved implementation
  slice. Tag creation, signing with owner keys, upload, release publication,
  promotion, or public install verification stops for approval of the exact
  tag, commit, digest set, fingerprint, destination, and public-boundary
  receipt.
- **DES-010 — Verified two-slot installation.** A consumer stages the selected
  archive in a new immutable version directory, verifies the manifest,
  signature, archive digest, member shape, and `bran --version`, then changes
  one consumer-local pin or stable link. The prior pin and bytes remain
  untouched until consumer acceptance passes.
- **DES-011 — Rollback is a tested transition.** Rollback restores the recorded
  prior pin/configuration, verifies the restored digest and command behavior,
  reruns the consumer's focused gate, and emits a receipt. Failed verification
  leaves the consumer blocked and retains both versions for recovery.

### Consumer migration and evidence gaps

- **DES-012 — One immutable consumer identity.** Every consumer slice begins
  with a canonical checkout path, repository identity, exact revision,
  cleanliness record, active legacy surface inventory, and selected BRAN
  artifact digest. A mismatch stops before mutation.
- **DES-013 — Frozen semantic parity.** Native and legacy validation/retrieval
  run read-only over the same frozen corpus. A small normalizer removes only
  representation differences defined in `CONTRACT-007`; raw outputs are always
  retained. Semantic differences remain failures.
- **DES-014 — Typed consumer gate state.** Each consumer ends in exactly one of
  `passed`, `failed`, `unavailable`, or `rolled_back`. Only `passed` contributes
  to global retirement eligibility.
- **DES-015 — Granular AlphaZede Sports boundary rules.** Native policy uses
  repository-relative path rules with an explicit classification. Rules are
  normalized and sorted by path specificity; the most-specific rule wins.
  Equal-specificity disagreement, unmatched required paths, invalid paths, or
  symlink escape is a configuration failure. A default classification must be
  explicit rather than inferred.
- **DES-016 — BetBot uses an owner-approved content split first.** The route
  does not raise BRAN's global text ceiling. The BetBot slice proposes an exact
  semantic split of the oversized knowledge document, preserves stable
  locators/relationships through explicit redirects or index links, verifies
  retrieval and content identity obligations, and can restore exact prior
  bytes. If a safe split cannot be approved and proven, BetBot remains blocked;
  bounded streaming is a future design, not an implicit fallback.
- **DES-017 — Retrieval unavailable is not parity.** Missing runner,
  unsupported legacy query, absent corpus, timeout, or malformed output yields
  a typed `unavailable` parity row and blocks that consumer.
- **DES-018 — HGTS absence is terminal for its slice only.** The HGTS slice may
  do only identity/discovery checks until the checkout exists. It cannot use a
  substitute repository, cached claim, or inferred pass.
- **DES-019 — Disjoint consumer writers.** A consumer slice owns only its
  checkout and consumer-local evidence. Shared BRAN release and proposal
  surfaces complete earlier under exclusive ownership. Consumer slices may run
  concurrently only when their resolved write sets are pairwise disjoint.

### Compatibility, retirement, upstream proposals, and planning

- **DES-020 — Compatibility lease.** Adapters, legacy hooks, `use-okf`,
  legacy entrypoints, configurations, and prior binaries form a retained
  compatibility set. A consumer migration may stop invoking a legacy surface
  only after its gate passes, but may not delete the surface.
- **DES-021 — Retirement barrier.** Global retirement evaluates six immutable
  consumer receipts plus a fresh active-reference audit. Any non-passing
  receipt, unexpected reference, rollback failure, or unavailable repository
  keeps the barrier closed.
- **DES-022 — Retirement is a separate destructive transaction.** The final
  slice has an exact deletion/write manifest, recovery archive, restoration
  rehearsal, owner approval, apply step, and post-removal integrated gate.
  Removal is never embedded in a consumer slice.
- **DES-023 — Two proposal drafts, two later owner gates.** Draft
  `UPSTREAM-OKF-BUNDLE-SCOPE` as a narrow normative clarification and
  `UPSTREAM-OKF-LAYERED-PROFILES` as a design issue. Keep them local before
  release; recommend submission only after portable conformance and stable
  public release evidence. Each external submission requires approval of exact
  text and destination.
- **DES-024 — Canonical plan source.** The authoritative artifacts live only
  in `docs/plans/2026-07-22-bran-okf-final-cutover`. The stale auto-slugged path
  may be used only as a temporary Bearing receipt alias if the runtime cannot
  yet retarget it; it must not hold divergent content and must be removed
  before planning completion.
- **DES-025 — Evidence state is explicit.** Every receipt labels observations
  as `planned`, `passed`, `failed`, `unavailable`, or `rolled_back`, with
  command ID, revision, inputs, exit, and retained evidence locator. A plan or
  prior receipt cannot be promoted into current passing evidence.

## Stable contracts

- **CONTRACT-001 — ProfileOutcome.** Fields:
  `schema_version`, `profile`, `status`, `selected`, `diagnostics[]`,
  `bundle_identity`, `command_id`. `profile` is exactly `okf-v0.1` or
  `bran-strict`; status is computed independently.
- **CONTRACT-002 — ReservedDocumentDiagnostic.** Fields:
  `path`, `kind`, `code`, `message`, `normative_locator`. `kind` is `index` or
  `log`; no strict-only field code is allowed in the portable result.
- **CONTRACT-003 — GateReceipt.** Fields:
  `command_id`, `repository`, `revision`, `started_at`, `exit_code`, `status`,
  `input_digests`, `evidence_paths`, `first_failure`. Deterministic evidence
  excludes credentials and raw private corpus bodies.
- **CONTRACT-004 — ReleaseIdentity.** Exact tuple:
  `tag`, `source_commit`, `lockfile_sha256`, five platform archive SHA-256
  values, `SHA256SUMS` digest, signature digest, signer fingerprint, signed
  time, manifest digest, and immutable asset URLs.
- **CONTRACT-005 — InstallSnapshot.** Fields:
  `consumer`, `consumer_revision`, `release_identity`, `prior_pin`,
  `prior_digest`, `staged_path`, `selected_pin`, `selected_digest`,
  `verification_status`.
- **CONTRACT-006 — RollbackReceipt.** Fields:
  `consumer`, `trigger`, `from_digest`, `to_digest`, `restored_paths`,
  `byte_checks`, `commands`, `status`. Success requires all restored digests
  and focused commands to pass.
- **CONTRACT-007 — ParityReceipt.** Fields:
  `consumer`, `corpus_digest`, `native_command`, `legacy_command`,
  `native_raw`, `legacy_raw`, `normalizer_version`, `semantic_rows`,
  `validation_status`, `retrieval_status`, `overall_status`. Normalization may
  map field names, exit categories, and deterministic ordering only; it may not
  discard selected locator, precedence, diagnostic code, conflict, or
  unavailable state.
- **CONTRACT-008 — BoundaryRuleSet.** Fields:
  `schema_version`, `default`, and ordered entries of
  `repository_relative_path`, `match_kind`, `classification`. Paths are
  normalized, root-contained, and conflict-checked before scanning.
- **CONTRACT-009 — OversizedDocumentPlan.** Fields:
  `consumer`, `source_path`, `source_digest`, `size`, `split_paths`,
  `relationship_map`, `semantic_checks`, `rollback_digest`, `owner_approval`.
  It cannot authorize a global ceiling increase.
- **CONTRACT-010 — ConsumerGateReceipt.** Fields:
  `consumer`, `repository`, `revision`, `release_identity`, `install`,
  `validation_parity`, `retrieval_parity`, `hook_check`, `reference_audit`,
  `rollback`, `status`, `blockers`.
- **CONTRACT-011 — RetirementManifest.** Fields:
  six `ConsumerGateReceipt` identities, active-reference audit digest, exact
  writes/deletions, recovery archive digest, restoration proof, owner approval
  reference, apply and post-apply commands.
- **CONTRACT-012 — UpstreamProposalDraft.** Fields:
  `proposal_id`, `kind`, `problem`, `normative_text`, `examples`,
  `non_goals`, `local_evidence`, `recommended_timing`, `submission_status`.
  `submission_status` remains `not-authorized` in planning.
- **CONTRACT-013 — RouteTrace.** Each requirement maps to design decision,
  contract, SEIT case, command/procedure, prospective slice, evidence, stop
  condition, and rollback or explicit non-applicability.

## Use Cases and Communication Flows

### UC-1 — Repair and prove shared gates

```text
frozen OKF source -> reserved-rule fixtures -> ProfileValidator
                  -> okf-v0.1 outcome
                  -> bran-strict outcome (separate)

BRAN physical script path -> BRAN root -> scoped git enumeration
                          -> public-boundary scan -> GateReceipt

named journey/command/fixture inventory -> test-budget inventory -> inventory proof
shared results -> CMD-FAST -> pass or first-failure receipt
```

The reserved validator refuses uncited rules. The public checker never derives
its root from an AlphaZede parent layout. The budget check remains first in
`CMD-FAST`, so registry drift fails before expensive tests.

### UC-2 — Build, seal, approve, and publish

```text
clean exact commit + exact tag + Cargo.lock
  -> five deterministic platform archives
  -> SHA256SUMS -> owner-controlled signature
  -> release manifest + provenance
  -> local seal verification
  -> owner publication approval
  -> immutable release upload
  -> exact public download and SHA/version verification
```

Unsigned dry-run evidence is not a stable release. A manifest mismatch, floating
URL, dirty tree, wrong tag, missing target, signer mismatch, or public-boundary
failure stops before publication.

### UC-3 — Install and migrate one consumer

```text
consumer identity + approved release identity + prior install snapshot
  -> stage archive -> verify signature/digest/member/version
  -> switch one local pin
  -> native and legacy checks on frozen corpus
  -> semantic parity + hook check + reference audit
  -> rollback rehearsal
  -> ConsumerGateReceipt
```

The consumer slice writes only its own repository. Failure after pin selection
triggers rollback; failure before selection leaves the prior install untouched.

### UC-4 — Handle blocked consumer evidence

```text
AlphaZede Sports -> resolve explicit granular rules -> prove allow/deny/conflict
BetBot -> approve exact content split -> prove locators/retrieval/rollback
retrieval runner missing -> unavailable parity row
HGTS missing -> unavailable consumer receipt
```

Unavailable rows are retained with first-failure evidence. They block only the
affected consumer and the retirement barrier.

### UC-5 — Evaluate and execute retirement

```text
six passing immutable consumer receipts
  + fresh zero-active-reference audit
  + recovery archive and restoration rehearsal
  -> owner approval of exact retirement manifest
  -> apply deletion/write set
  -> integrated gates
  -> retirement success or exact restoration
```

No component of this flow runs during planning. The owner approval is specific
to the final manifest, not inherited from consumer approvals.

### UC-6 — Draft and later submit upstream proposals

```text
local conformance evidence -> two local proposal drafts
stable public release -> submission recommendation
exact text + destination -> separate owner approval
approval -> external submission (later authority only)
```

The bundle-scope clarification precedes the layered-profile design issue in the
recommended external sequence. Neither proposal changes local conformance
evidence retroactively.

## Interface Option Check

| Surface | Options considered | Selected interface | Material reason |
| --- | --- | --- | --- |
| Reserved-document validation | legacy delegation; separate command; core profile subvalidator | core `okf-v0.1` subvalidator | One semantic owner, same envelope, offline conformance; avoids adapter-owned rules. |
| Public checker root | caller CWD; Git parent discovery; physical checker path | physical BRAN checker path plus scoped Git enumeration | Works in standalone and nested checkouts and removes the live doubled-`bran` failure. |
| Release contract | new package format; checksum-only; existing exact signed manifest | existing five-archive signed manifest and seal | Already tested, deterministic, exact-tagged, and provenance-aware. |
| Installation | overwrite binary; package-manager latest; verified version slot plus pin | verified version slot plus atomic consumer-local pin | Preserves prior bytes and makes rollback exact. |
| Parity | compare prose; central hard-coded consumer logic; shared semantic receipt with repo-local commands | shared receipt and normalizer, repo-local invocation | Common acceptance without hiding repository-specific commands. |
| AlphaZede Sports boundary | global label; per-file duplication; ordered path rules | explicit default plus most-specific path rules | Expresses granular subtrees deterministically and fails on ambiguity. |
| BetBot oversized document | global cap increase; new streaming parser; owner-approved semantic split | semantic split with exact rollback | Smallest bounded route; no global safety regression or speculative parser. |
| Retirement | delete during each migration; fixed date; global evidence barrier | separate evidence-barrier transaction | Preserves compatibility until every consumer passes and gives one rollback boundary. |
| Upstream contribution | submit immediately; one combined proposal; two staged drafts | two drafts, later separate submissions | Keeps clarification and design debate distinct and respects publication evidence. |

## CDD

- Contracts are versioned and exact: profiles, gate receipts, release identity,
  installs, parity, consumer completion, retirement, and proposals.
- Adapters translate into `CONTRACT-001` and `CONTRACT-007`; they do not own
  validation or retrieval rules.
- Unknown fields that affect behavior are diagnosed rather than silently
  dropped.
- Schema/semantic-oracle pairs remain synchronized; generated representations
  do not become authority.
- Stable IDs survive implementation slicing and review generation.

## SecDD

- Treat repositories, archives, manifests, policy paths, hooks, legacy output,
  and proposal text as untrusted input.
- Normalize paths lexically, bind them to a canonical root, reject absolute and
  parent traversal, and reject symlink escape before reading or writing.
- Never print credentials, raw auth state, private corpus bodies, hidden truth,
  or unsanitized provider traces into receipts.
- Exact digest and signature verification occurs before installation selection.
- Publication and deletion are separate owner-authorized side effects.
- Compatibility and prior install bytes remain recovery assets until final
  proof succeeds.

## RDD

- Every multi-step operation is a state machine with a terminal typed state.
- Build, seal, install, parity, and retirement commands are retry-safe when
  inputs are identical; mismatched identities stop rather than overwrite.
- Consumer progress is monotonic per immutable receipt but global readiness is
  recomputed from all six receipts and a fresh reference audit.
- Interrupted installation leaves either the prior selected pin or a blocked
  staged version; it cannot report success without readback.
- Rollback uncertainty is failure, never success with a warning.

## ODD

- Each command emits or is wrapped by `CONTRACT-003` with exact revision,
  input digests, exit, first failure, and evidence locators.
- Public claims derive from current receipts, not plan text.
- The review distinguishes planned, passed, failed, unavailable, and
  rolled-back states.
- Deterministic ordering and content-free digests allow comparison without
  leaking private bodies.
- Missing optional telemetry does not alter semantic status.

## OOPDSA Implementation Design

### Ownership model

- `ProfileValidator` owns `ProfileOutcome`; an `OkfReservedValidator` strategy
  contributes only portable reserved-document diagnostics.
- `PublicSurfaceRoot` is a value object constructed from the physical checker
  path and used by the public checker; callers cannot inject a broader root.
- Existing release scripts and `ReleaseIdentity` own release proof. No
  `ReleaseManager` framework is introduced.
- `ConsumerMigration` coordinates one `InstallSnapshot`, `ParityReceipt`,
  reference audit, and `RollbackReceipt`.
- `RetirementBarrier` is a pure evaluator over six consumer receipts and a
  fresh audit. A separate retirement procedure performs authorized writes.

### State machines

```text
Release: planned -> built -> sealed -> approved -> published -> verified
                       \-> failed

Consumer: discovered -> approved -> staged -> selected -> parity_checked
         -> rollback_proven -> passed
         \-> unavailable | failed -> rolled_back

Retirement: ineligible -> eligible -> approved -> applied -> verified
                                       \-> restoring -> restored | failed
```

Transitions require the exact preceding identity; there is no boolean
`ready=true` shortcut.

### Patterns used

- **Strategy:** portable reserved validation and strict readiness validation
  share bundle input while preserving separate rule sets and outcomes.
- **Adapter:** legacy commands/configuration map into native requests and
  semantic receipts only.
- **State machine:** release, consumer, and retirement lifecycles expose
  partial and recovery states.
- **Value objects:** revisions, digests, normalized repository paths, consumer
  IDs, and profile IDs reject malformed values at construction.

No dependency-injection framework, event bus, database, service, or generic
workflow engine is added.

### Deterministic data structures and algorithms

- Use `BTreeMap`/`BTreeSet` for diagnostics, profile rows, asset names,
  reference audits, and consumer identities.
- Sort diagnostics by `(path, code, message)` and parity rows by semantic key.
- Normalize boundary rules once, reject duplicate/conflicting normalized paths,
  then sort by descending path-component count and lexical path. The first
  matching rule wins; equal-specific conflicts are invalid.
- Compute SHA-256 in bounded chunks. Do not load release archives or oversized
  inputs solely to hash them.
- Represent implementation dependencies as a DAG. Use Kahn topological sorting
  with lexical slice-ID tie-breaking; reject cycles and overlapping write sets
  before execution.
- Compare exact write sets with normalized path-prefix intersection, treating
  a repository root as overlapping all descendants.

Complexity remains bounded by repository paths, rules, assets, and slices:
sorting is `O(n log n)`, matching is `O(r * p)` for small policy rule sets, and
hashing is `O(bytes)` with bounded memory.

## Prospective execution waves

Implementation drafting must preserve this dependency shape:

1. **Wave 1 — Shared gate truth:** reserved conformance, public-root repair,
   budget regression, independent profile reporting.
2. **Wave 2 — Local proposal drafts and release readiness:** two proposal
   drafts, release build/seal proof, publication packet.
3. **Wave 3 — Owner-gated publication and exact public verification:** no
   consumer mutation before a stable release identity exists.
4. **Wave 4 — Six disjoint consumer migrations:** parallel only after exact
   write-set comparison and per-consumer approval.
5. **Wave 5 — Global evidence audit:** six receipts, fresh references,
   rollback archive, retirement manifest.
6. **Wave 6 — Separately approved retirement:** destructive apply and
   restoration path.

## Design stop conditions

Stop when:

- normative reserved-file rules cannot be cited;
- the public checker must broaden beyond the BRAN/public export surface;
- an exact source/tag/artifact/signature identity cannot be proven;
- a consumer identity, revision, write set, or prior install cannot be read;
- parity requires discarding semantic differences;
- a boundary conflict or oversized-document split lacks an explicit safe
  resolution and rollback;
- HGTS or retrieval evidence remains unavailable for a claimed passing gate;
- any writer overlaps another active writer;
- compatibility removal is proposed before six passing receipts;
- publication, consumer mutation, proposal submission, or retirement lacks its
  exact owner approval; or
- the Bearing runtime would force divergent artifacts into the rejected
  auto-slugged plan directory.

## Handoff to implementation drafting

### Role and outcome

Act as the bounded Bearing implementation-drafting agent. Reuse this design and
`seit.md`; create traceable implementation slices only after the design/SEIT
checkpoint validates.

### Scope and authority

Write only `implementation.md` in the canonical plan directory. Do not execute,
publish, mutate consumers, submit proposals, remove compatibility, or hand-edit
`review.html`. Use only owner-supported route labels and reasoning levels.

### Execute now

Map each prospective SEIT row to one bounded slice with an exact write set,
dependency wave, model route, verification commands, retained evidence, stop
condition, owner gate, and rollback. Give each consumer exactly one migration
slice and reject overlapping writers.

### Verification and evidence

Prove complete bidirectional traceability, valid wave order, six consumer
slices, supported assignments, and a separate final retirement slice. Bearing,
not the agent, generates the baseline and final `review.html`.

### Return or stop conditions

Return only `implementation.md` and the Bearing-generated review artifacts at
the supplied checkpoint. Stop before execution and on any authority expansion,
unsupported model route, missing traceability, overlapping write set, or
runtime attempt to continue a divergent duplicate plan.

## 2026-07-23 Slice 2.1 wire-contract clarification

This section provides the exact append-only wire-contract clarification for Slice 2.1 to settle JSON wire shapes, evidence locator/digest bindings, reference classifications, and revision-bound freshness semantics without introducing new requirements, contracts, slices, design IDs, paths, commands, external authority, or consumer-specific semantics.

### 1. Common encoding and evidence binding

- **Encoding and key validation:** Every manifest and receipt is UTF-8 JSON. Duplicate keys within any JSON object are strictly rejected. Behavioral objects use exact documented keys; any unrecognized or unknown behavioral key causes verification failure.
- **Digests and revisions:** Digests are exact lowercase 64-character hexadecimal SHA-256 strings. Revisions are exact lowercase Git object IDs of 40 or 64 hexadecimal characters.
- **Evidence locators:** Evidence locators are normalized POSIX paths relative to the supplied evidence directory. Locators cannot be absolute, empty, single dot (`.`), parent traversal (`..`), or contain backslashes (`\`), and must resolve to non-symlink regular files physically contained within that evidence directory.
- **Evidence digest pairing:** Every evidence locator is paired with its SHA-256 digest. The verifier recomputes all evidence digests in bounded chunks during execution. The verifier never emits raw evidence bodies in receipts or diagnostic output.
- **Inert commands:** Commands embedded in receipts are inert strings or structured command records. Verifier modes treat commands strictly as read-only evidence and never execute them.
- **Canonical ordering and JSON hashing:** Deterministic canonical ordering is lexical by consumer, path, and semantic key. Duplicate keys, duplicate locators, or duplicate consumers cause immediate verification failure. Wherever canonical compact JSON is hashed, it is defined uniformly as UTF-8 `json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)` bytes, with arrays already in their required lexical order.

### 2. ReleaseIdentity wire shape (CONTRACT-004)

`ReleaseIdentity` is an exact JSON object containing these top-level keys:
`tag`, `source_commit`, `lockfile_sha256`, `archives`, `checksums_sha256`, `signature_sha256`, `signer_fingerprint`, `signed_at`, `manifest_sha256`, `asset_urls`.

- `archives` is an exact object keyed by the five approved target triples (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`), with each value being its lowercase 64-hex SHA-256 digest.
- `asset_urls` is an exact lexically sorted array of immutable direct URL strings for the five target archives plus `SHA256SUMS`, `SHA256SUMS.sig`, and `bran-release-manifest.json` (8 URLs total). URLs using `latest`, query parameters, URL fragments, duplicate entries, or tag mismatches are strictly rejected.
- Equality of `ReleaseIdentity` requires exact byte/field equality across this entire normalized object; tag-only identity matches are rejected.

### 3. ConsumerGateReceipt wire shape (CONTRACT-010)

`ConsumerGateReceipt` is an exact JSON object with the top-level keys:
`consumer`, `repository`, `revision`, `release_identity`, `install`, `validation_parity`, `retrieval_parity`, `hook_check`, `reference_audit`, `rollback`, `status`, `blockers`.

- `consumer` is any nonempty stable identifier string. `repository` is the canonical repository identity string. `revision` must equal the CLI revision argument and current clean checkout HEAD.
- `status` is one of the four-state vocabulary: `passed`, `failed`, `unavailable`, `rolled_back`. Only `passed` permits a successful exit code (0).
- `blockers` is a lexically sorted array of unique string blocker codes. `blockers` must be empty if and only if `status` is `passed`.

### 4. Nested object wire shapes

- **InstallSnapshot (CONTRACT-005):** Exact top-level keys: `consumer`, `consumer_revision`, `release_identity`, `prior_pin`, `prior_digest`, `staged_path`, `selected_pin`, `selected_digest`, `verification_status`. `staged_path` and `prior_pin` are evidence locators relative to the supplied evidence directory; their bytes must hash to `selected_digest` and `prior_digest` respectively. `selected_pin` is a nonempty inert string identifying the selected pin. `verification_status` is `passed` only when `staged_path` bytes hash to `selected_digest`, `prior_pin` bytes hash to `prior_digest`, the exact `ReleaseIdentity` matches, and prior pin/digest values are retained. This tool verifies a captured snapshot and does not switch the pin.
- **ParityReceipt (CONTRACT-007):** Exact top-level keys: `consumer`, `corpus_digest`, `native_command`, `legacy_command`, `native_raw`, `legacy_raw`, `normalizer_version`, `semantic_rows`, `validation_status`, `retrieval_status`, `overall_status`. `native_raw` and `legacy_raw` are evidence locator/digest objects. Raw evidence files must be UTF-8 JSON arrays whose normalized entries correspond one-to-one with semantic rows, allowing only the documented field-name, exit-category, and ordering transformations. `semantic_rows` is a lexically sorted list of objects containing `semantic_key`, `native`, and `legacy`. Every `semantic_rows[].native` and `.legacy` is an exact object with always-present keys `locator`, `precedence`, `diagnostic_code`, `conflict`, `unavailable`, plus `outcome` (nullable values are allowed, but the keys cannot be dropped). `normalizer_version` is a nonempty pinned version string. States (`validation_status`, `retrieval_status`, `overall_status`) use the four-state vocabulary (`passed`, `failed`, `unavailable`, `rolled_back`); any non-passing status blocks. The two `ConsumerGateReceipt` parity fields (`validation_parity`, `retrieval_parity`) are independently validated receipts over the same `consumer` and `corpus_digest`, nested under the authoritative `ConsumerGateReceipt` `ReleaseIdentity`; neither raw output may be absent.
- **Hook check:** Exact JSON object with keys `commands`, `evidence`, `status`. `commands` is an array of inert nonempty string commands, `evidence` is an array of locator/digest objects, and `status` must be `passed`.
- **Reference audit:** Exact JSON object with keys `consumer`, `revision`, `expected_compatibility`, `matches`, `inventory_digest`, `evidence`, `status`. `expected_compatibility` is a sorted unique list of exact objects with `path` and `kind`; paths are normalized consumer-relative POSIX paths and `kind` uses the existing kind vocabulary (`code`, `skill`, `hook`, `ci`, `configuration`, `historical-documentation`). `matches` is a sorted list of objects containing `path`, `line`, `kind`, `classification`, `match_sha256`. `match_sha256` is the SHA-256 of the exact matched source line bytes read from the clean consumer checkout at `path` and `line`, recomputed by the verifier. `kind` must be one of `code`, `skill`, `hook`, `ci`, `configuration`, `historical-documentation`. `classification` must be one of `native-active`, `compatibility-active`, `historical`, `unexpected-active`, `unclassified`. Passing status permits only `native-active`, `compatibility-active`, and `historical`, requires a matching `compatibility-active` match for every expected object in `expected_compatibility` and no `compatibility-active` match absent from that list, and rejects any `unexpected-active` or `unclassified` entries. This is manifest-driven and has no built-in per-consumer list. `inventory_digest` is SHA-256 of compact UTF-8 JSON encoding (`json.dumps(matches, sort_keys=True, separators=(",", ":"), ensure_ascii=False)` bytes) of the sorted `matches` array.
- **RollbackReceipt (CONTRACT-006):** Exact top-level keys: `consumer`, `trigger`, `from_digest`, `to_digest`, `restored_paths`, `byte_checks`, `commands`, `status`. Every `restored_paths` item is an exact object with keys `path`, `sha256`, `source`; `source` must be `evidence` or `consumer`. Evidence paths resolve beneath the evidence root; consumer paths resolve beneath the clean consumer checkout; both reject symlinks and path traversal escapes, and their bytes are hashed to `sha256`. `byte_checks` is a sorted list of records with keys `path`, `expected`, `actual`, `status`. `commands` is a sorted list of records with keys `command`, `exit_code`, `status`, `evidence`. Passing requires exact `from_digest` and `to_digest` matching, all byte checks and commands passing with status `passed`, and no missing items.

### 5. RetirementManifest wire shape (CONTRACT-011)

`RetirementManifest` is an exact JSON object with keys:
`consumers`, `active_reference_audit`, `writes`, `deletions`, `recovery_archive`, `restoration_proof`, `owner_approval_reference`, `apply_commands`, `post_apply_commands`.

- `consumers` is an exact array of six sorted consumer summary objects containing `consumer`, `repository`, `revision`, `release_identity`, `receipt`, `receipt_sha256`, `status` for `Alphazedehq`, `alphazede-sports`, `betbot`, `developers`, `hgts`, and `alphazede-markets`. Each referenced `receipt` is bound to a regular evidence file and independently validates as a passing `ConsumerGateReceipt` with matching revision and release identity.
- `active_reference_audit`, `recovery_archive`, and `restoration_proof` are evidence locator/digest objects.
- The file referenced by `active_reference_audit` is strict JSON with exact keys `consumers`, `status`. Its `consumers` array contains the exact six sorted objects with `consumer`, `repository`, `revision`, `release_identity`, `inventory_digest`, `status`, each matching the corresponding independently validated `ConsumerGateReceipt`; top-level and per-consumer status must be `passed`.
- The file referenced by `restoration_proof` is strict JSON with exact keys `byte_checks`, `commands`, `status`, using the same byte-check and command-record shapes as `RollbackReceipt`, and every item and status must pass.
- `recovery_archive` references the actual non-symlink regular archive file beneath the evidence directory and its digest is recomputed.
- `writes` and `deletions` are exact sorted unique lists of normalized repository-relative paths, validated as data structures only.
- `owner_approval_reference` is a nonempty inert string reference proving the manifest carries an approval tracking string; its presence does not imply verifier grant or confirmation of owner approval.
- `apply_commands` and `post_apply_commands` are arrays of nonempty inert strings and are never executed by `verify`.
- Retirement eligibility requires exact identity equality among these six current consumer receipts and the global audit (`active_reference_audit`), matching revision and release identity, live digest recomputation for all referenced evidence files, and passing status across all receipts and audits (no wall-clock threshold). Eligibility does not authorize execution of retirement commands.

### 6. Freshness and read-only verifier semantics

- **Identity-bound freshness:** Freshness is identity-bound rather than wall-clock-based. An audit or receipt is fresh if and only if its recorded revision equals the CLI revision argument and current clean checkout HEAD, its `ReleaseIdentity` equals the selected release identity, and every evidence file digest is recomputed from the supplied evidence directory during the current verifier invocation. Freshness for retirement is proven by exact identity equality among the six current receipts and the global audit plus live digest recomputation, with no wall-clock threshold.
- **Unavailable telemetry:** Missing or incomplete optional telemetry produces an `unavailable` state and is never normalized into a semantic pass.
- **Read-only enforcement:** Verifier modes (`install-verify`, `parity`, `reference-audit`, `rollback`, and retirement `verify`) are strictly read-only. They never generate receipts, alter pins, restore files, execute commands, sign artifacts, publish assets, upload packages, or mutate any repository or evidence directory.
