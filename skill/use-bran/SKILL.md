---
name: use-bran
description: Use BRAN to locate and cite repository evidence before repository-wide work or when asked for BRAN, OKF metadata, source selection, or context packets. Do not use for unrelated questions or already-bounded single-file work.
---

# Use BRAN

BRAN locates and cites repository evidence. It does not decide or implement the
outer task; the outer agent owns all decisions and changes.

1. Default to one read-only packet: `bran packet <repo-root> "<request>"`.
2. Use `bran query <repo-root> "<request>"` only for a focused follow-up the
   packet did not answer.
3. Use connected `bran -p` only when a connected profile is configured and the
   bounded current repository is explicitly trusted. Keep it read-only and ask
   only for evidence location, contents, cross-file context, and citations.
4. Read the entire result. Report failures and unavailable fields honestly,
   preserve provenance and citations, label estimates as estimates, and never
   invent results or compression.
5. Continue the outer task using the cited evidence; keep decisions and
   implementation with the outer agent.

Run `bran -h` for command and option help. Never pass credentials on the command
line.
