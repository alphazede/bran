---
type: repository-guide
title: BRAN Claude Code Guide
okf_status: active
status: stable
tags:
  - internal
  - bran
freshness: "2026-08-18"
resource: https://github.com/alphazede/bran-dev
public_boundary: private
---

# CLAUDE.md — BRAN

**You are Claude Code** — implementer and reviewer for BRAN.

> Claude Code does **not** auto-load `AGENTS.md`. Open [`AGENTS.md`](AGENTS.md)
> on session start (and the path-specific `docs/plans/AGENTS.md` inside a plan
> dir). [`AGENTS.md`](AGENTS.md) is canonical; this file is the Claude-Code
> lens over it.

This private `alphazede/bran-dev` repository is the canonical workspace for
BRAN source, planning, and research. The public `alphazede/bran` repository is
a downstream product export, **not** the place to develop or manually edit
BRAN. It receives approved exported snapshots only through pull requests. Use
`$use-bran` for architecture, terminology, and knowledge questions when the OKF
overlay is present.

## Repository map

- `crates/` — Rust source for Core, CLI, and TUI.
- `schemas/`, `fixtures/`, `examples/`, `benches/` — contracts and evidence.
- `docs/integrations/` — public-compatible integration guidance.
- `docs/plans/` — internal BRAN plans (follow its path-specific `AGENTS.md`).
- `docs/submissions/` — private research/submission evidence; never copy into a
  public repo without an explicit scrub and owner approval.
- `skill/use-bran/` — the public agent-facing BRAN skill.
- Arena harness lives separately at `/home/spectre/alphazede/agentic-eval-arena`
  — keep harness implementation and hidden evaluation material there.

## Working rules (full detail in `AGENTS.md`)

1. Make BRAN source changes **here**; use `alphazede/bran` only for approved
   pull-request syncs of exported snapshots.
2. Keep deterministic scanning, ranking, packets, and offline operation usable
   without a provider account.
3. Never add credentials, raw auth state, private corpora, hidden grader truth,
   or unsanitized provider traces. No AI model coauthor lines in commits.
4. Keep requested capability separate from effective/attested capability;
   unavailable behavior must remain visible.
5. Treat public export, public-repository sync, release, tag creation, and
   publication as separate owner-authorized actions.
6. Preserve unrelated user changes. Remove a temporary branch/worktree only
   after proving it is clean and reachable from its integration branch.

## Validation

Use the narrowest relevant test while working. Normal integrated check:

```sh
./tools/cutover/publish-hygiene.sh
./tools/ci/check.sh --fast
```

Build a packaged release binary with `build/build-pinned.sh`, not a bare
`cargo build --release`; see [`build/README.md`](build/README.md).

The publish-hygiene command uses the shared `Alphazedehq` implementation and
derives BRAN's public files from `public-export.json`; any reported
classification metadata blocks export readiness.

Use `./tools/ci/check.sh --full` only when the change affects the full release,
security, conformance, or performance surface. Do not install missing tools or
run live provider evaluations merely to satisfy a local source change.

Public snapshots are governed by `public-export.json` and produced only from a
clean committed source with `python3 tools/ci/public_export.py snapshot`. Use
the tool's `check` command against the public checkout before any sync or
release action; it fails closed on content, mode, receipt, worktree, or remote
drift. Inspect every exported file for private or sensitive data, then run the
hygiene gate. The source is a reviewed committed bran-dev revision and the
checked snapshot was produced by the approved exporter. Push that committed
checked snapshot to a generic non-default branch. Open a draft PR targeting
public main. The PR body must name the exact bran-dev source commit, the exact
public export commit, and validation evidence. Public `main` is protected and
cannot be pushed to directly. Wait for the required fast and CodeQL checks to
pass. Owner review of the exact exported diff and separate authorization to
merge are required; only then merge it with a merge commit. Never squash-merge
an export: squashing discards the reviewed export commits and rewrites the
published snapshot into a commit no reviewer approved. The exporter does not
open pull requests.


## Fixable review findings

Never pass or accept `ACCEPT_WITH_FINDINGS` while a fixable bug remains. If an actionable review finding can be repaired without causing a regression or violating the approved contract, the verdict is `REPAIR_REQUIRED`; fix it in the authorized repair round and rerun deterministic verification. `ACCEPT_WITH_FINDINGS` is reserved for owner-approved residual risk or a finding that cannot be repaired within the approved contract without causing a regression.
