#!/usr/bin/env python3
"""Run the single P1 exact-release contract journey without third-party packages."""

from __future__ import annotations

import json
import re
import sys
from datetime import datetime
from pathlib import Path
from typing import Any


SCHEMA_VERSION = "1.0.0"
REPOSITORY = "alphazede/bran"
TAG_PATTERN = re.compile(r"bran-v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\Z")
SHA256_PATTERN = re.compile(r"[0-9a-f]{64}\Z")
GIT_SHA_PATTERN = re.compile(r"[0-9a-f]{40}\Z")
FINGERPRINT_PATTERN = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})\Z")
STRICT_SIGNED_AT_REGEX = r"^(?:(?:000[1-9]|00[1-9][0-9]|0[1-9][0-9]{2}|[1-9][0-9]{3})-(?:(?:(?:0[13578]|1[02]))-(?:0[1-9]|[12][0-9]|3[01])|(?:(?:0[469]|11))-(?:0[1-9]|[12][0-9]|30)|02-(?:0[1-9]|1[0-9]|2[0-8]))|(?:[0-9]{2}(?:0[48]|[2468][048]|[13579][26])|(?:04|08|12|16|20|24|28|32|36|40|44|48|52|56|60|64|68|72|76|80|84|88|92|96)00)-02-29)T(?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]Z$"
SIGNED_AT_PATTERN = re.compile(STRICT_SIGNED_AT_REGEX)
RELEASE_BASE = "https://github.com/alphazede/bran/releases/download"
SEMANTIC_ORACLE = "tools/ci/release_contract_check.py"

# Expected schema constants for drift detection (exact strings must match schema)
# STRICT_SIGNED_AT_REGEX and EXPECTED_SIGNED_AT_PATTERN are identical (schema pattern uses $ anchors; datetime.strptime is semantic defense)
EXPECTED_FINGERPRINT_PATTERN = r"^(?:[0-9a-f]{40}|[0-9a-f]{64})$"
EXPECTED_SIGNED_AT_PATTERN = STRICT_SIGNED_AT_REGEX
EXPECTED_SIGNED_AT_FORMAT = "date-time"


def bran_root() -> Path:
    return Path(__file__).resolve().parents[2]


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"invalid JSON: {path.relative_to(bran_root())}: {error}") from error


def is_sha256(value: Any) -> bool:
    return isinstance(value, str) and SHA256_PATTERN.fullmatch(value) is not None


def is_git_sha(value: Any) -> bool:
    return isinstance(value, str) and GIT_SHA_PATTERN.fullmatch(value) is not None


def is_fingerprint(value: Any) -> bool:
    return isinstance(value, str) and FINGERPRINT_PATTERN.fullmatch(value) is not None


def is_strict_utc_datetime(value: Any) -> bool:
    """Strict UTC shape + real calendar/time via stdlib (no tz, no third-party).
    Regex (strict Gregorian via single anchored ECMA pattern) for lexical validity,
    then strptime for semantic range/Gregorian defense (incl. leap days).
    After regex+strptime, reconstruct with explicit zero-padding (no strftime roundtrip:
    avoids platform-dependent behavior that rejects contract-valid years 0001-0999).
    """
    if not isinstance(value, str) or SIGNED_AT_PATTERN.fullmatch(value) is None:
        return False
    try:
        dt = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ")
        # explicit numeric zero-pad formatting; datetime.strptime already validated semantics
        reconstructed = f"{dt.year:04d}-{dt.month:02d}-{dt.day:02d}T{dt.hour:02d}:{dt.minute:02d}:{dt.second:02d}Z"
        return reconstructed == value
    except ValueError:
        return False


def is_object_with_keys(value: Any, keys: set[str]) -> bool:
    return isinstance(value, dict) and set(value) == keys


def expected_asset_names(tag: str) -> tuple[str, ...]:
    return (
        f"{tag}-x86_64-unknown-linux-gnu.tar.gz",
        f"{tag}-aarch64-unknown-linux-gnu.tar.gz",
        f"{tag}-x86_64-apple-darwin.tar.gz",
        f"{tag}-aarch64-apple-darwin.tar.gz",
        f"{tag}-x86_64-pc-windows-msvc.zip",
        "SHA256SUMS",
        "SHA256SUMS.sig",
    )


