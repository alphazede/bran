---
type: design
name: bran-okf-migration
status: amended
date: 2026-07-21
applies_to: bran
plan_spec: ./plan-spec.md
lenses_applied: [CDD, SecDD, RDD, ODD]
lenses_skipped: [BizDD, DDD, EDD, GDD, PDD]
---

## Synthesis

BRAN becomes the sole owner of repository-knowledge policy, validation, retrieval,
and bounded repair semantics. A thin compatibility boundary preserves the legacy
`use-okf` skill, hook triggers, `tools/okf/okf` entrypoint, and
`tools/okf/config.yaml` inputs while consumers migrate. Compatibility code may
translate requests and results, but it may not implement validation rules, ranking,
repair authority, or source mutation.

The design has four layers:

1. `bran-core` owns the native policy model, deterministic validation, retrieval
   precedence, derived-state rebuilding, and repair state machine.
2. `bran-cli` owns versioned command envelopes, typed exits, native policy loading,
   and explicit maintenance authorization input.
3. A legacy adapter owns only old command/configuration translation and parity
   comparison. It never rewrites legacy configuration.
4. Repository hooks discover the pinned BRAN executable and invoke bounded,
   fail-open advisory operations. Explicit CLI validation and CI remain strict.

The native repository policy is `.bran/policy.yaml`, with an explicit
`schema_version`. It uses BRAN terms and is validated before repository scanning.
The choice reuses BRAN's existing dependency-free YAML value/parser surface,
keeps policy human-reviewable, and avoids claiming the legacy OKF file is the native
contract. Generated JSON schemas and reports remain BRAN-owned derived artifacts.

The pre-lens test stance remains contract-first and offline: frozen repositories
must prove native validation, legacy parity, source precedence, hook degradation,
and exact repair rollback without providers, network access, or live consumer
mutation. Lens analysis adds threat-boundary, fault-injection, receipt, and
observability assertions.

## Use Cases and Communication Flows

### Flow 1 - Native validation

```text
caller -> bran CLI -> .bran/policy.yaml loader -> repository scanner
       -> normalized bundle -> BRAN strict validator -> deterministic envelope + exit
```

Text equivalent: an explicit CLI or CI call loads and validates the versioned native
policy before scanning. BRAN normalizes repository evidence, runs strict rules, and
returns ordered diagnostics. Policy, scan, or validation errors remain distinct and
only the selected profile controls the terminal exit.

### Flow 2 - Legacy adapter during staged migration

```text
legacy caller -> use-okf/tools/okf adapter -> legacy config reader
              -> normalized BRAN policy request -> BRAN core/CLI
              -> legacy-shaped result + parity receipt
```

Text equivalent: the adapter accepts the old command and configuration without
rewriting either. It translates them into BRAN-native requests, invokes the same
core behavior as native callers, and translates the result shape. Shadow mode records
both outcomes and differences; it never gives the legacy implementation authority
over BRAN behavior.

### Flow 3 - Advisory hook

```text
SessionStart | relevant prompt | relevant edit
  -> dynamic repository discovery -> trusted pinned BRAN binary
  -> bounded advisory query/check -> compact message
  -> unavailable/timeout/malformed result => warning or silence, exit 0
```

Text equivalent: the existing three hook triggers remain. The hook resolves the
managed repository and trusted executable, applies its configured timeout, and emits
compact advice. Missing BRAN, timeout, invalid telemetry, or validation findings do
not block the agent and never mutate repository source.

### Flow 4 - Authorized repair and recovery

```text
maintain propose (read only) -> immutable proposal + digest
owner-reviewed authority + exact digest -> maintain apply
  -> stale/path checks -> staged write -> native revalidation
  -> pass: success receipt
  -> fail: exact rollback -> restoration receipt + validation failure
```

Text equivalent: proposal captures target, replacement, and original bytes without a
write. Apply requires explicit authority and the exact proposal digest. It refuses
stale or unsafe targets, stages the write, revalidates, and reports success only after
validation. Failed validation restores the exact original state and retains an
attributable failure receipt.

### Flow 5 - Consumer retirement

