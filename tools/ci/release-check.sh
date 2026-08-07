#!/bin/sh
# Strict local release verification. No signing, publishing, tags, or network.
set -eu

usage() {
    printf 'usage: %s --tag TAG --dist DIR [--dry-run-unsigned] [--certificate-identity IDENTITY] [--certificate-oidc-issuer ISSUER]\n' "$0" >&2
    exit 2
}

tag=
dist=
dry=
certificate_identity=
certificate_oidc_issuer=
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
        --certificate-identity)
            [ $# -ge 2 ] || usage
            certificate_identity=$2
            shift 2
            ;;
        --certificate-oidc-issuer)
            [ $# -ge 2 ] || usage
            certificate_oidc_issuer=$2
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
if [ -n "$certificate_identity" ]; then
    set -- "$@" --certificate-identity "$certificate_identity"
fi
if [ -n "$certificate_oidc_issuer" ]; then
    set -- "$@" --certificate-oidc-issuer "$certificate_oidc_issuer"
fi
exec "$@"
