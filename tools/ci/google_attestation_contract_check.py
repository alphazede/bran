#!/usr/bin/env python3
"""Validate the V1 Google source-attestation contract without third-party packages."""

from __future__ import annotations

import ast
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

from enterprise_contract_check import (
    MAX_TEXT_BYTES,
    canonical_bytes,
    classify as classify_envelope,
    envelope_digest,
    is_id,
    is_nonneg_int,
    is_sha256,
    keys_of,
    load_json,
    permute,
    sha256_hex,
    sorted_unique,
)


SCHEMA_VERSION = "1.0.0"
PROFILE_IDENTITY = "bran-google-enterprise-v1"
SCHEMA_ID = "https://schemas.alphazede.dev/bran/google-source-attestation/v1/schema.json"
SEMANTIC_ORACLE = "tools/ci/google_attestation_contract_check.py"
ADAPTER_RELATIVE = "tools/ci/document_ai_smoke_adapter.py"
RECORDED_DIR = "fixtures/google-attestation/recorded"
ADAPTER_ENVELOPE_NAME = "enterprise-document-evidence-envelope.json"
ADAPTER_ATTESTATION_NAME = "google-source-attestation.json"
RECORDED_PROCESSOR = "processor-create.json"
RECORDED_PROCESS = "process-layout.json"
RECORDED_PDF = "input.pdf"
RECORDED_MISSING_IDENTITY = "processor-missing-identity.json"
RECORDED_LEGACY_ONLY = "process-legacy-only.json"
RECORDED_EMPTY_LAYOUT = "process-empty-layout.json"
EXPECTED_PROCESSOR_LOCATOR = (
    "projects/synthetic-project/locations/us/processors/synthetic-processor/"
    "processorVersions/pretrained-layout-parser-v1.0-2024-06-03"
)
ENVELOPE_KIND = "enterprise-document-evidence-envelope"
DOCUMENT_AI_ENVELOPE = "fixtures/enterprise-documents/positive/pdf-fixed-layout.json"
ACCOUNT_PREFIX = "acct:opaque:"
MAX_ATTESTATION_BYTES = 1_048_576
MAX_FINDINGS = 64
MAX_FINDING_BYTES = 128
MAX_FILTERS = 16
MAX_RESULT_IDS = 64

GOOGLE_PRODUCTS = frozenset(
    {
        "agent-search",
        "document-ai",
        "gemini-enterprise",
        "knowledge-catalog",
    }
)
PRODUCTS = GOOGLE_PRODUCTS | {"bran-git", "bran-okf"}
CAPABILITY_STATES = frozenset(
    {
        "federated",
        "git-okf",
        "imported",
        "indexed",
        "metadata-only",
        "parsed",
        "unavailable",
    }
)
PRODUCT_STATES = {
    "agent-search": frozenset({"federated", "imported", "indexed", "unavailable"}),
    "bran-git": frozenset({"git-okf", "unavailable"}),
    "bran-okf": frozenset({"git-okf", "unavailable"}),
    "document-ai": frozenset({"parsed", "unavailable"}),
    "gemini-enterprise": frozenset({"federated", "imported", "indexed", "unavailable"}),
    "knowledge-catalog": frozenset({"metadata-only", "unavailable"}),
}
PRODUCT_OUTPUT = {
    "agent-search": "search-result",
    "bran-git": "git-okf",
    "bran-okf": "git-okf",
    "document-ai": ENVELOPE_KIND,
    "gemini-enterprise": "search-result",
    "knowledge-catalog": "catalog-entry",
}
OUTPUT_KINDS = frozenset(
    {ENVELOPE_KIND, "catalog-entry", "git-okf", "search-result", "unavailable"}
)
NORMALIZED_OUTPUT_KINDS = frozenset({"catalog-entry", "git-okf", "search-result"})
GOOGLE_LOCATOR_TEMPLATE = {
    "agent-search": (
        "projects",
        "project",
        "locations",
        "location",
        "collections",
        "collection",
        "dataStores",
        "data_store",
    ),
    "document-ai": (
        "projects",
        "project",
        "locations",
        "location",
        "processors",
        "processor",
        "processorVersions",
        "processor_version",
    ),
    "gemini-enterprise": (
        "projects",
        "project",
        "locations",
        "location",
        "collections",
        "collection",
        "engines",
        "engine",
    ),
    "knowledge-catalog": (
        "projects",
        "project",
        "locations",
        "location",
        "entryGroups",
        "entry_group",
        "entries",
        "entry",
    ),
}
BRAN_EVIDENCE_PRODUCTS = frozenset({"bran-git", "bran-okf"})
LOCATOR_SEGMENT = re.compile(r"[A-Za-z0-9@][A-Za-z0-9@._-]{0,126}$")
SCOPE_SEGMENT = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,126}$")
SNAPSHOT_IDENTITY = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})$")
OPAQUE_LOCATOR = "opaque:outside-owner-allowlist"
REVISION_KINDS = frozenset({"entry", "generation", "revision", "unavailable"})
PRODUCT_REVISION_KIND = {
    "agent-search": "revision",
    "bran-git": "revision",
    "bran-okf": "revision",
    "document-ai": "generation",
    "gemini-enterprise": "revision",
    "knowledge-catalog": "entry",
}
HISTORY_STATES = frozenset({"imported", "indexed"})
CHECKPOINT_STATES = frozenset({"conflict", "current", "stale", "unavailable"})
PERMISSION_STATUSES = frozenset({"attested", "partial", "unavailable"})
COST_STATUSES = frozenset({"recorded", "unavailable"})
QUOTA_STATES = frozenset({"exhausted", "unavailable", "within-quota"})
PERIMETER_STATES = frozenset({"allowed", "denied", "unavailable"})
NETWORK_STATES = frozenset({"disabled", "not-invoked"})
FAILURE_CODES = (
    "completeness-overclaim",
    "conflict",
    "dlp-findings",
    "history-incomplete",
    "location-mismatch",
    "mixed-revision",
    "network-disabled",
    "perimeter-denied",
    "permission-unavailable",
    "quota-exhausted",
    "secret-reflection",
    "stale",
    "tenant-escape",
    "unauthorized-action",
)
FAILURE_SET = frozenset(FAILURE_CODES)
CLASSIFICATION_VALUES = frozenset(
    {"internal", "private", "public", "public-compatible"}
)
REQUIRED_KEYS = (
    "admission",
    "attestation_digest",
    "attestation_id",
    "capability",
    "checkpoint",
    "cost",
    "failures",
    "output",
    "permission",
    "policy",
    "product",
    "profile",
    "runtime",
    "schema_version",
    "source",
    "tenancy",
    "truncation",
)
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
ACTION_MARKERS = (
    "drive.files.create",
    "drive.files.update",
    "generateAccessToken",
    "iam.serviceAccounts",
    "setIamPolicy",
)
OFFLINE_IMPORTS = frozenset(
    {
        "http",
        "http.client",
        "requests",
        "socket",
        "ssl",
        "urllib",
        "urllib.request",
    }
)
POSITIVE_NAMES = (
    "agent-search.json",
    "bran-git.json",
    "bran-okf.json",
    "document-ai.json",
    "gemini-enterprise.json",
    "knowledge-catalog.json",
)
NEGATIVE_FAILURES = {
    "completeness-overclaim.json": frozenset({"completeness-overclaim"}),
    "conflict-mixed-revision.json": frozenset({"conflict", "mixed-revision"}),
    "dlp-rejected.json": frozenset({"dlp-findings"}),
    "incomplete-permission-revision.json": frozenset(
        {"history-incomplete", "permission-unavailable", "stale"}
    ),
    "network-disabled.json": frozenset({"network-disabled"}),
    "opaque-locator-missing-scope.json": frozenset(
        {"location-mismatch", "tenant-escape"}
    ),
    "quota-location-perimeter.json": frozenset(
        {"location-mismatch", "perimeter-denied", "quota-exhausted"}
    ),
    "tenant-escape-secret-action.json": frozenset(
        {"secret-reflection", "tenant-escape", "unauthorized-action"}
    ),
}


