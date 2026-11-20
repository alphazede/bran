# Brief ranking

`bran query <root> --brief` ranks scanned files with a fielded lexical index.
The index is built from the scan of each query, so a changed, added or removed
file counts in the next query, committed or not. Nothing is persisted.

## Fields and scoring

Each file has four fields:

| Field | Text | Weight | Length normalization `b` |
|---|---|---|---|
| `filename` | final path component without its last extension; for an `index` or `README` document, also the enclosing directory name | 3 | 0.2 |
| `path` | enclosing directories | 1 | 0.2 |
| `body` | whole file text, including any header | 1 | 0.75 |
| `description` | leading lines (see below), plus a sidecar description | 1.5 | 0.6 |

The leading lines are the first four `# `, `title:`, `purpose:`,
`description:`, `aliases:` or `tags:` lines in the first 50 lines of a
document (`.md`, `.rst`, `.adoc`, `.sdoc`, `.html`). For other files they are
the first 12 comment lines (`#`, `//`, `"""`, `'''`, `;`) in the first 30
lines.

Query and file text are tokenized the same way. Letters split at a
lowercase-to-uppercase change and at any non-letter. Digit runs are separate
tokens. Tokens of one character and English function words are dropped. The
remaining letter tokens are reduced with the English Snowball (Porter2)
stemmer, so `studies`, `study` and `studied` match.

In the `filename` field only, a word that does not match directly also
matches through its agent-noun verb form. A stem that ends in `-er` after a
consonant loses that ending, and a doubled final consonant loses one copy.
So `parser` in a question matches a file named `parse`, and `parse` matches
a file named `parser`; `scanner` meets `scan`. The `body` and `description`
fields do not use this form, because it would merge common words such as
`runner` and `run` across all text.

An `index` or `README` document (`.md`, `.rst`, `.adoc`, `.sdoc`, `.html`)
describes its directory, so its `filename` field also holds the directory
name: `docs/garnet/README.md` matches `garnet` in its filename. A code file
named `index` usually only re-exports its siblings and keeps its own name.

For each distinct query stem and field, a file gains

```
idf × weight × 2.2 × tf / (tf + 1.2 × (1 − b + b × length / mean length))
idf = ln(1 + (N − df + 0.5) / (df + 0.5))
```

`N`, `df` (body document frequency) and the mean field lengths come from the
files in the leading tier of `.bran/ranking.yaml`. When that tier has no
tokens in a field, its mean length for scoring is 1. The score also adds
`0.5 × min(1023, document traits) / 400`. Document traits are status, canonical,
public-safe and confidence. Two rules adjust the score:

- **Test intent.** A test file (a code file with `test`, `tests`, `spec` or
  `specs` in its path; code extensions are `.sh`, `.py`, `.js`, `.jsx`,
  `.mjs`, `.cjs`, `.ts`, `.tsx`, `.mts`, `.cts`, `.rs`, `.go`, `.rb`, `.cpp`
  and `.java`) gains 4 when the question says `test`, `tests` or
  `testing`, and loses 4 otherwise.
- **Value source.** For a quantity or frequency question (`how often`,
  `how frequently`, `how long`, `how many`, `how much`, `frequency`), or a
  set question, a configuration or list file can gain score. A set question
  has `which`, an optional modifier, a plural noun and an auxiliary verb
  (`are`, `were`, `do`, `does`, `did`, `may`, `can`, `could`, `must`,
  `should`, `will`, `would`, `have`): `which owners may ...` or
  `which repository owners may ...`. A plural ends in `s` but not `ss`,
  `us` or `is` and has more than three letters. `which <noun> ...` asks for
  one file and `which <plural> <verb> ...` (`which scripts remove ...`) asks
  for the files that act, so neither is a set question. A relevant document or code
  file must name it (relative to the referring file, then to the root, then
  by a unique path suffix), or the configuration file must have a structured
  key that shares a query stem. For frequency questions, a schedule also
  qualifies. A strongly referenced file can enter the ranking through that
  reference alone.

A file must have a stem match in some field, or a strong value-source
reference, to be ranked. Tiers from `.bran/ranking.yaml` still sort first.
Lower tiers are tokenized when the leading tier has fewer than 32 matches or
misses a query word, so files in lower tiers remain findable. Ties break on
literal filename words, document traits, freshness, and then bundle and path.
A question without any indexable word uses the earlier scorer.

The shortlist holds up to 32 candidates. The rule keeps the leader and each
candidate that scores at least 30% of the leader's relevance score. An advisor
receives the selected candidates before the display limit applies.

## Reasons

A result `reason` is `<kind>:<fields>`, optionally followed by ` [<class>]`
for a `.bran/ranking.yaml` class.

- `kind` is `exact` when a hyphenated, underscored or dotted query identifier
  occurs in the file, or a query word equals the whole path or a whole
  metadata value. Otherwise it is `partial`.
- `fields` joins with `+` the fields that matched: `filename`, `path`,
  `body`, `description`, and `reference` when the value-source rule admitted
  or raised the file.

Example: `partial:filename+body+description [plan-packages]`.

## Description sidecar

An optional `.bran/descriptions.jsonl` adds a short description to files.
Each line is one JSON object with exactly these string keys:

```json
{"path": "tools/backup/run.sh", "sha256": "<64 lowercase hex>", "description": "Nightly backup of the shared volume to object storage."}
```

- `path` is the repository-relative path with `/` separators. Empty, `.` and
  `..` segments, a leading `/`, backslashes and NUL are rejected. The path
  must name a scanned file.
- `sha256` is the SHA-256 of the file's bytes. A description applies only
  while it matches the scanned file, so a stale description is ignored.
- `description` is 1 to 2,048 bytes and is added to the `description` field.

Limits: the file is at most 8 MiB, each line at most 4,096 bytes, and at most
50,000 entries are considered. The first valid entry for a path wins.
BRAN rejects symbolic links for the sidecar and its `.bran` directory.
On Unix, BRAN opens the sidecar relative to an open `.bran` directory handle.
A malformed, out-of-tree, oversized, or duplicate entry is rejected on its own.
Each JSON object must have unique keys.
The query never fails because of this file. When anything is ignored, one
warning reports it: `descriptions_ignored: <n> rejected, <m> stale`, or
`descriptions_ignored: <reason>` when the whole file is ignored. The sidecar
itself is never ranked. With no sidecar, ranking is as described above.

BRAN never writes this file. Produce it with any tool and refresh the hashes
when files change.

## Cost

The index is built in memory for every query, from the scan the query already
makes. Analysis runs on up to eight threads and gives the same result on any
thread count. On a repository of about 7,500 scanned files, 81 queries run one
at a time on a loaded machine took a median of 0.27 s and at most 0.56 s.
The previous scorer took a median of 0.57 s and at most 0.99 s on the same
machine.
