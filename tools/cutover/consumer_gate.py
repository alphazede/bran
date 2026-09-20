#!/usr/bin/env python3
"""Offline, read-only verifier for captured consumer cutover receipts."""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path, PurePosixPath
from urllib.parse import unquote, urlsplit

HEX = re.compile(r"[0-9a-f]{64}\Z")
REV = re.compile(r"[0-9a-f]{40}(?:[0-9a-f]{24})?\Z")
TAG = re.compile(r"bran-v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\Z")
OIDC_ISSUER = "https://token.actions.githubusercontent.com"
TIME = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z\Z")
STATE = {"passed", "failed", "unavailable", "rolled_back"}
KINDS = {"code", "skill", "hook", "ci", "configuration", "historical-documentation"}
CLASSES = {"native-active", "compatibility-active", "historical", "unexpected-active", "unclassified"}
NODE = {"locator", "precedence", "diagnostic_code", "conflict", "unavailable", "outcome"}
TARGETS = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-apple-darwin", "aarch64-apple-darwin", "x86_64-pc-windows-msvc")


def die(message: str) -> None: raise ValueError(message)


def exact(value: object, keys: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != keys: die(f"invalid {label} keys")
    return value


def strict_json(path: Path, label: str = "JSON") -> object:
    def pairs(items: list[tuple[str, object]]) -> dict:
        result = {}
        for key, value in items:
            if key in result: die(f"duplicate JSON key in {label}")
            result[key] = value
        return result
    try: return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=pairs)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as error: die(f"invalid {label}: {error}")


def canonical(value: object) -> bytes: return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def sha(value: object, label: str = "digest") -> str:
    if not isinstance(value, str) or not HEX.fullmatch(value): die(f"invalid {label}")
    return value


def physical_dir(root: Path, label: str) -> Path:
    root = root.absolute()
    if not root.is_dir() or any(part.is_symlink() for part in (root, *root.parents)): die(f"invalid {label} root")
    return root.resolve()


def norm_path(value: object, label: str) -> str:
    if not isinstance(value, str) or not value or value in {".", ".."} or "\\" in value or value.startswith("./") or value.endswith("/") or "//" in value: die(f"invalid {label}")
    path = PurePosixPath(value)
    if path.is_absolute() or any(part in {"", ".", ".."} for part in path.parts): die(f"invalid {label}")
    return value


def rel(root: Path, value: object, label: str) -> Path:
    root = physical_dir(root, label)
    path = norm_path(value, label)
    target = root.joinpath(*PurePosixPath(path).parts)
    current = root
    for part in PurePosixPath(path).parts:
        current = current / part
        if current.is_symlink(): die(f"symlink in {label}")
    try: resolved = target.resolve(strict=True)
    except OSError: die(f"invalid {label} file")
    try: resolved.relative_to(root)
    except ValueError: die(f"escaping {label}")
    if not resolved.is_file() or resolved.is_symlink(): die(f"invalid {label} file")
    return resolved


def digest(path: Path) -> str:
    if path.is_symlink() or not path.is_file(): die("not a regular non-symlink file")
    result = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""): result.update(chunk)
    return result.hexdigest()


def evidence(root: Path, value: object, label: str = "evidence") -> Path:
    item = exact(value, {"locator", "sha256"}, label)
    path = rel(root, item["locator"], label)
    if digest(path) != sha(item["sha256"], label + " digest"): die(f"{label} digest mismatch")
    return path


def records(values: object, label: str, key) -> list:
    if not isinstance(values, list): die(f"invalid {label}")
    keys = [key(item) for item in values]
    if keys != sorted(keys) or len(keys) != len(set(keys)): die(f"{label} must be sorted and unique")
    return values


def evidence_list(values: object, root: Path, label: str) -> list[dict]:
    items = records(values, label, lambda x: x.get("locator", "") if isinstance(x, dict) else "")
    return [exact(item, {"locator", "sha256"}, label) for item in items if evidence(root, item, label)]