def bran_root() -> Path:
    return Path(__file__).resolve().parents[2]


def attestation_digest(record: dict[str, Any]) -> str:
    body = {key: value for key, value in record.items() if key != "attestation_digest"}
    return sha256_hex(canonical_bytes(body))


def filter_digest(filters: object) -> str:
    return sha256_hex(canonical_bytes(filters))


def configured_digest(tenancy: dict[str, Any], locator: str, filters: object) -> str:
    return sha256_hex(
        canonical_bytes(
            {
                "account_ref": tenancy["account_ref"],
                "filters": filters,
                "locator": locator,
                "project": tenancy["project"],
                "tenant": tenancy["tenant"],
            }
        )
    )


def output_digest(normalized: object) -> str:
    return sha256_hex(canonical_bytes(normalized))


def checkpoint_digest(locator: str, revision_value: str, digest: str) -> str:
    return sha256_hex(
        canonical_bytes(
            {
                "locator": locator,
                "output_digest": digest,
                "revision": revision_value,
            }
        )
    )


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


def _safe_segment(value: str, pattern: re.Pattern[str]) -> bool:
    return bool(value) and ".." not in value and pattern.fullmatch(value) is not None


def _parse_template_locator(
    locator: str, template: tuple[str, ...]
) -> dict[str, str] | None:
    if any(marker in locator for marker in ("\\", "?", "#", " ")):
        return None
    parts = locator.split("/")
    if len(parts) != len(template):
        return None
    parsed: dict[str, str] = {}
    for index, expected in enumerate(template):
        part = parts[index]
        if index % 2 == 0:
            if part != expected:
                return None
            continue
        if not _safe_segment(part, LOCATOR_SEGMENT):
            return None
        parsed[expected] = part
    return parsed


def _parse_bran_git_locator(locator: str) -> dict[str, str] | None:
    prefix = "git:repository/"
    if not locator.startswith(prefix):
        return None
    parts = locator[len(prefix) :].split("/")
    if len(parts) != 3:
        return None
    repository, kind, snapshot = parts
    if kind not in {"commit", "tree"}:
        return None
    if not _safe_segment(repository, SCOPE_SEGMENT):
        return None
    if SNAPSHOT_IDENTITY.fullmatch(snapshot) is None:
        return None
    return {"location": kind, "project": repository, "snapshot": snapshot}


def _parse_bran_okf_locator(locator: str) -> dict[str, str] | None:
    prefix = "okf:bundle/"
    if not locator.startswith(prefix):
        return None
    parts = locator[len(prefix) :].split("/")
    if len(parts) != 3:
        return None
    bundle, token, snapshot = parts
    if token != "snapshot":
        return None
    if not _safe_segment(bundle, SCOPE_SEGMENT):
        return None
    if SNAPSHOT_IDENTITY.fullmatch(snapshot) is None:
        return None
    return {"location": "snapshot", "project": bundle, "snapshot": snapshot}


def parse_product_locator(product: str, locator: str) -> dict[str, str] | None:
    if not isinstance(locator, str) or not locator or len(locator) > 1024:
        return None
    if locator.startswith("/") or "\\" in locator or ".." in locator:
        return None
    template = GOOGLE_LOCATOR_TEMPLATE.get(product)
    if template is not None:
        parsed = _parse_template_locator(locator, template)
        if parsed is None:
            return None
        return {"location": parsed["location"], "project": parsed["project"]}
    if product == "bran-git":
        return _parse_bran_git_locator(locator)
    if product == "bran-okf":
        return _parse_bran_okf_locator(locator)
    return None


def project_from_locator(locator: str) -> str | None:
    parsed = parse_product_locator("gemini-enterprise", locator)
    if parsed is None:
        parsed = parse_product_locator("agent-search", locator)
    if parsed is None:
        parsed = parse_product_locator("document-ai", locator)
    if parsed is None:
        parsed = parse_product_locator("knowledge-catalog", locator)
    return None if parsed is None else parsed["project"]


def location_from_locator(locator: str) -> str | None:
    parsed = parse_product_locator("gemini-enterprise", locator)
    if parsed is None:
        parsed = parse_product_locator("agent-search", locator)
    if parsed is None:
        parsed = parse_product_locator("document-ai", locator)
    if parsed is None:
        parsed = parse_product_locator("knowledge-catalog", locator)
    return None if parsed is None else parsed["location"]


