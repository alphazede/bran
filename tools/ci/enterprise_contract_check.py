#!/usr/bin/env python3
"""Validate the V1 enterprise-document evidence-envelope contract without third-party packages."""

from __future__ import annotations

import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any


SCHEMA_VERSION = "1.0.0"
SCHEMA_ID = (
    "https://schemas.alphazede.dev/bran/enterprise-document-evidence-envelope/v1/schema.json"
)
SEMANTIC_ORACLE = "tools/ci/enterprise_contract_check.py"
SHA256_PATTERN = re.compile(r"[0-9a-f]{64}\Z")
ID_PATTERN = re.compile(r"^[A-Za-z0-9./:_-]+$")
FEATURE_PATTERN = re.compile(r"^[a-z][a-z0-9_]*$")
LANGUAGE_PATTERN = re.compile(r"^[a-z]{2}(?:-[A-Z]{2})?$")
ASSET_PATH_PATTERN = re.compile(r"^[A-Za-z0-9._-]+(?:/[A-Za-z0-9._-]+)*$")
MEDIA_TYPES = {
    "application/pdf": "fixed-layout",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document": "flow",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation": "presentation",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet": "grid",
}
FAMILIES = frozenset(MEDIA_TYPES.values())
FIDELITY_VALUES = ("exact", "normalized", "approximated", "unsupported")
NEVER_EXACT_FEATURES = frozenset(
    {
        "macros",
        "formulas",
        "javascript",
        "animations",
        "embedded_objects",
        "external_relationships",
        "round_trip",
    }
)
FAMILY_FEATURES = {
    "fixed-layout": (
        "text",
        "reading_order",
        "bounding_boxes",
        "tables",
        "figures",
        "ocr",
        "javascript",
    ),
    "flow": (
        "text",
        "paragraphs",
        "headings",
        "lists",
        "tables",
        "headers_footers",
        "macros",
    ),
    "presentation": (
        "text",
        "slides",
        "shapes",
        "speaker_notes",
        "z_order",
        "macros",
        "animations",
    ),
    "grid": (
        "text",
        "sheets",
        "cells",
        "formulas",
        "charts",
        "macros",
    ),
}
ANCHOR_ROLES = {
    "fixed-layout": frozenset(
        {"heading", "paragraph", "table", "figure", "header", "footer", "title", "list"}
    ),
    "flow": frozenset(
        {"heading", "paragraph", "table", "list", "header", "footer", "title"}
    ),
    "presentation": frozenset(
        {"heading", "paragraph", "table", "list", "title", "notes", "shape"}
    ),
    "grid": frozenset({"sheet", "cell", "range", "table", "header", "chart"}),
}
RELATION_KINDS = frozenset({"derived-from", "cites", "extracts", "summarizes"})
ACYCLIC_RELATIONS = frozenset({"derived-from", "extracts"})
REQUIRED_KEYS = (
    "schema_version",
    "evidence_id",
    "envelope_digest",
    "original",
    "parser",
    "source",
    "normalized",
    "anchors",
    "assets",
    "relations",
    "fidelity",
    "receipts",
    "hazards",
    "policy",
    "admission",
)
MAX_ENVELOPE_BYTES = 1_048_576
MAX_ORIGINAL_BYTES = 20_971_520
MAX_ANCHORS = 4096
MAX_ASSETS = 256
MAX_RELATIONS = 8192
MAX_TEXT_BYTES = 8192
MAX_ID_BYTES = 256
MAX_PATH_BYTES = 256
MAX_FINDINGS = 4096
MAX_FINDING_BYTES = 128
POSITIVE_NAMES = (
    "pdf-fixed-layout.json",
    "docx-flow.json",
    "pptx-presentation.json",
    "xlsx-grid.json",
)
NEGATIVE_REASONS = {
    "digest-mismatch.json": "digest-mismatch",
    "unsafe-asset-path.json": "unsafe-asset-path",
    "active-content.json": "active-content",
    "external-reference.json": "external-reference",
    "malformed-structure.json": "malformed-structure",
    "oversized.json": "oversized",
    "secret-reflection.json": "secret-reflection",
    "unsupported-evidence.json": "unsupported-evidence",
}
SECRET_MARKERS = (
    "-----BEGIN ",
    "AIza",
    "X-Goog-Credential=",
    "X-Goog-Signature=",
    "access_token=",
    "private_key",
    "refresh_token=",
    "ya29.",
)


