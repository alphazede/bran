---
type: seit
name: bran-okf-migration
status: amended
date: 2026-07-21
applies_to: bran
plan_spec: ./plan-spec.md
design: ./design.md
---

## Scope

Verification covers the BRAN-native repository policy, strict validation,
deterministic retrieval precedence, maintenance proposal/apply/revalidate lifecycle,
derived-state self-healing, legacy command/configuration translation, preserved hook
behavior, per-consumer parity evidence, and evidence-based adapter retirement.

The release-blocking baseline is offline and deterministic. Public publication,
installation from a public release, live provider use, direct migration of consumer
repositories, and final removal of adapters are separate owner-authorized actions.

## Required References

- `./plan-spec.md` - AC-1 through AC-7, RISK-1 through RISK-5, and owner decisions.
- `./design.md` - DES-1 through DES-8 and CONTRACT-1 through CONTRACT-9.
- `/home/spectre/alphazede/bran/AGENTS.md` - private BRAN boundary and integrated check.
- `/home/spectre/alphazede/bran/docs/plans/AGENTS.md` - plan ownership and publication boundary.
- `/home/spectre/alphazede/bran/crates/bran-core/src/profile.rs` - current dual-profile validation.
- `/home/spectre/alphazede/bran/crates/bran-core/src/repair/mod.rs` - current repair state machine.
- `/home/spectre/alphazede/bran/crates/bran-core/src/graph/query.rs` - current retrieval precedence.
- `/home/spectre/alphazede/bran/crates/bran-cli/src/main.rs` - current command envelope and typed exits.
- `/home/spectre/alphazede/Alphazedehq/tools/okf/okf` - compatibility command surface.
- `/home/spectre/alphazede/Alphazedehq/tools/okf/config.yaml` - legacy configuration input.
- `/home/spectre/alphazede/Alphazedehq/.grok/hooks/use-okf.sh` and
  `use-okf.json` - hook behavior to preserve.

## Required Commands

Commands marked planned are created by the implementation and must exist before their
mapped proof row can pass.

- **CMD-POLICY** - planned: `cargo test --manifest-path /home/spectre/alphazede/bran/Cargo.toml -p bran-core repository_policy`
- **CMD-PROFILE** - `cargo test --manifest-path /home/spectre/alphazede/bran/Cargo.toml -p bran-core profile::tests`
- **CMD-QUERY** - `cargo test --manifest-path /home/spectre/alphazede/bran/Cargo.toml -p bran-core graph::query::tests`
- **CMD-PACKET** - `cargo test --manifest-path /home/spectre/alphazede/bran/Cargo.toml -p bran-core packet::tests`
- **CMD-MIGRATION** - planned: `cargo test --manifest-path /home/spectre/alphazede/bran/Cargo.toml -p bran-core migration::tests`
- **CMD-SCAN** - `cargo test --manifest-path /home/spectre/alphazede/bran/Cargo.toml -p bran-core scan::tests`
- **CMD-REPAIR** - `cargo test --manifest-path /home/spectre/alphazede/bran/Cargo.toml -p bran-core repair::tests`
- **CMD-CLI** - `cargo test --manifest-path /home/spectre/alphazede/bran/Cargo.toml -p bran-cli maintain`
- **CMD-DERIVED** - planned: `cargo test --manifest-path /home/spectre/alphazede/bran/Cargo.toml -p bran-core derived_state_rebuild`
- **CMD-ADAPTER** - `python3 /home/spectre/alphazede/Alphazedehq/tools/okf/test_bran_runtime.py`
- **CMD-HOOK** - `/home/spectre/alphazede/Alphazedehq/.grok/hooks/test-use-okf.sh`
- **CMD-FAST** - `/home/spectre/alphazede/bran/tools/ci/check.sh --fast`
- **CMD-PUBLIC** - `python3 /home/spectre/alphazede/bran/tools/ci/public_boundary_check.py`
- **PROC-PARITY** - For each active consumer, run the pinned BRAN native validation and
  retrieval corpus, run the legacy adapter against the identical frozen inputs, normalize
  semantic results, retain the diff receipt, and do not mutate the consumer.
- **PROC-REFERENCES** - Search active code, skill, hook, CI, and configuration surfaces for
  `use-okf`, `tools/okf/okf`, and `tools/okf/config.yaml`; classify historical-only hits and
  retain the repository-relative audit.
- **PROC-PIN** - Build BRAN locally, compute and record its checksum, install only at the
  owner-approved stable local path, and verify the invoked binary matches the pin before
  shadow or hook use.
