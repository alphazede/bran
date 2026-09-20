---
type: bug-report
title: BRAN advisory hook fires on command tokens rather than target paths
okf_status: draft
status: draft
tags:
  - developer
  - internal
freshness: "2026-07-25"
resource: https://github.com/alphazede/bran-dev/blob/main/docs/bugs/2026-07-25-hook-fires-on-command-tokens-not-target-paths.md
public_boundary: private
---

# BRAN advisory hook fires on command tokens rather than target paths

Status: open, needs investigation. Found 2026-07-25 during an unrelated task in
`bearing-dev`.

## Symptom

The `PreToolUse:Bash` BRAN advisory fires whenever a command *string* contains
`grep` or `find`, regardless of what the command actually reads. It then names
the session's current working directory as the search target, even when no
argument points anywhere near a BRAN-covered repository.

## Observed false positives

All four fired the advisory. None touched a repository.

| Command | What it actually read |
|---|---|
| `gh auth status 2>&1 \| grep -i "scopes"` | GitHub CLI token scopes |
| `pgrep -a -f "chrome" \| grep -o "user-data-dir=[^ ]*"` | process table |
| `ls ~/.config/google-chrome/*/Extensions/` | a home-directory config path |
| `find "$e" -name manifest.json` inside `~/.config/google-chrome` | Chrome extension manifests |

Each time the advisory read:

> A raw grep repository search is about to run in
> `/home/spectre/alphazede/Alphazedehq/bearing-dev`, which has native BRAN
> coverage.

The shell's cwd was `bearing-dev/docs/plans`, so the hook reported the cwd as
the target. The commands' actual targets were `~/.config`, the process table,
and `gh` output.

## Hypothesis

Detection appears to be a token scan of the command string for `grep` / `find`,
with the target inferred from cwd rather than parsed from the command's path
arguments. Things to confirm:

1. Is the trigger a substring match on the command text? Does `grep` appearing
   only in a pipeline stage (never as the repo-reading step) still match?
2. Is the reported target ever derived from the command's actual arguments, or
   always from cwd?
3. Should a command whose path arguments all resolve outside any BRAN-covered
   root suppress the advisory entirely?
4. Do other tools in the same class (`rg`, `ls`, `cat`, `awk`) trigger it, and
   should they?

## Why this matters

The advisory is a guard. A guard that fires on process listings and CLI auth
output trains the reader to skim past it, which is precisely when it will be
ignored on the call that genuinely bypasses BRAN. Precision is the whole value.

## Related

Same root shape as
[`2026-07-25-query-ranking-favors-path-tokens-over-content.md`](./2026-07-25-query-ranking-favors-path-tokens-over-content.md):
matching on a surface token instead of on what the target actually is. Worth
checking whether both share a matching helper.

## Reproduction

From any cwd inside a BRAN-covered repository, run a command that pipes
unrelated output through `grep`:

```sh
gh auth status 2>&1 | grep -i scopes
```

The advisory fires and names the cwd as the search target.
