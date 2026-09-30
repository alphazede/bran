---
type: integration-guide
title: Agent setup
tags:
  - public
  - developer
---

# Agent setup

BRAN works offline without an account. Connecting an agent is optional and does
not change the repository index. Install an exact sealed BRAN release, then copy
`skill/use-bran` into the skill directory used by the agent host. If the host
uses a nonstandard location, set `BRAN_SKILL_PATH` to the copied `SKILL.md` only
for the doctor invocation. BRAN reports whether it can discover the file; it
does not modify global host settings.

Credentials belong in the host's credential store or documented environment
input. BRAN has no command-line credential option, and setup output must not
contain credential material.

## Journey 1: Offline repository guide

1. Run `bran tui`, choose the Quick flow, and review the resolved configuration.
   The safe default is offline, read-only, SQZ requested, voice and saved chat
   off, and zero-conversation retention. Uninstalled features remain
   `unavailable`.
2. Apply the settings, then run `bran doctor --onboarding`. Check `ready`,
   `settings_status`, the requested/effective capability states, and the offline
   return proof. The diagnostic must report zero provider, auth, and network
   calls.
3. Use deterministic retrieval directly:

   ```sh
   bran packet <repo-root> "<request>"
   bran query <repo-root> "<request>"
   bran check <repo-root> okf-v0.1
   bran check <repo-root> okf-v0.2
   bran check <repo-root> bran-strict
   ```

No generated answer is expected. Preserve provenance and label byte-derived
token counts as estimates. `okf-v0.1` remains supported; `okf-v0.2` is
additive. Only the selected profile controls the exit code.

## Journey 2: Optional connected task and removal

1. Reopen TUI settings and select Connected Agent. Review every unavailable or
   locked choice before applying. The connected-task total-token ceiling is
   unset unless you configure one with `tokens=N`. A configured ceiling is not
   proven effective until the host adapter attests enforcement; otherwise the
   doctor and task receipts must say `unavailable`. Leaving the ceiling unset
   does not block connected execution or claim token enforcement.
2. Inspect registered profiles and local readiness:

   ```sh
   bran agents list
   bran doctor --agent
   ```

   `doctor` checks local CLI and skill discovery, persisted workspace policy,
   SQZ capability, a deterministic packet round trip, and host attestation. It
   does not initialize a provider, network, or credential store just to probe a
   capability. Local setup may be ready while connected execution remains
   unavailable; in that state the command exits with validation status rather
   than claiming overall readiness.
3. Choose a profile from `agents list`. Reasoning accepts exactly
   `off|minimal|low|medium|high|xhigh`; tools are limited to `read,search`:

   ```sh
   bran -p --trust-current-root --agent <profile> --reasoning medium --tools read,search "review this change"
   bran -p --trust-current-root --agent <profile> --reasoning low --no-session "find the owning specification"
   ```

   External host calls time out after 30 seconds by default. Set
   `BRAN_EXTERNAL_HOST_TIMEOUT_SECONDS` to a whole number from 1 through 600
   when a reviewed host needs more time; invalid values fail setup closed.

   Read the whole receipt. Requested and effective profile, model, reasoning,
   and tool policy are distinct; an unattested effective value stays
   `unavailable`. `--no-session` requests no conversation session, while result
   receipts and explicitly referenced artifacts remain governed by their own
   bounded retention rules.
   A complete receipt exposes the canonical result ID under
   `stored_result_ref.result_id`; retrieve it before its bounded TTL expires:

   ```sh
   bran get <receipt.result_id>
   ```

### Grounded result contract

The external host remains provider-neutral. For a grounded `bran -p` request,
its result frame must emit aligned repeated values for `claim_id`, `claim_text`,
`claim_material`, `claim_locator`, `claim_content_digest`, and `claim_support`.
Every claim is material, `claim_text` and `claim_support` must be identical exact
text or a symbol copied from the current cited file, and `answer` must contain
the ordered claim texts separated only by newlines. `claim_locator` must also be
present as a normal `citation`, while `claim_content_digest` must echo the
SHA-256 supplied in BRAN's bounded packet. `claim_support` must be at least 12
bytes after trimming; shorter spans are rejected before verification.

Before storing a result, BRAN reopens every uniquely cited regular file at most
once, rejects symlinked or escaping paths, recomputes its SHA-256, and checks the
exact support bytes. Missing claims, invented symbols, stale files or digests,
unattested execution identity, degenerate support spans, and answer/claim
mismatches fail closed as an incomplete receipt. Claim verification adds bounded
local file I/O; it does not make another model call. Ungrounded provider calls
may omit the claim fields.