def validate_asset(asset: Any, tag: str, expected_names: set[str], errors: list[str]) -> None:
    if not is_object_with_keys(asset, {"name", "url", "sha256", "media_type"}):
        errors.append("asset must contain exactly name, url, sha256, and media_type")
        return

    name = asset["name"]
    url = asset["url"]
    if not isinstance(name, str) or name not in expected_names:
        errors.append(f"asset has an invalid name: {name!r}")
    if not isinstance(url, str) or url != f"{RELEASE_BASE}/{tag}/{name}":
        errors.append(f"asset {name!r} URL is not a direct URL under exact tag {tag}")
    if isinstance(url, str) and "/releases/latest" in url:
        errors.append(f"asset {name!r} URL must not use /releases/latest")
    if not is_sha256(asset["sha256"]):
        errors.append(f"asset {name!r} has an invalid sha256")
    if not isinstance(asset["media_type"], str) or not asset["media_type"].strip():
        errors.append(f"asset {name!r} has empty media_type")


def validate_manifest(manifest: Any) -> list[str]:
    errors: list[str] = []
    required_keys = {
        "schema_version", "tag", "repository", "source_commit", "lockfile_sha256",
        "immutable", "manifest_asset", "assets", "checksums", "signature", "provenance",
    }
    if not is_object_with_keys(manifest, required_keys):
        return ["manifest must contain exactly the required release-contract fields"]

    if manifest["schema_version"] != SCHEMA_VERSION:
        errors.append(f"schema_version must be {SCHEMA_VERSION}")
    tag = manifest["tag"]
    if not isinstance(tag, str) or TAG_PATTERN.fullmatch(tag) is None:
        errors.append("tag must be canonical bran-vX.Y.Z")
        tag = "<invalid-tag>"
    if manifest["repository"] != REPOSITORY:
        errors.append(f"repository must be {REPOSITORY}")
    if not is_git_sha(manifest["source_commit"]):
        errors.append("source_commit must be a lowercase 40-hex git SHA-1")
    if not is_sha256(manifest["lockfile_sha256"]):
        errors.append("lockfile_sha256 must be a lowercase 64-hex value")
    if manifest["immutable"] is not True:
        errors.append("immutable must be true")
    if manifest["manifest_asset"] != "bran-release-manifest.json":
        errors.append("manifest_asset must be bran-release-manifest.json")

    assets = manifest["assets"]
    expected_names = expected_asset_names(tag)
    expected_name_set = set(expected_names)
    if not isinstance(assets, list) or len(assets) != len(expected_names):
        errors.append("assets must contain exactly seven entries")
        assets = []
    names: list[Any] = []
    for asset in assets:
        validate_asset(asset, tag, expected_name_set, errors)
        if isinstance(asset, dict):
            names.append(asset.get("name"))
    if len(names) != len(set(names)):
        errors.append("assets must be unique")
    if set(names) != expected_name_set:
        errors.append("assets do not match the required release asset names")

    assets_by_name = {
        asset["name"]: asset for asset in assets
        if isinstance(asset, dict) and isinstance(asset.get("name"), str)
    }
    checksums = manifest["checksums"]
    if not is_object_with_keys(checksums, {"asset", "algorithm", "sha256"}):
        errors.append("checksums must contain exactly asset, algorithm, and sha256")
    else:
        if checksums["asset"] != "SHA256SUMS" or checksums["algorithm"] != "sha256":
            errors.append("checksums must describe SHA256SUMS with sha256")
        if not is_sha256(checksums["sha256"]):
            errors.append("checksums.sha256 must be lowercase 64-hex")
        elif assets_by_name.get("SHA256SUMS", {}).get("sha256") != checksums["sha256"]:
            errors.append("checksums.sha256 must match the SHA256SUMS asset")

    signature = manifest["signature"]
    if not is_object_with_keys(signature, {"asset", "format", "key_fingerprint", "signed_at"}):
        errors.append("signature must contain exactly asset, format, key_fingerprint, and signed_at")
    else:
        if signature["asset"] != "SHA256SUMS.sig" or signature["format"] != "openpgp":
            errors.append("signature must describe SHA256SUMS.sig in openpgp format")
        if not is_fingerprint(signature["key_fingerprint"]):
            errors.append("signature.key_fingerprint must be exactly 40 or 64 lowercase hex")
        if not is_strict_utc_datetime(signature["signed_at"]):
            errors.append("signature.signed_at must be strict UTC YYYY-MM-DDTHH:MM:SSZ")

    provenance = manifest["provenance"]
    provenance_keys = {
        "format", "predicate_type", "source_repository", "source_commit", "lockfile_sha256", "build_type",
    }
    if not is_object_with_keys(provenance, provenance_keys):
        errors.append("provenance must contain exactly the required provenance fields")
    else:
        if provenance["format"] != "https://slsa.dev/provenance/v1" or provenance["predicate_type"] != "https://slsa.dev/provenance/v1":
            errors.append("provenance format and predicate_type must be SLSA v1")
        if provenance["source_repository"] != REPOSITORY:
            errors.append(f"provenance.source_repository must be {REPOSITORY}")
        if not is_git_sha(provenance["source_commit"]):
            errors.append("provenance.source_commit must be lowercase 40-hex git SHA-1")
        elif provenance["source_commit"] != manifest["source_commit"]:
            errors.append("provenance.source_commit must match source_commit")
        if not is_sha256(provenance["lockfile_sha256"]):
            errors.append("provenance.lockfile_sha256 must be lowercase 64-hex")
        elif provenance["lockfile_sha256"] != manifest["lockfile_sha256"]:
            errors.append("provenance.lockfile_sha256 must match lockfile_sha256")
        if not isinstance(provenance["build_type"], str) or not provenance["build_type"].startswith("https://"):
            errors.append("provenance.build_type must be an https URL")

    return errors


