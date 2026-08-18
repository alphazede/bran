---
type: product-readme
title: BRAN
tags:
  - public
  - developer
resource: https://github.com/alphazede/bran
---

# BRAN — deterministic code search and context packets for LLM agents

**BRAN is a local-first Rust CLI for deterministic code search: it ranks a
repository offline and assembles a bounded context packet, so AI agents get the
right files without searching for them.** No embeddings and no index server. It
runs fully offline, or connected to a model you choose — an API key is optional
and unused by default.

[![CI](https://github.com/alphazede/bran/actions/workflows/bran-fast.yml/badge.svg)](https://github.com/alphazede/bran/actions/workflows/bran-fast.yml)
![Rust](https://img.shields.io/badge/rust-stable-orange)
![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)

## Why not just grep?

`rg` answers "which lines contain this string." An agent asking "where is
authentication handled?" gets an unranked wall of matches and burns tokens
sorting it. BRAN answers "which files are authoritative for this question,"
ranked, with the reason attached.

| | ripgrep / grep | Embedding RAG | BRAN |
|---|---|---|---|
| Ranking | none | similarity | declared authority + path + body + metadata |
| Determinism | yes | no | yes — identical input, identical output |
| Needs a model | no | yes | optional — works either way |
| Needs an index server | no | usually | no |
| Reports a miss | n/a | rarely | yes, explicitly |
| Offline | yes | rarely | yes |

## Install

Prebuilt archives for Linux, macOS, and Windows are attached to each release:

```sh
https://github.com/alphazede/bran/releases/download/bran-v0.1.0/
```

Or build from source:

```sh
cargo install --git https://github.com/alphazede/bran --tag bran-v0.1.0 bran-cli
```

## Quickstart

Rank the sources for a question:

```sh
bran query . "valid_sha256" | jq '{status, source_rankings: .data.source_rankings[:3], metrics}'
```

```json
{
  "status": "ok",
  "source_rankings": [
    { "rank": 1, "locator": "crates/bran-core/src/agent/runtime.rs",       "match_reason": "exact:body" },
    { "rank": 2, "locator": "crates/bran-core/src/agent/delegate.rs",      "match_reason": "exact:body" },
    { "rank": 3, "locator": "crates/bran-core/src/adapters/connected.rs",  "match_reason": "exact:body" }
  ],
  "metrics": {
    "candidate_source_bytes": 2702707,
    "selected_source_bytes": 133660,
    "context_bytes_avoided": 2569047,
    "estimated_tokens": 33415
  }
}
```

That is 2.7 MB of candidate sources narrowed to 134 KB. Trim the output with
`jq` so BRAN saves context instead of consuming it.

## A miss looks like a miss

An agent cannot tell a good answer from a confident wrong one. So when a
high-specificity entity has no match, BRAN returns nothing and says why,
instead of padding the result with files that matched the generic words around
it:

```sh
bran query . "nonexistent-collector-xyz"
```

```json
{
  "status": "ok",
  "source_rankings": [],
  "warnings": ["unmatched_query_terms: nonexistent-collector-xyz"]
}
```

Command success is not evidence coverage. Empty results are a feature.

## Commands

| Command | Purpose |
|---|---|
| `bran query <root> <request>` | Rank the sources for a request |
| `bran packet <root> <request>` | Assemble a bounded context packet |
| `bran check <root> <profile>` | Validate against `okf-v0.1`, `okf-v0.2`, or `bran-strict` |
| `bran maintain <propose\|apply\|revalidate>` | Bounded repair under explicit authority |
| `bran tui` | Browse the repository offline |
| `bran doctor --onboarding\|--agent` | Read-only local readiness check |
| `bran get <result-id>` | Retrieve a stored result |

Every command emits versioned JSON. `query`, `packet`, `check`, and `tui` need
no account and make no network calls.

## The schema layer: OKF

BRAN ranks on declared authority, not guesswork. That declaration is the
Open Knowledge Format, or [OKF](https://github.com/GoogleCloudPlatform/knowledge-catalog),
Google's open spec. YAML frontmatter turns ordinary markdown into a queryable knowledge graph:

```yaml
---
type: Concept
title: Ranking precedence
status: active
tags: [developer]
resource: https://github.com/alphazede/bran
---
```

`type` is the only required field. Optional families cover provenance
(`sources`, `usage_window`), trust (`generated`, `verified`), and lifecycle
(`status`, `stale_after`).

```sh
bran check . okf-v0.2
```

```json
{
  "selected_profile": "okf-v0.2",
  "selected_passed": true,
  "okf_compatibility": { "profile": "okf-v0.1", "status": "pass" },
  "okf_v0_2":          { "profile": "okf-v0.2", "status": "pass" },
  "bran_strict":       { "profile": "bran-strict", "status": "pass" }
}
```

All three results are reported independently and only the selected profile
controls the exit code, so OKF conformance is never confused with house rules.
`okf-v0.1` remains a supported selectable compatibility profile. `okf-v0.2` is
additive and does not replace it. BRAN producer extensions (`okf_status`,
`freshness`, `public_boundary`) stay valid and are not silently renamed to
upstream `status` or `stale_after`.

## Export the knowledge graph

`bran_core::export` emits an Obsidian-compatible vault from the graph, so a
repository can be browsed visually. See
[`examples/obsidian/usage.rs`](examples/obsidian/usage.rs).

## Offline or connected — both are first class

BRAN runs either way, and the same commands work in both modes.

**Offline** is the default and needs no account, no key, and no network.
Scanning, ranking, packets, validation, and the TUI are complete on their own —
this is not a trial tier.

**Connected** adds a model that reads what BRAN selected and answers with
citations. It is opt-in per invocation.

### Where the models go

If you connect a model, put it in the middle tier rather than the top:

1. **BRAN** decides *which* files matter. Deterministic, offline, free.
2. **A fast or local model** — a Flash-class model, or something on your own
   hardware — reads those files and condenses them.
3. **The frontier model** receives that clean, bounded context and reasons.

The expensive model should never be the thing hunting through a repository.
Retrieval is a search problem, not a reasoning problem.

### Connect a model

BRAN has **no API-key flag and never copies credentials**. You point it at a
profile; the account reference becomes an opaque one-way handle before any
request, receipt, or diagnostic is written.

1. Create a project-local `.bran/settings.conf` with `profile=connected-agent`.
2. Describe the connection through the environment — a reference, not a secret:

   ```sh
   export BRAN_AGENT_PROFILE=<profile>
   export BRAN_AGENT_PROVIDER=<provider>
   export BRAN_AGENT_MODEL=<fast-or-local-model>
   export BRAN_AGENT_REASONING=medium
   export BRAN_AGENT_ACCOUNT_REF=<reference>
   ```

3. Check what is actually available before relying on it:

   ```sh
   bran agents list
   bran doctor --agent
   ```

4. Run a bounded, grounded request:

   ```sh
   bran -p --agent <profile> --reasoning medium --tools read,search \
     --trust-current-root "which module owns frontmatter validation?"
   ```

`--tools read,search` limits it to repository read and search. `--no-session`
disables retention. `--offline` forces the deterministic profile even when a
profile is configured, so you can always fall back:

```sh
bran -p --agent <profile> --offline --no-session "offline return proof"
```

If a capability is unavailable, BRAN says `unavailable` rather than pretending
it worked. Requested and effective capability are always reported separately.

## Use it with an agent

Giving an agent access is not enough. Without a reminder it reaches for
built-in search, which is always available and never reports `unavailable`.
Install [`skill/use-bran`](skill/use-bran/SKILL.md) for the agent-facing
instructions, and see [Agent setup](docs/integrations/agent-setup.md) for hook
recipes, reasoning and tool configuration, and the offline return check.

## Releases

Each release uses an exact `bran-vX.Y.Z` tag. Downloads live under
`https://github.com/alphazede/bran/releases/download/bran-vX.Y.Z/` — exact tags
only, never `latest`.

Five platform archives are published:

- `bran-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
- `bran-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz`
- `bran-vX.Y.Z-x86_64-apple-darwin.tar.gz`
- `bran-vX.Y.Z-aarch64-apple-darwin.tar.gz`
- `bran-vX.Y.Z-x86_64-pc-windows-msvc.zip`

Alongside them: `SHA256SUMS`, `SHA256SUMS.sigstore`, and
`bran-release-manifest.json`, which records release provenance.

Signing is Sigstore keyless via GitHub OIDC — there is no long-lived key to
manage or leak. Verify a download:

```sh
cosign verify-blob SHA256SUMS \
  --bundle SHA256SUMS.sigstore \
  --certificate-identity "https://github.com/alphazede/bran/.github/workflows/release.yml@refs/tags/bran-v0.1.0" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com"
sha256sum -c SHA256SUMS --ignore-missing
```

SBOMs are not yet part of the release workflow.

## FAQ

**Does BRAN need an API key?** No. A key or auth session is entirely optional.
Scanning, ranking, packets, validation, and the TUI are fully offline and make
no network calls. A connected mode exists and is opt-in.

**If I add a model, which one?** A fast or local one. Use a Flash-class or
self-hosted model to read the files BRAN selected and hand the condensed result
to your frontier model. See [Where the models go](#where-the-models-go).

**Does it replace RAG?** For code and docs, often yes. It separates retrieval
from reasoning, so a cheap deterministic step feeds the expensive model.

**What languages does it support?** Ranking is language-agnostic; it operates on
paths, document bodies, structure, and OKF metadata.

**Why did my query return nothing?** Because nothing matched. Check the
`unmatched_query_terms` warning — that is BRAN refusing to guess.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