def admission_eligibility(failures: list[str]) -> tuple[str, str, str]:
    if failures:
        return "rejected", "ineligible", "ineligible"
    return "admitted", "eligible", "eligible"


def envelope_path_error(path: object, *, allow_leaf: bool = False) -> str | None:
    if not isinstance(path, str) or not path or len(path) > 256:
        return "malformed-structure"
    relative = Path(path)
    if relative.is_absolute() or ".." in relative.parts or "\\" in path:
        return "unsafe-locator"
    if not path.endswith(".json"):
        return "malformed-structure"
    if path.startswith("fixtures/enterprise-documents/positive/"):
        return None
    if allow_leaf and "/" not in path:
        return None
    return "unsupported-evidence"


def infer_failures(value: dict[str, Any]) -> list[str]:
    found: list[str] = []
    strings = walk_strings(value)
    if any(marker in text for text in strings for marker in SECRET_MARKERS):
        found.append("secret-reflection")
    if any(marker in text for text in strings for marker in ACTION_MARKERS):
        found.append("unauthorized-action")
    product = value["product"]["name"]
    tenancy = value["tenancy"]
    locator = value["source"]["locator"]
    parsed_locator = parse_product_locator(product, locator)
    if parsed_locator is None or parsed_locator["project"] != tenancy["project"]:
        found.append("tenant-escape")
    if parsed_locator is None or parsed_locator["location"] != tenancy["location"]:
        found.append("location-mismatch")
    if tenancy["perimeter"] == "denied":
        found.append("perimeter-denied")
    if product in GOOGLE_PRODUCTS and value["runtime"]["network"] == "disabled":
        found.append("network-disabled")
    else:
        if value["permission"]["status"] != "attested":
            found.append("permission-unavailable")
        attested = value["capability"]["attested"]
        revision_state = value["source"]["revision"]["state"]
        if attested in HISTORY_STATES and revision_state == "unavailable":
            found.append("history-incomplete")
    if value["permission"]["authorization_proof"] is True:
        found.append("completeness-overclaim")
    checkpoint_state = value["checkpoint"]["state"]
    if checkpoint_state == "stale":
        found.append("stale")
    if checkpoint_state == "conflict":
        found.append("conflict")
    source_revision = value["source"]["revision"]["value"]
    checkpoint_revision = value["checkpoint"]["revision_value"]
    if (
        isinstance(source_revision, str)
        and isinstance(checkpoint_revision, str)
        and source_revision != checkpoint_revision
    ):
        found.append("mixed-revision")
    if value["cost"]["quota"] == "exhausted":
        found.append("quota-exhausted")
    if value["policy"]["dlp"]["status"] == "findings":
        found.append("dlp-findings")
    return sorted(set(found))