```text
consumer inventory -> native policy + BRAN hook/skill migration
  -> validation parity + retrieval parity -> active-reference audit
  -> per-consumer complete -> all consumers complete -> owner removal approval
```

Text equivalent: each consumer moves independently and retains its evidence. A delayed
consumer stays on the adapter without invalidating completed consumers. Global removal
requires all six consumers, parity evidence, absence of active legacy references, and
owner approval.

## Test Strategy

### Pre-lens stance

Use deterministic fixtures and contract tests. Native BRAN behavior must be testable
without hooks or adapters; adapters must be testable against the same frozen corpus;
and repair behavior must retain byte-level evidence across all terminal states.

### Lens revisions

- CDD adds schema-version, command-envelope, typed-exit, and adapter contract tests.
- SecDD adds traversal, symlink, stale digest/source, authority, secret redaction, and
  public-boundary negative cases.
- RDD adds timeout, unavailable binary, malformed output, partial write, failed
  validation, rollback failure, and retry/idempotency cases.
- ODD adds deterministic receipt fields, parity deltas, missing-observability handling,
  and first-command diagnostic procedures.

### Per-slice approach

| Design area | Primary test layer | Cross-cutting proof |
| --- | --- | --- |
| Native policy and validation | core unit, schema, conformance | deterministic diagnostics and exits |
| CLI maintenance contracts | CLI contract and fault fixtures | authority, digest, rollback receipts |
| Legacy adapter and configuration | characterization and parity | no rewrite; same semantic outcome |
| Hooks and skill | shell fixtures and integration | trigger parity, bounded timeout, exit 0 |
| Consumer migration | repository procedure and audit | preserved per-consumer evidence |

### Cross-cutting checks

All cases run offline against disposable fixtures. Tests compare semantic outcomes,
not timestamps or path ordering accidents. Missing optional telemetry is retained as
`unavailable` and cannot change validation success. Public-boundary scans cover plans,
receipts, hook output, fixtures, and release artifacts.

## CDD

- **Surfaces touched.** `.bran/policy.yaml`; BRAN validation and maintenance CLI
  envelopes; `RepairProposal`, `RepairReceipt`, and `RepairTerminal`; legacy
  `tools/okf/okf` commands/config; `use-okf` skill and hook trigger/result behavior.
- **Contracts.** `DES-1`: the native policy begins with `schema_version`, rejects an
  unsupported version as a typed usage/configuration error, and models frontmatter,
  coverage classes, source links, tags/status, and public boundaries. `DES-2`: CLI
  commands return deterministic JSON envelopes and typed exits `0` success, `1`
  validation, `2` usage/configuration, and `3` operation/unavailable. `DES-3`:
  proposal is read-only; apply accepts the same target/replacement, exact digest, and
  non-blank authority; revalidation alone performs no source mutation. `DES-4`: the
  legacy adapter translates old inputs and outputs only and records parity differences.
- **Idempotency and ordering.** Validation and retrieval are read-only and repeatable.
  Derived-state rebuilds replace only BRAN-owned artifacts deterministically. Applying
  an already-consumed proposal encounters stale source rather than repeating a write.
  Diagnostics and parity rows sort by repository-relative path, code, then message.
- **Compatibility commitments.** Legacy calls remain accepted per consumer until AC-7
  evidence and owner approval. Unknown legacy fields are preserved or diagnosed, never
  silently discarded when they affect behavior. Native policy has no promise to serialize
  back to the legacy format.
- **Versioning strategy.** Native policy and structured receipts carry explicit schema
  versions. CLI command names and typed exits are stable within the initial native schema.
  Adapter compatibility is versioned by its mapping tests and release pin.
- **Interface option input.** Native policy location/serialization requires the global
  Interface Option Check. Maintenance authority and hook triggers retain approved shapes.
- **Validation and tests.** Parser/schema tests live in `bran-core`; CLI envelope and exit
  tests in `bran-cli`; adapter and hook characterization tests remain beside their
  compatibility surfaces until retirement.
- **Generated code provenance.** JSON schemas/reports are generated or checked from the
  BRAN-owned model by repository tooling. Generated artifacts never become policy input.
