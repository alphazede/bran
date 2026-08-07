#!/bin/sh
# build-release.sh: plan + per-target package for exact 5 cross artifacts.
# --plan validates names only (non-mutating). The build and packaging are
# delegated to the Rust xtask (cargo run -p xtask -- package), so the release
# path needs only cargo. Build uses --locked, fails on missing target.
set -eu

usage() {
    printf 'usage: %s --plan --tag TAG --dist DIR\n' "$0" >&2
    printf '       %s --target TRIPLE --tag TAG --dist DIR\n' "$0" >&2
    exit 2
}

plan_mode=false
target=""
tag=""
dist=""

while [ $# -gt 0 ]; do
    case "$1" in
        --plan)
            plan_mode=true
            shift
            ;;
        --target)
            [ $# -ge 2 ] || usage
            target="$2"
            shift 2
            ;;
        --tag)
            [ $# -ge 2 ] || usage
            tag="$2"
            shift 2
            ;;
        --dist)
            [ $# -ge 2 ] || usage
            dist="$2"
            shift 2
            ;;
        *)
            usage
            ;;
    esac
done

[ -n "$tag" ] || usage
[ -n "$dist" ] || usage
if $plan_mode; then
    [ -z "$target" ] || usage
else
    [ -n "$target" ] || usage
fi

# Reject line terminators before applying the canonical whole-string regex.
lf='
'
cr=''
case "$tag" in
    *"$lf"*|*"$cr"*)
        printf 'FAIL invalid tag (must be bran-vX.Y.Z): %s\n' "$tag" >&2
        exit 1
        ;;
esac
if ! printf '%s' "$tag" | grep -Eq '^bran-v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'; then
    printf 'FAIL invalid tag (must be bran-vX.Y.Z): %s\n' "$tag" >&2
    exit 1
fi

# Canonicalize dist to absolute BEFORE any cd/subsell so relative --dist works
# (do NOT mkdir in plan mode; plan must be non-mutating)
case "$dist" in
    /*) ;;
    *) dist="$PWD/$dist" ;;
esac

LX86=x86_64-unknown-linux-gnu
LARM=aarch64-unknown-linux-gnu
MX86=x86_64-apple-darwin
MARM=aarch64-apple-darwin
WX86=x86_64-pc-windows-msvc

artifact_name() {
    t=$1
    case $t in
        "$LX86"|"$LARM"|"$MX86"|"$MARM") printf '%s-%s.tar.gz\n' "$tag" "$t" ;;
        "$WX86") printf '%s-%s.zip\n' "$tag" "$t" ;;
        *) printf 'FAIL unknown target: %s\n' "$t" >&2; exit 1 ;;
    esac
}

if $plan_mode; then
    for t in $LX86 $LARM $MX86 $MARM $WX86; do
        artifact_name "$t"
    done
    exit 0
fi

# Reject unknown targets before delegating (artifact_name exits non-zero).
name=$(artifact_name "$target")
script_dir=$(CDPATH="" cd "$(dirname "$0")" && pwd -P)
bran_root=$(CDPATH="" cd "$script_dir/../.." && pwd -P)

if [ ! -f "$bran_root/Cargo.lock" ]; then
    printf 'FAIL required lockfile is missing: %s\n' "$bran_root/Cargo.lock" >&2
    exit 1
fi

# The xtask performs the build and the deterministic packaging; cargo is the
# only runtime the release path needs.
cd "$bran_root"
exec cargo run --locked -p xtask -- package --target "$target" --tag "$tag" --dist "$dist"