- **PROC-ROLLBACK** - In a disposable fixture, preserve target bytes and file-existence state,
  force validator failure after staged apply, and compare restored state byte-for-byte with
  the pre-apply state and receipt lifecycle.
- **PROC-PUBLICATION** - Inspect proposed public files and release assets for private plans,
  corpora, credentials, auth state, internal paths, hidden grader truth, and unsupported
  claims; publication remains blocked without explicit owner approval.

## CI Stage Inventory

| Stage | Backing command/procedure | State | Gate |
| --- | --- | --- | --- |
| Native policy contracts | CMD-POLICY, CMD-PROFILE | planned plus active baseline | required |
| Retrieval precedence | CMD-QUERY | active, expanded fixtures planned | required |
| Repair lifecycle | CMD-REPAIR, CMD-CLI, PROC-ROLLBACK | active, production authority expansion planned | required |
| Derived-state self-heal | CMD-DERIVED | planned | required |
| BRAN integrated fast gate | CMD-FAST | active | required |
| Compatibility adapter | CMD-ADAPTER | active, parity expansion planned | required while adapter exists |
| Hook characterization | CMD-HOOK | active, BRAN-backed cases planned | required while hooks exist |
| Consumer parity | PROC-PARITY | planned per consumer | required for that consumer's retirement |
| Active-reference audit | PROC-REFERENCES | planned per consumer and global | required for retirement |
| Public boundary | CMD-PUBLIC, PROC-PUBLICATION | active plus manual release review | required before publication |

Missing optional telemetry does not make a stage fail. A required stage fails only on
its semantic contract, executable error, unauthorized mutation, containment breach,
unsupported evidence, or genuine material error.

## Integration Test Procedures

### IT-1 - Native policy and deterministic validation

1. Load valid `.bran/policy.yaml` fixtures and verify the normalized policy and stable
   schema version.
2. Exercise missing, malformed, unsupported-version, unknown-field, duplicate path,
   overlapping classification, unsafe path, status/tag, source-link, and public-boundary
   fixtures.
3. Run the same fixture repeatedly and under permuted source discovery order.
4. Require byte-identical ordered diagnostic identities and matching typed exits.

### IT-2 - Legacy translation and semantic parity

1. Freeze legacy `tools/okf/config.yaml` inputs for every supported field and edge case.
2. Translate to the immutable native policy without creating or rewriting repository files.
3. Run legacy and native validation/retrieval over identical corpus identities.
4. Compare normalized rule, locator, classification, source-precedence, and terminal
   outcomes; retain formatting-only differences separately from semantic differences.
5. Verify absent telemetry is represented as unavailable and does not alter parity success.

### IT-3 - Hook fail-open behavior

1. Exercise SessionStart, relevant/irrelevant prompt, relevant/irrelevant edit, and dynamic
   repository discovery.
2. Exercise valid pin, missing binary, checksum mismatch, timeout, malformed output,
   validation findings, and unwritable optional receipt directory.
3. Require bounded compact output, no repository source writes, and exit zero for every hook
   event.
4. Run explicit validation separately and prove that genuine findings still exit non-zero.

### IT-4 - Repair authority and recovery

1. Prove proposal is read-only and captures exact absent/present original state.
2. Reject blank authority, wrong digest, stale same-length edits, traversal, absolute/NUL
   paths, symlink ancestors, and targets outside the repository root before mutation.
3. Exercise a successful staged write and require validation-passed receipt only after native
   revalidation.
4. Force validation failure for existing and newly created files; require exact restoration
   and validation-failed/restored receipt.
5. Force staged-write and rollback I/O failures; require operation/partial-write uncertainty,
   retained recovery evidence, and no false success.

### IT-5 - Derived-state self-healing

1. Corrupt or remove BRAN-owned index, cache, snapshot, report, and generated validator
   artifact fixtures.
2. Rebuild each from unchanged source/policy and compare deterministic output.
3. Present source, metadata, classification, configuration, and source-link targets to the
   auto-rebuilder and require refusal plus a proposal-only result.
4. Verify rebuild failure preserves the prior usable derived artifact when one exists.

### IT-6 - Consumer migration and retirement

1. Inventory Alphazedehq, alphazede-sports, betbot, developers, hgts, and
   alphazede-markets independently.
2. Verify the checksum-pinned local BRAN build before any shadow run.
3. Retain policy identity, corpus identity, BRAN pin, semantic parity result, missing fields,
   and active-reference audit for each consumer.
