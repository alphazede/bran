---
type: agent-instructions
title: BRAN Internal Repository Instructions
okf_status: active
status: stable
tags:
  - internal
  - bran
freshness: "2026-08-18"
resource: https://github.com/alphazede/bran-dev
public_boundary: private
---

# BRAN Agent Instructions

This is BRAN's main development workspace. Make every source, test,
documentation, planning, and research change here first. Send exported changes
to `alphazede/bran` only as reviewed snapshots through the approved export and
pull-request sync process; do not develop or manually edit BRAN in that
checkout.

The exporter builds a reviewed snapshot from a committed revision. That
snapshot must never run ahead of this repository or include private plans,
submission evidence, unpublished proposals, agent instructions, or local
`.bran` data.

## Repository map

See the [documentation index](docs/README.md) for integration guides, plans,
and submissions.

- `crates/`: Rust source for Core, CLI, and TUI.
- `schemas/`, `fixtures/`, `examples/`, and `benches/`: schemas, test data,
  examples, and benchmarks.
- `docs/integrations/`: integration guides that may ship with BRAN.
- `docs/plans/`: internal BRAN plans. Follow its path-specific `AGENTS.md`.
- `docs/submissions/`: private research and submission evidence. Keep it out of
  exports unless the owner approves a scrubbed artifact.
- `skill/use-bran/`: the BRAN skill for agent integrations.

The separate Arena harness lives at
`/home/spectre/alphazede/agentic-eval-arena`; keep harness implementation and
hidden evaluation material there.

## Working rules

1. Make BRAN source changes here. Use `alphazede/bran` only to receive an
   approved exported snapshot through a pull request; do not develop or
   manually edit BRAN there.
2. Keep scanning, ranking, packet generation, and offline browsing usable
   without a provider account.
3. Never add credentials, raw authentication state, private corpora, hidden
   grader truth, or unsanitized provider traces.
4. Report requested, effective, and attested capabilities separately. If
   something is unavailable, say so.
5. Treat exporting, syncing, tagging, releasing, and publishing as separate
   actions. Each requires owner approval.
6. Preserve unrelated user changes. Remove a temporary branch or worktree only
   after proving it is clean and reachable from its integration branch.

## Validation

Run the smallest relevant test while you work. For most integrated changes,
run:

```sh
./tools/cutover/publish-hygiene.sh
./tools/ci/check.sh --fast
```

`./tools/cutover/publish-hygiene.sh` calls the shared workspace guard and derives the
public surface from `public-export.json`. It fails closed when `okf_status` or
`public_boundary` frontmatter would be exported. A bare `type:` remains valid.
For `SKILL.md`, this is also a correctness check because agent hosts parse its
frontmatter.

A packaged release binary must be built through `build/build-pinned.sh`, which
compiles the StrictDoc bridge pins in and re-hashes every pinned file first. See
[`build/README.md`](build/README.md); a bare `cargo build --release` produces a
binary whose `sdoc` command fails closed as `sdoc_runtime_unavailable`.

Reserve `./tools/ci/check.sh --full` for changes that affect release, security,
conformance, or performance behavior. Do not install missing tools or contact
live providers solely to validate a local source change.

`okf-v0.1` remains a supported selectable compatibility profile. `okf-v0.2` is
additive and does not replace it. Native policy keeps the BRAN producer
extensions `okf_status`, `freshness`, and `public_boundary`. When a document
also carries the OKF v0.2 `status` field, the mapping is `draft` → `draft`,
`active` → `stable`, and `deprecated` → `deprecated`. Optional v0.2
`stale_after` is allowed only when a real expiry date exists; it is not a
rename of `freshness` and is not required.

## Preparing a clean snapshot

`public-export.json` lists what may and may not ship. Every tracked path must be
classified, and any unclassified path stops the export. The exporter reads
committed Git blobs, never files from an uncommitted working tree.

From a clean committed `bran-dev` checkout, create a new scrubbed snapshot in
an absent or empty directory:

```sh
python3 tools/ci/public_export.py snapshot --output /path/to/empty/snapshot
```

After committing the snapshot, compare its contents and file modes, export
receipt, Git state, and configured remote:

```sh
python3 tools/ci/public_export.py check --public-dir /path/to/bran
```

Before pushing, inspect every exported file for private or sensitive data. Then
run the hygiene gate.

The source is a reviewed committed bran-dev revision and the checked snapshot
was produced by the approved exporter. Push that committed checked snapshot to
a generic non-default branch in `alphazede/bran`. Open a draft PR targeting
public main. The PR body must name the exact bran-dev source commit, the exact
public export commit, and validation evidence. Public `main` is protected and
cannot be pushed to directly. Wait for the required fast and CodeQL checks to
pass. Owner review of the exact exported diff and separate authorization to
merge are required; only then merge the pull request with a merge commit. Never
squash-merge an export: squashing discards the reviewed export commits and
rewrites the published snapshot into a commit no reviewer approved. The exporter
does not open pull requests. Passing the export checks does not authorize tagging or
publishing a package.


## Fixable review findings

Never pass or accept `ACCEPT_WITH_FINDINGS` while a fixable bug remains. If an actionable review finding can be repaired without causing a regression or violating the approved contract, the verdict is `REPAIR_REQUIRED`; fix it in the authorized repair round and rerun deterministic verification. `ACCEPT_WITH_FINDINGS` is reserved for owner-approved residual risk or a finding that cannot be repaired within the approved contract without causing a regression.