def main() -> int:
    root = bran_root()
    schema_path = root / "schemas/bran-release-manifest.schema.json"
    manifest_path = root / "fixtures/release/valid-exact-release-manifest.json"
    try:
        schema = load_json(schema_path)
        manifest = load_json(manifest_path)
    except ValueError as error:
        print(f"FAIL release contract check: {error}")
        return 1

    if not isinstance(schema, dict) or schema.get("$id") != "https://schemas.alphazede.dev/bran/release-manifest/v1/schema.json":
        print("FAIL release contract check: schema is not the Bran v1 release-manifest schema")
        return 1
    if schema.get("x-semantic-oracle") != SEMANTIC_ORACLE:
        print("FAIL release contract check: schema missing or incorrect x-semantic-oracle annotation for stdlib checker")
        return 1

    signature = (schema.get("properties") or {}).get("signature", {}).get("properties", {}) or {}
    if signature.get("key_fingerprint", {}).get("pattern") != EXPECTED_FINGERPRINT_PATTERN:
        print("FAIL release contract check: schema key_fingerprint pattern drifted from expected constant")
        return 1
    signed_at = signature.get("signed_at", {})
    if signed_at.get("pattern") != EXPECTED_SIGNED_AT_PATTERN or signed_at.get("format") != EXPECTED_SIGNED_AT_FORMAT:
        print("FAIL release contract check: schema signed_at pattern or format drifted from expected constants")
        return 1

    accepted = validate_manifest(manifest)
    if accepted:
        print("FAIL release contract check: exact-release manifest was rejected")
        for error in accepted:
            print(f"  {error}")
        return 1

    manifest["assets"][0]["url"] = "https://github.com/alphazede/bran/releases/latest/download/bran-v1.2.3-x86_64-unknown-linux-gnu.tar.gz"
    rejected = validate_manifest(manifest)
    if not rejected:
        print("FAIL release contract check: exact-release lifecycle accepted a floating latest URL")
        return 1

    print("PASS P1-RELEASE: exact release accepted and floating latest URL rejected")
    return 0


if __name__ == "__main__":
    sys.exit(main())
