---
type: product-readme
title: BRAN
tags:
  - public
  - developer
resource: https://github.com/alphazede/bran
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

## Make an agent actually use BRAN

Giving an agent access to BRAN is not enough. Without a timely reminder, an
agent usually reaches for built-in search tools because they are always
available and never report `unavailable`. In one dev-node session on
2026-07-25, a BRAN banner appeared on every turn while the agent still used raw
search dozens of times without invoking BRAN.

Two structural issues caused that behavior:

1. A session-start or prompt-time reminder is stale by the time the agent forms
   a search. The reminder needs to run on the search tool call itself.
2. BRAN requires a native `.bran/policy.yaml` at the repository root. Without
   one, `bran_status: unavailable` is the correct result, and ordinary
   repository discovery is the correct fallback. Coverage is a precondition
   for adoption.

### Add coverage before reminders

Audit target repositories for `.bran/policy.yaml` before wiring hooks. Do not
nag an agent toward BRAN in a repository where it is unavailable; include the
known coverage gaps in local injected context instead.

When existing public-facing Markdown cannot carry BRAN classification
frontmatter, keep the native index private, classify existing documents with
`legacy_baseline`, and exclude private or generated state with `.branignore`.
This provides repository coverage without changing the published Markdown.

### Remind the agent at search time

Use a `PreToolUse` command hook and match the tool names emitted by the actual
harness. Tool vocabularies differ:

| Harness | Search path | Matcher |
|---|---|---|
| Claude Code | Native `Grep`/`Glob`, plus raw search through its shell tool | `Grep\|Glob\|Bash` |
| Codex | Unified shell commands such as `rg`, `grep`, `git grep`, and `find` | `Bash` |

If a Codex surface exposes a dedicated search function, match its reported tool
name as well. Use `/hooks` to inspect the active hook sources and observed tool
names instead of assuming that another harness's matcher vocabulary applies.
For a broad matcher such as `Bash`, the script should inspect `tool_input` and
stay silent unless the command is a repository search. Resolve native coverage
from the call's current working directory on every invocation instead of
hard-coding a list of covered or uncovered repositories; that list becomes
wrong as soon as a policy is added or removed.

Keep the hook in a script file and reference it by absolute path. Both
harnesses send a JSON payload on stdin. A Codex `PreToolUse` reminder returns an
event-specific JSON object like this:

```json
{
  "systemMessage": "BRAN_SEARCH_ALERT: raw rg repository search requested in a BRAN-covered checkout.",
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "additionalContext": "Use the verified BRAN binary for this repository-knowledge search."
  }
}
```

Codex treats non-empty hook stdout as JSON. Plain text on stdout causes an
`invalid ... JSON output` hook failure. Exit successfully with no output when
the hook does not apply.

Codex also requires review of every new or changed non-managed hook definition.
Open `/hooks`, inspect the source and exact command, and trust it; until then,
Codex intentionally skips the changed hook. Test the stored command first, and
start a fresh session if an already-running session still has the previous
matcher set loaded. Do not use a trust-bypass flag as normal installation
guidance.

### Make raw-search fallback visible

A search hook sees the raw tool call but cannot reliably prove that a BRAN
query succeeded earlier in the conversation. Treat every matching raw search
as an observable fallback: return a top-level `systemMessage` beginning with a
stable marker such as `BRAN_SEARCH_ALERT`, and use `additionalContext` to make
the agent report whether BRAN was used and why the fallback is still needed.
This lets an owner find adoption loopholes without blocking legitimate
diagnostic searches or maintaining fragile per-session state.

In a covered repository, the alert should require `bran_status` plus a bounded
fallback reason. In an uncovered repository, it should explicitly report
`bran_status: unavailable` and allow ordinary discovery. Stay silent for
unrelated shell commands so the warning remains useful instead of becoming
background noise.

The injected context should tell the agent to:

- Resolve the pinned BRAN binary and verify its SHA-256 against the release pin.
- Use ordinary discovery immediately when the repository has no native policy.
- Trim query output so BRAN saves context instead of consuming it.
- Report `bran_status` as `hit`, `miss`, `stale`, `conflict`, or `unavailable`.

For example, keep the highest-ranked sources and top-level metrics while
dropping the duplicate provenance payload:

```sh
bran query <repo-root> "<request>" |
  jq '{status, source_rankings: .data.source_rankings[:8], metrics, warnings, failures}'
```

### Test both hook directions

Do not assume that a stored hook configuration is valid. Inline shell embedded
in JSON is easy to damage through escaping, so prefer an executable script and
test the command exactly as stored:

```sh
echo '{"hook_event_name":"PreToolUse","tool_name":"Grep","tool_input":{"pattern":"x"}}' |
  /absolute/path/bran-search.sh
```

Confirm that a matching payload produces valid JSON with non-empty
`additionalContext`. Then send an unrelated payload and confirm the hook emits
nothing. A noisy hook that fires on every command will eventually be disabled.

In the 2026-07-25 dev-node observation, one repository query narrowed 5.9 MB of
candidate sources to 411 KB, placed the correct file at rank 2, and reported
about 102,000 estimated tokens of context avoided. This is an observed result,
not a general performance guarantee, and it only helps when the hook fires and
the returned JSON is trimmed.

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
