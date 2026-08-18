---
name: use-bran
description: Use BRAN to locate and cite repository evidence before repository-wide work or when asked for BRAN, OKF metadata, source selection, or context packets. Do not use for unrelated questions or already-bounded single-file work.
---

# Use BRAN

BRAN locates and cites repository evidence. It does not decide or implement the
outer task; the outer agent owns all decisions and changes.

1. Default to one read-only packet: `bran packet <repo-root> "<request>"`.
2. Use `bran query <repo-root> "<request>"` only for a focused follow-up the
   packet did not answer. After the primary root, repeat `--add-dir <repo-root>`
   to ask the same question across multiple policy-bearing roots. Multi-root
   results list `requested_roots` in argument order and put `bundle` on every
   ranked source. If any requested root lacks native policy or cannot scan,
   the whole request is refused and only that root argument is named.
3. Use connected `bran -p` only when a connected profile is configured and the
   bounded current repository is explicitly trusted. Keep it read-only and ask
   only for evidence location, contents, cross-file context, and citations.
4. Read the entire result. Treat `data.query_outcome` as the semantic result,
   not command `status`. `grounded` means every extracted query term matched
   repository evidence. `miss` means there is no ranked evidence; empty
   selections are a miss, not a successful answer. `partial_unanchored` means
   some terms did not match — rankings may still be useful but are not a
   complete answer to the named request. Never treat command success or a
   non-empty ranking as full grounding. Report failures and unavailable fields honestly,
   preserve provenance and citations, label estimates as estimates, and never
   invent results or compression.
5. Continue the outer task using the cited evidence; keep decisions and
   implementation with the outer agent.

`bran check <repo-root> okf-v0.1`, `okf-v0.2`, and `bran-strict` are
independent selectable profiles. `okf-v0.1` remains supported; `okf-v0.2` is
additive. Only the selected profile controls the exit code.

Run `bran -h` for command and option help. Never pass credentials on the command
line.
