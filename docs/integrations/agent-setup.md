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
   ```

No generated answer is expected. Preserve provenance and label byte-derived
token counts as estimates.

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

## Reading unavailable results

Unavailable is a result, not a silent fallback. Keep using offline retrieval,
repair the missing local setup, or connect a separately reviewed host adapter.
Do not claim effective reasoning, SQZ, token enforcement, or generated output
from requested settings alone.
