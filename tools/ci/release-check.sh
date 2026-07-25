#!/bin/sh
# Strict local release verification. No signing, publishing, tags, or network.
set -eu

usage() {
    printf 'usage: %s --tag TAG --dist DIR [--dry-run-unsigned] [--fingerprint FINGERPRINT]\n' "$0" >&2
    exit 2
}

tag=
dist=
dry=
fingerprint=
while [ $# -gt 0 ]; do
    case "$1" in
        --tag)
            [ $# -ge 2 ] || usage
            tag=$2
            shift 2
            ;;
        --dist)
            [ $# -ge 2 ] || usage
            dist=$2
            shift 2
            ;;
        --fingerprint)
            [ $# -ge 2 ] || usage
            fingerprint=$2
            shift 2
            ;;
        --dry-run-unsigned)
            dry=--dry-run-unsigned
            shift
            ;;
        *)
            usage
            ;;
    esac
done
[ -n "$tag" ] || usage
[ -n "$dist" ] || usage

script_dir=$(CDPATH="" cd -- "$(dirname -- "$0")" && pwd)
set -- python3 "$script_dir/release_seal.py" --tag "$tag" --dist "$dist"
if [ -n "$dry" ]; then
    set -- "$@" "$dry"
fi
if [ -n "$fingerprint" ]; then
    set -- "$@" --fingerprint "$fingerprint"
fi
exec "$@"
