---
type: upstream-proposal
proposal_id: UPSTREAM-OKF-LAYERED-PROFILES
kind: layered-profile-design
submission_status: upstream-discussion-open
title: "Upstream OKF Proposal: Layered Profile Separation for Conformance and Readiness"
okf_status: draft
status: draft
tags:
  - internal
  - developer
freshness: "2026-07-25"
resource: https://github.com/GoogleCloudPlatform/knowledge-catalog/issues/212#issuecomment-5081662199
public_boundary: public-compatible
---

# Upstream OKF Proposal: Layered Profile Separation for Conformance and Readiness

## Current status

The owner approved a short design comment on the existing upstream profile discussion,
[issue #212](https://github.com/GoogleCloudPlatform/knowledge-catalog/issues/212). The
comment was submitted by `1wgrumph` on 2026-07-25:

> Building on the “compatible superset, not a fork” idea, I think it would help to make
> one thing explicit: OKF conformance and profile warnings should be reported separately.
>
> A bundle might pass OKF v0.2 while `civic-v1` reports a missing field. That warning
> shouldn’t be described as OKF nonconformance. Likewise, passing the profile shouldn’t
> override a real OKF failure.
>
> I’m not suggesting a central registry or standard result format—just a short note in
> §11 that keeps those results separate. Would that be useful?

The exact record is [issue comment 5081662199](https://github.com/GoogleCloudPlatform/knowledge-catalog/issues/212#issuecomment-5081662199).
There is no maintainer response or follow-up pull request recorded yet.

## The reporting boundary

OKF v0.2 §11 defines portable conformance. An authoring or validation tool may also run
a stricter profile, but the two results answer different questions:

| OKF result | Profile result | Accurate description |
|---|---|---|
| pass | pass | OKF-conformant and profile-clean |
| pass | warning or fail | OKF-conformant with profile diagnostics |
| fail | pass | Not OKF-conformant; the profile result does not repair it |
| fail | warning or fail | Not OKF-conformant and also has profile diagnostics |

The contribution asks only that tools preserve this distinction in terminology and
diagnostics. It does not ask OKF to define a central profile registry, standard result
envelope, profile identifiers, consumer enforcement, or BRAN-specific metadata.

## BRAN evidence and limits

BRAN's `ProfileValidator` reports its portable compatibility result separately from
`bran-strict`, and only the selected profile controls the process exit. The focused test
is:

```sh
cargo test --manifest-path Cargo.toml -p bran-core profile::tests::p1_profiles
```

This is implementation evidence for keeping outcomes separate, not a request to make
BRAN's envelope or `bran-strict` part of OKF. The submitted comment recorded BRAN's
then-current compatibility identifier as `okf-v0.1`. Subsequent repository work
added a selectable `okf-v0.2` profile; `okf-v0.1` remains supported and is not
removed. That later additive profile is not implied by the upstream comment.

## Next step

Wait for maintainer feedback on issue #212. If maintainers want conformance wording,
prepare a small §11 change for another owner review before opening a pull request. Until
then, the contribution remains an open design suggestion rather than an accepted rule.
