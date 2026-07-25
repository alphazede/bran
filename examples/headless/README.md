# Headless CLI

All commands emit a JSON envelope in this order: `schema_version`, `command`,
`status`, `data`, `warnings`, `failures`, `provenance`, `metrics`. `data` is
command-specific; `warnings` and `failures` are arrays; `provenance` and
`metrics` are objects.

```sh
bran packet <repo-root> "<request>"
bran query <repo-root> "<request>"
bran check <repo-root> <profile>
bran maintain propose <repo-root> <target> <replacement>
bran maintain apply <repo-root> <target> <replacement> <digest> <authority>
bran maintain revalidate <repo-root>
```

`query`, `packet`, and `check` are read-only. `maintain propose` makes zero
mutations. `maintain apply` requires explicit authority and the exact proposal
digest, then revalidates. `maintain revalidate` validates the current root.

Typed exits are `0` success, `1` validation, `2` usage, and `3` operation.

Actual model-token counts may be unavailable; bytes-divided-by-four values are
estimates. `bran packet` honors project `sqz=true` without contacting a model
or provider. Claim compression only when its complete SQZ receipt reports it.
