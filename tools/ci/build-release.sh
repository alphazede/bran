#!/bin/sh
# build-release.sh: plan + per-target package for exact 5 cross artifacts.
# --plan validates names only (non-mutating). Build uses --locked, fails on missing target.
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

name=$(artifact_name "$target")
script_dir=$(CDPATH="" cd "$(dirname "$0")" && pwd -P)
bran_root=$(CDPATH="" cd "$script_dir/../.." && pwd -P)

if [ ! -f "$bran_root/Cargo.lock" ]; then
    printf 'FAIL required lockfile is missing: %s\n' "$bran_root/Cargo.lock" >&2
    exit 1
fi

mkdir -p "$dist"
printf 'BUILD target=%s tag=%s artifact=%s\n' "$target" "$tag" "$name"

cd "$bran_root"
cargo build --release --locked --target "$target" --bin bran

case "$target" in
    "$WX86") bin_path="target/$target/release/bran.exe" ;;
    *) bin_path="target/$target/release/bran" ;;
esac

[ -f "$bin_path" ] || { printf 'FAIL binary not found: %s\n' "$bin_path" >&2; exit 1; }

out="$dist/$name"
case "$target" in
    "$WX86") member=bran.exe; package=zip ;;
    *) member=bran; package=tar.gz ;;
esac

python3 - "$bin_path" "$out" "$member" "$package" <<'PY'
import gzip
import io
import os
import sys
import tarfile
import zipfile

binary, output, member, package = sys.argv[1:]
with open(binary, "rb") as source:
    data = source.read()

if package == "tar.gz":
    with open(output, "wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
                info = tarfile.TarInfo(member)
                info.type = tarfile.REGTYPE
                info.mode = 0o755
                info.uid = info.gid = info.mtime = 0
                info.uname = info.gname = ""
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
elif package == "zip":
    info = zipfile.ZipInfo(member, (1980, 1, 1, 0, 0, 0))
    info.create_system = 3
    info.external_attr = 0o100755 << 16
    info.compress_type = zipfile.ZIP_STORED
    info.extra = info.comment = b""
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_STORED) as archive:
        archive.comment = b""
        archive.writestr(info, data)
else:
    raise RuntimeError("unsupported package format")
PY
[ -f "$out" ] || { printf 'FAIL no archive: %s\n' "$out" >&2; exit 1; }
printf 'CREATED %s\n' "$out"
exit 0
