#!/usr/bin/env python3
"""Verify one complete local BRAN release without network or mutation."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "ci"))
import release_contract_check as contract  # noqa: E402
import release_seal  # noqa: E402


def fail(message: str) -> int:
    print(f"FAIL: {message}")
    return 1


def load_json(path: Path) -> object:
    def unique_pairs(pairs: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate JSON key: {key}")
            result[key] = value
        return result

    try:
        return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique_pairs)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
        raise ValueError(f"invalid manifest JSON: {error}") from None


def regular(path: Path, label: str) -> None:
    if path.is_symlink():
        raise ValueError(f"symlink not permitted: {label}")
    if not path.is_file():
        raise ValueError(f"not a regular file: {label}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--asset", required=True, type=Path,
                        help="one release asset; all seven are verified in its directory")
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--certificate-identity", required=True)
    parser.add_argument("--certificate-oidc-issuer", required=True)
    args = parser.parse_args()
    manifest, selected = args.manifest, args.asset
    try:
        regular(manifest, "manifest")
        regular(selected, "asset")
        data = load_json(manifest)
        errors = contract.validate_manifest(data)
        if errors:
            raise ValueError("manifest contract: " + "; ".join(sorted(errors)))
        if not isinstance(data, dict):  # kept explicit for type narrowing
            raise ValueError("manifest root must be an object")
        if args.source_sha != data["source_commit"]:
            raise ValueError("source SHA mismatch")
        if args.certificate_identity != data["signature"]["certificate_identity"]:
            raise ValueError("signer certificate identity mismatch")
        if args.certificate_oidc_issuer != data["signature"]["certificate_oidc_issuer"]:
            raise ValueError("signer OIDC issuer mismatch")
        tag = data["tag"]
        names = contract.expected_asset_names(tag)
        if selected.name not in names:
            raise ValueError("--asset is not a release asset named by the manifest")
        directory = selected.parent
        disk_assets = {path.name for path in directory.iterdir()
                       if path.name.endswith((".tar.gz", ".zip"))}
        expected_archives = set(names[:5])
        if disk_assets != expected_archives:
            raise ValueError("missing or extra platform assets")
        entries = {asset["name"]: asset for asset in data["assets"]}
        for name in names:
            path = directory / name
            regular(path, name)
            if sha256(path) != entries[name]["sha256"]:
                raise ValueError(f"SHA-256 mismatch: {name}")
        expected_sums = "".join(f"{sha256(directory / name)}  {name}\n"
                                for name in sorted(names[:5]))
        if (directory / "SHA256SUMS").read_text(encoding="utf-8") != expected_sums:
            raise ValueError("SHA256SUMS does not match the five platform assets")
        verified_identity, verified_issuer, verified_at = release_seal.verify_signature(
            directory / "SHA256SUMS", directory / "SHA256SUMS.sigstore",
            certificate_identity=args.certificate_identity,
            certificate_oidc_issuer=args.certificate_oidc_issuer)
        if verified_identity != data["signature"]["certificate_identity"]:
            raise ValueError("verified signer certificate identity mismatch")
        if verified_issuer != data["signature"]["certificate_oidc_issuer"]:
            raise ValueError("verified signer OIDC issuer mismatch")
        if verified_at != data["signature"]["signed_at"]:
            raise ValueError("verified signature time mismatch")
    except (OSError, ValueError, TypeError, KeyError) as error:
        return fail(str(error))
    print("PASS: exact local release identity verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