def release(value: object) -> dict:
    data = exact(value, {"tag", "source_commit", "lockfile_sha256", "archives", "checksums_sha256", "signature_sha256", "certificate_identity", "certificate_oidc_issuer", "signed_at", "manifest_sha256", "asset_urls"}, "ReleaseIdentity")
    if not isinstance(data["tag"], str) or not TAG.fullmatch(data["tag"]) or not REV.fullmatch(data["source_commit"]) or not isinstance(data["certificate_identity"], str) or not data["certificate_identity"].startswith("https://") or data["certificate_oidc_issuer"] != OIDC_ISSUER or not isinstance(data["signed_at"], str) or not TIME.fullmatch(data["signed_at"]): die("invalid release identity")
    try:
        from datetime import datetime
        datetime.strptime(data["signed_at"], "%Y-%m-%dT%H:%M:%SZ")
    except ValueError: die("invalid signed_at")
    for key in ("lockfile_sha256", "checksums_sha256", "signature_sha256", "manifest_sha256"): sha(data[key], key)
    archives = exact(data["archives"], set(TARGETS), "archives")
    for item in archives.values(): sha(item, "archive digest")
    names = {f"{data['tag']}-{target}.tar.gz" if "windows" not in target else f"{data['tag']}-{target}.zip" for target in TARGETS} | {"SHA256SUMS", "SHA256SUMS.sigstore", "bran-release-manifest.json"}
    urls = records(data["asset_urls"], "asset_urls", lambda x: x if isinstance(x, str) else "")
    if len(urls) != 8: die("invalid asset URLs")
    base = f"/alphazede/bran/releases/download/{data['tag']}/"
    found = set()
    for url in urls:
        if not isinstance(url, str): die("invalid asset URL")
        parsed = urlsplit(url)
        if parsed.scheme != "https" or parsed.hostname != "github.com" or parsed.username or parsed.password or parsed.port not in (None, 443) or parsed.query or parsed.fragment or "%" in parsed.path or unquote(parsed.path) != parsed.path or not parsed.path.startswith(base): die("invalid asset URL")
        name = parsed.path[len(base):]
        if "/" in name or name not in names: die("invalid asset URL")
        found.add(name)
    if found != names: die("invalid asset URLs")
    return data


def state(value: object, label: str) -> None:
    if value not in STATE: die(f"invalid {label} status")


def validate_install(value: object, root: Path, identity: dict, consumer: str, revision: str) -> None:
    d = exact(value, {"consumer", "consumer_revision", "release_identity", "prior_pin", "prior_digest", "staged_path", "selected_pin", "selected_digest", "verification_status"}, "InstallSnapshot")
    if d["consumer"] != consumer or d["consumer_revision"] != revision or release(d["release_identity"]) != identity or not isinstance(d["selected_pin"], str) or not d["selected_pin"]: die("install identity mismatch")
    if digest(rel(root, d["prior_pin"], "prior_pin")) != sha(d["prior_digest"], "prior_digest") or digest(rel(root, d["staged_path"], "staged_path")) != sha(d["selected_digest"], "selected_digest") or d["verification_status"] != "passed": die("install verification failed")


def node(value: object) -> dict:
    d = exact(value, NODE, "semantic node")
    if not isinstance(d["unavailable"], bool) or any(isinstance(d[key], (dict, list)) for key in NODE - {"unavailable"}): die("invalid semantic node")
    return d


def validate_parity(value: object, root: Path, consumer: str) -> str:
    d = exact(value, {"consumer", "corpus_digest", "native_command", "legacy_command", "native_raw", "legacy_raw", "normalizer_version", "semantic_rows", "validation_status", "retrieval_status", "overall_status"}, "ParityReceipt")
    if d["consumer"] != consumer or not isinstance(d["normalizer_version"], str) or not d["normalizer_version"]: die("invalid parity identity")
    sha(d["corpus_digest"], "corpus_digest")
    if not all(isinstance(d[key], str) and d[key] for key in ("native_command", "legacy_command")): die("invalid inert command")
    rows = records(d["semantic_rows"], "semantic_rows", lambda x: x.get("semantic_key", "") if isinstance(x, dict) else "")
    normalized = []
    for row in rows:
        row = exact(row, {"semantic_key", "native", "legacy"}, "semantic row")
        if not isinstance(row["semantic_key"], str) or not row["semantic_key"]: die("invalid semantic key")
        normalized.append(row); node(row["native"]); node(row["legacy"])
    for side, key in (("native", "native_raw"), ("legacy", "legacy_raw")):
        raw = strict_json(evidence(root, d[key], key), key)
        if not isinstance(raw, list) or len(raw) != len(normalized): die("parity rows do not normalize one-to-one")
        raw_keys, raw_bytes = [], set()
        for index, item in enumerate(raw):
            item = exact(item, {"semantic_key"} | NODE, "raw semantic parity entry")
            if not isinstance(item["semantic_key"], str) or not item["semantic_key"]: die("invalid raw semantic key")
            encoded = canonical(item)
            if encoded in raw_bytes: die("duplicate semantic raw entry")
            raw_bytes.add(encoded)
            raw_keys.append(item["semantic_key"])
            if item != {"semantic_key": normalized[index]["semantic_key"], **normalized[index][side]}: die("raw semantic parity mismatch")
        if raw_keys != sorted(raw_keys) or len(raw_keys) != len(set(raw_keys)) or raw_keys != [row["semantic_key"] for row in normalized]: die("duplicate semantic raw entry")
    for key in ("validation_status", "retrieval_status", "overall_status"):
        state(d[key], key)
        if d[key] == "unavailable": die("parity unavailable")
        if d[key] != "passed": die("parity not passed")
    return d["corpus_digest"]


