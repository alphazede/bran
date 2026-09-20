---
type: submission-artifact
title: BRAN Build Week Demo Video Outline
okf_status: draft
status: draft
tags:
  - internal
  - bran
freshness: "2026-07-24"
resource: https://github.com/alphazede/bran-dev
public_boundary: private
---

# BRAN Build Week Demo Video Outline

Private HQ submission artifact. The timed demo shows BRAN product behavior and
contains no model comparison. Comparative research remains in the evidence
appendix and research paper.

## Target: 2:45

Record from an exact owner-authorized release with audio and stop before 3:00.
Use only behavior reproduced during the clean judge rehearsal.

| Time | Visual | Narration and proof |
|---|---|---|
| `0:00-0:15` | A large repository with conflicting evidence | Explain that agents spend context on discovery before solving a task. |
| `0:15-0:35` | Public repository and raven hero | Introduce BRAN as a local Rust evidence router. State the build-period extension and required build-tool/model attribution in private submission copy only. |
| `0:35-1:05` | `bran tui` Quick and Advanced flows | Show environment checks, optional agent connection, profile/model/reasoning, BRAN mode, SQZ, voice, retention, readiness review, practice run, and repair mode. Unconfigured features must visibly remain unavailable or off. |
| `1:05-1:35` | Offline `bran query` and `bran packet` on the public-safe sample | Show the correct evidence paths, `why_selected`, provenance, candidate/selected bytes, warnings, and stable machine-readable fields. Do not call the packet an LLM answer. |
| `1:35-1:55` | Optional connected receipt or offline boundary | If rehearsed, show that a configured agent reads the packet and authors the response. Otherwise show the typed offline/unavailable result. Never imply BRAN silently repairs files. |
| `1:55-2:20` | Reproducible Core evidence card | Show only deterministic, exact-release measurements and label estimates and unavailable telemetry. Do not show the plain/Core/connected comparison. |
| `2:20-2:35` | Offline architecture and public/private boundary | Explain that Core works without an LLM; connected synthesis, SQZ, Obsidian export, voice, and history are explicit choices. |
| `2:35-2:45` | Exact tag, checksums, install command, public repository | Close with a measured local claim and invite judges to reproduce it without rebuilding. |

## Judge walkthrough

1. Verify the public repository, dual license, exact tag, release manifest,
   checksums, trusted signature, and supported platform archive.
2. Install the exact tag:

   ```sh
   cargo install --git https://github.com/alphazede/developers --tag bran-vX.Y.Z --locked bran-cli
   ```

3. Run `bran tui`, choose Quick, inspect Advanced, and read back every requested
   versus effective or unavailable setting.
4. On the bundled public-safe sample, run:

   ```sh
   bran query <sample-root> "Locate the misleading-symptom retry bug source and its visible validation evidence."
   bran packet <sample-root> "Locate the misleading-symptom retry bug source and its visible validation evidence."
   ```

5. Point to selected sources, provenance, bytes, warnings, and unavailable
   fields. Attribute any diagnosis or proposal to the configured agent using
   BRAN.
6. Demonstrate explicit-authority repair and its receipt only if the final
   rehearsal proves that exact path; otherwise omit it.
7. Confirm offline return behavior, then remove the sample installation using
   the public documentation.

## Evidence appendix — not part of the timed demo

The completed controlled comparison is exact to BRAN
`33e86f9ef36bf71130013bcbdcb4e3ad37d150d7`, Arena
`ecc39c83275c0d5930a60a3841c9b9514379c415`, and corrected 512-file corpus
`cb4fab25ecc7dfa132b65608608afb6fa2245e8f3248a1a7f2a1f3752aa7d6d5`.
It is not evidence about the later reviewed code.

| Arm | Cells | Median wall time | Median actual tokens | Correct | Material errors | Failed conditions |
|---|---:|---:|---:|---:|---:|---:|
| Plain | 4 | 47.030 s | 72,589 | 4/4 | 0 | 0 |
| BRAN Core | 4 | 118.060 s | 195,941 | 4/4 | 0 | 0 |
| BRAN connected | 12 | 108.975 s | 208,050 | 12/12 | 4 | 5 |

Each heterogeneous cell has `n=1`; medians are descriptive. On our controlled
benchmark, every eligible agent found the intended bug and account-scoped fix,
but plain was descriptively fastest and lowest-token on this easy task. See the
[research paper](./research-paper.md) and
[private aggregate](./evidence/arena-matrix-20260720.json). Dollar spend and
separately itemized initialization/indexing time are `unavailable`.

Post-native-review integrated revisions are BRAN
`36574d01c7c6acad9fb97e09d3aad2c7cf683122` and Arena
`83253293a08380aeddb16874616e90cd21d2a081`. Their final local full gates
passed: BRAN completed its 20-test inventory within the 25-test cap; Arena
completed 374 pytest tests in 57.49 seconds, Ruff, strict Mypy over 50 files,
and Bandit with warnings only and exit 0. Hosted CI was not run because the
exact local commits were not pushed and Arena has no remote. A signed release
also remains `unavailable`.

A separate post-review connected smoke at those revisions passed in 15.41
seconds: Spark using BRAN produced the correct bug/fix/test answer with 74,207
actual provider tokens and eight exact packet citations accepted by BRAN. It is
`n=1` compatibility evidence, not a matrix row or material for the timed demo.
The [scrubbed record](./evidence/connected-smoke-20260720.json) preserves the
preceding preflight and citation-validation failures.

Understand Anything was downloaded and scanned only. It was not installed or
executed because the pinned supply-chain record contains unresolved
Critical/High findings. Obsidian evidence covers deterministic provider-free
export, not a competing retrieval system or native plugin.

## Recording assets and gates

- Public repository:
  [alphazede/developers/bran](https://github.com/alphazede/developers/tree/main/bran)
- Repository hero:
  [`bran/assets/brand/bran-repository-raven.png`](https://github.com/alphazede/developers/blob/main/bran/assets/brand/bran-repository-raven.png)
- TUI raven sources: public `bran/assets/tui/`
- Capture only real exact-release TUI, onboarding, headless receipt, checksum,
  signature, and manifest screenshots; scrub personal paths and private data.
- The owner must authorize the exact tag/release, final recording, screenshots,
  upload, repository publication, and Devpost submission. No avatar change is
  implied by the present raven asset.
