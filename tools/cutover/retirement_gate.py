#!/usr/bin/env python3
"""Offline, read-only verifier for BRAN retirement evidence."""
from __future__ import annotations
import argparse
import json
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
import consumer_gate as gate

NAMES = ["Alphazedehq", "alphazede-markets", "alphazede-sports", "betbot", "developers", "hgts"]


def inert_paths(values: object, label: str) -> list[str]:
    paths = gate.records(values, label, lambda x: x if isinstance(x, str) else "")
    for path in paths: gate.norm_path(path, label)
    return paths


def validate(manifest: object, root: Path) -> None:
    d = gate.exact(manifest, {"consumers", "active_reference_audit", "writes", "deletions", "recovery_archive", "restoration_proof", "owner_approval_reference", "apply_commands", "post_apply_commands"}, "RetirementManifest")
    consumers = gate.records(d["consumers"], "consumers", lambda x: x.get("consumer", "") if isinstance(x, dict) else "")
    if len(consumers) != 6: gate.die("wrong consumer count")
    receipts, identity = {}, None
    for item in consumers:
        item = gate.exact(item, {"consumer", "repository", "revision", "release_identity", "receipt", "receipt_sha256", "status"}, "consumer summary")
        path = gate.rel(root, item["receipt"], "receipt")
        if gate.digest(path) != gate.sha(item["receipt_sha256"], "receipt_sha256"): gate.die("receipt digest mismatch")
        receipt = gate.validate_captured_consumer(gate.strict_json(path, "consumer receipt"), root)
        if item["status"] != "passed" or any(item[key] != receipt[key] for key in ("consumer", "repository", "revision", "release_identity")): gate.die("receipt summary mismatch")
        current = gate.release(receipt["release_identity"])
        if identity is None: identity = current
        elif current != identity: gate.die("release identity mismatch")
        receipts[receipt["consumer"]] = receipt
    if list(receipts) != NAMES: gate.die("consumer inventory mismatch")
    audit = gate.exact(gate.strict_json(gate.evidence(root, d["active_reference_audit"], "active reference audit"), "active reference audit"), {"consumers", "status"}, "active reference audit")
    if audit["status"] != "passed": gate.die("active reference audit failed")
    rows = gate.records(audit["consumers"], "audit consumers", lambda x: x.get("consumer", "") if isinstance(x, dict) else "")
    if len(rows) != 6: gate.die("active reference audit failed")
    for item in rows:
        item = gate.exact(item, {"consumer", "repository", "revision", "release_identity", "inventory_digest", "status"}, "audit consumer")
        receipt = receipts.get(item["consumer"])
        if receipt is None or item["status"] != "passed" or any(item[key] != receipt[key] for key in ("repository", "revision", "release_identity")) or item["inventory_digest"] != receipt["reference_audit"]["inventory_digest"]: gate.die("audit consumer mismatch")
        gate.sha(item["inventory_digest"], "inventory_digest")
    if [item["consumer"] for item in rows] != NAMES: gate.die("audit consumer inventory mismatch")
    archive = gate.evidence(root, d["recovery_archive"], "recovery archive")
    proof = gate.exact(gate.strict_json(gate.evidence(root, d["restoration_proof"], "restoration proof"), "restoration proof"), {"byte_checks", "commands", "status"}, "restoration proof")
    gate.validate_rollback({"consumer": "retirement", "trigger": "proof", "from_digest": "0" * 64, "to_digest": "0" * 64, "restored_paths": [{"path": d["recovery_archive"]["locator"], "sha256": d["recovery_archive"]["sha256"], "source": "evidence"}], **proof}, root, root, "retirement")
    writes, deletions = inert_paths(d["writes"], "writes"), inert_paths(d["deletions"], "deletions")
    all_paths = writes + deletions
    if len(all_paths) != len(set(all_paths)): gate.die("write/deletion path overlap")
    for left in all_paths:
        for right in all_paths:
            if left != right and (left.startswith(right + "/") or right.startswith(left + "/")): gate.die("write/deletion path overlap")
    if not isinstance(d["owner_approval_reference"], str) or not d["owner_approval_reference"]: gate.die("missing approval reference")
    for name in ("apply_commands", "post_apply_commands"):
        commands = gate.records(d[name], name, lambda x: x if isinstance(x, str) else "")
        if not commands or not all(isinstance(x, str) and x for x in commands): gate.die("invalid inert commands")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__); parser.add_argument("mode", choices=("verify",)); parser.add_argument("--manifest", required=True, type=Path); parser.add_argument("--evidence", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.manifest.is_symlink() or not args.manifest.is_file(): gate.die("invalid manifest")
        validate(gate.strict_json(args.manifest, "retirement manifest"), gate.physical_dir(args.evidence, "evidence"))
        print('{"mode":"verify","status":"eligible"}'); return 0
    except ValueError as error:
        print(json.dumps({"mode": "verify", "status": "failed", "blocker": str(error)}, sort_keys=True, separators=(",", ":"))); return 1


if __name__ == "__main__": raise SystemExit(main())