def validate_hook(value: object, root: Path) -> None:
    d = exact(value, {"commands", "evidence", "status"}, "hook check")
    commands = records(d["commands"], "hook commands", lambda x: x if isinstance(x, str) else "")
    state(d["status"], "hook check")
    if d["status"] != "passed" or not commands or not all(isinstance(x, str) and x for x in commands): die("hook check failed")
    evidence_list(d["evidence"], root, "hook evidence")


def validate_reference(value: object, root: Path, consumer_root: Path | None, consumer: str, revision: str) -> dict:
    d = exact(value, {"consumer", "revision", "expected_compatibility", "matches", "inventory_digest", "evidence", "status"}, "reference audit")
    state(d["status"], "reference audit")
    if d["consumer"] != consumer or d["revision"] != revision or d["status"] != "passed": die("reference audit identity/status")
    expected = records(d["expected_compatibility"], "expected compatibility", lambda x: (x.get("path", ""), x.get("kind", "")) if isinstance(x, dict) else ("", ""))
    exp = set()
    for item in expected:
        item = exact(item, {"path", "kind"}, "expected compatibility")
        norm_path(item["path"], "consumer path")
        if item["kind"] not in KINDS: die("invalid compatibility kind")
        exp.add((item["path"], item["kind"]))
    matches = records(d["matches"], "matches", lambda x: (x.get("path", ""), x.get("line", 0), x.get("kind", ""), x.get("classification", "")) if isinstance(x, dict) else ("", 0, "", ""))
    active = set()
    for item in matches:
        item = exact(item, {"path", "line", "kind", "classification", "match_sha256"}, "reference match")
        norm_path(item["path"], "consumer path")
        if not isinstance(item["line"], int) or isinstance(item["line"], bool) or item["line"] < 1 or item["kind"] not in KINDS or item["classification"] not in CLASSES: die("invalid reference match")
        sha(item["match_sha256"], "match digest")
        if consumer_root is not None:
            path = rel(consumer_root, item["path"], "consumer path")
            try: line = path.read_bytes().splitlines(keepends=True)[item["line"] - 1]
            except IndexError: die("matched line missing")
            if hashlib.sha256(line).hexdigest() != item["match_sha256"]: die("matched line digest")
        if item["classification"] in {"unexpected-active", "unclassified"}: die("active reference failure")
        if item["classification"] == "compatibility-active": active.add((item["path"], item["kind"]))
    if active != exp or hashlib.sha256(canonical(matches)).hexdigest() != sha(d["inventory_digest"], "inventory_digest"): die("compatibility inventory mismatch")
    d["evidence"] = evidence_list(d["evidence"], root, "audit evidence")
    return d


def validate_rollback(value: object, root: Path, consumer_root: Path | None, expected_consumer: str | None = None, captured_proofs: list[dict] = []) -> None:
    d = exact(value, {"consumer", "trigger", "from_digest", "to_digest", "restored_paths", "byte_checks", "commands", "status"}, "RollbackReceipt")
    state(d["status"], "rollback")
    if (expected_consumer is not None and d["consumer"] != expected_consumer) or not isinstance(d["consumer"], str) or not d["consumer"] or not isinstance(d["trigger"], str) or not d["trigger"] or d["status"] != "passed": die("rollback failed")
    sha(d["from_digest"], "from_digest"); sha(d["to_digest"], "to_digest")
    proof_pairs = {(item["locator"], item["sha256"]) for item in captured_proofs}
    commands = records(d["commands"], "commands", lambda x: x.get("command", "") if isinstance(x, dict) else "")
    if not commands: die("missing rollback proof")
    for item in commands:
        item = exact(item, {"command", "exit_code", "status", "evidence"}, "commands")
        if not isinstance(item["command"], str) or not item["command"] or item["exit_code"] != 0 or item["status"] != "passed": die("command receipt failed")
        evidence(root, item["evidence"], "command evidence")
        proof_pairs.add((item["evidence"]["locator"], item["evidence"]["sha256"]))
    restored = records(d["restored_paths"], "restored paths", lambda x: x.get("path", "") if isinstance(x, dict) else "")
    if not restored: die("missing restored paths")
    for item in restored:
        item = exact(item, {"path", "sha256", "source"}, "restored path")
        if item["source"] not in {"evidence", "consumer"}: die("invalid restored source")
        norm_path(item["path"], "restored path"); sha(item["sha256"], "restored digest")
        if consumer_root is None and item["source"] == "consumer":
            if (item["path"], item["sha256"]) not in proof_pairs: die("missing captured consumer-byte proof")
        else:
            target_root = root if item["source"] == "evidence" else consumer_root
            if digest(rel(target_root, item["path"], "restored path")) != item["sha256"]: die("restored digest mismatch")
    checks = records(d["byte_checks"], "byte_checks", lambda x: x.get("path", "") if isinstance(x, dict) else "")
    if not checks: die("missing rollback proof")
    for item in checks:
        item = exact(item, {"path", "expected", "actual", "status"}, "byte_checks")
        norm_path(item["path"], "byte check path")
        if item["status"] != "passed" or sha(item["expected"], "expected") != sha(item["actual"], "actual"): die("byte_checks failed")


