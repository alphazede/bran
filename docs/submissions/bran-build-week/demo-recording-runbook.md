---
type: submission-artifact
title: BRAN Immediate Demo Recording Runbook
okf_status: draft
status: draft
tags:
  - internal
  - bran
freshness: "2026-07-24"
resource: https://github.com/alphazede/bran-dev
public_boundary: private
---

# BRAN Immediate Demo Recording Runbook

This is the shortest honest product demo. It does not show or claim a model
comparison. Record locally only after the clean rehearsal; publishing or
uploading requires separate owner authorization.

## Before recording

- Use post-native-review BRAN revision
  `36574d01c7c6acad9fb97e09d3aad2c7cf683122` for the local rehearsal. For the
  final judge video, replace it only with the exact owner-authorized tag and
  verify its manifest, checksum, trusted signature, and platform archive.
- Install the authorized exact tag with:

  ```sh
  cargo install --git https://github.com/alphazede/developers --tag bran-vX.Y.Z --locked bran-cli
  ```

- Use a public-safe 512-file sample that passes the full OKF repository check.
  Do not reuse private provider traces or expose a local owner path.
- Resize the terminal and scrub repository roots, auth material, run IDs, and
  private corpus content from every frame.
- Have the public repository and raven hero ready:
  [alphazede/developers/bran](https://github.com/alphazede/developers/tree/main/bran)
  and
  [`bran-repository-raven.png`](https://github.com/alphazede/developers/blob/main/bran/assets/brand/bran-repository-raven.png).

## 2:30 recording

### 0:00-0:20 — problem

Narrate: “Large workspaces make agents spend context on discovery before they
can solve the task. BRAN is a local Rust evidence router that works without a
model and gives an agent a bounded, attributed context packet.”

### 0:20-0:45 — TUI

Run `bran tui`. Show Quick, Advanced, environment checks, optional agent
connection, readiness review, and the practice path. Do not enable or imply
connected synthesis, SQZ, voice, retained history, or repair behavior unless
the exact release labels it configured and the rehearsal proves it.

### 0:45-1:20 — query

Run:

```text
bran query <sample-root> "Locate the misleading-symptom retry bug source and its visible validation evidence."
```

Point to the returned paths, `why_selected`, provenance, candidate bytes,
selected bytes, avoided bytes, and any explicit `unavailable` telemetry.

### 1:20-1:55 — packet

Run the same request through:

```text
bran packet <sample-root> "Locate the misleading-symptom retry bug source and its visible validation evidence."
```

Narrate: “The packet is what a configured agent reads. BRAN finds and accounts
for evidence; the agent using BRAN performs the diagnosis, explanation, or
owner-authorized repair.”

### 1:55-2:15 — exact-release local evidence

Show only the deterministic card reproduced against the exact recording
revision. Label byte-derived token values as estimates, provider tokens as
actual only when provider telemetry exists, and absent telemetry as
`unavailable`. Do not show the controlled model-comparison table in the video.

### 2:15-2:30 — close

Narrate: “BRAN Core is model-agnostic and offline. Connected agents, SQZ,
Obsidian export, voice, and retained history are explicit configuration
choices.” Show the exact tag, checksums, public repository, and install command.

## Recording acceptance

- Total duration is below three minutes and includes audio.
- The timed video contains no plain/Core/connected comparison, universal win,
  competitor claim, or automatic-repair claim.
- Every shown behavior and number was reproduced against the exact recording
  revision; estimates and unavailable fields are labeled.
- No personal path, auth/state, private corpus, raw provider trace, fabricated
  screenshot, unavailable feature, or private model/Devpost copy appears.
- BRAN’s final local full gate is green with 20 tests. Arena’s final local full
  gate is green at revision `83253293a08380aeddb16874616e90cd21d2a081`
  with 374 pytest tests in 57.49 seconds, Ruff, strict Mypy over 50 files, and
  Bandit exit 0 with warnings only. These are local results; hosted CI was not
  run because the exact local commits were not pushed and Arena has no remote.
- The owner approves the final screenshots, recording, upload, repository
  publication, and Devpost submission. This runbook grants none of those
  external actions.

## Research reference outside the video

The completed comparison remains private in the
[research paper](./research-paper.md) and
[sealed evidence](./evidence/arena-matrix-20260720.json). It evaluated BRAN
`33e86f9ef36bf71130013bcbdcb4e3ad37d150d7` and Arena
`ecc39c83275c0d5930a60a3841c9b9514379c415`; do not relabel it as evidence for
the later reviewed revisions used by the recording rehearsal.

The separate [connected smoke](./evidence/connected-smoke-20260720.json)
records `n=1` post-review compatibility at BRAN
`36574d01c7c6acad9fb97e09d3aad2c7cf683122` and Arena
`83253293a08380aeddb16874616e90cd21d2a081`. Spark using BRAN produced the
correct grounded answer, but this smoke is neither a matrix row nor a demo
comparison claim.