def structural_reason(
    value: object,
    *,
    output_root: Path | None = None,
    envelope: dict[str, Any] | None = None,
) -> str | None:
    if not isinstance(value, dict):
        return "malformed-structure"
    if set(value) != set(REQUIRED_KEYS):
        return "malformed-structure"
    if value.get("schema_version") != SCHEMA_VERSION:
        return "malformed-structure"
    if not is_id(value.get("attestation_id")) or not is_sha256(
        value.get("attestation_digest")
    ):
        return "malformed-structure"

    profile = value.get("profile")
    if not keys_of(profile, {"identity", "version"}):
        return "malformed-structure"
    if profile["identity"] != PROFILE_IDENTITY or profile["version"] != SCHEMA_VERSION:
        return "malformed-structure"

    product = value.get("product")
    if not keys_of(product, {"name", "identity", "component", "version"}):
        return "malformed-structure"
    if product["name"] not in PRODUCTS:
        return "unsupported-evidence"
    for field in ("identity", "component", "version"):
        if not isinstance(product[field], str) or not product[field].strip():
            return "malformed-structure"
        if len(product[field]) > 512:
            return "malformed-structure"

    capability = value.get("capability")
    if not keys_of(capability, {"requested", "effective", "attested"}):
        return "malformed-structure"
    for field in ("requested", "effective", "attested"):
        if capability[field] not in CAPABILITY_STATES:
            return "unsupported-evidence"
    if capability["attested"] not in PRODUCT_STATES[product["name"]]:
        return "unsupported-evidence"
    if capability["attested"] == "unavailable" and capability["effective"] != "unavailable":
        return "malformed-structure"

    tenancy = value.get("tenancy")
    if not keys_of(
        tenancy, {"tenant", "project", "location", "account_ref", "perimeter"}
    ):
        return "malformed-structure"
    for field in ("tenant", "project", "location"):
        if not isinstance(tenancy[field], str) or not tenancy[field].strip():
            return "malformed-structure"
        if len(tenancy[field]) > 128:
            return "malformed-structure"
    account_ref = tenancy["account_ref"]
    if (
        not isinstance(account_ref, str)
        or not account_ref.startswith(ACCOUNT_PREFIX)
        or not is_id(account_ref)
    ):
        return "malformed-structure"
    if tenancy["perimeter"] not in PERIMETER_STATES:
        return "malformed-structure"

    source = value.get("source")
    if not keys_of(
        source,
        {"locator", "filters", "configured_digest", "filter_digest", "revision"},
    ):
        return "malformed-structure"
    locator = source["locator"]
    if not isinstance(locator, str) or not locator.strip() or len(locator) > 1024:
        return "malformed-structure"
    filters = source["filters"]
    if not isinstance(filters, dict) or not (1 <= len(filters) <= MAX_FILTERS):
        return "malformed-structure"
    for key, item in filters.items():
        if not isinstance(key, str) or not key or not isinstance(item, str) or not item:
            return "malformed-structure"
        if len(key) > 64 or len(item) > 256:
            return "malformed-structure"
    if not is_sha256(source["configured_digest"]) or not is_sha256(source["filter_digest"]):
        return "malformed-structure"
    if source["filter_digest"] != filter_digest(filters):
        return "digest-mismatch"
    if source["configured_digest"] != configured_digest(tenancy, locator, filters):
        return "digest-mismatch"
    revision = source["revision"]
    if not keys_of(revision, {"state", "kind", "value"}):
        return "malformed-structure"
    if revision["state"] == "unavailable":
        if revision["kind"] != "unavailable" or revision["value"] is not None:
            return "malformed-structure"
    elif revision["state"] == "attested":
        if revision["kind"] != PRODUCT_REVISION_KIND[product["name"]]:
            return "unsupported-evidence"
        if not isinstance(revision["value"], str) or not revision["value"].strip():
            return "malformed-structure"
        if len(revision["value"]) > 256:
            return "malformed-structure"
    else:
        return "malformed-structure"
    if revision["kind"] not in REVISION_KINDS:
        return "malformed-structure"
    parsed_locator = parse_product_locator(product["name"], locator)
    if (
        product["name"] in BRAN_EVIDENCE_PRODUCTS
        and revision["state"] == "attested"
        and parsed_locator is not None
        and parsed_locator.get("snapshot") != revision["value"]
    ):
        return "malformed-structure"

    permission = value.get("permission")
    if not keys_of(permission, {"status", "authorization_proof"}):
        return "malformed-structure"
    if permission["status"] not in PERMISSION_STATUSES:
        return "malformed-structure"
    if type(permission["authorization_proof"]) is not bool:
        return "malformed-structure"

    output = value.get("output")
    if not keys_of(output, {"kind", "digest", "envelope_path", "normalized"}):
        return "malformed-structure"
    if output["kind"] not in OUTPUT_KINDS:
        return "unsupported-evidence"
    attested = capability["attested"]
    if attested == "unavailable":
        if (
            output["kind"] != "unavailable"
            or output["digest"] is not None
            or output["envelope_path"] is not None
            or output["normalized"] is not None
        ):
            return "malformed-structure"
    elif output["kind"] != PRODUCT_OUTPUT[product["name"]]:
        return "unsupported-evidence"

    envelope_path = output["envelope_path"]
    normalized = output["normalized"]
    digest = output["digest"]
    if output["kind"] == ENVELOPE_KIND:
        path_error = envelope_path_error(
            envelope_path,
            allow_leaf=output_root is not None or envelope is not None,
        )
        if path_error is not None:
            return path_error
        if normalized is not None or not is_sha256(digest):
            return "malformed-structure"
        loaded = envelope
        if loaded is None:
            envelope_file = (output_root if output_root is not None else bran_root()) / envelope_path
            if not envelope_file.is_file():
                return "unsupported-evidence"
            try:
                loaded = load_json(envelope_file)
            except ValueError:
                return "malformed-structure"
        if classify_envelope(loaded) is not None:
            return "unsupported-evidence"
        if not isinstance(loaded, dict):
            return "malformed-structure"
        parser = loaded.get("parser")
        if not isinstance(parser, dict):
            return "unsupported-evidence"
        if parser.get("identity") != product["identity"]:
            return "unsupported-evidence"
        if parser.get("processor") != product["component"]:
            return "unsupported-evidence"
        if loaded.get("envelope_digest") != digest:
            return "digest-mismatch"
        if envelope_digest(loaded) != digest:
            return "digest-mismatch"
    elif output["kind"] in NORMALIZED_OUTPUT_KINDS:
        if envelope_path is not None or not is_sha256(digest):
            return "malformed-structure"
        if not keys_of(normalized, {"item_count", "result_ids"}):
            return "malformed-structure"
        result_ids = normalized["result_ids"]
        if not isinstance(result_ids, list) or not (1 <= len(result_ids) <= MAX_RESULT_IDS):
            return "oversized" if isinstance(result_ids, list) and len(result_ids) > MAX_RESULT_IDS else "malformed-structure"
        if not all(is_id(item) for item in result_ids) or not sorted_unique(result_ids):
            return "malformed-structure"
        if type(normalized["item_count"]) is not int or normalized["item_count"] != len(
            result_ids
        ):
            return "malformed-structure"
        if digest != output_digest(normalized):
            return "digest-mismatch"
    elif output["kind"] != "unavailable":
        return "unsupported-evidence"

    checkpoint = value.get("checkpoint")
    if not keys_of(checkpoint, {"id", "digest", "state", "revision_value"}):
        return "malformed-structure"
    if not is_id(checkpoint["id"]) or checkpoint["state"] not in CHECKPOINT_STATES:
        return "malformed-structure"
    if checkpoint["state"] == "unavailable":
        if checkpoint["digest"] is not None or checkpoint["revision_value"] is not None:
            return "malformed-structure"
    else:
        if not is_sha256(checkpoint["digest"]):
            return "malformed-structure"
        if checkpoint["state"] == "current":
            if revision["state"] == "attested":
                if checkpoint["revision_value"] != revision["value"]:
                    return "malformed-structure"
                if not isinstance(digest, str) or checkpoint["digest"] != checkpoint_digest(
                    locator, revision["value"], digest
                ):
                    return "digest-mismatch"
            elif checkpoint["revision_value"] is not None:
                return "malformed-structure"
        elif checkpoint["revision_value"] is not None:
            if not isinstance(checkpoint["revision_value"], str) or not checkpoint[
                "revision_value"
            ].strip():
                return "malformed-structure"
            if len(checkpoint["revision_value"]) > 256:
                return "malformed-structure"

    truncation = value.get("truncation")
    if not keys_of(
        truncation, {"truncated", "omitted_bytes", "omitted_item_count", "reason"}
    ):
        return "malformed-structure"
    if type(truncation["truncated"]) is not bool:
        return "malformed-structure"
    if not is_nonneg_int(truncation["omitted_bytes"], MAX_ATTESTATION_BYTES):
        return "malformed-structure"
    if not is_nonneg_int(truncation["omitted_item_count"], MAX_RESULT_IDS):
        return "malformed-structure"
    if truncation["truncated"]:
        if not isinstance(truncation["reason"], str) or not truncation["reason"].strip():
            return "malformed-structure"
        if len(truncation["reason"]) > 256:
            return "malformed-structure"
    elif (
        truncation["reason"] is not None
        or truncation["omitted_bytes"] != 0
        or truncation["omitted_item_count"] != 0
    ):
        return "malformed-structure"

    cost = value.get("cost")
    if not keys_of(cost, {"status", "quota"}):
        return "malformed-structure"
    if cost["status"] not in COST_STATUSES or cost["quota"] not in QUOTA_STATES:
        return "malformed-structure"
    if cost["status"] == "unavailable" and cost["quota"] != "unavailable":
        return "malformed-structure"

    runtime = value.get("runtime")
    if not keys_of(runtime, {"network"}):
        return "malformed-structure"
    if runtime["network"] not in NETWORK_STATES:
        return "malformed-structure"
    if (
        runtime["network"] == "disabled"
        and product["name"] in GOOGLE_PRODUCTS
        and capability["attested"] != "unavailable"
    ):
        return "malformed-structure"

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
        if classification["value"] not in CLASSIFICATION_VALUES:
            return "malformed-structure"
    else:
        return "malformed-structure"
    dlp = policy["dlp"]
    if not keys_of(dlp, {"status", "findings"}):
        return "malformed-structure"
    if dlp["status"] not in {"findings", "not-evaluated", "passed"}:
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
    elif boundary["value"] not in CLASSIFICATION_VALUES:
        return "malformed-structure"
    if boundary["outcome"] == "admit-export" and boundary["value"] != "public":
        return "malformed-structure"

    failures = value.get("failures")
    if not isinstance(failures, list) or len(failures) > len(FAILURE_CODES):
        return "malformed-structure"
    if not all(isinstance(item, str) and item in FAILURE_SET for item in failures):
        return "unsupported-evidence"
    if not sorted_unique(failures):
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
    if not isinstance(reasons, list) or not all(
        isinstance(item, str) and item.strip() and len(item) <= 128 for item in reasons
    ):
        return "malformed-structure"
    if not sorted_unique(reasons):
        return "malformed-structure"

    computed = infer_failures(value)
    if failures != computed:
        return "completeness-overclaim"
    if computed:
        if (
            admission["status"] != "rejected"
            or admission["packet"] != "ineligible"
            or admission["query"] != "ineligible"
            or reasons != computed
        ):
            return "malformed-structure"
    elif (
        admission["status"] != "admitted"
        or admission["packet"] != "eligible"
        or admission["query"] != "eligible"
        or reasons
    ):
        return "malformed-structure"

    if len(canonical_bytes(value)) > MAX_ATTESTATION_BYTES:
        return "oversized"
    if value["attestation_digest"] != attestation_digest(value):
        return "digest-mismatch"
    return None