def clean_head(root: Path, revision: str) -> None:
    if not REV.fullmatch(revision): die("invalid revision")
    try:
        head = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip(); dirty = subprocess.check_output(["git", "-C", str(root), "status", "--porcelain"], text=True)
    except (OSError, subprocess.CalledProcessError): die("consumer is not a git checkout")
    if dirty or head != revision: die("consumer revision is not clean HEAD")


def receipt_root(receipt: object) -> dict:
    d = exact(receipt, {"consumer", "repository", "revision", "release_identity", "install", "validation_parity", "retrieval_parity", "hook_check", "reference_audit", "rollback", "status", "blockers"}, "ConsumerGateReceipt")
    if not isinstance(d["consumer"], str) or not d["consumer"] or not isinstance(d["repository"], str) or not d["repository"] or not isinstance(d["revision"], str) or not REV.fullmatch(d["revision"]): die("consumer receipt identity")
    state(d["status"], "consumer")
    if not isinstance(d["blockers"], list) or d["blockers"] != sorted(set(d["blockers"])) or not all(isinstance(x, str) and x for x in d["blockers"]): die("invalid blockers")
    if (d["status"] == "passed") != (not d["blockers"]): die("consumer blocker/status mismatch")
    if d["status"] != "passed": die("consumer receipt not passed")
    return d


def validate_receipt(receipt: object, evidence_root: Path, consumer_root: Path | None, revision: str | None = None) -> dict:
    d = receipt_root(receipt)
    if revision is not None and d["revision"] != revision: die("consumer receipt revision")
    identity = release(d["release_identity"])
    validate_install(d["install"], evidence_root, identity, d["consumer"], d["revision"])
    if validate_parity(d["validation_parity"], evidence_root, d["consumer"]) != validate_parity(d["retrieval_parity"], evidence_root, d["consumer"]): die("parity corpus mismatch")
    validate_hook(d["hook_check"], evidence_root)
    audit = validate_reference(d["reference_audit"], evidence_root, consumer_root, d["consumer"], d["revision"])
    validate_rollback(d["rollback"], evidence_root, consumer_root, d["consumer"], audit["evidence"])
    return d


def validate_consumer(receipt: object, evidence_root: Path, consumer_root: Path, revision: str) -> dict:
    clean_head(physical_dir(consumer_root, "consumer"), revision)
    return validate_receipt(receipt, evidence_root, consumer_root, revision)


def validate_captured_consumer(receipt: object, evidence_root: Path) -> dict:
    return validate_receipt(receipt, evidence_root, None)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("install-verify", "parity", "reference-audit", "rollback")); parser.add_argument("--consumer", required=True, type=Path); parser.add_argument("--revision", required=True); parser.add_argument("--manifest", required=True, type=Path); parser.add_argument("--evidence", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.manifest.is_symlink() or not args.manifest.is_file(): die("invalid manifest")
        receipt = validate_consumer(strict_json(args.manifest, "consumer manifest"), physical_dir(args.evidence, "evidence"), physical_dir(args.consumer, "consumer"), args.revision)
        print(json.dumps({"consumer": receipt["consumer"], "mode": args.mode, "status": "passed"}, sort_keys=True, separators=(",", ":"))); return 0
    except ValueError as error:
        print(json.dumps({"mode": args.mode, "status": "failed", "blocker": str(error)}, sort_keys=True, separators=(",", ":"))); return 1


if __name__ == "__main__": raise SystemExit(main())