- **Notable omissions.** No network API, service protocol, or provider contract is added.

## SecDD

- **Threat model.** A repository author may craft paths, symlinks, metadata, or links to
  escape the root or cross public/private boundaries. A stale or malicious caller may
  replay a proposal or forge authority text. A compromised compatibility script may try
  to bypass native validation. Accidental output may expose private paths or corpus text.
- **Trust boundaries and validation.** `DES-5`: all policy paths become normalized
  repository-relative paths and are checked against the canonical root without following
  symlink escapes. Legacy configuration is untrusted adapter input and must pass the
  native policy validator. Only BRAN core decides validation and repair terminal state.
- **Authn / authz.** Offline read-only commands require no identity. Source apply requires
  an explicit invocation authority reason and exact digest; no hook or adapter infers it.
  Publication, release, and adapter removal remain owner-authorized operations.
- **Secrets.** No new secret is introduced. Hooks use a checksum-pinned local binary and a
  sanitized environment. Policy, receipts, logs, and fixtures must reject or redact auth
  state, credentials, host-private paths, and hidden evaluation material.
- **Sensitive data.** Private repository content stays local. Diagnostics contain bounded
  repository-relative locators and rule codes, not arbitrary source bodies.
- **Audit trail.** Repair receipts record schema version, digest, target, authority tag,
  and lifecycle. Parity records name consumer, BRAN pin, corpus/policy identity, outcome,
  and semantic differences. Receipts are evidence, not permission tokens.
- **Abuse cases.** Traversal, absolute paths, NULs, symlink ancestors, stale snapshots,
  digest mismatch, blank authority, config ambiguity, secret-like values, and public-link
  violations are refused before mutation.
- **Notable omissions.** No remote authentication or cryptographic signer is required for
  local migration. The digest is an identity/staleness check, not a security signature.

## RDD

- **Failure modes.** `DES-6`: missing/timed-out BRAN affects only the triggering hook and
  yields advisory unavailable; malformed policy stops explicit validation before scanning;
  stale source/digest stops apply before writing; I/O failure returns operation failure;
  failed validation rolls back; rollback failure reports partial-write uncertainty and
  preserves recovery artifacts. A consumer parity mismatch delays only that consumer.
- **Timeouts and retry budget.** Hook timeouts remain 12 seconds for SessionStart, 15 for
  prompt submit, and 20 for post-edit. Hooks do not retry. Explicit commands rely on their
  caller/CI budget. Apply is never blindly retried; callers must propose again after stale
  or uncertain outcomes.
- **Degradation behavior.** Hooks fail open and exit success. Explicit validation and CI
  fail on native validation errors. Missing metrics or receipts reduce observability but do
  not rewrite semantic success. Adapter parity mismatch is recorded and blocks only that
  consumer's retirement.
- **Recovery and repair.** Derived state may be rebuilt automatically. Source recovery uses
  exact backup bytes retained through revalidation. Partial-write uncertainty stops further
  mutation and directs the operator to inspect the receipt/backup before a fresh proposal.
- **Backup and restore.** No repository-wide backup is introduced. The repair coordinator's
  same-directory staged backup is the transactional recovery boundary and is deleted only
  after successful revalidation.
- **Notable omissions.** No queue, daemon, distributed retry, or background reconciliation.

## ODD

- **Logs.** Structured command envelopes and parity/repair receipts are the operational
  record. Fields are schema version, command, status, rule/error codes, bounded locators,
  pin/policy identity, and lifecycle. Source bodies, secrets, auth state, and host-private
  absolute paths are excluded.
- **Metrics.** `DES-7`: per consumer retain validation parity, retrieval parity, legacy
  active-reference count, hook unavailable/timeout count, and migration state. Metrics are
  bounded by consumer and rule code; repository paths remain evidence fields, not labels.
- **Traces.** No distributed tracing. A proposal digest correlates propose/apply/revalidate;
  a parity run ID correlates legacy and BRAN outcomes.
- **Health checks.** `bran --version` plus release-pin verification answers executable
  readiness. Native policy parse/validate answers repository readiness. Hook success alone
  never claims validation readiness.
