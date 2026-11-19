---
name: use-bran
description: Use BRAN to locate and cite repository evidence before repository-wide work or when asked for BRAN, OKF metadata, source selection, or context packets. Do not use for unrelated questions or already-bounded single-file work.
---

# Use BRAN

BRAN locates and cites repository evidence. It does not decide or implement the
outer task; the outer agent owns all decisions and changes.

1. Start with a short query: `bran query <repo-root> --brief "<request>"`
   (optional `--top N`, default `8`, max `32`). It returns ranked exact
   paths in `data.results`, one-line reasons, `coverage_basis: "sources"`
   (derived from source rankings without enterprise document evidence), and a
   `result_id`. Consuming repositories should add `.bran/results/` to
   `.gitignore` so brief queries leave `git status` clean.
2. Expand from `<repo-root>` when needed: `bran get <result-id>` returns the
   stored summary, and `bran get <result-id> --full` re-runs the full default
   query (check `warnings` for `"stale: true"` if HEAD changed). When `.bran/`
   is absent or registered-root mode is active, `result_id` is `null` with a
   warning; follow up directly with the full query or packet.
3. For full context, Default to one read-only packet:
   `bran packet <repo-root> "<request>"`, or Use `bran query <repo-root> "<request>"`
   only for a focused follow-up that `--brief` or the packet did not answer.
   In default `bran query`, `selected_locators` stays alphabetical; relevance
   order is `source_rankings` in default `query` and `results` in `--brief`.
   After the primary root, repeat `--add-dir <repo-root>` to ask the same
   question across multiple policy-bearing roots. Multi-root results list
   `requested_roots` in argument order and put `bundle` on every ranked
   source. If any requested root lacks native policy or cannot scan, the whole
   request is refused and only that root argument is named.
4. Use connected `bran -p` only when a connected profile is configured and the
   bounded current repository is explicitly trusted. Keep it read-only and ask
   only for evidence location, contents, cross-file context, and citations.
5. Read the entire result. Treat `data.query_outcome` as the semantic result,
   not command `status`. `grounded` means every extracted query term matched
   repository evidence. `miss` means there is no ranked evidence; empty
   selections are a miss, not a successful answer. `partial_unanchored` means
   some terms did not match — rankings may still be useful but are not a
   complete answer to the named request. Never treat command success or a
   non-empty ranking as full grounding. Report failures and unavailable fields honestly,
   preserve provenance and citations, label estimates as estimates, and never
   invent results or compression.
6. Continue the outer task using the cited evidence; keep decisions and
   implementation with the outer agent.

Exact symbol navigation comes through the same packet and query. When the
repository root holds an existing SCIP index at `index.scip`, a ranked source
may carry `symbols`: each item names a `definition`, `reference`, or
`implementation` with its name, qualified name, kind, exact SCIP `id`,
one-based `span`, and `source: "scip"`. To find where a symbol is defined,
used, or implemented, name it in the request instead of running ad hoc
repository-wide grep. Read `data.symbol_navigation`: `outcome` is `hit`,
`miss`, or `unavailable`, and `scip.status` is `available`, `partial`
(freshness not proven; confirm a span before relying on it), `stale` (no
symbol evidence; the index must be regenerated), or `unavailable`. BRAN never
builds the index, and a symbol hit is evidence, not authority: it never changes
ranking.

Enterprise documents (DOCX, XLSX, PPTX, PDF) surface separately. When the
root holds any, the same packet and query append `data.document_evidence`:
admitted `sources` with their `source_digest` and `fidelity`, ranked anchor
`matches` with `native_locator` and `derivation`, plus typed `refusals` and
`unsupported` formats. All four native adapters are supported; malformed
DOCX and PPTX files now return typed refusals rather than `unsupported`.
A `derivation` of `ocr` means derived text: cite it as
derived, never as byte-derived source text. Document files never appear in
the Markdown rankings, and roots without them return unchanged output. Use
`bran document inspect <repo-root> <path>` to read one document's evidence
envelope without writing anything.

`bran check <repo-root> okf-v0.1`, `okf-v0.2`, and `bran-strict` are
independent selectable profiles. `okf-v0.1` remains supported; `okf-v0.2` is
additive. Only the selected profile controls the exit code.

Run `bran -h` for command and option help. Never pass credentials on the command
line.
