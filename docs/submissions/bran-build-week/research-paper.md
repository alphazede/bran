---
type: research-paper
title: "BRAN: Deterministic Evidence Routing for Repository-Scale Coding Agents"
okf_status: draft
status: draft
tags:
  - internal
  - bran
freshness: "2026-07-24"
resource: https://github.com/alphazede/bran-dev
public_boundary: private
---

# BRAN: Deterministic Evidence Routing for Repository-Scale Coding Agents

## Abstract

Coding agents must find authoritative sources before they can reason about a
repository task. BRAN is a model-agnostic Rust evidence router that indexes OKF
metadata, selects bounded source packets, and records provenance without
requiring a language model. Our latest controlled enterprise screen ran Plain,
OKF, BRAN Core, BRAN Connected, and BRAN Connected+SQZ through all seven DMA-417
stages at BRAN `917b6dc` and Arena `3b7d847`, producing 35 sealed receipts. No
arm passed final hidden acceptance. All five encountered the same stage-5 and
stage-6 mutation failures because public prompts requested generated,
validation, compatibility, and operations updates that the hidden mutation
allowlists rejected. Final acceptance also required undisclosed exact output
paths. The run therefore establishes harness completion and comparative cost
observations, but it cannot select a model-quality winner. Connected had the
lowest outer input usage and shortest wall-clock envelope; Connected+SQZ had
the longest envelope, with every SQZ receipt unavailable. Earlier failed and
successful pilots remain preserved as historical evidence.

## 1. Introduction

Broad repository search can work, but it provides weak source-precedence
guarantees and may load stale or unrelated material. BRAN separates evidence
routing from model synthesis. Its deterministic Core can run offline; a working
agent can use a Core packet directly, or BRAN can ask a configured inner agent
to synthesize a grounded answer. The working agent remains responsible for the
diagnosis, plan, explanation, or owner-authorized repair.

This study asks:

1. Can each arm find the correct defect, fix, and focused validation evidence?
2. What wall time, actual provider-token usage, tool activity, source selection,
   and failure behavior appears in the controlled matrix?
3. Which limitations must qualify any claim about an agent using BRAN?

## 2. System

BRAN Core, its CLI, and its TUI are implemented in Rust; the isolated Arena
harness and scorer are Python. The `use-bran` skill tells a candidate agent how
to request a packet while preserving provenance, warnings, failures, and
unavailable fields.

BRAN Core scans workspace sources, parses OKF metadata, builds deterministic
indexes, follows declared task/implementation/validation relationships, and
returns bounded locators plus byte accounting. It works without an LLM.
Connected-agent synthesis, SQZ response processing, voice, and retained history
are separately configurable.

There is no default connected-task total-token ceiling. A user may explicitly
configure one with `tokens=N`. The separate 65,536-byte connected-answer bound
is a byte-safety limit, not a token budget or token estimate.

## 3. Evaluation method

### 3.1 Corpus, task, and oracle

The corrected controlled corpus contains exactly 512 candidate-visible files
and has logical digest
`cb4fab25ecc7dfa132b65608608afb6fa2245e8f3248a1a7f2a1f3752aa7d6d5`.
Its full `okf-v0.1` repository check passed before dispatch. Neutral
`AGENTS.md` and `CLAUDE.md` files provided the same simulated-repository
instructions to every arm.

The human-simple request was:

> Investigate why events from different accounts sometimes collapse in the retry queue. Explain the bug, the fix, and how to test it.

The oracle evidence is:

- `capsules/misleading_symptom_root_cause/TASK.md`
- `capsules/misleading_symptom_root_cause/src/retry_queue.py`
- `capsules/misleading_symptom_root_cause/tests/visible_tests.py`

The defect omits `account_id` from a retry de-duplication key. Events from
different accounts therefore collide when their normalized recipient and
minute bucket match. The intended fix adds the account boundary while
preserving recipient normalization and minute bucketing.

### 3.2 Arms and model conditions

Every condition used a fresh isolated corpus copy and an unset token ceiling.
The four outer-agent conditions were Sol Medium, Sol XHigh, Terra Medium, and
Terra XHigh.

- **Plain (4 cells):** the outer agent saw no BRAN skill.
- **Core (4 cells):** the outer agent used deterministic BRAN evidence without
  an inner agent.