def evaluate(
    value: object,
    *,
    output_root: Path | None = None,
    envelope: dict[str, Any] | None = None,
) -> tuple[str | None, list[str]]:
    reason = structural_reason(value, output_root=output_root, envelope=envelope)
    if reason is not None:
        return reason, []
    assert isinstance(value, dict)
    return None, list(value["failures"])


def with_digests(record: dict[str, Any]) -> dict[str, Any]:
    filled = json_clone(record)
    source = filled["source"]
    tenancy = filled["tenancy"]
    source["filter_digest"] = filter_digest(source["filters"])
    source["configured_digest"] = configured_digest(
        tenancy, source["locator"], source["filters"]
    )
    output = filled["output"]
    if output["kind"] == ENVELOPE_KIND:
        envelope = load_json(bran_root() / output["envelope_path"])
        output["digest"] = envelope["envelope_digest"]
        output["normalized"] = None
    elif output["kind"] in NORMALIZED_OUTPUT_KINDS:
        output["digest"] = output_digest(output["normalized"])
        output["envelope_path"] = None
    else:
        output["digest"] = None
        output["envelope_path"] = None
        output["normalized"] = None
    checkpoint = filled["checkpoint"]
    revision = source["revision"]
    if (
        checkpoint["state"] == "current"
        and revision["state"] == "attested"
        and isinstance(output["digest"], str)
    ):
        checkpoint["revision_value"] = revision["value"]
        checkpoint["digest"] = checkpoint_digest(
            source["locator"], revision["value"], output["digest"]
        )
    filled["attestation_digest"] = attestation_digest(filled)
    return filled


def json_clone(value: dict[str, Any]) -> dict[str, Any]:
    return json.loads(json.dumps(value))


def validate_schema(schema: object) -> list[str]:
    errors: list[str] = []
    if not isinstance(schema, dict):
        return ["schema root must be an object"]
    if schema.get("$id") != SCHEMA_ID:
        errors.append("schema $id is not the V1 Google source-attestation id")
    if schema.get("x-semantic-oracle") != SEMANTIC_ORACLE:
        errors.append("schema missing or incorrect x-semantic-oracle annotation")
    required = schema.get("required")
    if required != list(REQUIRED_KEYS):
        errors.append("schema required keys drifted from the semantic oracle")
    capabilities = schema.get("x-product-capabilities")
    if not isinstance(capabilities, dict):
        errors.append("schema missing x-product-capabilities")
    else:
        expected = {
            name: sorted(states) for name, states in sorted(PRODUCT_STATES.items())
        }
        actual = {
            name: list(states) if isinstance(states, list) else states
            for name, states in capabilities.items()
        }
        if actual != expected:
            errors.append("schema x-product-capabilities drifted from the oracle")
    properties = schema.get("properties")
    if not isinstance(properties, dict):
        errors.append("schema properties must be an object")
        return errors
    version = properties.get("schema_version", {})
    if not isinstance(version, dict) or version.get("const") != SCHEMA_VERSION:
        errors.append("schema_version const drifted from 1.0.0")
    product = properties.get("product", {})
    name = (
        product.get("properties", {}).get("name", {})
        if isinstance(product, dict)
        else {}
    )
    if not isinstance(name, dict) or set(name.get("enum", [])) != set(PRODUCTS):
        errors.append("product.name enum drifted from the V1 managed-product set")
    if set(PRODUCT_OUTPUT) != set(PRODUCTS):
        errors.append("PRODUCT_OUTPUT drifted from the schema-declared product set")
    locator_products = set(GOOGLE_LOCATOR_TEMPLATE) | BRAN_EVIDENCE_PRODUCTS
    if locator_products != set(PRODUCTS):
        errors.append("locator grammars drifted from the schema-declared product set")
    if set(PRODUCT_OUTPUT.values()) - (OUTPUT_KINDS - {"unavailable"}):
        errors.append("PRODUCT_OUTPUT contains an undeclared output kind")
    output = properties.get("output", {})
    kind = (
        output.get("properties", {}).get("kind", {}) if isinstance(output, dict) else {}
    )
    if not isinstance(kind, dict) or set(kind.get("enum", [])) != set(OUTPUT_KINDS):
        errors.append("output.kind enum drifted from the V1 output set")
    failures = properties.get("failures", {})
    items = failures.get("items", {}) if isinstance(failures, dict) else {}
    if not isinstance(items, dict) or items.get("enum") != list(FAILURE_CODES):
        errors.append("failures enum drifted from the typed failure vocabulary")
    return errors


