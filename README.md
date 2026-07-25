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

BRAN helps you understand and validate a repository locally. Use the headless
`bran` command in scripts and agent workflows, or open the optional terminal
interface to browse. Scanning, focused evidence packets, validation, and
offline browsing work without an agent account.

## Build and try it

Run the fast checks:

```sh
./tools/ci/check.sh --fast
```

Try a quick smoke test from the repository root:

```sh
cargo run --quiet --bin bran -- smoke
```

The command prints a versioned JSON response. Start the TUI with:

```sh
cargo run --quiet --bin bran -- tui
```

On first launch, BRAN shows the requested and available settings for offline
mode, SQZ, connected agents, voice, history, and saved chats. If a capability
is unavailable, BRAN says so instead of pretending it worked. The default setup
is offline, read-only, and keeps no conversation history.

To cap a connected task, set `tokens=N`. BRAN treats this as a requested host
limit until the connected adapter confirms enforcement. Leaving it unset does
not block connected work or imply that a token limit is enforced. Version 2
settings migrate the old numeric default to `unset`; set `tokens=N` again if
you want an explicit limit. The separate 65,536-byte answer limit protects
storage. It is not a token limit or token-usage measurement.

During onboarding, you can choose `agent=`, `model=`, and `reasoning=` values,
including `reasoning=max`, for the current TUI session. You can also choose
`retention=none|structured|saved`. These options control BRAN's existing
history and saved-chat behavior. They never accept credentials or change
global configuration, and provider-side conversation retention remains
disabled.

After onboarding, inspect local readiness without contacting an account:

```sh
bran doctor --onboarding
bran doctor --agent
bran agents list
```

Both doctor modes are read-only. Their JSON output shows unavailable
capabilities and attestation details, and confirms that they made no provider,
authentication, or network calls. `bran doctor --agent` continues to return
validation status until the connected runtime and host attestation are active,
even when `local_setup_ready` is true. See
[Agent setup](docs/integrations/agent-setup.md) for the two supported setup
journeys, reasoning and tool recipes, no-session operation, and the offline
return check. To let an external agent host call BRAN, install the instructions
in [`skill/use-bran`](skill/use-bran/SKILL.md).

Connected tasks require a valid project-local `.bran/settings.conf` with
`profile=connected-agent`. Set `BRAN_AGENT_PROFILE`, `BRAN_AGENT_PROVIDER`,
`BRAN_AGENT_MODEL`, `BRAN_AGENT_REASONING`, and `BRAN_AGENT_ACCOUNT_REF` to
describe the agent connection.
`BRAN_EXTERNAL_HOST_EXECUTABLE`, `BRAN_EXTERNAL_HOST_SHA256`, and
`BRAN_SQZ_EXECUTABLE` identify the local adapters. The external host timeout is
30 seconds by default; set `BRAN_EXTERNAL_HOST_TIMEOUT_SECONDS` to a whole
number from 1 through 600 for a slower call.

These values are references, not credentials. BRAN has no API-key flag and
never copies credentials. It validates every value and converts the account
reference into an opaque, one-way handle before creating requests, receipts,
diagnostics, or `agents list` output. The raw environment value is never
echoed.

The approved SQZ 1.1.1 digest identifies the verified platform artifact.
Platforms without that exact artifact report connected SQZ as unavailable.
`bran packet` also honors project `sqz=true` without contacting a model or
provider, and returns the post-policy packet with a complete SQZ receipt. When
SQZ is off, BRAN makes no SQZ process call. When it is on, BRAN fails visibly if
the executable, identity, fidelity, DLP, or output contract is invalid or
unavailable.

Settings alone never give an agent permission to run. Add
`--trust-current-root` to each connected `bran -p` call, or enter
`trust-current-root` for the current TUI session. BRAN scans the repository,
builds a bounded evidence packet, applies the configured SQZ policy, and then
calls the configured host. It stores completed results and lossless artifacts
under the IDs in `receipt.stored_result_ref`. Storage is limited by item count,
total bytes, and TTL, and remains separate from conversation history. Run
`bran get <receipt.result_id>` to retrieve the decoded answer and its citations.

## Releases

BRAN does not have a published release yet. When releases begin, each version
will use an exact `bran-vX.Y.Z` tag. Downloads will be available under:

```text
https://github.com/alphazede/bran/releases/download/bran-vX.Y.Z/
```

Each release will include these five platform archives:

- `bran-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
- `bran-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz`
- `bran-vX.Y.Z-x86_64-apple-darwin.tar.gz`
- `bran-vX.Y.Z-aarch64-apple-darwin.tar.gz`
- `bran-vX.Y.Z-x86_64-pc-windows-msvc.zip`

Each release will also include `SHA256SUMS`, `SHA256SUMS.sig`, and
`bran-release-manifest.json`.

- `bran-release-manifest.json` records release provenance.
- SBOMs are not yet part of the release workflow.
- Release notes belong to the tagged release. Installation and downloads use
  exact tags rather than `latest`.

To install a specific release with Cargo:

```sh
cargo install --git https://github.com/alphazede/bran --tag bran-vX.Y.Z --locked bran-cli
```

## License

BRAN is available under your choice of the [Apache License 2.0](LICENSE-APACHE)
or the [MIT License](LICENSE-MIT).