- **Connected (12 cells):** each outer condition used Luna Low, Luna Medium, or
  Spark Medium inside BRAN.

Plain exposed no skills. BRAN conditions exposed only `use-bran`; candidate
workspaces did not expose `use-okf`, Ponytail, owner memory, authentication
state, or sibling repositories. No source mutation was permitted.
SQZ was off in every matrix cell, so this study makes no SQZ comparison.

### 3.3 Outcomes and telemetry

The correctness gate required the right capsule, implementation and validation
paths, missing-`account_id` cause, account-scoped fix, preserved normalization
and minute bucket, and focused validation. The scorer also retained wall time,
tool and failed-tool counts, source precision/recall, grounding, material
errors, specification drift, planning quality, architecture quality, and failed
conditions. Code-review quality was not applicable because this was a
read-only diagnosis task.

Actual token totals use provider telemetry. For one provider run, total tokens
equal input plus output. Cached input is a subset of input, and reasoning output
is a subset of output, so neither subset is added again. Connected totals add
the outer and inner provider totals once. BRAN Core packet token figures use
bytes divided by four and remain labeled estimates; they are excluded from
actual model-token totals. Dollar spend was unavailable.

Reported wall duration is the recorded end-to-end outer candidate duration.
The aggregate does not separately itemize initialization or indexing, so those
component times are unavailable.

### 3.4 Enterprise seven-stage screen

The authoritative enterprise campaign used one live replication for each of
five arms: Plain, OKF, BRAN Core, BRAN Connected, and BRAN Connected+SQZ. Each
arm received the same seven ordered DMA-417 prompts and a fresh isolated
workspace. Raw provider events, stage receipts, hash-chained evidence ledgers,
and final eligibility records were retained.

Task success used only final hidden acceptance, terminal completion, authorized
mutations, boundary safety, unsupported citations, and semantic material
errors. Retrieval rank and recall, searches, files, tokens, timing, inner-agent
usage, and SQZ receipts remained descriptive metrics. Missing telemetry did not
invalidate an otherwise successful task.

## 4. Results

### 4.1 Enterprise seven-stage screen

All five arms completed 7/7 stage invocations. Across 35 receipts, 25 stages
were terminal-successful and ten were marked failed: stage 5 and stage 6 for
every arm. All 35 receipts were boundary-safe and recorded zero unsupported
citations. No arm passed any of the five final hidden acceptance cases.

| Arm | Wall-clock envelope | Outer input | Outer output | Terminal success | Retrieval diagnostic passes | Hidden cases | Task success |
|---|---:|---:|---:|---:|---:|---:|---|
| BRAN Connected | 49m 29.911s | 9,181,381 | 130,858 | 5/7 | 0/7 | 0/5 | false |
| OKF | 57m 04.303s | 11,053,148 | 154,197 | 5/7 | 1/7 | 0/5 | false |
| BRAN Core | 58m 37.800s | 11,349,682 | 162,465 | 5/7 | 0/7 | 0/5 | false |
| Plain | 59m 14.713s | 11,524,423 | 160,731 | 5/7 | 1/7 | 0/5 | false |
| BRAN Connected+SQZ | 60m 40.924s | 12,645,716 | 149,563 | 5/7 | 0/7 | 0/5 | false |

These elapsed values are per-arm filesystem wall-clock envelopes, not
provider-only latency. Aggregate outer usage was 55,754,350 input tokens and
757,814 output tokens. Cached input (52,358,912) is a subset of input, and
reasoning output (237,876) is a subset of output. Inner usage was unavailable
for both Connected arms. SQZ receipts were unavailable for all seven
Connected+SQZ stages. Those gaps are metrics only.

The outcome is dominated by a benchmark-contract defect. Stage 5 instructed
agents to update owning RTL, register definitions, and generated interfaces,
but the hidden allowlist rejected reasonable generated register/interface
paths used by every arm. Stage 6 instructed agents to keep validation,
compatibility, and operator documentation consistent, while its hidden
allowlist excluded reasonable validation, compatibility, and operations paths
used by every arm. Those mutations were rolled back. Final acceptance then
required exact undisclosed files under `modernization/registers`,
`modernization/rtl`, `modernization/driver`, and
`modernization/evidence-map.md`. The recorded five material errors per arm are
the five failed hidden cases after those rollbacks, not five independent
semantic adjudications.