def bran_root() -> Path:
    return Path(__file__).resolve().parents[2]


def canonical_bytes(value: object) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode("utf-8")


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def text_digest(text: str) -> str:
    return sha256_hex(text.encode("utf-8"))


def walk_strings(value: object) -> list[str]:
    found: list[str] = []
    pending: list[object] = [value]
    while pending:
        current = pending.pop()
        if isinstance(current, str):
            found.append(current)
        elif isinstance(current, dict):
            pending.extend(current.values())
        elif isinstance(current, list):
            pending.extend(current)
    return found


def contains_secret(value: object) -> bool:
    return any(
        marker in text for text in walk_strings(value) for marker in SECRET_MARKERS
    )


def envelope_digest(envelope: dict[str, Any]) -> str:
    body = {key: value for key, value in envelope.items() if key != "envelope_digest"}
    return sha256_hex(canonical_bytes(body))


def normalized_digest(content: object) -> str:
    return sha256_hex(canonical_bytes(content))


def is_sha256(value: object) -> bool:
    return isinstance(value, str) and SHA256_PATTERN.fullmatch(value) is not None


def is_id(value: object) -> bool:
    return (
        isinstance(value, str)
        and 1 <= len(value) <= MAX_ID_BYTES
        and ID_PATTERN.fullmatch(value) is not None
    )


def is_nonneg_int(value: object, maximum: int) -> bool:
    return type(value) is int and 0 <= value <= maximum


def is_pos_int(value: object, maximum: int) -> bool:
    return type(value) is int and 1 <= value <= maximum


def object_without_duplicates(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate object key {key!r}")
        result[key] = value
    return result


def load_json(path: Path) -> Any:
    try:
        return json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=object_without_duplicates,
        )
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
        raise ValueError(f"invalid JSON: {path.as_posix()}: {error}") from error


def permute(value: object) -> object:
    if isinstance(value, dict):
        items = list(value.items())
        items.reverse()
        return {key: permute(item) for key, item in items}
    if isinstance(value, list):
        return [permute(item) for item in value]
    return value


def keys_of(value: object, expected: set[str]) -> bool:
    return isinstance(value, dict) and set(value) == expected


def sorted_unique(values: list[str]) -> bool:
    return values == sorted(set(values))


def unsafe_asset_path(path: object) -> bool:
    if not isinstance(path, str) or not path or len(path) > MAX_PATH_BYTES:
        return True
    if path.startswith("/") or path.startswith("\\") or "\\" in path or "\x00" in path:
        return True
    if ASSET_PATH_PATTERN.fullmatch(path) is None:
        return True
    return any(part in {".", ".."} for part in path.split("/"))


