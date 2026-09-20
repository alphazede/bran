---
type: submission-package
title: BRAN Build Week Private Submission Package
okf_status: draft
status: draft
tags:
  - internal
  - bran
freshness: "2026-07-24"
resource: https://github.com/alphazede/bran-dev
public_boundary: private
---

# BRAN Build Week Private Submission Package

This private package contains submission copy and measured local evidence. It
is not publication, release, benchmark-upload, or submission authorization.
Nothing here may be copied into BRAN's public source tree without an explicit
public-boundary scrub, claim review, and owner approval.

Canonical artifacts:

- [demo-video-outline.md](./demo-video-outline.md)
- [demo-recording-runbook.md](./demo-recording-runbook.md)
- [research-paper.md](./research-paper.md)
- [submission-checklist.md](./submission-checklist.md)
- [customer-setup/README.md](./customer-setup/README.md)
- [controlled Arena evidence](./evidence/arena-matrix-20260720.json)
- [post-review connected smoke evidence](./evidence/connected-smoke-20260720.json)
- [provider-free Obsidian evidence](./evidence/obsidian-core-20260720.json)
- [Understand Anything supply-chain evidence](./evidence/ua-supply-chain-20260720.json)

## Submission position

- **Entered category:** Developer Tools.
- **Supporting narrative:** Work & Productivity examples can demonstrate the
  same evidence-routing engine without changing the entered category.
- **Core promise:** BRAN gives an agent or developer bounded,
  provenance-bearing repository evidence before action.
- **Attribution:** The agent using BRAN performs the review, diagnosis, plan,
  explanation, or owner-authorized repair. BRAN supplies evidence and receipts.