Accordingly, the screen has **no valid comparative winner**. Connected was
descriptively fastest and used the fewest outer input tokens; OKF used 471,275
fewer outer input tokens than Plain. Neither observation establishes task
quality because no arm could satisfy the contradictory oracle. See
[`evidence/enterprise-live-20260721.json`](./evidence/enterprise-live-20260721.json).

### 4.2 Historical corrected staged task screen

The corrected screen used BRAN
`ea96baf24875490fd8c743e4971412cacacfbef8` and Arena
`7c825c7007aeeb4bdfde4fe21e80f8451ae9645e`. Candidate prompts were frozen.
A grader-only source-truth map scored ordered BRAN ranking receipts for Core
and Connected and ordered search/open discoveries for Plain. The five staged
steps required cross-file discovery and implementation across configuration,
persistence, CLI, runtime behavior, validation, tests, and documentation.

| Configuration | Step records | Passed | Hit@1 | Hit@3 | Recall@5 | Precision@5 | MRR | Canonical rank | Terminal |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Plain, Sol Medium | 3 | 2 | 0.000 | 0.333 | 0.333 | 0.067 | 0.278 | 4.667 | failed step 3 |
| Core, SQZ off, Sol Medium | 2 | 1 | 0.500 | 0.500 | 0.500 | 0.200 | 0.571 | 5.000 | failed step 2 |
| Core, SQZ on, Sol Medium | 3 | 2 | 0.500 | 0.500 | 0.500 | 0.200 | 0.571 | 5.000 | failed step 3; two ranking samples |
| Core, SQZ off, Sol High | 3 | 2 | 0.333 | 0.667 | 0.833 | 0.267 | 0.583 | 5.333 | failed step 3 |
| Connected variants | 3 | 0 | unavailable | unavailable | unavailable | unavailable | unavailable | unavailable | strict receipt failures |

Across seven valid cells, actual outer usage was 5,833,423 input tokens,
5,175,296 cached-input tokens (a subset of input), and 74,411 output tokens.
No token ceiling was imposed. Time to first correct source is `unavailable`
because retained raw JSONL lacks exact event timestamps. Exact context-window,
remaining-token, and compaction values are also `unavailable`; none were
estimated. Repeated-read and per-turn token fields are preserved where emitted.

No cell met the finalist gate, so there were zero replications and **no
winner**. OKF+RAG, Obsidian task-backend, and wiki-LLM confirmations were not
simulated: this evaluated code has no executable adapters for them.
Provider-free Obsidian export remains separate evidence. See
[`evidence/targeted-multistep-20260721.json`](./evidence/targeted-multistep-20260721.json).

### 4.3 Historical easy-task pilot

On the historical controlled pilot, all 20 eligible agents found the correct target
and produced a correct final answer. Each condition has `n=1`; the arm medians
below are descriptive summaries across heterogeneous cells, not paired
statistics.

| Arm | Cells | Median wall time | Median actual tokens | Median tools | Median failed tools | Correct | Material-error cells | Failed-condition cells |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Plain | 4 | 47.030 s | 72,589 | 5.5 | 2 | 4/4 | 0 | 0 |
| BRAN Core | 4 | 118.060 s | 195,941 | 12 | 4 | 4/4 | 0 | 0 |
| BRAN connected | 12 | 108.975 s | 208,050 | 15.5 | 4 | 12/12 | 4 | 5 |

Plain and connected median direct-path precision and recall were both 1.0.
Core recall was 1.0, while its median packet-locator precision was
0.065217: it recovered all required evidence but admitted a much broader
packet. These precision values are not perfectly interchangeable because Core
scores packet locators while the other arms score directly read target paths.

Connected execution produced nine complete receipts and seven clean connected
results. Three cells recorded grounding failures. The five failed-condition
cells also include final-answer receipt misreporting and one duplicate BRAN
invocation. Four connected cells contained a material reporting or protocol
error even though the outer agent's final defect diagnosis remained correct.
One connected cell contained minor unrequested concurrency and migration
speculation, recorded as specification drift but not as a material correctness
failure.