What this contract does and does not prove. Support is verified by exact
substring existence against the current file, with no uniqueness or position
requirement beyond the minimum length. A validated claim therefore proves that
the quoted span **exists verbatim in the cited file at the digest BRAN
supplied** — that is, the claim is not fabricated and not stale. It does not
prove that the span is the *relevant* occurrence, nor that it answers the
question asked. Treat a grounded result as evidence against fabrication, not as
a correctness or attribution guarantee.

4. Disable Connected Agent in TUI settings, or prove the same boundary directly:

   ```sh
   bran -p --agent <profile> --offline --no-session "offline return proof"
   bran packet <repo-root> "<request>"
   ```

   The first command must return a typed incomplete offline receipt, not a
   generated answer. The following packet remains deterministic and must not
   initialize provider, auth, or network ports.

## Exact symbol navigation

`bran packet` and `bran query` can attach exact definitions, references, and
implementations to ranked sources. Agents keep using BRAN for this; they do not
need a second retrieval tool or repository-wide grep for normal discovery.

BRAN reads an existing [SCIP](https://github.com/sourcegraph/scip) index at
`index.scip` in the repository root. It never generates, downloads, or updates
the index. Produce it with a SCIP indexer for the language, for example
`rust-analyzer scip .` for a Rust workspace, and regenerate it after source
changes. Any language in the index is navigable. The index must be a regular
file of at most 256 MiB, not a symlink. The repository scanner lists the binary
index as an `UnsupportedInput` warning, as it does for other binary files. A
malformed index, including bad protobuf framing, an empty symbol, or an
occurrence range that is reversed, has more than four coordinates, or exceeds
the protobuf `int32` range, is `unavailable`.

Every run of letters, digits, `_`, `+`, `-`, or `$` in the request is a
candidate name, with no length or stop-word filter; a symbol whose name equals
a candidate, ignoring ASCII case, is selected. Each ranked source whose file
holds evidence for a selected symbol gains a `symbols` array:

```json
{
  "locator": "src/render.rs",
  "match_reason": "partial:body",
  "symbols": [
    {
      "role": "implementation",
      "name": "Html",
      "qualified_name": "render::Html",
      "kind": "struct",
      "id": "rust-analyzer cargo demo 0.1.0 render/Html#",
      "source": "scip",
      "span": {"start_line": 5, "end_line": 5},
      "implements": "rust-analyzer cargo demo 0.1.0 render/Renderer#"
    }
  ]
}
```

`role` is `definition`, `reference`, or `implementation`; an implementation is
the definition of a symbol whose SCIP relationship marks it as implementing the
selected one. Lines are one-based and inclusive. A packet adds the same facts to
the ranked source's payload as one `scip_symbols:` line. Symbol evidence never
changes ranking, scores, `match_reason`, or authority. At most 64 items are
attached per result, in rank order; one query selects at most 1,024 symbols and
1,024 implementation pairs and examines at most 1,048,576 candidate facts.

Every result also carries `data.symbol_navigation`
(`schemas/symbol-navigation.schema.json`, version `1.0.0`):

| Field | Values |
| --- | --- |
| `outcome` | `hit` (the result keeps evidence), `miss` (usable index, no evidence for the sources the result keeps), `unavailable` |
| `truncated` | `true` when the index holds more evidence than the result keeps, including sources a packet dropped |
| `scip.status` | `available`, `partial`, `stale`, `unavailable` |
| `scip.reason` | `null`, `freshness_unverified`, `index_stale`, `index_missing`, `index_unreadable`, `multi_root_unsupported`, `native_policy_unavailable` |
| `lsp.status` | always `unavailable` (`not_implemented`) |

Freshness is proven only when every indexed document records its source text
and that text equals the scanned file; then the status is `available`. If a
document records no text or is outside the scan, the status is `partial` and
the spans should be confirmed before use. If any recorded text differs from the
file, the status is `stale` and no symbol evidence is returned. A missing,
unreadable, or malformed index is `unavailable`, never a guess. `query
--add-dir` reports `multi_root_unsupported`, and a query on a root without
native policy reports `native_policy_unavailable`. With no index, every other
part of the result is unchanged.

## Reading unavailable results

Unavailable is a result, not a silent fallback. Keep using offline retrieval,
repair the missing local setup, or connect a separately reviewed host adapter.
Do not claim effective reasoning, SQZ, token enforcement, or generated output
from requested settings alone.