- **Dashboards and alerts.** No service dashboard or paging. Migration evidence is a
  deterministic report; explicit CI failures surface normally. Hook failures are compact
  warnings and aggregated evidence, not pages.
- **Operator first-five-minutes runbook stub.** Verify pin, run native validation directly,
  inspect its typed envelope, then run adapter parity for only the affected consumer.
- **Questions answerable from telemetry alone.** Which BRAN build ran? Which policy and
  consumer were checked? Did native and legacy semantics differ? Was source mutated? Was a
  failed mutation restored? Which consumer still references legacy behavior? Which fields
  are unavailable?
- **Notable omissions.** No uptime SLO, telemetry backend, or remote collector.

## Interface Option Check

Three repository-policy interfaces were considered:

| Option | Shape | Compatibility | Main tradeoff |
| --- | --- | --- | --- |
| A | `.bran/policy.yaml`, versioned BRAN schema | legacy adapter maps old YAML read-only | explicit ownership and human review; one migration step |
| B | keep `tools/okf/config.yaml` as native | zero initial path migration | makes legacy names and schema permanent BRAN API |
| C | infer policy from repository contents | fewer files | ambiguous authority, weak reproducibility, unsafe defaults |

`interface_options: selected - Option A (.bran/policy.yaml)`

`DES-8`: Option A is selected. `bran-core` owns parsing and normalized policy
semantics; `bran-cli` discovers the file only from an explicit repository root; the
legacy adapter maps `tools/okf/config.yaml` into the same in-memory policy without
writing `.bran/policy.yaml`. Schema version, unknown-field diagnostics, deterministic
serialization fixtures, and migration parity are mandatory. A future format change
requires a new schema version rather than heuristic parsing.

## OOPDSA Implementation Design

### Requirements trace

| Requirement ID | OOPDSA owner | Proof obligation |
| --- | --- | --- |
| AC-1, AC-7 | `LegacyOkfAdapter` and migration inventory | native ownership plus evidence-based retirement |
| AC-2, AC-5 | `RepositoryPolicyLoader`, `ProfileValidator`, query engine | strict policy and deterministic precedence |
| AC-3, AC-4, AC-6 | `RepairCoordinator` | explicit authority, exact digest, revalidate, rollback |
| RISK-1, RISK-5 | `ParityRecorder` | independent consumer state and semantic deltas |
| RISK-2 | hook adapter | trigger parity and fail-open behavior |
| RISK-3 | release/public-boundary procedure | no implicit publication |
| RISK-4 | `RepositoryPolicyLoader` | selected versioned native policy contract |

### Ownership contract

| Object / Service | Responsibility | Owns Data? | Key Methods / Entry Points | Collaborators | Boundary / Interface | Test Focus | Must Not Own |
| --- | --- | ---: | --- | --- | --- | --- | --- |
| `RepositoryPolicyLoader` | parse/version/normalize `.bran/policy.yaml` | yes, immutable policy value | `load(root)`, `normalize()` | scanner, validator | filesystem to policy | versions, paths, unknown fields | repository mutation |
| `ProfileValidator` | evaluate compatibility and strict rules | no | `validate(bundle, profile)` | policy, bundle | normalized evidence to diagnostics | deterministic rules/exits | adapter shapes |
| query engine | canonical retrieval and precedence | yes, derived index | existing query/packet entrypoints | scanner, graph | query to ranked evidence | rank stability, precedence | policy migration |
| `RepairCoordinator` | proposal/apply/revalidate/rollback | yes, proposal snapshot and staged backup | `propose`, `apply` | validator | explicit mutation boundary | all terminal states, exact restore | inferred authority |
| `LegacyOkfAdapter` | translate legacy commands/config/results | no | `check`, `doctor`, shadow/parity | BRAN CLI/core | legacy to native | characterization and parity | validation/ranking rules |
| hook adapter | preserve triggers and bounded advisory calls | no | SessionStart, UserPromptSubmit, PostToolUse | pinned CLI | agent hook boundary | timeout, unavailable, exit 0 | source mutation or strict gate |
| `ParityRecorder` | retain per-consumer semantic comparison | yes, derived receipts | `record`, `summarize` | adapter, inventory | evidence ledger | missing fields, deterministic deltas | task success or authority |