Planning and architecture were scored for the 16 BRAN cells. All 16 scoped the
architecture to the account boundary. Fourteen named a focused correct test
plan, one used a focused fallback plan after connected failure, and one added
unrequested cases. Plain planning and architecture were not scored. Code-review
quality was not applicable to this diagnosis-only task.

The result does not show a speed, token, or correctness advantage for BRAN on
this task. Plain was descriptively fastest and lowest-token. The task was easy
enough for every outer agent to find the three target files, while Core's broad
packet and connected synthesis added work. The connected failures are useful
product evidence: they show why requested model settings, grounded receipts,
and final-answer reporting require separate attestation.

## 5. Fixture and non-comparative evidence

Provider-free checks are separate from the 20 provider trials. The corrected
corpus passed the full OKF check and a provider-free Arena self-test. Earlier
Core fixture evidence demonstrated deterministic packet construction and used
bytes-divided-by-four estimates only; it is not provider telemetry.

A separate post-review connected compatibility smoke used BRAN
`36574d01c7c6acad9fb97e09d3aad2c7cf683122`, Arena
`83253293a08380aeddb16874616e90cd21d2a081`, and the same corrected corpus.
Spark Medium, running inside BRAN with read/search only, returned the correct
bug, account-scoped fix, and focused test in 15.41 seconds. BRAN validated all
eight citations as exact packet locators. Actual provider telemetry was 68,555
input and 5,652 output tokens, totaling 74,207; the 53,376 cached-input and
4,727 reasoning-output values are included subsets and are not added again.
The packet's bytes-divided-by-four figures remain estimates and are excluded.
This `n=1` smoke tests post-review compatibility only and does not change the
frozen matrix or support a performance advantage claim.

Two failed attempts remain visible: a 0.06-second state-permissions preflight
failure occurred before provider invocation, and an earlier Spark attempt
failed exact citation validation before telemetry publication. See the
[scrubbed smoke evidence](./evidence/connected-smoke-20260720.json).

BRAN's provider-free Obsidian export check passed deterministic YAML,
wikilink, graph-edge, reparse, property-preservation, and unsafe-link rejection
checks. No Obsidian GUI or native plugin was exercised, and Obsidian was an
export surface, not a benchmark competitor.

Understand Anything was retained only as a category-reference supply-chain
record. Its pinned download was not installed or executed because the scan
reported unresolved Critical/High findings. No category-reference performance
comparison is claimed.

## 6. Related evaluation systems