4. Mark only consumers with native policy, BRAN-backed skill/hook, passing parity, and no
   active legacy reference eligible for retirement.
5. Prove a failed consumer does not change completed consumer evidence, and global removal
   remains blocked until all consumers pass and the owner approves.

## Traceability Matrix

| SEIT row ID | Acceptance/risk ID | Design/contract ID | Boundary/test layer | Positive case | Negative/failure case | Command/procedure ID | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| SEIT-1 | AC-1 | DES-4, CONTRACT-1 | adapter contract | old call reaches native behavior | adapter implements divergent rule | CMD-ADAPTER | normalized invocation/result fixture |
| SEIT-2 | AC-2 | DES-1, DES-8, CONTRACT-3 | core policy/schema | valid native policy passes | malformed/version/path rule fails typed | CMD-POLICY | policy fixture and ordered diagnostics |
| SEIT-3 | AC-2 | DES-2 | core validation | strict categories pass | frontmatter/source/public violation fails | CMD-PROFILE | dual-profile outcomes and exits |
| SEIT-4 | AC-3 | DES-3, CONTRACT-2 | repair unit/CLI | exact authorized proposal applies | blank authority or wrong digest refused | CMD-REPAIR, CMD-CLI | terminal-state and mutation trace |
| SEIT-5 | AC-4 | DES-6, CONTRACT-2, CONTRACT-9 | recovery integration | revalidation passes after stage | validator failure restores exact state | PROC-ROLLBACK | before/after digest and receipt |
| SEIT-6 | AC-5 | DES-4, CONTRACT-7 | retrieval parity | stronger canonical source wins | permuted/tied input changes outcome | CMD-QUERY, PROC-PARITY | ordered canonical ranks and semantic diff |
| SEIT-7 | AC-6 | DES-3, DES-5, CONTRACT-9 | mutation security | owned derived rebuild succeeds | source/config auto-rewrite refused | CMD-DERIVED, CMD-REPAIR | path class and write-set receipt |
| SEIT-8 | AC-7 | DES-7, CONTRACT-5 | migration acceptance | all consumer evidence complete | active legacy reference blocks removal | PROC-PARITY, PROC-REFERENCES | six-consumer matrix and owner decision slot |
| SEIT-9 | RISK-1 | DES-6, DES-7, CONTRACT-5 | migration isolation | completed consumer remains complete | one mismatch invalidates global evidence | PROC-PARITY | independent consumer states |
| SEIT-10 | RISK-2 | DES-6, CONTRACT-4 | hook integration | three triggers invoke bounded BRAN | unavailable/timeout blocks agent | CMD-HOOK | exit codes, timings, write audit |
| SEIT-11 | RISK-3 | DES-5 | release boundary | approved scrubbed artifact remains separate | private material enters proposed release | CMD-PUBLIC, PROC-PUBLICATION | boundary report and approval status |
| SEIT-12 | RISK-4 | DES-1, DES-8, CONTRACT-3 | interface/schema | native version round-trips | implicit/legacy schema becomes authority | CMD-POLICY | schema fixture and adapter no-write audit |
| SEIT-13 | RISK-5 | DES-7, CONTRACT-5, CONTRACT-8 | evidence/observability | partial metrics retained | missing metric declares no result | PROC-PARITY | receipt with explicit unavailable fields |
| SEIT-14 | AC-3, AC-6 | DES-5, CONTRACT-2 | security negative | root-relative regular target accepted | traversal/symlink/stale input mutates | CMD-REPAIR | pre-write path and stale checks |
| SEIT-15 | AC-1, AC-7 | DES-7 | supply/release identity | invoked BRAN matches checksum pin | unpinned/replaced binary is used | PROC-PIN | artifact checksum and invocation receipt |
| SEIT-16 | AC-2, RISK-4 | DES-9, CONTRACT-3 | CLI policy input | file and stdin policies produce identical ordered results | missing, conflicting, oversized, malformed, or secret-bearing stdin is accepted or logged | CMD-POLICY, CMD-CLI | input-source identity, typed exit, ordered diagnostics, no-write audit |
| SEIT-17 | AC-1, AC-5, AC-7 | DES-10, CONTRACT-10 | legacy semantic parity | coverage, metadata, sources, boundaries, packets, and body-preservation outcomes match | adapter drops an affecting field or owns a rule | CMD-POLICY, CMD-PROFILE, CMD-PACKET, CMD-MIGRATION, CMD-ADAPTER, PROC-PARITY | normalized per-contract diff and unsupported-field receipt |
| SEIT-18 | AC-1, AC-2, AC-7, RISK-4 | DES-11, CONTRACT-11 | native parser and adapter serialization | apostrophes, quotes, backslashes, hashes, colons, and surrounding spaces round-trip identically through file and stdin policy sources | malformed delimiters, unsupported escapes, controls, secret-bearing duplicates, or arbitrary child output are accepted, changed, or echoed | CMD-POLICY, CMD-CLI, CMD-ADAPTER | native parser regression output, adapter sentinel output, real-binary envelope, and no-write audit |
| SEIT-19 | AC-1, AC-2, AC-7, RISK-4 | DES-12, CONTRACT-12 | scanner input classification | oversized PNG and NUL-bearing binary are bounded-probed, reported unsupported, and do not prevent both profiles from evaluating | oversized valid UTF-8 text, symlink escape, changing file, or binary prefix ambiguity bypasses limits or is fully buffered | CMD-SCAN, CMD-CLI, CMD-ADAPTER | focused scan tests, read/byte accounting, both selected-profile envelopes, and no-write snapshot |
| SEIT-20 | AC-1, AC-2, AC-7, RISK-4 | DES-13, CONTRACT-13 | native check scan scope | oversized generated non-candidate text is unopened and both profiles evaluate | oversized admitted Markdown, filtered/full mismatch, or a non-check scanner silently drops source text | CMD-SCAN, CMD-CLI, CMD-ADAPTER | shared-predicate unit tests, generic-scanner regression, both selected-profile envelopes, and no-write snapshot |

