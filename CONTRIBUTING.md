# Contributing to BRAN

Thanks for taking a look. Bug reports are genuinely useful, and the ranking
heuristics are where I've been wrong most often, so that's a good place to
push.

## First, a note about this repository

This repository is a published snapshot. BRAN is developed somewhere else, and
the code here is exported from there and signed.

**That means pull requests opened here can't be merged.** Not because they
aren't welcome, but because the next export would overwrite them. Sorry. If you
want to change something, open an issue and we'll work out the shape of it
first. If a change is worth making, I'll carry it upstream and credit you in
the commit.

Issues, questions, and bug reports are all in the right place here.

## Building and testing

You need a stable Rust toolchain. Nothing else.

```sh
cargo test                      # the test suite
cargo run --bin bran -- smoke   # quick sanity check
./tools/ci/check.sh --fast      # the gate that has to pass
```

`check.sh --fast` runs formatting, clippy, the tests, and the boundary checks.
If it passes, the change is in reasonable shape. Use `--full` only when you've
touched release, security, or conformance behaviour, since it's much slower.

## Filing a good bug

The most useful reports include the exact command you ran and what came back.
BRAN prints versioned JSON, so paste it. The templates ask for this, but the
short version is:

- the command, verbatim
- the output, including `warnings` and `failures`
- what you expected instead
- `bran --version`

If it's a ranking problem, say which file you expected to see and where it
actually ranked. "It returned the wrong thing" is hard to act on. "I asked X,
expected `path/to/file.rs`, and it came back at rank 14 behind four test files"
is something I can chase.

Please don't include anything private. Repository paths, source excerpts, and
query text often carry more than you'd expect.

## Things worth knowing

**Determinism is the point.** The same repository and the same question have to
produce the same ranking every time. Any change that makes results depend on
timing, machine state, or hidden history is a change to what BRAN is. If you
have an idea that needs that, open an issue and let's talk it through.

**Offline is the default.** Scanning, ranking, packets, validation, and the TUI
work with no account and no network calls. Keep it that way.

**Nothing unavailable gets faked.** If a capability isn't there, BRAN says
`unavailable` instead of pretending. Requested and effective capability are
reported separately, on purpose.

## Security

Don't open a public issue for a security problem. Email 1wgrumph@gmail.com
instead and I'll deal with it.

## Licence

Contributions are dual-licensed under MIT or Apache-2.0, matching the project.