The protocol follows the reproducible repository-task direction of
[SWE-bench](https://arxiv.org/abs/2310.06770) and the agent-computer-interface
analysis of [SWE-agent](https://arxiv.org/abs/2405.15793). The dataset, solver,
scorer, and transcript abstractions in [Inspect AI](https://inspect.aisi.org.uk/)
and the versioned custom-evaluation patterns in
[OpenAI Evals](https://github.com/openai/evals) are relevant to future repeated
studies. Neither framework was used to generate this matrix.

## 7. Threats to validity

- The enterprise screen has one replication per arm and a shared benchmark
  contract defect. Its wall-clock and token measurements are descriptive; its
  hidden correctness result cannot rank agent quality.
- Enterprise stage-5 and stage-6 mutation allowlists contradict the requested
  work surfaces, and final acceptance depends on undisclosed exact output paths.
- BRAN retrieval metrics were unavailable in the enterprise receipts; this is
  an instrumentation limitation, not a task-failure condition.
- Every heterogeneous condition has only one trial; no variance estimate or
  inferential claim is possible.
- The repository and defect are synthetic and controlled.
- The task favored simple source discovery; harder review, planning,
  architecture, and specification-drift tasks were not evaluated here.
- Source-precision definitions differ between Core packet locators and direct
  path reads.
- Pytest was unavailable inside some candidate environments; agents reproduced
  the current collision or named the focused test, but no repair was applied or
  post-fix test run performed in these read-only trials.
- Initialization and indexing were not separately itemized.
- Provider pricing was not applied; dollar spend is `unavailable`.
- The results are exact to the evaluated revisions below; later review fixes
  require separate smoke evidence and do not retroactively change the matrix.

## 8. Reproducibility and evidence identity

- Authoritative enterprise campaign:
  `/home/spectre/alphazede/bran-enterprise-live-3b7d847-20260721`
- Enterprise BRAN revision:
  `917b6dc0565f1be54b83298d97f111877fb2f012`
- Enterprise Arena revision:
  `3b7d847919ead2251434b1f0cbad65fb434aaf07`
- Enterprise structured evidence:
  [`evidence/enterprise-live-20260721.json`](./evidence/enterprise-live-20260721.json)
- BRAN revision:
  `33e86f9ef36bf71130013bcbdcb4e3ad37d150d7`
- Arena revision:
  `ecc39c83275c0d5930a60a3841c9b9514379c415`
- Corrected corpus SHA-256:
  `cb4fab25ecc7dfa132b65608608afb6fa2245e8f3248a1a7f2a1f3752aa7d6d5`
- Private aggregate:
  [`evidence/arena-matrix-20260720.json`](./evidence/arena-matrix-20260720.json)
- Post-review connected smoke:
  [`evidence/connected-smoke-20260720.json`](./evidence/connected-smoke-20260720.json)
- Private aggregate SHA-256:
  `38317b314c0eb31532590c882571c2c463b35873bc6f8244ad74e4ceefd49a1c`
- Source aggregate SHA-256:
  `de3059b27e516a6894ff7a11850fc2d79bb76c491ac78ac7ae1d6cb00cdc5508`
- Source comparison SHA-256:
  `cc90396c82b56ee5c944a22d60b33f39ab5bb4ff9e024b27132c93b87d78d2be`
- Invalid-run ledger SHA-256:
  `763b75c0240e796ef1ffea171452ed325284ff4f525479af605c98d205553ab6`

An earlier 21-run population used corpus digest
`f11a1ff6ebbd0e798766782d6f7d5689534030a1d12246559266c4a4a6517ffa`.
That corpus failed the repository OKF precondition because `AGENTS.md` and
`CLAUDE.md` lacked required `type` metadata. Its raw evidence and seals were
preserved, but every run was classified
`invalid_precondition_okf_repository_fail` and excluded from these metrics.

Raw provider traces remain private because they may contain prompts, commands,
paths, provider run identifiers, and model output. Publication requires a
separate public scrub and owner authorization.

## 9. Conclusion

On our corrected controlled benchmark, no tested configuration completed the
five-stage task, so no experimental configuration or product default was
selected. Core/Sol High improved observed retrieval recall but did not pass
hidden acceptance. Connected execution exposed missing inner/SQZ receipts and
failed closed. This is a reproducible, failure-preserving baseline, not a speed,
token-saving, hallucination-elimination, or automatic-repair claim.

## 10. Historical failures, hypotheses, and planned enterprise protocol

### 10.1 Separate failure narratives

The historical easy-task pilot is historical evidence, not a successful product
selection experiment. All 20 eligible agents found its intended target, but the
task was simple, each heterogeneous condition had `n=1`, Core packets were
broad, connected execution had grounding and receipt failures, and SQZ was off.
It therefore cannot establish a causal efficiency or correctness advantage.

The corrected targeted five-step screen is measured, retained, and also
non-selecting: seven valid cells produced 14 step records, no cell completed
all hidden stages, zero replications were eligible, and there was no winner.
Its failures remain evidence; they must not be overwritten, reframed as a
success, or used to infer a product default. The future protocol below is
unmeasured and does not alter either historical classification.

### 10.2 Hypotheses

On our controlled benchmark, the planned study tests whether structured
knowledge, deterministic BRAN routing, Connected synthesis, and SQZ reduce
enterprise implementation-team discovery work while preserving specification,
design, implementation, and validation correctness. Its predeclared causal
comparisons are no-OKF to OKF, OKF to Core, Core to Connected, and Connected to
Connected+SQZ. These are hypotheses, not claims of an advantage. A Core+SQZ
sixth arm and heterogeneous-model aggregation are excluded unless a later owner
amendment changes the protocol.

### 10.3 Future, unmeasured enterprise work order

The fixed work order modernizes a legacy PCIe telemetry DMA IP block by adding
configurable per-channel interrupt moderation. One candidate-visible mounted
enterprise file share disperses approved and obsolete requirements, IP/domain
specifications, register maps, diagrams, RTL, driver, generated artifacts,
errata, validation, security, performance, operations, and release evidence
after the senior owner has retired. Prompts may name that share root and a small
set of work-order entry documents, never the complete required-source map or
hidden acceptance answer.

The frozen SDLC sequence is: evidence/legacy reconstruction; requirements
reconciliation; replacement specification; implementation design with updated
Mermaid diagrams; canonical register/driver contract implementation; IP-block
implementation; and validation/release reconciliation. The hidden truth for
each stage includes required sources, canonical owner, accepted aliases or
generated paths, superseded/prohibited evidence, required relationships,
requirements trace, spec/design consistency, code/validation acceptance, and
unauthorized mutation.

### 10.4 Fixed configurations and execution

The BRAN target revision is `ea96baf24875490fd8c743e4971412cacacfbef8`; the
Arena target revision is `7c825c7007aeeb4bdfde4fe21e80f8451ae9645e`. Corpus,
entry documents, prompts, Sol High foreground model/reasoning, tools except the
arm capability, context window, and isolation are byte-identical across exactly
these five arms:

| Identifier | Fixed capability |
|---|---|
| `llm-no-okf` | Normal filesystem search/open/code/test tools only. |
| `llm-okf` | Evaluation-safe public OKF query/traversal; the LLM follows metadata/source relationships and opens sources itself, without private `use-okf`, owner memory, or AlphaZede metadata. |
| `llm-bran-core` | Deterministic Core ranking with bounded locators/excerpts; no inner model; SQZ off. |
| `llm-bran-connected` | Identical Core retrieval plus one fixed read-only Spark Medium inner synthesis profile; SQZ off. |
| `llm-bran-connected-sqz` | Byte-identical Connected configuration with SQZ only after grounded synthesis; source selection, ranking, inner model, and citations are identical to Connected off. |

Core uses deterministic structural parsing; clean-build-parity incremental
indexing with change/delete/rename detection; ownership/lifecycle,
dependency/impact, requirement/design/code/test, domain/architecture, and
business-flow edges; graph-integrity validation; newly changed/created-artifact
indexing; seed- and stage-aware traversal; domain-balanced packets;
sufficiency/conflict receipts; and canonical-first ranking. Ranking precedence
is security/lifecycle eligibility, canonical approval/ownership,
revision/supersession, generated/source/archive/rejected status, direct
relationships, query relevance, required-domain coverage, frozen historical
search frequency, then deterministic path order. That frequency is frozen
before attempts, capped at 5% of score, limited to scrubbed qualifying access,
unique-team coverage, accepted-artifact citations, and time decay; it excludes
bots, current-attempt, cross-attempt, and personal activity and cannot override
authority.

Provider-free gates run first, then exactly one isolated live cell per arm runs.
No numeric token ceiling is invented; actual platform quota is the external
ceiling. An attempt is eligible only if every SDLC stage and hidden acceptance
pass, required recall is 1.0, unsupported citations and material errors are
zero, its terminal state succeeds, and no unauthorized mutation occurs. Each
eligible configuration is replicated three times while quota remains; all
failures are retained and paired results report sample counts.

### 10.5 Planned telemetry and claim boundary

The planned telemetry includes Hit@1/3, Recall@5, Precision@5, MRR, canonical
rank, time to first correct/canonical source, total/unique/repeated/reformulated
searches, searches before canonical and after sufficiency, files opened/
irrelevant/reopened, bytes, per-turn exact tokens when emitted, outer/inner/SQZ
components, tools/failures, packet/excerpt sizes, context compaction/window/
remaining values or `unavailable`, grounding, unsupported claims, drift, hidden
correctness, and isolation. This future protocol does not manufacture results,
advantages, defaults, universal speed or token claims, hallucination
elimination, or automatic repair.

The enterprise harness completed all 35 stage invocations and retained their
raw evidence, but the benchmark contract prevented a valid quality comparison.
Every arm hit the same two mutation-allowlist contradictions and then failed
the same five exact-path hidden cases. Connected was fastest and lowest-input
in this single replication, while Connected+SQZ was slowest and emitted no SQZ
receipts; these remain efficiency observations, not winner evidence. No
configuration or product default is selected. The result is a
failure-preserving baseline and a benchmark-correction requirement, not a
speed, token-saving, hallucination-elimination, or automatic-repair claim.
