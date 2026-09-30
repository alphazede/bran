---
type: Concept
title: Cloud and server deployment profile
okf_status: active
tags:
  - public
  - developer
freshness: "2026-09-30"
resource: https://github.com/alphazede/bran
public_boundary: public
---

# Cloud and server deployment profile

This is the stage-1 contract for [issue #28](https://github.com/alphazede/bran/issues/28):
the smallest safe way to run BRAN on a cloud VM or container host without
giving up its local-first, bounded-root, read-only defaults.

## Decision

BRAN ships a staged profile. Only stage 1 exists.

| Stage | Shape | Status |
|---|---|---|
| 1 | **Remote CLI.** Operators run the existing headless commands over SSH, a job runner, or an orchestrator. BRAN opens no listening port. | Implemented here |
| 2 | **Single-tenant service.** An opt-in daemon with a versioned job API. | Not built |
| 3 | **Multi-tenant service.** | Deferred |

Why remote CLI first:

- `check`, `query`, and `packet` are already deterministic, bounded,
  read-only, and emit versioned JSON with typed exit codes. A runner that
  invokes them over SSH gets the same evidence a local user gets.
- No listener means no new attack surface. Authentication, transport
  security, scheduling, retries, and job records stay with systems built for
  them (SSH, the job runner, systemd, the container runtime).
- Stage 1 persists no BRAN state. Nothing needs backup, migration, or
  crash recovery beyond the repository mounts BRAN already treats as
  read-only.

A service is justified only by a workload that needs durable asynchronous
jobs or controlled API access that a runner cannot provide. No such use case
is stated today, so stage 2 is not built. Its entry conditions are listed in
[Stage 2 entry conditions](#stage-2-entry-conditions). Multi-tenancy stays
unavailable until namespace, authorization, storage, and side-channel
isolation have their own approved security design.

The default `bran` binary is unchanged: no listener, no network call, no
new required configuration.

## Registered-root mode

Stage 1 adds one opt-in control. When the environment variable
`BRAN_REGISTERED_ROOTS` is set, the CLI runs in registered-root mode.
The variable is deployment configuration, set by the operator in the image,
unit, or runner — never by a request.

The value is a list of repository roots separated like `PATH` (`:` on
Unix). Each entry must be an absolute path to an existing directory, spelled
exactly as its canonical path: no symlink component, no `.` or `..`, no
trailing slash. If any entry fails, every command fails with
`registered_roots_invalid` (exit 3), including `help`, `--version`, and
`smoke`. An empty value is invalid, so an image can default to empty and fail
closed until the operator registers a root.

In registered-root mode:

- Only `check`, `query`, and `packet` run. Every other command fails with
  `unavailable_in_registered_root_mode` (exit 2). That includes `help`,
  `--version`, `smoke`, `maintain`, `evidence`, `-p`, `get`, `tui`,
  `agents`, `doctor`, and `body-preserved`.
- `query --record` and `check --policy-stdin` fail the same way. The first
  writes into the repository. The second lets a request replace repository
  policy.
- Every root argument, including each `query --add-dir` root, must equal a
  registered entry byte for byte. Anything else fails with
  `root_not_registered` (exit 2) before BRAN touches that path. This rejects
  unregistered roots, `..` traversal, alternative spellings, and symlink
  aliases.
- Request text for `query` and `packet` is limited to 8192 UTF-8 bytes
  (`request_oversized`, exit 2). It must also pass BRAN's DLP screen
  (`is_public_dlp_safe`), or it fails with `request_dlp_rejected` (exit 2).
  The screen is substring matching, not a DLP product. It rejects
  credential assignments such as `token=` or `password:`, bearer and
  prefixed tokens, private keys, and home-directory paths. A legitimate
  question that contains one of those markers is rejected too. That is the
  fail-closed trade-off.
- Rejections never echo the rejected root or request text.

The scanner already refuses to follow symlinks inside a root, so a link in a
registered repository that points outside it contributes nothing.

### Source revision

In registered-root mode, every `check`, `query`, and `packet` envelope for an
admitted root adds `provenance.source_revisions`, including operation-error
envelopes such as a scanner quota overflow and policy-load errors. It holds
one entry per requested root, in request order: the primary root, then each
`--add-dir` root. Rejected requests carry no revision.

```json
{"root":"/repos/example","kind":"git-head","status":"attested",
 "ref":"refs/heads/main","value":"<full commit id>","reason":null}
```

BRAN reads the commit that the root's git `HEAD` names from git metadata
directly. It never runs git, so no hook, config, or filter executes. It reads
only these files:

- `.git` in the root: a directory, or a `gitdir:` file for a linked worktree
  or submodule
- `HEAD` and `commondir` in that git directory
- for per-worktree refs (`refs/worktree/`, `refs/bisect/`,
  `refs/rewritten/`), the loose ref in that git directory; these refs are
  never packed
- for every other ref, the loose ref, then `packed-refs`, in the common
  directory

Every file BRAN reads must resolve, through any symlinks, to a regular file
inside its directory: the root for `.git`, the git directory for `HEAD`,
`commondir`, and per-worktree refs, and the common directory for shared refs
and `packed-refs`. BRAN resolves the path before opening it and never reads
a file outside that directory. It opens without following a final symlink
and without blocking, and rejects FIFOs, sockets, and devices. Each read is
bounded: 4 KiB per file and 64 MiB for `packed-refs`. BRAN follows at most
five symbolic refs, counting `HEAD`, and only names under `refs/` without
empty, `.`, or `..` components. `value` is a full lowercase SHA-1 or SHA-256
object id. `ref` is `null` for a detached `HEAD`.

When BRAN cannot resolve the commit, `status` is `unavailable`, `value` is
`null`, and `reason` says why. BRAN never guesses.

| `reason` | Meaning |
|---|---|
| `not_a_git_checkout` | The root has no `.git` |
| `git_metadata_invalid` | `.git` is a symlink, not a file or directory, or a malformed `gitdir:` file; the git or common directory is missing; or `commondir` is malformed |
| `git_metadata_unsafe` | `HEAD` or `commondir` resolves outside the git directory or is not a regular file (for example a FIFO) |
| `reftable_unsupported` | The repository uses the reftable ref store |
| `head_unreadable` | `HEAD` is missing or cannot be read |
| `head_corrupt` | `HEAD` is neither `ref: refs/...` nor a full object id |
| `ref_unsafe` | `HEAD` or a symbolic ref names something outside `refs/`, or a loose ref or `packed-refs` resolves outside its directory or is not a regular file |
| `ref_unresolved` | The branch has no loose or packed ref, for example before the first commit |
| `ref_corrupt` | A loose ref or its `packed-refs` line holds no full object id |
| `ref_unreadable` | A ref file or `packed-refs` cannot be read |
| `ref_cycle` | Symbolic refs go deeper than five |

The revision is the commit `HEAD` named when the request ran. It does not
attest that the files match that commit: uncommitted edits, untracked files,
and concurrent updates are not reflected. BRAN does not open the commit
object, so it does not prove that the object exists. A linked worktree's git
directory lies outside its root. Mount that directory read-only too;
otherwise the revision is `unavailable` with `git_metadata_invalid`.

Default mode never reads git metadata and never emits `source_revisions`.
Apart from that field, a registered-root result is byte-identical to the
default-mode result, and registered-root mode changes no ranking, packet, or
validation result.

## Runtime boundary

| Requirement | Stage-1 mechanism |
|---|---|
| Non-root user, read-only executable | Image user `65532:65532`, binary mode `0555`; container runs with `--read-only`; systemd unit uses `DynamicUser=yes` and `ProtectSystem=strict` |
| Repositories at explicitly registered roots | Read-only bind mount per repository plus `BRAN_REGISTERED_ROOTS` |
| Bounded writable volume for BRAN state | Not needed. `check`, `query`, and `packet` write nothing, and registered-root mode refuses every command that writes. Stage 1 mounts no writable volume. |
| Network egress disabled | `--network none`; `PrivateNetwork=yes`, `IPAddressDeny=any`, `RestrictAddressFamilies=none` |
| Explicit capability configuration for connected agents and adapters | Connected agents (`-p`) are unavailable in registered-root mode. No database, object-storage, or other network adapter exists. |
| Item, byte, TTL, timeout, cancellation, concurrency limits | Existing scanner limits (10,000 files, 1 MiB per file, 64 MiB total), packet limits, and the request limit above. Timeout, cancellation, and concurrency belong to the runner (`TimeoutStartSec=`, `timeout`, job-runner concurrency). |
| No mutation of repositories, Git state, hooks, or host configuration | Read-only mounts, mutating commands refused, `ProtectSystem=strict`; git metadata is parsed for the source revision, and git is never run |
| No credentials in arguments, bodies, logs, metrics, receipts, or job metadata | Request DLP screen, no echo on rejection, no credential-bearing environment in the image |

## Deployment artifacts

### OCI image

[`deploy/oci/build_image.py`](../deploy/oci/build_image.py) builds an OCI
image layout from:

- the exact release archive (`bran-vX.Y.Z-<linux target>.tar.gz`), checked
  against the SHA-256 in the signed release manifest. The release must be
  bran 0.2.0 or later, the first release with registered-root mode, and its
  binary must carry the guard's strings (`BRAN_REGISTERED_ROOTS` and
  `unavailable_in_registered_root_mode`). An older binary ignores the
  registry, so `build` refuses it and `verify` rejects an image holding it.
- a base image layout pinned by manifest digest (a glibc runtime base,
  because release binaries link glibc dynamically; for example a distroless
  `cc` image with no shell)
- a dependency-license inventory read from `cargo metadata --locked
  --offline` at the release commit

It adds one uncompressed layer with only `/usr/local/bin/bran` (mode `0555`),
the two licence texts, and the inventory. It writes canonical JSON with
fixed timestamps. The same inputs produce the same image digest on any
host. The image config sets user `65532:65532`, entrypoint
`/usr/local/bin/bran`, no default command, and working directory `/`. It sets
the environment to only `PATH` and an empty `BRAN_REGISTERED_ROOTS`, so the
image fails closed. It declares no exposed ports and no volumes. Labels bind
the BRAN version, source commit, lockfile digest, release-archive digest,
binary digest, and base manifest digest.

`build_image.py verify <layout>` checks the image deterministically:

- every blob digest and size; `diff_ids` against the layer bytes
- user, entrypoint, working directory, and environment; no default command,
  exposed ports, or volumes
- the BRAN layer: exact entries, owners, modes, and fixed timestamps, and
  the binary digest against its label
- the release: version label 0.2.0 or later and the registered-root guard
  strings in the binary
- every layer: no setuid or setgid bit, no file-capability xattr, no device
  node, no world-writable path except sticky directories, and no absolute or
  `..` member name
- the licence inventory: every dependency licence expression must be
  satisfiable with MIT or Apache-2.0, matching `deny.toml`

Base-image licences (for example glibc) belong to the pinned base. The
verifier records the base digest and does not claim to have checked them.

Build and run:

```sh
python3 deploy/oci/build_image.py build \
  --release-archive dist/bran-v0.2.0-x86_64-unknown-linux-gnu.tar.gz \
  --release-manifest dist/bran-release-manifest.json \
  --base-layout base-oci/ \
  --base-manifest-digest sha256:<pinned base manifest digest> \
  --out bran-oci/
python3 deploy/oci/build_image.py verify bran-oci/

docker run --rm --read-only --network none --user 65532:65532 \
  --cap-drop ALL --security-opt no-new-privileges \
  --pids-limit 64 --memory 1g \
  --mount type=bind,source=/srv/bran/repos/example,target=/repos/example,readonly \
  --env BRAN_REGISTERED_ROOTS=/repos/example \
  <image@sha256:digest> packet /repos/example "where is ledger rotation handled"
```

Reference the image by digest, never by tag.

### Building on a real base

This repository's checks do not build an image on a real base. The base
must be pulled over the network by digest, and the gate runs offline. The
contract check therefore builds only on a synthetic base. An operator with
registry access runs these commands; they were not run here:

```sh
skopeo copy --override-os linux --override-arch amd64 \
  docker://gcr.io/distroless/cc-debian12@sha256:<pinned digest> oci:base-oci
python3 -c 'import json; print(json.load(open("base-oci/index.json"))["manifests"][0]["digest"])'
python3 deploy/oci/build_image.py build \
  --release-archive dist/bran-v0.2.0-x86_64-unknown-linux-gnu.tar.gz \
  --release-manifest dist/bran-release-manifest.json \
  --base-layout base-oci/ \
  --base-manifest-digest <digest printed above> \
  --out bran-oci/
python3 deploy/oci/build_image.py verify bran-oci/
skopeo copy oci:bran-oci docker-daemon:bran:0.2.0
```

Pin the platform manifest digest that `index.json` lists, not a
multi-platform index digest. `build` rejects any other digest. Review the
base image's licence notices before first use.

### systemd on a single VM

[`deploy/systemd/bran-check@.service`](../deploy/systemd/bran-check@.service)
is a hardened oneshot template. `systemctl start bran-check@example` runs
`bran check /srv/bran/repos/example bran-strict` with a dynamic user,
network isolation, an empty capability set, and a read-only view in which
`/srv/bran/repos` shows only the one instance repository. Instance names are
repository directory names (`[a-z0-9-]+`).

To check the unit offline before installing it, replace the binary path with
a stand-in that exists, because `systemd-analyze verify` checks that
`ExecStart` is executable, and verify it under an instance name:

```sh
work=$(mktemp -d)
sed 's#/usr/local/bin/bran#/usr/bin/true#' deploy/systemd/bran-check@.service \
  > "$work/bran-check@.service"
systemd-analyze verify "$work/bran-check@example.service"
systemd-analyze security --offline=yes "$work/bran-check@example.service"
rm -r "$work"
```

Ad-hoc `query` and `packet` runs use the same properties through
`systemd-run`:

```sh
systemd-run --pipe --wait --collect \
  -p DynamicUser=yes -p PrivateNetwork=yes -p IPAddressDeny=any \
  -p RestrictAddressFamilies=none -p CapabilityBoundingSet= \
  -p NoNewPrivileges=yes -p ProtectSystem=strict -p ProtectHome=yes \
  -p TemporaryFileSystem=/srv/bran/repos:ro \
  -p BindReadOnlyPaths=/srv/bran/repos/example \
  -p Environment=BRAN_REGISTERED_ROOTS=/srv/bran/repos/example \
  -p RuntimeMaxSec=300 \
  /usr/local/bin/bran query /srv/bran/repos/example "ledger rotation"
```

### Health and readiness

- Liveness does not apply. Stage 1 runs no long-lived process, and
  registered-root mode refuses `smoke`, `help`, and `--version`.
- Readiness: run `bran query <registered root> readiness` for each
  registered root, with the same `BRAN_REGISTERED_ROOTS` as real requests.
  The exit code alone is not enough: a root without native policy exits 0
  with status `unavailable`, and a skipped oversized file exits 0 with a
  warning. The root is ready only when all of these hold:
  - the exit code is 0
  - the envelope `status` is `ok`, which means native policy loaded
  - `failures` is empty
  - every warning is either `unmatched_query_terms: readiness` or a skipped
    in-repository symlink (`Symlink { ... }`). An `OversizedInput`,
    `Unreadable`, or any other warning means BRAN could not see the whole
    repository.

  A repository over the file or byte quota fails with exit 3 and
  `LimitExceeded`. For example, with `jq`:

  ```sh
  bran query "$root" readiness > ready.json 2>&1; code=$?
  [ "$code" -eq 0 ] && jq -e '.status == "ok" and .failures == [] and
    all(.warnings[]; startswith("unmatched_query_terms:") or startswith("Symlink {"))' \
    ready.json > /dev/null
  ```

- Validation: `bran check <registered root> <profile>`, as in the systemd
  unit. Exit 1 means the repository fails validation. `check` reads only
  knowledge documents, so it does not prove the whole repository fits the
  scanner quotas.
- There is no state store or network adapter in stage 1, so neither can be
  unavailable.

### Graceful shutdown and cancellation

A stage-1 invocation is one process. It prints its envelope only after the
command completes. On `SIGTERM` or `SIGKILL`, it exits by signal and leaves
nothing behind, because it writes nothing. The runner must treat only exit
status 0 or 1 plus a complete JSON envelope as a result. Any other outcome,
including a truncated stdout, is a failed run. There is no in-flight job
store to cancel or recover.

### Backup and restore

Stage 1 owns no persistent state. Back up the repositories and the
deployment configuration: the image digest, the unit or runner definition,
and the registered-root list. Restore is redeploying those. Do not back up
`.bran/cache` or `.bran/results` from a stage-1 host, because stage 1 never
writes them.

## Upgrade and rollback

Record this tuple for every deployment:

| Field | Source |
|---|---|
| Image digest | `sha256:` digest of the manifest `build_image.py` prints |
| BRAN version and source commit | `org.opencontainers.image.version` and `.revision` labels; on a VM, `bran --version` run without `BRAN_REGISTERED_ROOTS` |
| Release-archive and binary digests | Image labels; release manifest |
| Envelope schema version | `schema_version` in every envelope, for example the readiness query |
| Repository policy schema | `schema_version` in each repository's `.bran/policy.yaml` |
| Stored-state schema | None in stage 1 |

Upgrade: build and verify the new image, run readiness against each
registered root, then switch the runner to the new digest. Roll back by
switching back to the recorded previous digest. Stage 1 has no stored state
to migrate or downgrade. A future stage that stores state must version it and
refuse to start on an unknown stored-state version. Rolling back across a
stored-state migration then requires restoring the pre-upgrade backup.

## Threat model

| Asset | Threat | Control |
|---|---|---|
| Host filesystem | A request names a path outside the approved repositories | Registered-root exact match before any file access; container and unit see only mounted repositories |
| Repository contents | A request mutates a repository, Git state, or hooks | Read-only mounts; mutating commands refused in registered-root mode |
| Repository contents | A symlink inside a repository points at host files | The scanner does not follow symlinks |
| Repository metadata | Reading the source revision runs a repository's git hooks, config, or filters, reads host files through symlinks, or hangs on a FIFO | BRAN parses `HEAD`, `commondir`, and refs itself and never runs git. It reads only regular files that resolve inside their git directory, without blocking, with bounded reads. It emits only full object ids, safe `refs/` names, and fixed reason codes |
| Credentials | A credential passed in a request is echoed into output or logs | DLP screen rejects the request without echo; the image holds no credentials; errors never echo roots or request text |
| Network | BRAN is used for exfiltration or as a pivot | No listener or socket API in the workspace sources, no network crate in `Cargo.lock`, `--network none` or `PrivateNetwork=yes` |
| Host resources | A large repository or request exhausts memory or time | Scanner and packet limits, request limit, runner timeouts, and container or unit memory and task limits |
| Image supply chain | A tampered binary or base | Release manifest digest check, pinned base digest, deterministic build, `verify` |
| Privilege | Escalation inside the container | Non-root user, `--cap-drop ALL`, `no-new-privileges`, no setuid, setgid, or file-capability files |

Out of scope in stage 1: authentication and transport security (SSH or the
runner owns them), tenant isolation beyond one registry per process, and
side channels between co-located containers.

## Operator checklist

- [ ] Image referenced by digest; `build_image.py verify` passed on it.
- [ ] Base image pinned by manifest digest, with a reviewed licence notice.
- [ ] Each repository mounted read-only at its own path and listed in
      `BRAN_REGISTERED_ROOTS` with its exact canonical spelling.
- [ ] Nothing else from the host is mounted. No writable volume is mounted.
- [ ] Container: `--read-only --network none --user 65532:65532 --cap-drop ALL
      --security-opt no-new-privileges` plus memory and PID limits. Unit:
      `deploy/systemd/bran-check@.service` properties.
- [ ] No credentials in the image, unit, environment, or arguments.
- [ ] Runner enforces a timeout and treats only exit 0 or 1 with a complete
      envelope as a result.
- [ ] Readiness rule passes for each root after every upgrade.
- [ ] Deployment tuple recorded for rollback.

## Security questions

- **Authentication.** Delegated. SSH, the job runner, or the orchestrator
  authenticates the operator. BRAN implements none in stage 1.
- **Network egress.** None. No stage-1 command needs it.
- **Repository registration and tenant ownership.** Registration is
  deployment configuration (`BRAN_REGISTERED_ROOTS` plus mounts). One
  process or container serves one tenant. Separate tenants use separate
  containers or units.
- **Retention and encryption.** Stage 1 retains nothing. Results go to the
  runner, which owns retention and encryption.
- **Audit evidence.** The runner records the deployment tuple, the
  invocation (command, registered root, exit status), and the SHA-256 of the
  envelope. The request text may be recorded only under the runner's own
  data policy. BRAN envelopes carry controls, limits, selected locators,
  derivation (`why_selected`, rankings), DLP status, and in registered-root
  mode the source revision of each root. BRAN does not log.
- **Resource limits.** BRAN enforces scanner, packet, and request limits.
  CPU, memory, PIDs, and wall time are container or unit limits. BRAN has no
  decompression or model workloads in stage 1.

## Stage 2 entry conditions

Build a single-tenant service only after an owner-approved use case needs
durable asynchronous jobs or API access a runner cannot give. It must then
meet the issue's service boundary: loopback or private bind, TLS at the
service or a trusted ingress, short-lived identities or credential
references, per-operation authorization against the registry, versioned
schemas, idempotency keys, cancellable and quota-bounded jobs, opaque
identifiers, separate liveness and readiness, content-free metrics, and an
audit receipt. Job and result state starts on BRAN's bounded local result
store (`.bran/results`: 16 entries, 4 MiB total, 1 MiB per item, 24-hour
TTL, staged writes whose leftovers are discarded on the next open). Any
database-backed state follows the EvidenceSource/StateStore separation in
[issue #27](https://github.com/alphazede/bran/issues/27). Kubernetes, Helm,
autoscaling, and a hosted control plane wait for a stable single-node
contract. Provider-specific infrastructure stays outside `bran-core`, which
keeps zero dependencies.

## Acceptance status

| Criterion | Status | Evidence |
|---|---|---|
| Design document chooses a shape and explains why | Done | [Decision](#decision) |
| Default binary opens no listener and makes no network call | Done | `tools/ci/deployment_contract_check.py` (no socket API in `crates/`, no network crate in `Cargo.lock`); `p8_deployment_profile` traces `check`, `query`, and `packet` for socket calls when `strace` is installed |
| Smoke test runs check, query, and packet against a mounted synthetic repository | Done for stage 1 | `p8_deployment_profile`: registered, read-only synthetic repository, cleared environment, read-only working directory, repository digest unchanged |
| Negative tests | Partial | `p8_deployment_profile` covers unregistered roots, path traversal, symlink escape (root alias, registry entry, in-repository link), cross-namespace access (a root registered to another process), oversized requests, quota overflow (a repository over the 10,000-file scanner quota fails with exit 3), and secret reflection. `registered_mode_admits_only_check_query_packet` checks that an empty or invalid registry refuses every command, including `help`, `--version`, and `smoke`. `readiness_rule_rejects_policy_and_quota_gaps` checks that the readiness rule rejects a root without native policy, an oversized file, and a quota overflow. Not done: expired authentication, because stage 1 has no BRAN authentication (SSH or the runner authenticates). Cancellation races are also not done for stage 1: it has no BRAN cancellation interface, and the runner ends the process. The connected cancel-versus-publish race is covered by the existing `p3_headless_cli_and_maintain_lifecycle`. |
| Restart recovery never reports a partial job as successful | Partial | Stage 1 has no BRAN job state, and exit status is authoritative. The bounded result store discards staged writes on the next open and keeps prior results when publication fails (existing `p3_headless_cli_and_maintain_lifecycle`). Job records are stage 2. |
| Results retain source revision, packet, DLP, derivation, and storage provenance | Done for stage 1 | [Source revision](#source-revision), added in registered-root mode by owner decision of 2026-09-30. `p8_source_revision` covers a loose-ref checkout, detached `HEAD` (SHA-1 and SHA-256), packed ref, linked worktree (absolute and relative `gitdir:`), non-git root, and corrupt, unsafe, unresolved, cyclic, and reftable metadata. It also cross-checks loose, packed, worktree, and detached results against real git. `git_metadata_symlinks_stay_confined`, `git_metadata_fifos_never_block`, and `worktree_local_refs_resolve_in_worktree_git_dir` cover outside symlinks, FIFOs, and per-worktree refs, with a real-git cross-check. `registered_revision_is_confined_and_never_blocks` and `error_envelopes_keep_source_revisions` cover the same through the CLI. `p8_deployment_profile` checks an attested revision through the CLI for a real git checkout and `unavailable` for a non-git root. It also checks packet, DLP, and derivation fields, and that the output is byte-identical to default mode apart from `source_revisions`. Storage provenance does not apply: stage 1 stores nothing. |
| OCI image contents, user, entrypoint, ports, writable paths, capabilities, and licences verified deterministically | Partial | `deployment_contract_check.py` builds twice from a synthetic base and archive, compares digests, and checks that `verify` rejects each contract violation. It also checks that `build` refuses, and `verify` rejects, a release before 0.2.0 or a binary without the registered-root guard. **Not done:** no image on a real base has been built or verified. That needs a network pull of a digest-pinned base image, and this repository's checks run offline. By owner decision of 2026-09-30 this stays not done. The operator commands are in [Building on a real base](#building-on-a-real-base). |
| Server can use the bounded local state store first; database state follows #27 | Done by design | Stage 1 stores nothing; [Stage 2 entry conditions](#stage-2-entry-conditions) |
| Upgrade and rollback path binds image digest, version, schema versions, and state migration | Done | [Upgrade and rollback](#upgrade-and-rollback); image labels |
| Hosted-provider infrastructure optional and outside the portable core | Done | `deployment_contract_check.py` checks that `bran-core` has no dependencies; nothing provider-specific ships |