### Pattern decisions

| Decision ID | Pressure | Candidate Pattern | Chosen Pattern / Plain Code | Why | Simpler Alternative | Tradeoffs | Review Trigger |
| --- | --- | --- | --- | --- | --- | --- | --- |
| CONTRACT-1 | legacy/native shapes differ | Adapter | one thin `LegacyOkfAdapter` | isolates retirement and prevents rule duplication | conditionals in CLI | temporary extra surface | adapter implements semantics |
| CONTRACT-2 | repair has explicit lifecycle | State machine | retain typed `RepairTerminal` transitions | impossible to claim success before revalidation | booleans/errors | more variants | terminal state loses evidence |
| CONTRACT-3 | validators need a policy value | immutable value object | `RepositoryPolicy` parsed once | single owner and deterministic consumers | raw YAML maps | schema migration code | consumers read raw fields |
| CONTRACT-4 | hooks need graceful calls | plain shell boundary | preserve small fail-open script | no daemon/framework needed | shared runtime service | repeated process startup | hook gains mutation/retry |
| CONTRACT-5 | migration evidence is partial | append-only derived receipts | records available fields plus `unavailable` | observability cannot invalidate work | strict ledger schema | consumers handle missing values | receipt becomes eligibility gate |

### DSA decisions

| Decision ID | Owner | Operation | Expected Scale | Chosen Structure | Chosen Algorithm | Time | Space | Reason | Simpler Alternative | Edge Cases |
| --- | --- | --- | ---: | --- | --- | --- | --- | --- | --- | --- |
| CONTRACT-6 | `RepositoryPolicy` | membership/classification | repository paths | ordered maps/sets | normalize then lexical traversal | O(n log n) | O(n) | deterministic diagnostics and overlap detection | vectors | duplicate/overlap/excluded roots |
| CONTRACT-7 | query engine | precedence/ranking | repository evidence nodes | existing graph indexes + stable tuples | canonical/status/freshness/authority ordering | existing bounded query contract | existing index budget | preserve proven retrieval behavior | full scan/sort | ties, stale and legacy sources |
| CONTRACT-8 | `ParityRecorder` | semantic diff | six consumers, bounded results | ordered map keyed by semantic identity | normalize, join, compare, lexical emit | O(n log n) | O(n) | ignores formatting/order noise | raw text diff | missing telemetry, duplicate codes |
| CONTRACT-9 | `RepairCoordinator` | stale detection and rollback | one bounded target | byte snapshot plus staged backup | exact compare, staged replace, validate, restore | O(b) | O(b) | exact recovery is more important than cleverness | timestamps | absent file, same-length edits, rollback I/O failure |

### Invariants and edge cases

- Native and adapter requests with equivalent policy have the same semantic validation
  and retrieval outcomes.
- Only BRAN-owned derived paths may be automatically replaced.
- No apply reaches a write with blank authority, mismatched digest, stale bytes, unsafe
  path, or symlink escape.
- Validation failure cannot produce success; successful rollback restores byte identity.
- Hook timeout, missing executable, malformed output, or metric absence always exits zero.
- One consumer mismatch cannot erase another consumer's completed evidence.
- Active legacy references outside historical documents prevent only retirement.

The implementation must satisfy `CONTRACT-1` through `CONTRACT-9`; the prospective
proof map is owned by `seit.md`.

## Owner-approved product parity amendment — 2026-07-22

`DES-9`: Repository policy is an immutable validated value with two explicit input
sources. The default native command loads `.bran/policy.yaml` from the explicit
repository root. `--policy-stdin` reads the same schema without persistence. The
sources are mutually exclusive, policy validation precedes scanning, stdin is
bounded, and policy content is not echoed into diagnostics or logs.

`DES-10`: BRAN owns the remaining semantic OKF customer contracts: document coverage
classification and migration-state rules, preserved frontmatter keys, source/link/
citation integrity, public-boundary allowlists, packet supersession validation, and
body-preservation validation. Existing BRAN graph, metadata, packet, and policy types
are extended rather than duplicating these rules in Python or shell adapters.