def locator_error(family: str, locator: object) -> str | None:
    if not isinstance(locator, dict) or locator.get("family") != family:
        return "malformed-structure"
    if family == "fixed-layout":
        if set(locator) != {"family", "page", "block", "bbox"}:
            return "malformed-structure"
        bbox = locator["bbox"]
        if not keys_of(bbox, {"x0", "y0", "x1", "y1"}):
            return "malformed-structure"
        if not is_pos_int(locator["page"], 1_000_000) or not is_pos_int(
            locator["block"], 1_000_000
        ):
            return "malformed-structure"
        for name in ("x0", "y0", "x1", "y1"):
            if not is_nonneg_int(bbox[name], 1_000_000):
                return "malformed-structure"
        if bbox["x1"] < bbox["x0"] or bbox["y1"] < bbox["y0"]:
            return "malformed-structure"
        return None
    if family == "flow":
        if set(locator) != {"family", "section", "ordinal"}:
            return "malformed-structure"
        if not isinstance(locator["section"], str) or not locator["section"].strip():
            return "malformed-structure"
        if len(locator["section"]) > MAX_ID_BYTES:
            return "malformed-structure"
        if not is_pos_int(locator["ordinal"], 1_000_000):
            return "malformed-structure"
        return None
    if family == "presentation":
        if set(locator) != {"family", "slide", "shape", "z_index"}:
            return "malformed-structure"
        if not is_pos_int(locator["slide"], 1_000_000) or not is_pos_int(
            locator["shape"], 1_000_000
        ):
            return "malformed-structure"
        if not is_nonneg_int(locator["z_index"], 1_000_000):
            return "malformed-structure"
        return None
    if family == "grid":
        if set(locator) != {"family", "sheet", "row", "column"}:
            return "malformed-structure"
        if not isinstance(locator["sheet"], str) or not locator["sheet"].strip():
            return "malformed-structure"
        if len(locator["sheet"]) > MAX_ID_BYTES:
            return "malformed-structure"
        if not is_pos_int(locator["row"], 1_048_576) or not is_pos_int(
            locator["column"], 16_384
        ):
            return "malformed-structure"
        return None
    return "unsupported-evidence"


def has_cycle(nodes: set[str], edges: list[tuple[str, str]]) -> bool:
    outgoing: dict[str, list[str]] = {node: [] for node in nodes}
    for start, end in edges:
        outgoing.setdefault(start, []).append(end)
        outgoing.setdefault(end, [])
    visiting: set[str] = set()
    visited: set[str] = set()

    def walk(node: str) -> bool:
        if node in visiting:
            return True
        if node in visited:
            return False
        visiting.add(node)
        for nxt in outgoing.get(node, []):
            if walk(nxt):
                return True
        visiting.remove(node)
        visited.add(node)
        return False

    return any(walk(node) for node in sorted(outgoing))