The category references below come from the approved plan's pin of the
[Build Week overview](https://openai.devpost.com/) and
[official rules](https://openai.devpost.com/rules), rechecked on 2026-07-18.
The owner must read back the live rules immediately before submission.

## Final Devpost copy

**Title:** BRAN

**Tagline:** Bounded repository evidence for agents and developers.

**Public repository:**
[alphazede/developers/bran](https://github.com/alphazede/developers/tree/main/bran)

**Description:**

BRAN is a local repository-intelligence engine that helps an agent or developer
find bounded, provenance-rich evidence before acting. Its deterministic Rust
core can scan, query, validate, and build focused context packets without an
LLM or provider account. A connected inner agent can synthesize a grounded
answer when explicitly configured. SQZ response processing, voice, and saved
history remain explicit capabilities; unavailable behavior is never simulated.

The product surface includes a headless CLI, terminal onboarding, named agent
profiles, requested-versus-effective receipts, release-readiness checks, a
public `use-bran` skill, deterministic Obsidian export, and an isolated Arena
protocol. A connected-task total-token ceiling is unset unless the user
configures `tokens=N`. The independent 65,536-byte connected-answer limit is a
byte-safety bound, not a token ceiling.

On our controlled 512-file benchmark, all 20 eligible agents found the intended
retry-queue bug and correct account-scoped fix. Each heterogeneous condition
has `n=1`. Plain was descriptively fastest and lowest-token on this easy task;
BRAN Core selected broader evidence, while connected use added provider work
and exposed receipt and grounding failures. These results are a transparent
baseline, not a universal claim that BRAN improves speed, cost, or correctness.

## Claim policy

Use this form for any measured statement:

> On our controlled benchmark (`n=<sample size>`), the agent using BRAN
> `<measured outcome>` while `<correctness or grounding condition>`.
> `<Token field>` was `<actual, estimated, or unavailable>`.

Required qualifications:

- Say “on our controlled benchmark,” never “for every repository.”
- State `n=1` for each heterogeneous matrix cell.
- Use provider input plus output for actual tokens. Cached input and reasoning
  output are subsets and must not be added again.
- Label Core packet bytes divided by four as an estimate, never provider use.
- Report initialization/indexing separately when measured; otherwise report it
  as `unavailable`.
- Retain failed and invalidated runs and explain exclusions.
- Report dollar spend as `unavailable` until provider cost evidence exists.
- Attribute completed work to the agent using BRAN.
- Keep Understand Anything as an unexecuted category reference.

Prohibited claims include “eliminates hallucinations,” “always faster,” “uses
fewer tokens than every competitor,” “beats Understand Anything,” universal
cost savings, hidden failed runs, or automatic document repair when an agent
performed the repair.

## Controlled Arena comparison

Exact evidence scope:

- BRAN: `33e86f9ef36bf71130013bcbdcb4e3ad37d150d7`
- Arena: `ecc39c83275c0d5930a60a3841c9b9514379c415`
- Corpus:
  `cb4fab25ecc7dfa132b65608608afb6fa2245e8f3248a1a7f2a1f3752aa7d6d5`
- Private aggregate SHA-256:
  `38317b314c0eb31532590c882571c2c463b35873bc6f8244ad74e4ceefd49a1c`
- Population: 20/20 eligible corrected trials, `n=1` per heterogeneous cell
- Matrix: four plain, four Core, twelve connected
- Outer conditions: Sol Medium/XHigh and Terra Medium/XHigh
- Connected inner conditions: Luna Low/Medium and Spark Medium
- SQZ: off in this comparison

The full corrected corpus passed `okf-v0.1` before dispatch. Plain saw no
skills; BRAN arms saw only `use-bran`. Every arm used a fresh isolated corpus
copy with neutral `AGENTS.md` and `CLAUDE.md` instructions.

| Arm | Cells | Median wall time | Median actual tokens | Median tools | Median failed tools | Correct | Material errors | Failed conditions |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Plain | 4 | 47.030 s | 72,589 | 5.5 | 2 | 4/4 | 0 | 0 |
| BRAN Core | 4 | 118.060 s | 195,941 | 12 | 4 | 4/4 | 0 | 0 |
| BRAN connected | 12 | 108.975 s | 208,050 | 15.5 | 4 | 12/12 | 4 | 5 |

The medians summarize heterogeneous one-trial cells and are descriptive only.
Wall duration is the recorded end-to-end outer candidate duration;
initialization and indexing were not separately itemized. Dollar spend is
`unavailable`.

Plain and connected median direct-path precision and recall were 1.0. Core
packet-locator recall was 1.0 and precision was 0.065217, showing that it found
all required evidence but admitted a broader packet. Connected execution had
nine complete receipts, seven clean results, three grounding failures, four
material reporting/protocol errors, and five failed-condition cells. All final
defect answers still passed the correctness gate.

The earlier population with corpus digest `f11a1ff6...` failed the repository
OKF precondition because the simulated instruction files lacked required
metadata. Its 21 runs remain preserved but are classified
`invalid_precondition_okf_repository_fail` and contribute no metrics.

See the [research paper](./research-paper.md) and
[sealed private aggregate](./evidence/arena-matrix-20260720.json) for definitions
and artifact hashes.

## Post-review connected compatibility smoke

A separate real CLI smoke exercised BRAN
`36574d01c7c6acad9fb97e09d3aad2c7cf683122` with Arena
`83253293a08380aeddb16874616e90cd21d2a081`. Spark Medium, running inside BRAN
with read/search only, produced the correct bug, fix, and test answer in 15.41
seconds. BRAN accepted all eight citations as exact packet locators. Provider
telemetry recorded 68,555 input and 5,652 output tokens, or 74,207 total; cached
input and reasoning output are included subsets, not additions. This is one
post-review compatibility smoke (`n=1`), not a matrix row or benchmark win.

The evidence also preserves a 0.06-second permissions preflight failure before
provider invocation and an earlier provider attempt that failed exact citation
validation before telemetry publication. See the
[scrubbed smoke record](./evidence/connected-smoke-20260720.json).

## Fixture and provider boundaries

| Evidence | Classification | Allowed statement |
|---|---|---|
| Corrected provider matrix | `measured_limited` | On our controlled benchmark, every agent found the bug; report all descriptive costs and failures with `n=1`. |
| Post-review connected smoke | `measured_compatibility_n1` | Spark using BRAN produced a grounded answer at the reviewed revisions; do not add it to the frozen matrix. |
| Core packet bytes divided by four | `estimated_fixture_only` | A labeled packet-size estimate; never actual provider tokens. |
| Obsidian core export | `measured_provider_free` | Deterministic export tests passed without an agent or LLM. No GUI/native-plugin claim. |
| Understand Anything | `blocked_category_reference` | Download and supply-chain scan only; no installation, execution, or performance comparison. |
| Dollar spend | `unavailable` | No currency or cost-savings claim. |
| Hosted CI | `unavailable_not_run` | Exact local commits were not pushed, and Arena has no remote; do not infer pass status from local gates. |
| Signed multi-platform release | `unavailable` | Local revisions are not a public release. |

The Obsidian evidence verifies export behavior, not a competing retrieval
system. The Understand Anything pin remains blocked because its supply-chain
scan found unresolved Critical/High issues; the security policy must not be
bypassed.

## Customer and judge setup

Use [customer-setup/README.md](./customer-setup/README.md) for the local CLI/TUI
walkthrough and the public repository's
[`bran/docs/integrations/agent-setup.md`](https://github.com/alphazede/developers/blob/main/bran/docs/integrations/agent-setup.md)
for canonical recipes. Use only an exact owner-authorized release for the final
judge rehearsal.

## Security and public-boundary notes

- BRAN Core works without provider authentication or network initialization.
- Credentials remain in the reviewed agent host; BRAN has no credential CLI
  flag.
- Receipts distinguish requested from effective model, reasoning, tools, SQZ,
  token policy, and retention settings.
- Public source must exclude private corpora, owner paths, auth/state, raw
  provider traces, run identifiers, and private submission copy.
- No private Devpost or model-specific copy belongs in the public BRAN tree.
- No sealed multi-platform release, trusted signature, stable internal
  promotion, or hosted-CI result is claimed here.

## Screenshot and asset shot list

| Shot | Source | State |
|---|---|---|
| Repository hero | [`bran/assets/brand/bran-repository-raven.png`](https://github.com/alphazede/developers/blob/main/bran/assets/brand/bran-repository-raven.png) | Present; avatar candidate only |
| TUI hero | Public `bran/assets/tui/` raven assets | Present; capture exact release |
| Onboarding | Advanced readiness review | Capture after final exact-release QA |
| Headless receipt | Query/packet provenance and failures | Capture exact release; scrub local paths |
| Benchmark card | Controlled table above | Private copy ready; public scrub required |
| Release proof | Checksums, signatures, supported assets | `unavailable` until owner-authorized release |

Do not create staged or synthetic screenshots. Do not mutate the GitHub avatar
without separate owner authorization.

## External owner actions

1. Supply macOS x86_64/arm64 and Windows MSVC build environments, trusted
   signing authority, and exact tag/release authorization.
2. Approve a clean no-build judge rehearsal, final screenshots, and timed video
   after the exact release exists.
3. Verify the eligible `/feedback` identifier and read the live rules/deadline.
4. Separately authorize repository/video publication, Devpost submission,
   stable internal installation or promotion, and any GitHub avatar mutation.
5. Clear a future Understand Anything pin through supply-chain policy before
   any category-reference installation or execution.

See [submission-checklist.md](./submission-checklist.md) for the final launch
sequence. This package generation performed no external submission,
publication, upload, release, deployment, purchase, or avatar mutation.