`CONTRACT-10`: `LegacyOkfAdapter` converts legacy YAML into the versioned native
policy schema, sends it through `--policy-stdin`, translates the native envelope back
to the legacy command result, and records parity. Unknown behavior-affecting fields
produce a typed configuration failure. The adapter never writes `.bran/policy.yaml`
and never falls back to a second semantic validator.

Customer-facing consequences are additive: repository policy remains the recommended
interface, while read-only checkouts, centrally managed CI policy, editors, and
migration tools can supply an equivalent transient policy. Legacy validator build and
self-heal machinery, benchmarking, SQZ evaluation, and issue automation remain outside
the BRAN semantic product boundary.

This owner-approved amendment refines AC-1, AC-2, AC-5, AC-7, RISK-3, and RISK-4.
The amended implementation must satisfy `CONTRACT-1` through `CONTRACT-10`.

## Design Amendment — 2026-07-22 — Native quoted-scalar interoperability

Implementation exposed an interoperability gap at the native-policy boundary: the
legacy adapter can serialize quoted YAML, but BRAN's bounded parser currently strips
delimiters without decoding the quote forms needed to preserve exact values.

`DES-11`: BRAN's single native policy parser owns a bounded, explicitly documented
quoted-scalar subset. Single-quoted scalars decode doubled apostrophes. Double-quoted
scalars decode only the minimal escapes required for exact adapter transport (`\"` and
`\\`); unsupported escapes, literal CR/LF, control characters, and unmatched delimiters
fail with the existing non-echoing malformed-policy error. Unquoted behavior and policy
semantics remain unchanged.

`CONTRACT-11`: `LegacyOkfAdapter` selects a lossless representation from that native
subset, never silently coerces a behavior-affecting value, and returns static field/index
diagnostics that cannot expose raw policy values, repository paths, operating-system
errors, or arbitrary child-process output. Equivalent file and stdin policies parse to
the same immutable `RepositoryPolicy` value.

This amendment changes serialization interoperability and error sanitization only. It
does not change validation rules, authority, mutation scope, the default
`.bran/policy.yaml` interface, compatibility-retirement criteria, or publication scope.
The Interface Option Check remains unchanged because both policy sources still converge
on the same parser. OOPDSA ownership remains `RepositoryPolicy` plus the thin
`LegacyOkfAdapter`; the parser uses a bounded linear state machine with O(n) time and
O(n) output for each quoted scalar.

## Design Amendment — 2026-07-22 — Oversized binary scan isolation

Full-config adapter proof exposed a pre-profile failure on an unrelated 1.9 MiB image.
Legacy OKF treats non-text assets as outside repository-knowledge validation, while BRAN
currently applies its text-source byte limit before it can classify an oversized binary
as unsupported input.

`DES-12`: When a regular file advertises a size above the configured per-file text limit,
the scanner reads only a fixed bounded prefix before deciding. A prefix containing NUL or
invalid UTF-8 is recorded as `UnsupportedInput` and the file is not buffered, parsed, or
charged to accepted-source byte totals. A valid UTF-8 prefix remains subject to the
existing hard size failure. The same rule applies to full and incremental scans, retains
symlink/root-escape checks, and never guesses that oversized text is safe.

`CONTRACT-12`: Oversized binary isolation is a scanner input-classification rule, not a
policy exclusion or adapter exception. The probe is constant-space and bounded O(1) by a
fixed byte ceiling; accepted text retains existing limits and identity semantics. This
restores profile evaluation for repositories containing unrelated binary assets without
weakening protection against oversized textual knowledge inputs.

## Design Amendment — 2026-07-22 — Check-time knowledge-candidate alignment

Full-config adapter proof next exposed an oversized generated `review.html`. BRAN's
general-purpose repository scanner correctly supports source and other text inputs, but
the `check` pipeline later discards every entry except the native Markdown knowledge
formats. Applying accepted-source limits before that existing admissibility decision lets
non-knowledge generated text block profile evaluation.

