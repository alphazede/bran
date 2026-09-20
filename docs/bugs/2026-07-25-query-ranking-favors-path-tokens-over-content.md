---
type: bug-report
title: BRAN query ranking favors path-token matches over canonical content
okf_status: draft
status: draft
tags:
  - developer
  - internal
freshness: "2026-07-25"
resource: https://github.com/alphazede/bran-dev/blob/main/docs/bugs/2026-07-25-query-ranking-favors-path-tokens-over-content.md
public_boundary: private
---

# Query ranking favors path-token matches over canonical content

Status: open, needs investigation. Found 2026-07-25 while using `bran query`
for ordinary product-knowledge retrieval in two repositories.

## Symptom

`bran query` ranks files whose **path** happens to contain a query token above
the documents that actually answer the question. Content is not what wins. In
both observations below the correct answer lived in a `README.md` that the
query either ranked low or did not select at all.

This is the core retrieval promise, so it matters more than an ordinary
ranking nit: precedence is what BRAN sells over grep. See the product
[README](../../README.md) for the stated ranking promise.

## Observation 1 — canonical README never selected

```sh
bran query <repo>/Alphazedehq/bearing-dev \
  "What is Bearing as a product, who is it for, and what problem does it solve?"
```

Ranking returned (all eight):

| Rank | Locator | match_reason | confidence |
|---|---|---|---|
| 1 | `test/bearing-store.test.ts` | `exact:path` | 50 |
| 2 | `src/store/bearing-store.ts` | `exact:path` | 50 |
| 3 | `plugin-skills/bearing/SKILL.md` | `exact:path` | 50 |
| 4 | `.github/workflows/bearing-quality.yml` | `exact:path` | 40 |
| 5 | `.github/workflows/bearing-publish.yml` | `exact:path` | 40 |
| 6 | `.bran/tags.md` | `partial:title` | 50 |
| 7 | `.bran/index.md` | `partial:title` | 50 |
| 8 | `skills/set-bearings/SKILL.md` | `partial:title` | 50 |

`README.md` does not appear in `selected_locators` at all — yet it answers the
question verbatim in its opening line. A test file outranked it because the
path contains the token `bearing`.

Note also that `.bran/index.md` and `.bran/tags.md` carry `canonical: 1` and
`active: 3` and still lost to files scoring `exact: 1` with no canonical or
active signal.

## Observation 2 — semantically unrelated top three

```sh
bran query <repo>/Alphazedehq/bran-dev \
  "Where are bugs, defects, and known issues recorded in this repository?"
```

| Rank | Locator | match_reason |
|---|---|---|
| 1 | `benches/repository_scan.rs` | `exact:path` |
| 2 | `schemas/repository-scan-snapshot.schema.json` | `exact:path` |
| 3 | `assets/brand/bran-repository-raven.provenance.json` | `exact:path` |

All three matched on the token `repository` in the path. None relates to bugs,
defects, or issues. The correct answer — that no bug-tracking location exists
in this repository — was not derivable from the ranking; it took raw discovery
to establish.

## Hypothesis

Scoring appears to be dominated by the `exact` component, which is computed
against path and title tokens rather than document content. An `exact: 1`
path-token hit outranks a document carrying `canonical: 1` and `active: 3`
that only scores `partial: title`. Effects to confirm:

1. Does any scoring input read document body content, or only path and title?
2. What are the relative weights of `exact`, `partial`, `active`, `canonical`,
   `public_safe`? Can a canonical active document ever beat a path-token match?
3. Are common structural tokens (`repository`, `store`, `test`, `index`) worth
   suppressing as ranking signal, or down-weighted by document role?
4. Should `README.md` and other role-bearing documents carry an intrinsic
   floor so they are always at least *selected*?

## Reproduction

Both queries above reproduce against the pinned runtime:

```
version:       bran 0.1.0
source_commit: 99cd22e07c075dba2f24cc7ef6349fcd60edc1eb
sha256:        71f282da26d3c9a3601ed9ddedd6489b02c9f76b54e7e266ae8ba7e3f73f533d
```

Binary verified against `tools/bran/runtime/bran-release-pin.json` before both
runs. `bran check <bran-dev> bran-strict` was `ok` with no failures or
warnings at the time of observation.

## Not in scope of this report

Context reduction worked as advertised in both runs (~1.9 MB and ~2.5 MB of
candidate bytes avoided). The defect is ordering and selection, not volume.