def assert_offline_imports(root: Path) -> str | None:
    for relative in (SEMANTIC_ORACLE, ADAPTER_RELATIVE):
        source = (root / relative).read_text(encoding="utf-8")
        tree = ast.parse(source)
        imported: set[str] = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                imported.update(alias.name.split(".", 1)[0] for alias in node.names)
                imported.update(alias.name for alias in node.names)
            elif isinstance(node, ast.ImportFrom) and node.module:
                imported.add(node.module.split(".", 1)[0])
                imported.add(node.module)
        blocked = sorted(imported & OFFLINE_IMPORTS)
        if blocked:
            return f"{relative} imports provider or network modules: " + ", ".join(blocked)
    return None


def load_named_json(path: Path) -> tuple[object | None, str | None]:
    try:
        return load_json(path), None
    except ValueError:
        return None, "malformed-structure"


def invoke_recorded_adapter(
    root: Path,
    output_dir: Path,
    processor: Path,
    process: Path,
    pdf: Path,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            str(root / ADAPTER_RELATIVE),
            "--processor-json",
            str(processor),
            "--process-json",
            str(process),
            "--input-pdf",
            str(pdf),
            "--output-dir",
            str(output_dir),
            "--account-ref",
            "acct:opaque:synthetic-001",
            "--tenant",
            "synthetic-tenant",
        ],
        cwd=str(root),
        capture_output=True,
        text=True,
    )


def recorded_adapter_errors(root: Path) -> list[str]:
    recorded = root / RECORDED_DIR
    required = {
        RECORDED_PROCESSOR: recorded / RECORDED_PROCESSOR,
        RECORDED_PROCESS: recorded / RECORDED_PROCESS,
        RECORDED_PDF: recorded / RECORDED_PDF,
        RECORDED_MISSING_IDENTITY: recorded / RECORDED_MISSING_IDENTITY,
        RECORDED_LEGACY_ONLY: recorded / RECORDED_LEGACY_ONLY,
        RECORDED_EMPTY_LAYOUT: recorded / RECORDED_EMPTY_LAYOUT,
        ADAPTER_RELATIVE: root / ADAPTER_RELATIVE,
    }
    missing = [
        path.as_posix()
        for path in required.values()
        if not path.is_file()
    ]
    if missing:
        return [f"missing recorded adapter input: {path}" for path in missing]

    errors: list[str] = []
    with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
        first_dir = Path(first)
        second_dir = Path(second)
        first_run = invoke_recorded_adapter(
            root,
            first_dir,
            required[RECORDED_PROCESSOR],
            required[RECORDED_PROCESS],
            required[RECORDED_PDF],
        )
        if first_run.returncode != 0:
            detail = first_run.stderr.strip() or first_run.stdout.strip() or "no adapter output"
            return [f"recorded adapter failed: {detail}"]
        second_run = invoke_recorded_adapter(
            root,
            second_dir,
            required[RECORDED_PROCESSOR],
            required[RECORDED_PROCESS],
            required[RECORDED_PDF],
        )
        if second_run.returncode != 0:
            detail = second_run.stderr.strip() or second_run.stdout.strip() or "no adapter output"
            return [f"recorded adapter replay failed: {detail}"]
        for name in (ADAPTER_ENVELOPE_NAME, ADAPTER_ATTESTATION_NAME):
            left = first_dir / name
            right = second_dir / name
            if not left.is_file() or not right.is_file():
                errors.append(f"recorded adapter did not write {name}")
                continue
            if left.read_bytes() != right.read_bytes():
                errors.append(f"recorded adapter replay bytes differ for {name}")
        if errors:
            return errors
        envelope = load_json(first_dir / ADAPTER_ENVELOPE_NAME)
        envelope_reason = classify_envelope(envelope)
        if envelope_reason is not None:
            errors.append(
                f"recorded adapter envelope is structurally invalid: {envelope_reason}"
            )
        elif not isinstance(envelope, dict):
            errors.append("recorded adapter envelope is not an object")
        else:
            admission = envelope.get("admission")
            required_reasons = {
                "dlp-not-evaluated",
                "hazard-evidence-unavailable",
                "response-input-binding-unavailable",
            }
            if not isinstance(admission, dict):
                errors.append("recorded adapter envelope admission is missing")
            else:
                reasons = admission.get("reasons")
                if admission.get("status") != "rejected":
                    errors.append("recorded adapter envelope must be rejected")
                if admission.get("packet") != "ineligible":
                    errors.append("recorded adapter envelope packet must be ineligible")
                if admission.get("query") != "ineligible":
                    errors.append("recorded adapter envelope query must be ineligible")
                if not isinstance(reasons, list) or not required_reasons <= set(reasons):
                    errors.append(
                        "recorded adapter envelope missing unavailable-check reasons"
                    )
            pdf_digest = sha256_hex(required[RECORDED_PDF].read_bytes())
            original = envelope.get("original")
            source = envelope.get("source")
            if not isinstance(original, dict) or original.get("sha256") != pdf_digest:
                errors.append("recorded adapter envelope original digest drifted")
            if not isinstance(source, dict) or source.get("locator") != f"sha256:{pdf_digest}":
                errors.append("recorded adapter envelope source is not the input digest")
        attestation, parse_reason = load_named_json(first_dir / ADAPTER_ATTESTATION_NAME)
        reason, typed = (
            (parse_reason, [])
            if parse_reason is not None
            else evaluate(attestation, output_root=first_dir)
        )
        if reason is not None:
            errors.append(f"recorded adapter attestation is structurally invalid: {reason}")
        elif not isinstance(attestation, dict):
            errors.append("recorded adapter attestation is not an object")
        else:
            if attestation["source"]["locator"] != EXPECTED_PROCESSOR_LOCATOR:
                errors.append("recorded adapter locator is not the saved processor version")
            if attestation["runtime"]["network"] != "not-invoked":
                errors.append("recorded adapter must record runtime.network=not-invoked")
            if attestation["permission"]["status"] == "attested":
                errors.append("recorded adapter invented permission evidence")
            if attestation["source"]["revision"]["state"] == "attested":
                errors.append("recorded adapter invented revision evidence")
            if attestation["cost"]["status"] != "unavailable":
                errors.append("recorded adapter invented cost evidence")
            if attestation["tenancy"]["perimeter"] != "unavailable":
                errors.append("recorded adapter invented perimeter evidence")
            if attestation["capability"]["requested"] != "parsed":
                errors.append("recorded adapter requested capability drifted")
            if attestation["capability"]["effective"] != "parsed":
                errors.append("recorded adapter effective capability drifted")
            if attestation["capability"]["attested"] != "parsed":
                errors.append("recorded adapter attested capability drifted")
            if attestation["admission"]["status"] != "rejected":
                errors.append("recorded adapter attestation must remain rejected")
            if attestation["admission"]["packet"] != "ineligible":
                errors.append("recorded adapter attestation packet must be ineligible")
            if "permission-unavailable" not in typed:
                errors.append("recorded adapter must leave permission unavailable")
        closed = (
            ("missing-identity", required[RECORDED_MISSING_IDENTITY], required[RECORDED_PROCESS]),
            ("legacy-only", required[RECORDED_PROCESSOR], required[RECORDED_LEGACY_ONLY]),
            ("empty-layout", required[RECORDED_PROCESSOR], required[RECORDED_EMPTY_LAYOUT]),
        )
        for label, processor, process in closed:
            dest = first_dir / label
            dest.mkdir()
            result = invoke_recorded_adapter(
                root,
                dest,
                processor,
                process,
                required[RECORDED_PDF],
            )
            if result.returncode == 0:
                errors.append(f"recorded adapter accepted {label}")
        mismatch = load_json(required[RECORDED_PROCESS])
        if isinstance(mismatch, dict) and isinstance(mismatch.get("document"), dict):
            mismatch["document"]["mimeType"] = "text/plain"
            mismatch_path = first_dir / "process-mime-mismatch.json"
            mismatch_path.write_bytes(canonical_bytes(mismatch) + b"\n")
            mime_run = invoke_recorded_adapter(
                root,
                first_dir / "mime-mismatch",
                required[RECORDED_PROCESSOR],
                mismatch_path,
                required[RECORDED_PDF],
            )
            if mime_run.returncode == 0:
                errors.append("recorded adapter accepted mime-mismatch")
        else:
            errors.append("recorded process fixture missing document")
        errors.extend(recorded_oversized_text_errors(root, first_dir, required))
        errors.extend(recorded_overwrite_errors(root, first_dir, required))
    return errors


