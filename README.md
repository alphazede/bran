---
type: product-readme
title: BRAN
okf_status: active
tags:
  - public
  - developer
freshness: "2026-07-25"
resource: https://github.com/alphazede/bran
public_boundary: public
---

# BRAN

![BRAN seated between two ravens beneath the memory tree](assets/brand/bran-repository-raven.png)

BRAN is a local repository-intelligence engine with a headless `bran` executable
and an optional terminal interface. Deterministic repository scanning, focused
packets, validation, and offline browsing do not require an agent account.

This is the public product surface exported from the private canonical
`alphazede/bran-dev` development repository. Product changes are made and
reviewed in `bran-dev`; `alphazede/bran` contains only the approved public
snapshot. Private plans, submissions, unpublished proposals, agent instructions,
and local `.bran` runtime data are not part of this repository.

## Build and try it

Build and test the current scaffold:

```sh
./tools/ci/check.sh --fast
```

Run the smoke command from the repository root:

```sh
cargo run --quiet --bin bran -- smoke
```

It writes a versioned JSON envelope. Start the TUI with:

```sh
cargo run --quiet --bin bran -- tui
```

First-run onboarding shows requested and effective settings for offline mode,
SQZ, connected-agent mode, voice, structured history, and saved chat. A missing
capability remains visible as unavailable; BRAN does not simulate it. The safe
default is offline, read-only, zero-conversation retention. A connected-task
total-token ceiling is unset until the user configures one with `tokens=N`.
When configured, it remains a requested host limit until a connected adapter
attests enforcement. Leaving it unset does not block connected execution or
claim token enforcement. Loading version 2 settings retires its former numeric
default to `unset`; re-enter `tokens=N` to configure an explicit ceiling.
No output-token cap is synthesized while that ceiling is unset. The separate
65,536-byte connected-answer bound remains a byte-safety limit, not a token
default or token-usage measurement.

Advanced onboarding accepts volatile `agent=`, `model=`, and `reasoning=`
choices (including `max`) for the current TUI process. It also accepts
`retention=none|structured|saved`, mapped to the existing local structured
history and saved-chat controls. These choices never accept credentials or
change global configuration; provider conversation retention remains
zero-conversation.

After onboarding, inspect local readiness without contacting an account:

```sh
bran doctor --onboarding
bran doctor --agent
bran agents list
```

Both doctor modes are read-only. Their envelopes report unavailable capability
and attestation fields explicitly and include zero provider, auth, and network
call metrics. Agent doctor exits with validation status until connected runtime
and host attestation are effective, even when `local_setup_ready` is true. See
[Agent setup](docs/integrations/agent-setup.md) for the two
supported setup journeys, reasoning/tool recipes, no-session operation, and the
offline-return check. Install the public agent instructions from
[`skill/use-bran`](skill/use-bran/SKILL.md) when an external agent host should
call BRAN.

Connected execution additionally requires a valid project-local
`.bran/settings.conf` with `profile=connected-agent`. Configure the
provider-neutral descriptor with `BRAN_AGENT_PROFILE`, `BRAN_AGENT_PROVIDER`,
`BRAN_AGENT_MODEL`, `BRAN_AGENT_REASONING`, and `BRAN_AGENT_ACCOUNT_REF`.
`BRAN_EXTERNAL_HOST_EXECUTABLE`, `BRAN_EXTERNAL_HOST_SHA256`, and
`BRAN_SQZ_EXECUTABLE` identify the local adapters. The external host timeout is
30 seconds by default; set `BRAN_EXTERNAL_HOST_TIMEOUT_SECONDS` to a whole
number from 1 through 600 for a slower call. These values are non-secret
references; BRAN accepts no API-key flag and does not copy credentials.
BRAN validates descriptor values at the public boundary and converts the
account reference into a one-way opaque handle before it reaches host requests,
receipts, diagnostics, or `agents list`; the raw environment value is never
echoed.
The currently approved SQZ 1.1.1 digest identifies the verified platform
artifact. Platforms without that exact approved artifact report connected SQZ
as unavailable rather than claiming cross-platform attestation.
Deterministic `bran packet` also honors project `sqz=true` without contacting a
model or provider. Its envelope contains the actual post-policy packet payload
and a complete SQZ receipt. SQZ-off makes no SQZ process call; SQZ-on fails the
operation visibly if the approved executable, identity, fidelity, DLP, or
output contract is unavailable or invalid.

Repository settings never grant agent authority. Add `--trust-current-root` to
each connected `bran -p` call, or enter `trust-current-root` in the TUI for that
TUI process. BRAN scans the current root, assembles a bounded evidence packet,
applies the configured SQZ policy, and only then calls the configured host.
Completed canonical result bytes and lossless artifacts are stored under the
exact IDs in `receipt.stored_result_ref`, with bounded count, bytes, and TTL;
this is not conversation history. Retrieve the decoded answer and citations
with `bran get <receipt.result_id>`.

## Future release contract

No BRAN release is published by this scaffold. A supported future release must use an exact `bran-vX.Y.Z` tag and direct asset URLs rooted at:

```text
https://github.com/alphazede/bran/releases/download/bran-vX.Y.Z/
```

That release shape requires these five platform archives:

- `bran-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
- `bran-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz`
- `bran-vX.Y.Z-x86_64-apple-darwin.tar.gz`
- `bran-vX.Y.Z-aarch64-apple-darwin.tar.gz`
- `bran-vX.Y.Z-x86_64-pc-windows-msvc.zip`

The fixed release assets are the five archives, `SHA256SUMS`, `SHA256SUMS.sig`, and `bran-release-manifest.json`.

- Provenance lives in `bran-release-manifest.json`.
- Separate SBOM evidence is unavailable in this readiness workflow and deferred to an owner-authorized real release; it is not an extra fixed asset.
- Release notes are release metadata. Use exact tags only: no `latest`.

For a published exact tag, the locked Cargo install form is:

```sh
cargo install --git https://github.com/alphazede/bran --tag bran-vX.Y.Z --locked bran-cli
```

## License

BRAN is licensed, at the recipient's choice, under either the Apache License 2.0 (see [LICENSE-APACHE](LICENSE-APACHE)) or the MIT license (see [LICENSE-MIT](LICENSE-MIT)).