def classify(value: object) -> str | None:
    if not isinstance(value, dict):
        return "malformed-structure"
    if set(value) != set(REQUIRED_KEYS):
        return "malformed-structure"
    if value.get("schema_version") != SCHEMA_VERSION:
        return "malformed-structure"
    if not is_id(value.get("evidence_id")) or not is_sha256(value.get("envelope_digest")):
        return "malformed-structure"

    original = value.get("original")
    if not keys_of(original, {"media_type", "byte_length", "sha256"}):
        return "malformed-structure"
    if not isinstance(original["media_type"], str) or not original["media_type"]:
        return "malformed-structure"
    if type(original["byte_length"]) is not int or original["byte_length"] < 1:
        return "malformed-structure"
    if not is_sha256(original["sha256"]):
        return "malformed-structure"

    parser = value.get("parser")
    if not keys_of(parser, {"identity", "processor", "version", "attestation"}):
        return "malformed-structure"
    for field in ("identity", "processor", "version"):
        if not isinstance(parser[field], str) or not parser[field].strip():
            return "malformed-structure"
        if len(parser[field]) > 512:
            return "malformed-structure"
    if parser["attestation"] not in {"attested", "unavailable"}:
        return "malformed-structure"

    source = value.get("source")
    if not keys_of(source, {"locator", "revision"}):
        return "malformed-structure"
    if not isinstance(source["locator"], str) or not source["locator"].strip():
        return "malformed-structure"
    if len(source["locator"]) > 1024:
        return "malformed-structure"
    revision = source["revision"]
    if not keys_of(revision, {"state", "value"}):
        return "malformed-structure"
    if revision["state"] == "unavailable":
        if revision["value"] is not None:
            return "malformed-structure"
    elif revision["state"] == "attested":
        if not isinstance(revision["value"], str) or not revision["value"].strip():
            return "malformed-structure"
        if len(revision["value"]) > 256:
            return "malformed-structure"
    else:
        return "malformed-structure"

    normalized = value.get("normalized")
    if not keys_of(normalized, {"digest", "content"}):
        return "malformed-structure"
    if not is_sha256(normalized["digest"]):
        return "malformed-structure"
    content = normalized["content"]
    if not keys_of(content, {"family", "text", "language"}):
        return "malformed-structure"
    family = content["family"]
    if family not in FAMILIES:
        return "unsupported-evidence"
    if not isinstance(content["text"], str) or not content["text"].strip():
        return "malformed-structure"
    if len(content["text"].encode("utf-8")) > MAX_TEXT_BYTES:
        return "oversized"
    if not isinstance(content["language"], str) or LANGUAGE_PATTERN.fullmatch(
        content["language"]
    ) is None:
        return "malformed-structure"

    anchors = value.get("anchors")
    assets = value.get("assets")
    relations = value.get("relations")
    if not isinstance(anchors, list) or not isinstance(assets, list) or not isinstance(
        relations, list
    ):
        return "malformed-structure"
    if not (1 <= len(anchors) <= MAX_ANCHORS):
        return "oversized" if len(anchors) > MAX_ANCHORS else "malformed-structure"
    if len(assets) > MAX_ASSETS or len(relations) > MAX_RELATIONS:
        return "oversized"
    if original["byte_length"] > MAX_ORIGINAL_BYTES:
        return "oversized"
    if len(canonical_bytes(value)) > MAX_ENVELOPE_BYTES:
        return "oversized"

    media_family = MEDIA_TYPES.get(original["media_type"])
    if media_family is None:
        return "unsupported-evidence"
    if media_family != family:
        return "unsupported-evidence"

    seen_ids: set[str] = set()
    for anchor in anchors:
        if not keys_of(anchor, {"id", "family", "role", "text", "text_digest", "locator"}):
            return "malformed-structure"
        if not is_id(anchor["id"]) or anchor["id"] in seen_ids:
            return "malformed-structure"
        seen_ids.add(anchor["id"])
        if anchor["family"] != family:
            return "unsupported-evidence"
        if anchor["role"] not in ANCHOR_ROLES[family]:
            return "malformed-structure"
        if not isinstance(anchor["text"], str) or not anchor["text"].strip():
            return "malformed-structure"
        if len(anchor["text"].encode("utf-8")) > MAX_TEXT_BYTES:
            return "oversized"
        if not is_sha256(anchor["text_digest"]):
            return "malformed-structure"
        loc_error = locator_error(family, anchor["locator"])
        if loc_error is not None:
            return loc_error
    if [anchor["id"] for anchor in anchors] != sorted(anchor["id"] for anchor in anchors):
        return "malformed-structure"

    for asset in assets:
        if not keys_of(
            asset, {"id", "path", "media_type", "byte_length", "sha256", "role"}
        ):
            return "malformed-structure"
        if not is_id(asset["id"]) or asset["id"] in seen_ids:
            return "malformed-structure"
        seen_ids.add(asset["id"])
        if unsafe_asset_path(asset["path"]):
            return "unsafe-asset-path"
        if not isinstance(asset["media_type"], str) or not asset["media_type"].strip():
            return "malformed-structure"
        if not is_pos_int(asset["byte_length"], MAX_ENVELOPE_BYTES):
            return "oversized" if type(asset["byte_length"]) is int and asset[
                "byte_length"
            ] > MAX_ENVELOPE_BYTES else "malformed-structure"
        if not is_sha256(asset["sha256"]):
            return "malformed-structure"
        if not isinstance(asset["role"], str) or not asset["role"].strip():
            return "malformed-structure"
    if [asset["id"] for asset in assets] != sorted(asset["id"] for asset in assets):
        return "malformed-structure"

    relation_keys: list[tuple[str, str, str]] = []
    graph_edges: list[tuple[str, str]] = []
    for relation in relations:
        if not keys_of(relation, {"from", "to", "kind"}):
            return "malformed-structure"
        if relation["from"] not in seen_ids or relation["to"] not in seen_ids:
            return "malformed-structure"
        if relation["from"] == relation["to"]:
            return "malformed-structure"
        if relation["kind"] not in RELATION_KINDS:
            return "malformed-structure"
        relation_keys.append((relation["from"], relation["to"], relation["kind"]))
        if relation["kind"] in ACYCLIC_RELATIONS:
            graph_edges.append((relation["from"], relation["to"]))
    if relation_keys != sorted(set(relation_keys)):
        return "malformed-structure"
    if has_cycle(seen_ids, graph_edges):
        return "malformed-structure"

    fidelity = value.get("fidelity")
    required_features = FAMILY_FEATURES[family]
    if not isinstance(fidelity, dict) or set(fidelity) != set(required_features):
        return "malformed-structure"
    for feature, status in fidelity.items():
        if FEATURE_PATTERN.fullmatch(feature) is None or status not in FIDELITY_VALUES:
            return "malformed-structure"
        if feature in NEVER_EXACT_FEATURES and status == "exact":
            return "unsupported-evidence"

    receipts = value.get("receipts")
    if not keys_of(receipts, {"truncation", "malformed_input", "unavailable"}):
        return "malformed-structure"
    truncation = receipts["truncation"]
    if not keys_of(
        truncation, {"truncated", "omitted_bytes", "omitted_anchor_count", "reason"}
    ):
        return "malformed-structure"
    if type(truncation["truncated"]) is not bool:
        return "malformed-structure"
    if not is_nonneg_int(truncation["omitted_bytes"], MAX_ORIGINAL_BYTES):
        return "malformed-structure"
    if not is_nonneg_int(truncation["omitted_anchor_count"], MAX_ANCHORS):
        return "malformed-structure"
    if truncation["truncated"]:
        if not isinstance(truncation["reason"], str) or not truncation["reason"].strip():
            return "malformed-structure"
    elif truncation["reason"] is not None or truncation["omitted_bytes"] != 0 or truncation[
        "omitted_anchor_count"
    ] != 0:
        return "malformed-structure"
    malformed_input = receipts["malformed_input"]
    if not keys_of(malformed_input, {"present", "reason"}):
        return "malformed-structure"
    if type(malformed_input["present"]) is not bool:
        return "malformed-structure"
    if malformed_input["present"]:
        if not isinstance(malformed_input["reason"], str) or not malformed_input[
            "reason"
        ].strip():
            return "malformed-structure"
    elif malformed_input["reason"] is not None:
        return "malformed-structure"
    unavailable = receipts["unavailable"]
    if not keys_of(unavailable, {"features", "revision", "parser_attestation"}):
        return "malformed-structure"
    features = unavailable["features"]
    if not isinstance(features, list) or not all(isinstance(item, str) for item in features):
        return "malformed-structure"
    if not sorted_unique(features) or not all(
        item in required_features for item in features
    ):
        return "malformed-structure"
    expected_unavailable = sorted(
        feature for feature, status in fidelity.items() if status == "unsupported"
    )
    if features != expected_unavailable:
        return "malformed-structure"
    if type(unavailable["revision"]) is not bool or type(
        unavailable["parser_attestation"]
    ) is not bool:
        return "malformed-structure"
    if unavailable["revision"] != (revision["state"] == "unavailable"):
        return "malformed-structure"
    if unavailable["parser_attestation"] != (parser["attestation"] == "unavailable"):
        return "malformed-structure"

    hazards = value.get("hazards")
    if not keys_of(hazards, {"active_content", "external_references"}):
        return "malformed-structure"
    active = hazards["active_content"]
    if not keys_of(active, {"present", "kinds"}):
        return "malformed-structure"
    if type(active["present"]) is not bool or not isinstance(active["kinds"], list):
        return "malformed-structure"
    if not all(isinstance(item, str) and item.strip() and len(item) <= 64 for item in active["kinds"]):
        return "malformed-structure"
    if not sorted_unique(active["kinds"]):
        return "malformed-structure"
    if active["present"] != bool(active["kinds"]):
        return "malformed-structure"
    if active["present"]:
        return "active-content"
    external = hazards["external_references"]
    if not keys_of(external, {"present", "count"}):
        return "malformed-structure"
    if type(external["present"]) is not bool or not is_nonneg_int(external["count"], 1_000_000):
        return "malformed-structure"
    if external["present"] != (external["count"] > 0):
        return "malformed-structure"
    if external["present"]:
        return "external-reference"

    policy = value.get("policy")
    if not keys_of(policy, {"classification", "dlp", "public_boundary"}):
        return "malformed-structure"
    classification = policy["classification"]
    if not keys_of(classification, {"status", "value"}):
        return "malformed-structure"
    if classification["status"] == "unavailable":
        if classification["value"] is not None:
            return "malformed-structure"
    elif classification["status"] == "evaluated":
        if classification["value"] not in {
            "public",
            "public-compatible",
            "private",
            "internal",
        }:
            return "malformed-structure"
    else:
        return "malformed-structure"
    dlp = policy["dlp"]
    if not keys_of(dlp, {"status", "findings"}):
        return "malformed-structure"
    if dlp["status"] not in {"not-evaluated", "passed", "findings"}:
        return "malformed-structure"
    findings = dlp["findings"]
    if not isinstance(findings, list) or len(findings) > MAX_FINDINGS:
        return "malformed-structure" if not isinstance(findings, list) else "oversized"
    if not all(
        isinstance(item, str) and item.strip() and len(item) <= MAX_FINDING_BYTES
        for item in findings
    ):
        return "malformed-structure"
    if not sorted_unique(findings):
        return "malformed-structure"
    if dlp["status"] == "findings":
        if not findings:
            return "malformed-structure"
    elif findings:
        return "malformed-structure"
    boundary = policy["public_boundary"]
    if not keys_of(boundary, {"outcome", "value"}):
        return "malformed-structure"
    if boundary["outcome"] not in {"admit-export", "reject-export", "unavailable"}:
        return "malformed-structure"
    if boundary["outcome"] == "unavailable":
        if boundary["value"] is not None:
            return "malformed-structure"
    elif boundary["value"] not in {"public", "public-compatible", "private", "internal"}:
        return "malformed-structure"
    if boundary["outcome"] == "admit-export" and boundary["value"] != "public":
        return "malformed-structure"

    admission = value.get("admission")
    if not keys_of(admission, {"status", "packet", "query", "reasons"}):
        return "malformed-structure"
    if admission["status"] not in {"admitted", "rejected"}:
        return "malformed-structure"
    if admission["packet"] not in {"eligible", "ineligible"}:
        return "malformed-structure"
    if admission["query"] not in {"eligible", "ineligible"}:
        return "malformed-structure"
    reasons = admission["reasons"]
    if not isinstance(reasons, list) or not all(isinstance(item, str) and item.strip() for item in reasons):
        return "malformed-structure"
    if not sorted_unique(reasons):
        return "malformed-structure"
    blocked = dlp["status"] == "findings"
    if blocked:
        if (
            admission["status"] != "rejected"
            or admission["packet"] != "ineligible"
            or admission["query"] != "ineligible"
            or "dlp-findings" not in reasons
        ):
            return "malformed-structure"
    elif admission["status"] == "admitted":
        if admission["packet"] != "eligible" or admission["query"] != "eligible" or reasons:
            return "malformed-structure"
    else:
        if admission["packet"] != "ineligible" or admission["query"] != "ineligible" or not reasons:
            return "malformed-structure"
    if contains_secret(value):
        return "secret-reflection"

    for anchor in anchors:
        if anchor["text_digest"] != text_digest(anchor["text"]):
            return "digest-mismatch"
    if normalized["digest"] != normalized_digest(content):
        return "digest-mismatch"
    if value["envelope_digest"] != envelope_digest(value):
        return "digest-mismatch"
    return None


