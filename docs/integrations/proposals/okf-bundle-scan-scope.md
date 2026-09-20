---
type: upstream-proposal
proposal_id: UPSTREAM-OKF-BUNDLE-SCOPE
kind: normative-clarification
submission_status: pull-request-open
title: "Upstream OKF Proposal: Explicit Bundle Scan Scope and Traversal Rules"
okf_status: draft
status: draft
tags:
  - internal
  - developer
freshness: "2026-07-25"
resource: https://github.com/GoogleCloudPlatform/knowledge-catalog/pull/232
public_boundary: public-compatible
---

# Upstream OKF Proposal: Explicit Bundle Scan Scope and Traversal Rules

## Current status

The owner approved this contribution and submitted it to Google Cloud's Knowledge
Catalog repository as [pull request #232](https://github.com/GoogleCloudPlatform/knowledge-catalog/pull/232),
"Clarify the OKF bundle conformance boundary."

- **Upstream specification:** [OKF v0.2](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md)
- **Submitted by:** `1wgrumph`
- **Source fork:** `alphazede/knowledge-catalog`
- **Source branch:** `proposal/okf-bundle-scan-boundary`
- **Source commit:** `a0801c480d473f998718d698c4677b5dc71b8c45`
- **State recorded 2026-07-25:** open; Google CLA and `check-changes` pass; no maintainer review or disposition yet

The pull request is the source of truth for the wording under review. This file records
the reasoning and status; it does not imply acceptance or merge.

## What the pull request proposes

The change adds a narrow bundle-root and conformance-boundary rule to OKF v0.2:

- Each conformance evaluation establishes one bundle root.
- The conformance corpus contains regular Markdown entries beneath that root, excluding
  symbolic-link entries.
- Candidate roots outside the selected root receive separate conformance results.
- Non-Markdown resources may live inside a bundle without becoming OKF documents.
- Links, path-valued fields, and symbolic links do not expand the conformance corpus or
  authorize dereferencing their targets.
- The conformance and version-history sections use that same boundary consistently.

This is intentionally a format-boundary clarification. It does not standardize command
syntax, root auto-discovery, archive extraction, filesystem access, resource limits, link
resolution, execution, or BRAN policy.

## Why the clarification matters

OKF v0.2 allows a bundle to be a repository, archive, or subdirectory of a larger
repository, while its conformance section evaluates Markdown files "in the tree."
Without an explicit selected root, two validators can accidentally evaluate different
corpora, merge sibling bundles, or inspect unrelated repository content.

The submitted rule makes the evaluated document set deterministic without treating
neighboring files as safe or exempt from separate security and data-spill reviews.

## BRAN evidence and limits

BRAN's scanner and bundle logic already use an explicit repository root, deterministic
containment, and non-expanding link behavior. That implementation experience motivated
the contribution, but BRAN is not a normative dependency of the proposal.

The earlier internal draft targeted OKF v0.1 and proposed broader implementation-level
symlink language. OKF v0.2 now supersedes v0.1, and the owner-reviewed pull request above
contains the narrower text actually submitted. No stable BRAN release or public receipt
is asserted as upstream proof.

## Next step

Keep the fork and source branch available while the pull request is open. Respond only
to maintainer feedback or requested changes, and continue to describe the change as a
proposal until upstream merges it.