`DES-13`: The native `check` pipeline supplies the scanner a deterministic knowledge-
candidate predicate equal to the predicate used to derive its validation bundle. Paths
that cannot enter that bundle are not opened, parsed, or charged to check-time accepted-
source totals. The general-purpose scanner remains unchanged by default so other BRAN
features can scan source and text files. Full and incremental filtered scans use the same
predicate, and containment, symlink, file-count, and byte-limit checks remain mandatory
for every admitted candidate.

`CONTRACT-13`: Knowledge-candidate selection is a BRAN-native check contract, not an
adapter rule or configurable extension allow/deny facility. One shared predicate owns the
currently supported Markdown path forms and is reused by scanning and bundle derivation.
An oversized admitted Markdown document still fails closed; a generated non-candidate
file cannot prevent profile selection. This removes duplicate downstream filtering and
generalizes to every customer without changing policy, repository content, or limits.

## Authorization-gated upstream OKF contribution candidates

Implementation produced two reusable format-level lessons that are suitable for a
separately authorized issue or pull request against the upstream Open Knowledge Format
specification. They remain proposals, not migration deliverables:

- `UPSTREAM-1 — bundle scan scope`: clarify that non-OKF files may coexist beside a
  bundle, do not participate in OKF conformance, and should not be opened merely to
  validate the bundle unless explicitly referenced. This prevents unrelated binary or
  generated text from blocking conformance while leaving implementation resource limits
  outside the format contract.
- `UPSTREAM-2 — layered profile separation`: permit implementations to expose stricter
  organizational readiness profiles only when their results are reported separately from
  OKF conformance. An implementation-specific failure must not be characterized as OKF
  nonconformance when the portable v0.1 floor passes.

BRAN keeps `okf-v0.1` as the portable interoperability outcome and `bran-strict` as the
additive repository-policy outcome. Both are computed independently and only the selected
profile controls the command exit. Before BRAN claims complete OKF v0.1 certification, a
separate bounded follow-up must close the known reserved-file coverage gap: the current
compatibility profile validates concept frontmatter and `type` but does not yet validate
the upstream `index.md` and `log.md` structural requirements.

No upstream issue, pull request, comment, push, or publication is authorized by this
plan. The Conductor must first present the exact proposed upstream text, target, and
scope, then obtain explicit owner authorization for that external write.

### Closeout update — 2026-07-25

The paragraphs above preserve the state and authorization boundary at plan approval.
Subsequent owner-authorized work changed that state:

- The reserved-file coverage gap is closed. `ProfileValidator` now validates the
  structural rules for `index.md` and `log.md`, with positive and negative fixtures in
  `fixtures/conformance/okf-v0.1-index-*.fixture` and
  `fixtures/conformance/okf-v0.1-log-*.fixture` exercised by
  `profile::tests::p1_conformance`.
- Upstream OKF v0.2 supersedes v0.1. BRAN's existing `okf-v0.1` profile identifier is
  still a product compatibility label; renaming it or claiming full v0.2 coverage
  requires a separate versioned migration.