def validate_schema(schema: object) -> list[str]:
    errors: list[str] = []
    if not isinstance(schema, dict):
        return ["schema root must be an object"]
    if schema.get("$id") != SCHEMA_ID:
        errors.append("schema $id is not the V1 enterprise-document evidence-envelope id")
    if schema.get("x-semantic-oracle") != SEMANTIC_ORACLE:
        errors.append("schema missing or incorrect x-semantic-oracle annotation")
    required = schema.get("required")
    if required != list(REQUIRED_KEYS):
        errors.append("schema required keys drifted from the semantic oracle")
    properties = schema.get("properties")
    if not isinstance(properties, dict):
        errors.append("schema properties must be an object")
        return errors
    version = properties.get("schema_version", {})
    if not isinstance(version, dict) or version.get("const") != SCHEMA_VERSION:
        errors.append("schema_version const drifted from 1.0.0")
    original = properties.get("original", {})
    media = (
        original.get("properties", {}).get("media_type", {})
        if isinstance(original, dict)
        else {}
    )
    if not isinstance(media, dict) or set(media.get("enum", [])) != set(MEDIA_TYPES):
        errors.append("original.media_type enum drifted from the four V1 media types")
    fidelity = properties.get("fidelity", {})
    additional = fidelity.get("additionalProperties") if isinstance(fidelity, dict) else None
    if not isinstance(additional, dict) or additional.get("enum") != list(FIDELITY_VALUES):
        errors.append("fidelity value enum drifted from exact/normalized/approximated/unsupported")
    return errors


