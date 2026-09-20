---
type: customer-setup
title: Customer and judge walkthrough
okf_status: draft
status: draft
tags:
  - internal
  - bran
freshness: "2026-07-24"
resource: https://github.com/alphazede/bran-dev
public_boundary: private
---

# Customer and judge walkthrough

This is the private rehearsal guide. Use the public canonical
[`bran/docs/integrations/agent-setup.md`](https://github.com/alphazede/developers/blob/main/bran/docs/integrations/agent-setup.md)
for supported recipes. Local tests do not replace an exact signed release or a
clean judge-machine rehearsal.

## First-run onboarding

Start the terminal interface:

```sh
bran tui
```

Quick and Advanced flows expose environment checks, optional agent connection,
profile/model/reasoning choices, BRAN mode, SQZ, voice, retention, readiness,
practice run, and repair mode. Review requested and effective settings before
applying them. Offline, read-only operation with zero-conversation retention is
the safe baseline; missing capabilities remain `unavailable`.

Then run:

```sh
bran doctor --onboarding
```

Check `ready`, `settings_status`, requested/effective capability states, and the
offline-return proof. The diagnostic should record zero provider, auth, and
network calls.

## Offline BRAN Core

Deterministic Core needs no agent or provider account:

```sh
bran packet <repo-root> "<request>"
bran query <repo-root> "<request>"
```

Preserve locators and provenance. Any bytes-divided-by-four token field is an
estimate, not provider usage.

## Install the agent skill

Copy the public `skill/use-bran` directory into the external agent host's skill
directory. If that host uses a nonstandard path, set `BRAN_SKILL_PATH` to the
copied `SKILL.md` only for doctor discovery. BRAN reports discovery; it does not
modify global agent configuration.

Credentials stay in the reviewed host credential store or documented
environment input. BRAN accepts no credential CLI flag and must not echo secret
material.

## Optional connected agent

Enable Connected Agent in the TUI, configure the reviewed host adapter, and
inspect profiles and readiness:

```sh
bran agents list
bran doctor --agent
```

Agent doctor validates local CLI and skill discovery, workspace policy, SQZ
capability, a deterministic packet round trip, and host attestation. It does not
contact a provider merely to probe capability. Local setup may pass while
connected execution remains `unavailable`; the command must not claim overall
readiness without effective host attestation.

A connected-task total-token ceiling is unset unless the user explicitly
configures `tokens=N`. When set, it remains requested until the host attests
enforcement. Leaving it unset does not block connected execution and does not
claim token enforcement. The separate 65,536-byte connected-answer bound is an
independent byte-safety limit.

Reasoning accepts `off|minimal|low|medium|high|xhigh`; task tools are limited to
`read,search`:

```sh
bran -p --trust-current-root --agent <profile> --reasoning medium --tools read,search "review this change"
bran -p --trust-current-root --agent <profile> --reasoning low --no-session "find the owning specification"
```

Read the complete receipt. Requested and effective profile, model, reasoning,
tools, SQZ, session, and token-policy fields are distinct. Unattested effective
values stay `unavailable`. `--no-session` requests no conversation session;
bounded result artifacts remain governed by their own retention rules.

Retrieve an explicitly retained result before its TTL expires:

```sh
bran get <receipt.result_id>
```

## Controlled Arena evidence

The completed comparison is exact to BRAN
`33e86f9ef36bf71130013bcbdcb4e3ad37d150d7`, Arena
`ecc39c83275c0d5930a60a3841c9b9514379c415`, and corrected 512-file corpus
`cb4fab25ecc7dfa132b65608608afb6fa2245e8f3248a1a7f2a1f3752aa7d6d5`.
The full corpus passed `okf-v0.1` before dispatch.

Each heterogeneous cell has `n=1`: four plain, four BRAN Core, and twelve BRAN
connected cells. Outer agents were Sol Medium/XHigh and Terra Medium/XHigh;
connected inner agents were Luna Low/Medium and Spark Medium. Every agent found
the missing-`account_id` retry-key defect and correct fix. SQZ was off in every
cell, so this matrix does not evaluate SQZ.

| Arm | Cells | Median wall time | Median actual tokens | Median tools | Median failed tools | Correct |
|---|---:|---:|---:|---:|---:|---:|
| Plain | 4 | 47.030 s | 72,589 | 5.5 | 2 | 4/4 |
| BRAN Core | 4 | 118.060 s | 195,941 | 12 | 4 | 4/4 |
| BRAN connected | 12 | 108.975 s | 208,050 | 15.5 | 4 | 12/12 |

On our controlled benchmark, plain was descriptively fastest and lowest-token
on this easy task. Core recovered all target evidence but selected a broader
packet. Connected execution added provider work and recorded four material
reporting/protocol errors, five failed-condition cells, and three grounding
failures. These are heterogeneous single-cell descriptions, not universal
performance claims.

Actual provider totals are input plus output. Cached input and reasoning output
are subsets and are not added again; connected totals add outer and inner
provider totals once. Core packet bytes divided by four remain estimates and
are excluded from actual totals. Dollar spend is `unavailable`.

The recorded end-to-end wall time includes the candidate execution path, but
initialization and indexing are not separately itemized and are therefore
`unavailable`. The earlier `f11a1ff6...` corpus runs failed the repository OKF
precondition and are preserved but excluded.

See [arena-matrix-20260720.json](../evidence/arena-matrix-20260720.json) and the
[research paper](../research-paper.md).

## Post-review connected smoke

One real CLI compatibility smoke exercised BRAN
`36574d01c7c6acad9fb97e09d3aad2c7cf683122` with Arena
`83253293a08380aeddb16874616e90cd21d2a081`:

```sh
bran -p --trust-current-root --agent spark-medium --reasoning medium --tools read,search --no-session "Investigate why events from different accounts sometimes collapse in the retry queue. Explain the bug, the fix, and how to test it."
```

Spark using BRAN produced the correct answer, and BRAN validated eight exact
packet citations. The run took 15.41 seconds and recorded 74,207 actual
provider tokens. It used no token ceiling; the independent answer limit was
65,536 bytes. Treat this as `n=1` compatibility evidence, not a new matrix row
or a performance claim. The scrubbed record also preserves the preflight and
citation-validation failures that preceded the successful run. See
[connected-smoke-20260720.json](../evidence/connected-smoke-20260720.json).

## Obsidian export

BRAN Core's provider-free Obsidian export test passed deterministic frontmatter,
wikilink, graph-edge, reparse, property-preservation, and unsafe-link rejection
checks. No provider or agent was used. No Obsidian GUI or native plugin was
tested, and Obsidian is an export surface rather than a benchmark competitor.
See [obsidian-core-20260720.json](../evidence/obsidian-core-20260720.json).

## Voice and saved history

Voice and saved-history behavior must reflect actual installation and policy.
If unavailable, leave them off and display `unavailable`; do not simulate them.

## Return to offline mode

Disable Connected Agent in the TUI or verify the boundary directly:

```sh
bran -p --agent <profile> --offline --no-session "offline return proof"
bran packet <repo-root> "<request>"
```

The first command returns a typed incomplete offline receipt rather than a
generated answer. The following packet remains deterministic and must not
initialize provider, auth, or network paths.

## Judge path

Use only an exact owner-authorized release. Rehearse onboarding, Core retrieval,
optional connected execution, receipt reading, result retrieval, and offline
return on a clean machine. Show every `unavailable` field plainly. Do not claim
signed multi-platform availability, hosted CI, provider spend, GUI evidence, or
publication until the corresponding owner-controlled evidence exists.
