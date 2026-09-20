---
type: submission-artifact
title: BRAN Build Week Submission Checklist
okf_status: draft
status: draft
tags:
  - internal
  - bran
freshness: "2026-07-24"
resource: https://github.com/alphazede/bran-dev
public_boundary: private
---

# BRAN Build Week Submission Checklist

Private HQ only. This checklist records local readiness separately from actions
that require the owner, hosted platforms, trusted builders, or signing keys.

## Position and public links

- [x] Enter **Developer Tools**. Work & Productivity is supporting use-case
  evidence, not a second category.
- [x] Attribute diagnoses, plans, reviews, and repairs to the agent using BRAN;
  BRAN supplies evidence and receipts.
- [x] Public repository:
  [alphazede/developers/bran](https://github.com/alphazede/developers/tree/main/bran).
- [x] Public install template for an owner-authorized exact tag:

  ```sh
  cargo install --git https://github.com/alphazede/developers --tag bran-vX.Y.Z --locked bran-cli
  ```

- [x] Public integration guide:
  [`bran/docs/integrations/agent-setup.md`](https://github.com/alphazede/developers/blob/main/bran/docs/integrations/agent-setup.md).
- [x] Public raven asset and avatar candidate:
  [`bran/assets/brand/bran-repository-raven.png`](https://github.com/alphazede/developers/blob/main/bran/assets/brand/bran-repository-raven.png).

## Local implementation and evidence

- [x] Controlled comparison is exact to BRAN
  `33e86f9ef36bf71130013bcbdcb4e3ad37d150d7`, Arena
  `ecc39c83275c0d5930a60a3841c9b9514379c415`, and corrected 512-file corpus
  `cb4fab25ecc7dfa132b65608608afb6fa2245e8f3248a1a7f2a1f3752aa7d6d5`.
- [x] Post-native-review integrated revisions are recorded separately: BRAN
  `36574d01c7c6acad9fb97e09d3aad2c7cf683122` and Arena
  `83253293a08380aeddb16874616e90cd21d2a081`.
- [x] BRAN final local full gate passed; test inventory is 20, within the
  25-test plan cap.
- [x] Arena final local full gate passed at the exact revision above: 374
  pytest tests in 57.49 seconds,
  Ruff passed, strict Mypy passed over 50 files, and Bandit completed with
  warnings only and exit 0.
- [x] Post-review connected CLI compatibility smoke passed with Spark Medium
  inside BRAN: exit 0, 15.41 seconds, 74,207 actual provider tokens, correct
  bug/fix/test answer, and eight exact packet citations accepted by BRAN. This
  is `n=1` compatibility evidence and is not part of the frozen matrix.
- [x] The smoke evidence preserves one permissions preflight failure before
  provider invocation and one earlier exact-citation validation failure.
- [x] One native Arena phase review returned `patch incorrect`; valid findings
  were remediated without a duplicate general review.
- [ ] Hosted CI is `unavailable` and was not run for these exact local commits:
  they were not pushed, and Arena has no remote.
- [x] Corrected corpus passed the full `okf-v0.1` repository check. Plain saw no
  skills; BRAN arms saw only `use-bran`; neutral `AGENTS.md` and `CLAUDE.md`
  applied identically to every arm.
- [x] Earlier runs whose instruction metadata failed the OKF precondition remain
  preserved, classified invalid, and excluded from comparison metrics.
- [x] Provider-free Obsidian export evidence passed; no GUI or native-plugin
  behavior is claimed.
- [x] Understand Anything remains an unexecuted category reference because its
  pinned supply-chain scan reported unresolved Critical/High findings.

## Controlled benchmark card

Each heterogeneous cell has `n=1`; medians are descriptive, not statistical.
See the [research paper](./research-paper.md) and
[private aggregate](./evidence/arena-matrix-20260720.json).

| Arm | Cells | Median wall time | Median actual tokens | Correct | Material errors | Failed conditions |
|---|---:|---:|---:|---:|---:|---:|
| Plain | 4 | 47.030 s | 72,589 | 4/4 | 0 | 0 |
| BRAN Core | 4 | 118.060 s | 195,941 | 4/4 | 0 | 0 |
| BRAN connected | 12 | 108.975 s | 208,050 | 12/12 | 4 | 5 |

- [x] State only: “On our controlled benchmark, all 20 eligible agents found
  the intended bug and account-scoped fix.”
- [x] State that plain was descriptively fastest and lowest-token on this easy
  task; this comparison did not demonstrate a BRAN advantage.
- [x] Actual provider tokens use input plus output once. Cached input and
  reasoning output are subsets and are not added again.
- [x] Core bytes-divided-by-four values remain estimates. Dollar spend and
  separately itemized initialization/indexing time remain `unavailable`.
- [x] Report grounding failures, receipt/reporting errors, failed conditions,
  invalidated runs, and the one-trial sample count.

## Security and public/private boundary

- [x] BRAN Core works locally without an LLM or provider account.
- [x] Connected agents, SQZ response processing, voice, and retained history are
  explicit configuration choices. No default connected-task token ceiling is
  claimed; `tokens=N` is user-configured.
- [x] Public source contains the public `README.md` and `use-bran` skill, not
  private Devpost/model copy, private corpora, auth/state, owner paths, raw
  provider traces, or submission evidence.
- [x] No claim says BRAN eliminates hallucinations, is always faster, beats
  every competitor, or independently performs an agent-authored repair.
- [x] No release, tag, upload, submission, spend, deployment, publication, or
  avatar mutation was performed by this closeout.

## Judge and recording flow

1. Open the public repository, license, exact-release manifest, checksums, and
   installation command.
2. Run the offline TUI and deterministic query/packet path on the public-safe
   sample; show selected evidence, provenance, bytes, warnings, and
   `unavailable` fields.
3. Explain that a configured agent reads the packet and authors the diagnosis
   or proposal; BRAN does not silently modify the repository.
4. Show one explicit-authority repair receipt only if the exact release
   rehearsal proves it.
5. Show the controlled evidence card outside the timed comparison-free demo,
   with exact revisions, `n=1`, failures, and unavailable telemetry.
6. Remove the sample installation through the documented path.

## Real screenshot and asset list

- [x] Repository raven source asset is present; GitHub avatar is unchanged.
- [x] Public TUI raven sources are present under `bran/assets/tui/`.
- [ ] Capture the exact-release TUI hero and onboarding readiness review.
- [ ] Capture a scrubbed headless query/packet receipt with provenance and
  failure fields.
- [ ] Capture the controlled benchmark card from the private aggregate after a
  public-boundary scrub.
- [ ] Capture checksums, trusted signature, manifest, and supported platform
  assets from the owner-authorized release.
- [ ] Verify every final screenshot contains no personal path, credential,
  private corpus, raw trace, or fabricated state.

## Exact owner-only actions

- [ ] Push the approved commits and authorize repository/publication state.
- [ ] Authorize and submit the Devpost entry in Developer Tools.
- [ ] Record and upload the under-three-minute demo with audio; capture and
  approve the real screenshots and final public URLs.
- [ ] Supply and verify the eligible `/feedback` session identifier.
- [ ] Separately authorize any GitHub avatar change.
- [ ] Supply trusted macOS x86_64, macOS arm64, and Windows MSVC builders,
  signing credentials, and exact tag/release authorization; publish only the
  exact checksummed and signed assets.
- [ ] Separately authorize any stable internal installation, deployment, or
  promotion.
- [ ] Remediate or repin Understand Anything, rerun the supply-chain scan, and
  clear policy before any installation or execution.
- [ ] Read back the live rules/deadline, final entry, repository URL, demo URL,
  evidence qualifications, and every `unavailable` item immediately before
  submission.

## Deterministic closeout

- [ ] Confirm all private-package links and final public URLs resolve.
- [ ] Confirm the timed demo remains comparison-free and below three minutes.
- [ ] Confirm numeric claims match the sealed private aggregate.
- [ ] Confirm exact tested revisions are not mislabeled as the later reviewed
  revisions.
- [ ] Confirm hosted CI, signed release, screenshots, video, and Devpost status
  are reported from readback, never inferred.