- The bundle-boundary clarification is open as
  [GoogleCloudPlatform/knowledge-catalog#232](https://github.com/GoogleCloudPlatform/knowledge-catalog/pull/232).
- The profile-separation suggestion was added to the existing upstream discussion in
  [issue #212](https://github.com/GoogleCloudPlatform/knowledge-catalog/issues/212#issuecomment-5081662199).

Neither upstream contribution is recorded as accepted or merged. The live pull request
and issue discussion are authoritative for their current wording and disposition; the
original contribution briefs below remain historical design context.

## Design Amendment — 2026-07-23 — Upstream OKF rationale and sequencing

This amendment expands `UPSTREAM-1` and `UPSTREAM-2` into reviewable contribution
briefs. It does not authorize an external write, add BRAN-specific behavior to OKF, or
change the completed migration acceptance boundary.

### UPSTREAM-1 — Bundle scan scope

**Problem.** OKF v0.1 defines a knowledge bundle as a directory tree of Markdown files
and permits the bundle to be a subdirectory of a larger repository. Its conformance
rules apply to non-reserved Markdown files "in the tree," but the specification does not
explicitly say that validators can restrict conformance I/O to the designated bundle
root. Implementations can therefore disagree: one validates only the bundle while
another recursively opens unrelated repository assets, generated reports, databases, or
binaries.

**Representative layout.** In the following repository, only `docs/okf/` is the
designated bundle:

```text
customer-repository/
├── docs/okf/
│   ├── index.md
│   └── concepts/orders.md
├── application/
├── screenshots/
├── build/
├── database.sqlite
└── generated-report.html
```

The application, screenshots, build outputs, database, and generated report do not
participate in OKF conformance and need not be opened merely to validate `docs/okf/`.
Within the designated bundle root, every non-reserved `.md` file still participates
under the existing specification, reserved files retain their structural obligations,
and referenced content remains subject to the relevant link or citation behavior.

**Recommended normative direction.** Add a narrow clarification to the bundle-structure
or conformance section:

> Conformance is evaluated within a designated bundle root. Files outside that root do
> not participate. Within the bundle root, reserved and non-reserved Markdown files
> participate as specified; other files may coexist and need not be opened for
> conformance unless explicitly referenced by a participating document.

**Benefits.** The clarification makes OKF practical in monorepos, prevents unrelated
binary or generated files from changing format outcomes, reduces unnecessary I/O, and
gives independent validators the same corpus boundary.

**Non-goals and safety boundary.** This proposal does not standardize BRAN's scanner,
knowledge-candidate predicate, byte limits, file-count limits, or security policy. It
does not declare neighboring files safe or exempt them from repository security scans.
OKF conformance validation and whole-repository security analysis remain separate
operations.

**Recommended contribution vehicle.** Submit a small specification pull request because
this clarifies the existing rule that a bundle may be a subdirectory rather than adding
a new document shape.

### UPSTREAM-2 — Layered profile separation

**Problem.** OKF intentionally defines a permissive interoperability floor. Organizations
still need stronger readiness, governance, freshness, source-integrity, and
public-boundary policies. Without profile-reporting guidance, an implementation can
collapse a stricter organizational failure into a generic failure and incorrectly imply
that a portable OKF bundle is nonconformant.

**Required outcome separation.** A bundle can legitimately produce two independent
results:

```text
OKF v0.1:    PASS
BRAN Strict: FAIL — missing public_boundary
```

This means the bundle satisfies the portable format floor but is not ready under one
implementation's organizational policy. `bran-strict` remains a BRAN profile; it is not
proposed as an upstream OKF profile.

**Recommended normative direction.** After maintainer discussion, add a short rule to
the conformance section:

> Implementations MAY provide additional validation profiles beyond OKF conformance.
> Such profiles MUST report their outcomes separately. Failure of an
> implementation-specific profile MUST NOT be described as OKF nonconformance when the
> bundle satisfies the selected OKF version.

**Benefits.** Implementations can add security or operational readiness checks without
fragmenting the portable format. Producers retain exchange compatibility, consumers can
distinguish interoperability from organizational readiness, and vendor-specific policy
cannot silently redefine OKF conformance.

**Non-goals.** This proposal does not standardize `bran-strict`, require organizations to
offer a strict profile, create a central profile registry, or add BRAN policy fields to
OKF. Profile identifiers, policy contents, and enforcement mechanisms remain
implementation-owned.

**Recommended contribution vehicle.** Open a design issue first because profile
separation adds normative reporting guidance. Draft a small conformance-section pull
request only after upstream maintainers agree with the distinction.

### Recommended timing

Draft both contributions now, but approach upstream after BRAN is publicly inspectable.
Before publication, close BRAN's reserved `index.md` and `log.md` validation gap and
describe the current `okf-v0.1` result only as the OKF v0.1 concept-document
interoperability floor. Then:

1. publish a stable BRAN release with accurate conformance claims and reproducible tests;
2. submit `UPSTREAM-1` as a narrow specification clarification;
3. open `UPSTREAM-2` as a design issue;
4. submit profile-separation wording only after maintainer agreement.

A public implementation gives maintainers inspectable evidence, while separating the
two contributions keeps discussion, review, and disposition independent. Every external
issue, pull request, comment, branch push, or publication still requires the owner's
explicit approval of the exact text, repository, and destination.
