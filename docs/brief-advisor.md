---
type: Reference
title: Brief-mode advisor protocol
okf_status: active
tags:
  - public
  - developer
freshness: "2026-11-19"
resource: https://github.com/alphazede/bran
public_boundary: public
---

# Brief-mode advisor protocol

`bran query <root> --brief` picks its results with a deterministic rule. A
caller can add an advisor: an external program that reorders or narrows the
rule's candidates. The advisor is off unless the caller sets
`BRAN_ADVISOR_CONFIG`. Any advisor fault keeps the rule's answer, and
`data.selection.source` is `advisor` only when BRAN accepted the advisor's
answer.

## Configuration

`BRAN_ADVISOR_CONFIG` names a configuration file. BRAN ignores the file when it
is inside a queried root, or when a symlink to it is inside a queried root.

```yaml
advisor: /opt/example/advisor          # absolute path to the executable
sha256: <64 lowercase hex digits>      # digest of the executable's bytes
timeout_ms: 5000                       # 1..=600000
send_fields: [question, path, excerpt] # what goes on the advisor's stdin
requires_env: EXAMPLE_ADVISOR_ON       # optional: run only when this is set
pass_env: [EXAMPLE_ADVISOR_TOKEN]      # optional: the only variables passed
```

`send_fields` lists one or more of:

| Field      | Sent per                                                           |
|------------|--------------------------------------------------------------------|
| `question` | request: the query text                                            |
| `path`     | candidate: the path relative to its root (`bundle` in multi-root)  |
| `reason`   | candidate: the one-line match reason                               |
| `rank`     | candidate: the rule rank (integer)                                 |
| `excerpt`  | candidate: a bounded excerpt of the file's text                    |

Every candidate also carries its `id`, the rule rank as a string.

### Excerpt

`excerpt` sends repository file text to the advisor, so it is sent only when
`send_fields` lists it. Each excerpt is:

- the file's leading non-empty lines, up to about 300 bytes, then the lines
  that contain a query term (ASCII case-insensitive);
- trimmed lines joined by `\n`, at most 900 bytes, cut at a character
  boundary;
- read from at most the first 256 KiB of the file;
- empty when the file is not valid UTF-8, is not a regular file, or resolves
  outside its queried root.

## Request (stdin)

BRAN sends one JSON object. `options` holds up to 32 candidates that pass
the rule selection cut, in rule order. BRAN applies `--top` after advice. The
advisor can promote any of them into the returned set.

```json
{
  "question": "how are lamp wicks trimmed",
  "options": [
    {"id": "1", "path": "docs/lamps.md", "excerpt": "# Lamps\nWicks are trimmed weekly."},
    {"id": "2", "path": "notes/wick-care.md", "excerpt": "# Wick care\nTrim the wick before lighting."}
  ]
}
```

## Response (stdout)

The advisor exits 0 within `timeout_ms` and writes at most 64 KiB of JSON in
one of two forms.

A path list, a reorder or subset of the candidates:

```json
{"paths": ["notes/wick-care.md", "docs/lamps.md"]}
```

In multi-root queries a path entry may also be a candidate `id` or
`{"bundle": .., "path": ..}`.

A score per candidate, each from 0 to 1:

```json
{"scores": [{"id": "2", "score": 0.82}, {"id": "1", "score": 0.4}]}
```

BRAN orders candidates by score, descending. Ties keep the rule order.
Candidates the advisor did not score follow the scored ones, in rule order.
BRAN checks range limits before comparing scores at double precision.

BRAN rejects the whole answer and keeps the rule's result when:

- the output is not one of the two forms, or has both `paths` and `scores`;
- a JSON object contains a repeated field name;
- a path is not a candidate, is ambiguous, or repeats; the list is empty;
- a score entry is not exactly `{"id": "<string>", "score": <number>}`;
- an `id` is unknown or repeats; `scores` is empty;
- a score is not a finite JSON number from 0 to 1 (`NaN`, `1e999`, `"0.5"`
  and `1.5` are rejected);
- the output exceeds 64 KiB, the advisor exits non-zero, or it misses the
  deadline (BRAN kills its process group).