def recorded_oversized_text_errors(
    root: Path,
    work_dir: Path,
    required: dict[str, Path],
) -> list[str]:
    extra = 64
    oversized = {
        "document": {
            "documentLayout": {
                "blocks": [
                    {
                        "blockId": "1",
                        "pageSpan": {"pageEnd": 1, "pageStart": 1},
                        "textBlock": {
                            "text": "A" * (MAX_TEXT_BYTES + extra),
                            "type": "paragraph",
                        },
                    }
                ]
            }
        }
    }
    process_path = work_dir / "process-oversized-text.json"
    process_path.write_bytes(canonical_bytes(oversized) + b"\n")
    dest = work_dir / "oversized-text"
    result = invoke_recorded_adapter(
        root,
        dest,
        required[RECORDED_PROCESSOR],
        process_path,
        required[RECORDED_PDF],
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "no adapter output"
        return [f"recorded adapter rejected oversized-text probe: {detail}"]
    errors: list[str] = []
    envelope, envelope_parse = load_named_json(dest / ADAPTER_ENVELOPE_NAME)
    if envelope_parse is not None:
        return [f"oversized-text envelope is malformed: {envelope_parse}"]
    envelope_reason = classify_envelope(envelope)
    if envelope_reason is not None:
        return [f"oversized-text envelope is structurally invalid: {envelope_reason}"]
    attestation, attestation_parse = load_named_json(dest / ADAPTER_ATTESTATION_NAME)
    reason, _typed = (
        (attestation_parse, [])
        if attestation_parse is not None
        else evaluate(attestation, output_root=dest)
    )
    if reason is not None:
        return [f"oversized-text attestation is structurally invalid: {reason}"]
    if not isinstance(envelope, dict) or not isinstance(attestation, dict):
        return ["oversized-text outputs are not objects"]
    envelope_truncation = envelope.get("receipts", {})
    if isinstance(envelope_truncation, dict):
        envelope_truncation = envelope_truncation.get("truncation")
    attestation_truncation = attestation.get("truncation")
    if not isinstance(envelope_truncation, dict) or not isinstance(
        attestation_truncation, dict
    ):
        return ["oversized-text truncation receipts are missing"]
    omitted_bytes = envelope_truncation.get("omitted_bytes")
    omitted_items = envelope_truncation.get("omitted_anchor_count")
    if envelope_truncation.get("truncated") is not True or omitted_bytes in {None, 0}:
        errors.append("oversized-text envelope did not report nonzero omission")
    if (
        attestation_truncation.get("truncated") != envelope_truncation.get("truncated")
        or attestation_truncation.get("omitted_bytes") != omitted_bytes
        or attestation_truncation.get("omitted_item_count") != omitted_items
        or attestation_truncation.get("reason") != envelope_truncation.get("reason")
    ):
        errors.append("oversized-text #13 truncation disagrees with #5")
    return errors


def recorded_overwrite_errors(
    root: Path,
    work_dir: Path,
    required: dict[str, Path],
) -> list[str]:
    dest = work_dir / "sentinel-overwrite"
    dest.mkdir()
    envelope_path = dest / ADAPTER_ENVELOPE_NAME
    attestation_path = dest / ADAPTER_ATTESTATION_NAME
    envelope_bytes = b"sentinel-envelope\n"
    attestation_bytes = b"sentinel-attestation\n"
    envelope_path.write_bytes(envelope_bytes)
    attestation_path.write_bytes(attestation_bytes)
    result = invoke_recorded_adapter(
        root,
        dest,
        required[RECORDED_PROCESSOR],
        required[RECORDED_PROCESS],
        required[RECORDED_PDF],
    )
    errors: list[str] = []
    if result.returncode == 0:
        errors.append("recorded adapter overwrote existing outputs")
    if envelope_path.read_bytes() != envelope_bytes:
        errors.append("recorded adapter mutated existing envelope bytes")
    if attestation_path.read_bytes() != attestation_bytes:
        errors.append("recorded adapter mutated existing attestation bytes")
    return errors


def main() -> int:
    root = bran_root()
    offline_error = assert_offline_imports(root)
    if offline_error is not None:
        print(f"FAIL google attestation contract check: {offline_error}")
        return 1

    schema_path = root / "schemas/google-source-attestation.schema.json"
    fixture_root = root / "fixtures/google-attestation"
    positive_root = fixture_root / "positive"
    negative_root = fixture_root / "negative"
    try:
        schema = load_json(schema_path)
    except ValueError as error:
        print(f"FAIL google attestation contract check: {error}")
        return 1

    schema_errors = validate_schema(schema)
    if schema_errors:
        print("FAIL google attestation contract check: schema drifted")
        for error in schema_errors:
            print(f"  {error}")
        return 1

    contract_errors: list[str] = []
    for name in sorted(PRODUCTS):
        if name not in PRODUCT_OUTPUT:
            contract_errors.append(f"missing output contract for {name}")
        if parse_product_locator(name, OPAQUE_LOCATOR) is not None:
            contract_errors.append(f"{name} accepted an unrecognized locator")
    if contract_errors:
        print("FAIL google attestation contract check: product contract incomplete")
        for error in contract_errors:
            print(f"  {error}")
        return 1

    missing = [
        path.as_posix()
        for path in (
            [positive_root / name for name in POSITIVE_NAMES]
            + [negative_root / name for name in NEGATIVE_FAILURES]
        )
        if not path.is_file()
    ]
    if missing:
        print("FAIL google attestation contract check: missing fixtures")
        for path in missing:
            print(f"  {path}")
        return 1

    failures: list[str] = []
    accepted = 0
    golden_bytes: bytes | None = None
    golden_digest: str | None = None
    for name in POSITIVE_NAMES:
        path = positive_root / name
        record, parse_reason = load_named_json(path)
        reason, typed = (
            (parse_reason, []) if parse_reason is not None else evaluate(record)
        )
        if reason is not None or typed:
            shown = reason if reason is not None else ",".join(typed)
            failures.append(f"{path.relative_to(root).as_posix()} rejected as {shown}")
            continue
        assert isinstance(record, dict)
        if record["admission"]["status"] != "admitted":
            failures.append(
                f"{path.relative_to(root).as_posix()} is structurally valid but not admitted"
            )
            continue
        if name == "document-ai.json" and record["output"]["envelope_path"] != DOCUMENT_AI_ENVELOPE:
            failures.append("document-ai positive must reference the integrated #5 PDF envelope")
            continue
        accepted += 1
        if golden_bytes is None:
            golden_bytes = canonical_bytes(record)
            golden_digest = record["attestation_digest"]
            permuted = permute(record)
            if canonical_bytes(permuted) != golden_bytes:
                failures.append(
                    "permuted in-memory JSON did not serialize to the golden canonical bytes"
                )
            if not isinstance(permuted, dict) or attestation_digest(permuted) != golden_digest:
                failures.append(
                    "permuted in-memory JSON did not reproduce the golden attestation digest"
                )

    gemini_path = positive_root / "gemini-enterprise.json"
    gemini_record, gemini_reason = load_named_json(gemini_path)
    if gemini_reason is not None or not isinstance(gemini_record, dict):
        failures.append("gemini-enterprise positive is required for locator closure")
    else:
        opaque_record = with_digests(gemini_record)
        opaque_record["source"]["locator"] = OPAQUE_LOCATOR
        opaque_record = with_digests(opaque_record)
        opaque_failures = infer_failures(opaque_record)
        if not {"location-mismatch", "tenant-escape"} <= set(opaque_failures):
            failures.append(
                "opaque locator did not emit location-mismatch and tenant-escape"
            )
        _status, packet_state, query_state = admission_eligibility(opaque_failures)
        if packet_state != "ineligible" or query_state != "ineligible":
            failures.append("opaque locator remained packet or query eligible")

    rejected = 0
    for name, expected in NEGATIVE_FAILURES.items():
        path = negative_root / name
        record, parse_reason = load_named_json(path)
        reason, typed = (
            (parse_reason, []) if parse_reason is not None else evaluate(record)
        )
        if reason is not None:
            failures.append(
                f"{path.relative_to(root).as_posix()} expected {','.join(sorted(expected))}, got {reason}"
            )
            continue
        if frozenset(typed) != expected:
            failures.append(
                f"{path.relative_to(root).as_posix()} expected {','.join(sorted(expected))}, got {','.join(typed)}"
            )
            continue
        rejected += 1

    extra = sorted(
        path.name
        for directory, expected_names in (
            (positive_root, set(POSITIVE_NAMES)),
            (negative_root, set(NEGATIVE_FAILURES)),
        )
        for path in directory.iterdir()
        if path.is_file() and path.name not in expected_names
    )
    if extra:
        failures.append("unexpected fixture files: " + ", ".join(extra))

    failures.extend(recorded_adapter_errors(root))

    if failures:
        print("FAIL google attestation contract check")
        for failure in failures:
            print(f"  {failure}")
        return 1

    print(
        f"PASS google source attestation: positives={accepted} negatives={rejected}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