def load_named_json(path: Path) -> tuple[object | None, str | None]:
    try:
        return load_json(path), None
    except ValueError:
        return None, "malformed-structure"


def main() -> int:
    root = bran_root()
    schema_path = root / "schemas/enterprise-document-evidence-envelope.schema.json"
    fixture_root = root / "fixtures/enterprise-documents"
    positive_root = fixture_root / "positive"
    negative_root = fixture_root / "negative"
    try:
        schema = load_json(schema_path)
    except ValueError as error:
        print(f"FAIL enterprise contract check: {error}")
        return 1

    schema_errors = validate_schema(schema)
    if schema_errors:
        print("FAIL enterprise contract check: schema drifted")
        for error in schema_errors:
            print(f"  {error}")
        return 1

    missing = [
        path.as_posix()
        for path in (
            [positive_root / name for name in POSITIVE_NAMES]
            + [negative_root / name for name in NEGATIVE_REASONS]
        )
        if not path.is_file()
    ]
    if missing:
        print("FAIL enterprise contract check: missing fixtures")
        for path in missing:
            print(f"  {path}")
        return 1

    failures: list[str] = []
    accepted = 0
    golden_bytes: bytes | None = None
    golden_digest: str | None = None
    for name in POSITIVE_NAMES:
        path = positive_root / name
        envelope, parse_reason = load_named_json(path)
        reason = parse_reason if parse_reason is not None else classify(envelope)
        if reason is not None:
            failures.append(f"{path.relative_to(root).as_posix()} rejected as {reason}")
            continue
        assert isinstance(envelope, dict)
        if envelope["admission"]["status"] != "admitted":
            failures.append(f"{path.relative_to(root).as_posix()} is structurally valid but not admitted")
            continue
        accepted += 1
        if golden_bytes is None:
            golden_bytes = canonical_bytes(envelope)
            golden_digest = envelope["envelope_digest"]
            permuted = permute(envelope)
            if canonical_bytes(permuted) != golden_bytes:
                failures.append("permuted in-memory JSON did not serialize to the golden canonical bytes")
            if not isinstance(permuted, dict) or envelope_digest(permuted) != golden_digest:
                failures.append("permuted in-memory JSON did not reproduce the golden envelope digest")

    rejected = 0
    for name, expected in NEGATIVE_REASONS.items():
        path = negative_root / name
        envelope, parse_reason = load_named_json(path)
        reason = parse_reason if parse_reason is not None else classify(envelope)
        if reason != expected:
            failures.append(
                f"{path.relative_to(root).as_posix()} expected {expected}, got {reason}"
            )
            continue
        rejected += 1

    extra = sorted(
        path.name
        for directory, expected in (
            (positive_root, set(POSITIVE_NAMES)),
            (negative_root, set(NEGATIVE_REASONS)),
        )
        for path in directory.iterdir()
        if path.is_file() and path.name not in expected
    )
    if extra:
        failures.append("unexpected fixture files: " + ", ".join(extra))

    if failures:
        print("FAIL enterprise contract check")
        for failure in failures:
            print(f"  {failure}")
        return 1

    print(
        f"PASS enterprise document evidence envelope: positives={accepted} negatives={rejected}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