## Cross-cutting Checks

- Determinism: repeat and permute discovery order; compare semantic output identities.
- Mutation containment: snapshot the fixture tree before and after every read-only, parity,
  and hook case.
- Public/private boundary: scan sources, fixtures, receipts, logs, and proposed artifacts.
- Unsupported evidence: every cited locator must exist in the frozen corpus; unavailable
  evidence is explicit.
- Partial telemetry: preserve present values and mark absent values unavailable without
  changing semantic validation or task status.
- Compatibility: native BRAN is the only rule owner; adapters are characterized until
  evidence-based retirement.
- Recovery: successful rollback is byte-exact; uncertain rollback cannot report success.

## Optional / Unavailable Tools

- Live provider or network evaluation is unavailable by design and unnecessary.
- Public release installation is deferred until owner-authorized publication.
- A centralized telemetry backend is not required; deterministic local receipts suffice.
- Consumer CI that is unavailable during a migration run is recorded as unavailable; native
  fixture parity and later repository CI evidence remain separately visible.

## Gate Evidence

Retain command, exact BRAN commit and artifact checksum, policy/corpus identity, exit code,
structured output, fixture mutation audit, and normalized semantic diff for every required
row. Required evidence is prospective until implementation executes it. No future success is
claimed in this design pass.

Pre-implementation readiness requires all planned commands to exist, SEIT-1 through SEIT-17
to have an implementation owner, `CMD-FAST` to pass on the integrated BRAN diff, compatibility
tests to pass in their owning repository, and publication to remain unperformed unless the
owner separately authorizes it.

## SEIT Amendments

### 2026-07-22 owner-approved amendment

The owner approved an additive native `--policy-stdin` source and incorporation of
semantic legacy OKF capabilities missing from BRAN. SEIT-16 and SEIT-17 cover the new
input boundary and product-parity obligations. No provider, publication, deployment,
consumer mutation, legacy removal, or dependency-install authority was added.

### 2026-07-22 delegated native quoted-scalar amendment

The owner delegated bounded product-quality decisions during execution. A real adapter
invocation proved that standard YAML apostrophe escaping was not decoded by BRAN, and a
sentinel proved that an adapter error echoed a raw excluded-document path. SEIT-18
requires the smallest parser and adapter repair plus direct real-binary proof before
Slice 3.1 can complete.

### 2026-07-22 oversized binary scan-isolation amendment

The unchanged AlphaZedeHQ corpus reached native policy parsing but aborted before profile
selection on an unrelated oversized PNG. SEIT-19 requires bounded binary classification,
preserves hard limits for oversized UTF-8 text, and proves that both canonical profiles
evaluate the unchanged corpus without consumer mutation.

### 2026-07-22 check-time knowledge-candidate amendment

After binary isolation, the unchanged corpus reached an oversized generated
`review.html` that the native check bundle cannot consume. SEIT-20 aligns only the check-
time scanner with its existing knowledge-candidate predicate while preserving the
general-purpose scanner, all accepted Markdown limits, and adapter translation-only
ownership.
