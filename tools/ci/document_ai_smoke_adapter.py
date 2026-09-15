#!/usr/bin/env python3
"""Replay a saved Document AI response into #5/#13 artifacts. No network."""

from __future__ import annotations

import argparse
import hashlib
import os
import sys
from pathlib import Path
from typing import Any

from enterprise_contract_check import (
    MAX_ORIGINAL_BYTES,
    MAX_TEXT_BYTES,
    canonical_bytes,
    classify,
    envelope_digest,
    is_id,
    load_json,
    normalized_digest,
    text_digest,
)
from google_attestation_contract_check import (
    ACCOUNT_PREFIX,
    ENVELOPE_KIND,
    GOOGLE_LOCATOR_TEMPLATE,
    MAX_ATTESTATION_BYTES,
    MAX_RESULT_IDS,
    PROFILE_IDENTITY,
    SCHEMA_VERSION,
    _parse_template_locator,
    admission_eligibility,
    attestation_digest,
    configured_digest,
    envelope_path_error,
    filter_digest,
    infer_failures,
    structural_reason,
)


PROCESSOR_TEMPLATE = (
    "projects",
    "project",
    "locations",
    "location",
    "processors",
    "processor",
)
ENVELOPE_NAME = "enterprise-document-evidence-envelope.json"
ATTESTATION_NAME = "google-source-attestation.json"
PARSER_IDENTITY = "google-document-ai"
PRODUCT_VERSION = "1.0"
REQUESTED_CAPABILITY = "parsed"
BBOX_SCALE = 1000
BBOX_MAX = 1_000_000
ENVELOPE_INELIGIBLE_REASONS = (
    "dlp-not-evaluated",
    "hazard-evidence-unavailable",
    "response-input-binding-unavailable",
)


class AdapterError(Exception):
    pass


def contained(base: Path, target: Path) -> bool:
    try:
        target.resolve().relative_to(base.resolve())
    except ValueError:
        return False
    return True


def clip_text(text: str) -> tuple[str, bool]:
    raw = text.encode("utf-8")
    if len(raw) <= MAX_TEXT_BYTES:
        return text, False
    return raw[:MAX_TEXT_BYTES].decode("utf-8", errors="ignore"), True


def page_of(block: dict[str, Any], inherited: int) -> int:
    span = block.get("pageSpan")
    if isinstance(span, dict):
        start = span.get("pageStart")
        if type(start) is int and start >= 1:
            return start
    return inherited


def bbox_of(block: dict[str, Any]) -> tuple[dict[str, int], bool]:
    empty = {"x0": 0, "y0": 0, "x1": 0, "y1": 0}
    box = block.get("boundingBox")
    if not isinstance(box, dict):
        return empty, False
    vertices = box.get("normalizedVertices")
    if not isinstance(vertices, list):
        return empty, False
    xs: list[int] = []
    ys: list[int] = []
    for vertex in vertices:
        if not isinstance(vertex, dict):
            continue
        x, y = vertex.get("x"), vertex.get("y")
        if isinstance(x, (int, float)) and isinstance(y, (int, float)):
            xs.append(min(max(int(round(float(x) * BBOX_SCALE)), 0), BBOX_MAX))
            ys.append(min(max(int(round(float(y) * BBOX_SCALE)), 0), BBOX_MAX))
    if not xs:
        return empty, False
    return {"x0": min(xs), "y0": min(ys), "x1": max(xs), "y1": max(ys)}, True


def collect_texts(blocks: object) -> list[str]:
    found: list[str] = []
    if not isinstance(blocks, list):
        return found
    for block in blocks:
        if not isinstance(block, dict):
            continue
        text_block = block.get("textBlock")
        if isinstance(text_block, dict):
            text = text_block.get("text")
            if isinstance(text, str) and text.strip():
                found.append(text.strip())
            found.extend(collect_texts(text_block.get("blocks")))
        table_block = block.get("tableBlock")
        if isinstance(table_block, dict):
            table_text = table_block_text(table_block)
            if table_text:
                found.append(table_text)
    return found


def table_block_text(table_block: dict[str, Any]) -> str:
    parts: list[str] = []
    caption = table_block.get("caption")
    if isinstance(caption, str) and caption.strip():
        parts.append(caption.strip())
    for key in ("headerRows", "bodyRows"):
        rows = table_block.get(key)
        if not isinstance(rows, list):
            continue
        for row in rows:
            if not isinstance(row, dict):
                continue
            cells = row.get("cells")
            if not isinstance(cells, list):
                continue
            for cell in cells:
                if not isinstance(cell, dict):
                    continue
                cell_text = " ".join(collect_texts(cell.get("blocks")))
                if cell_text:
                    parts.append(cell_text)
    return " ".join(parts)


def role_of(block_type: object) -> str:
    if not isinstance(block_type, str):
        return "paragraph"
    lowered = block_type.strip().lower()
    if lowered.startswith("heading"):
        return "heading"
    if lowered in {"footer", "header", "list", "title"}:
        return lowered
    return "paragraph"


def derive_locator(processor: object) -> tuple[str, dict[str, str]]:
    if not isinstance(processor, dict):
        raise AdapterError("processor response is not an object")
    name = processor.get("name")
    default_version = processor.get("defaultProcessorVersion")
    version_locator: str | None = None
    processor_locator: str | None = None
    if isinstance(default_version, str) and default_version.strip():
        parsed = _parse_template_locator(
            default_version, GOOGLE_LOCATOR_TEMPLATE["document-ai"]
        )
        if parsed is None:
            raise AdapterError("default processor version is not a document-ai locator")
        version_locator = default_version
    if isinstance(name, str) and name.strip():
        as_version = _parse_template_locator(name, GOOGLE_LOCATOR_TEMPLATE["document-ai"])
        as_processor = _parse_template_locator(name, PROCESSOR_TEMPLATE)
        if as_version is not None:
            if version_locator is not None and name != version_locator:
                raise AdapterError("processor name and default version disagree")
            version_locator = name
        elif as_processor is not None:
            processor_locator = name
        else:
            raise AdapterError("processor name is not a processor identity")
    if version_locator is None:
        raise AdapterError("full processor identity and default version are unavailable")
    parsed_version = _parse_template_locator(
        version_locator, GOOGLE_LOCATOR_TEMPLATE["document-ai"]
    )
    if parsed_version is None:
        raise AdapterError("processor version locator is not a document-ai locator")
    if processor_locator is not None:
        parsed_processor = _parse_template_locator(processor_locator, PROCESSOR_TEMPLATE)
        if parsed_processor is None:
            raise AdapterError("processor identity is incomplete")
        prefix = processor_locator + "/processorVersions/"
        if not version_locator.startswith(prefix):
            raise AdapterError("locator and processor identity disagree")
        if (
            parsed_processor["project"] != parsed_version["project"]
            or parsed_processor["location"] != parsed_version["location"]
            or parsed_processor["processor"] != parsed_version["processor"]
        ):
            raise AdapterError("locator project or location disagrees")
    return version_locator, parsed_version


def document_of(process: object) -> dict[str, Any]:
    if not isinstance(process, dict):
        raise AdapterError("process response is not an object")
    document = process.get("document")
    if not isinstance(document, dict):
        raise AdapterError("process response has no document")
    mime_type = document.get("mimeType")
    if mime_type is not None and mime_type != "application/pdf":
        raise AdapterError("process document mimeType is not application/pdf")
    return document


def layout_blocks(document: dict[str, Any]) -> list[object]:
    layout = document.get("documentLayout")
    if not isinstance(layout, dict):
        raise AdapterError("documentLayout blocks contain no usable text")
    blocks = layout.get("blocks")
    if not isinstance(blocks, list):
        raise AdapterError("documentLayout blocks contain no usable text")
    return blocks


class LayoutSink:
    def __init__(self) -> None:
        self.anchors: list[dict[str, Any]] = []
        self.texts: list[str] = []
        self.omitted_anchors = 0
        self.omitted_bytes = 0
        self.saw_table = False
        self.saw_bbox = False

    def add(self, role: str, text: str, page: int, bbox: dict[str, int], saw_bbox: bool) -> None:
        clipped, truncated = clip_text(text.strip())
        if not clipped.strip():
            return
        if truncated:
            self.omitted_bytes += max(len(text.encode("utf-8")) - MAX_TEXT_BYTES, 0)
        if len(self.anchors) >= 4096:
            self.omitted_anchors += 1
            return
        index = len(self.anchors) + 1
        self.anchors.append(
            {
                "id": f"anc:pdf:{page:04d}:{index:04d}",
                "family": "fixed-layout",
                "role": role,
                "text": clipped,
                "text_digest": text_digest(clipped),
                "locator": {
                    "family": "fixed-layout",
                    "page": page,
                    "block": index,
                    "bbox": bbox,
                },
            }
        )
        self.texts.append(clipped)
        if saw_bbox:
            self.saw_bbox = True
        if role == "table":
            self.saw_table = True

    def walk(self, blocks: object, inherited_page: int) -> None:
        if not isinstance(blocks, list):
            return
        for block in blocks:
            if not isinstance(block, dict):
                continue
            page = page_of(block, inherited_page)
            bbox, saw_bbox = bbox_of(block)
            table_block = block.get("tableBlock")
            if isinstance(table_block, dict):
                table_text = table_block_text(table_block)
                if table_text:
                    self.add("table", table_text, page, bbox, saw_bbox)
                continue
            text_block = block.get("textBlock")
            if isinstance(text_block, dict):
                nested = text_block.get("blocks")
                before = len(self.anchors)
                if isinstance(nested, list) and nested:
                    self.walk(nested, page)
                if len(self.anchors) == before:
                    text = text_block.get("text")
                    if isinstance(text, str) and text.strip():
                        self.add(role_of(text_block.get("type")), text, page, bbox, saw_bbox)


def load_pdf(path: Path) -> tuple[int, str]:
    try:
        data = path.read_bytes()
    except OSError as error:
        raise AdapterError(f"cannot read PDF: {error}") from error
    if not data.startswith(b"%PDF"):
        raise AdapterError("input is not a PDF")
    if not (1 <= len(data) <= MAX_ORIGINAL_BYTES):
        raise AdapterError("PDF is empty or exceeds the recorded input bound")
    return len(data), hashlib.sha256(data).hexdigest()


def write_json(path: Path, value: object) -> None:
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL)
    except FileExistsError as error:
        raise AdapterError("refusing to overwrite existing adapter output") from error
    with os.fdopen(fd, "wb") as handle:
        handle.write(canonical_bytes(value) + b"\n")


def refuse_existing_outputs(envelope_path: Path, attestation_path: Path) -> None:
    if envelope_path.exists() or attestation_path.exists():
        raise AdapterError("refusing to overwrite existing adapter output")


def attestation_truncation(envelope: dict[str, Any]) -> dict[str, Any]:
    receipts = envelope.get("receipts")
    if not isinstance(receipts, dict):
        raise AdapterError("envelope truncation receipt is unavailable")
    source = receipts.get("truncation")
    if not isinstance(source, dict):
        raise AdapterError("envelope truncation receipt is unavailable")
    truncated = source.get("truncated")
    omitted_bytes = source.get("omitted_bytes")
    omitted_items = source.get("omitted_anchor_count")
    reason = source.get("reason")
    if type(truncated) is not bool:
        raise AdapterError("envelope truncation receipt is malformed")
    if type(omitted_bytes) is not int or omitted_bytes < 0:
        raise AdapterError("envelope truncation receipt is malformed")
    if type(omitted_items) is not int or omitted_items < 0:
        raise AdapterError("envelope truncation receipt is malformed")
    if omitted_bytes > MAX_ATTESTATION_BYTES or omitted_items > MAX_RESULT_IDS:
        raise AdapterError(
            "envelope truncation cannot be represented on the attestation"
        )
    if truncated:
        if not isinstance(reason, str) or not reason.strip() or len(reason) > 256:
            raise AdapterError(
                "envelope truncation cannot be represented on the attestation"
            )
    elif reason is not None or omitted_bytes != 0 or omitted_items != 0:
        raise AdapterError("envelope truncation receipt is malformed")
    return {
        "truncated": truncated,
        "omitted_bytes": omitted_bytes,
        "omitted_item_count": omitted_items,
        "reason": reason,
    }


def build_envelope(
    *,
    source_locator: str,
    processor_version: str,
    pdf_len: int,
    pdf_digest: str,
    sink: LayoutSink,
) -> dict[str, Any]:
    joined = " ".join(sink.texts)
    text, truncated_text = clip_text(joined)
    if not text.strip():
        raise AdapterError("documentLayout blocks contain no usable text")
    omitted_bytes = sink.omitted_bytes
    if truncated_text:
        omitted_bytes += max(len(joined.encode("utf-8")) - MAX_TEXT_BYTES, 0)
    truncated = bool(sink.omitted_anchors or omitted_bytes)
    content = {"family": "fixed-layout", "language": "en", "text": text}
    fidelity = {
        "bounding_boxes": "approximated" if sink.saw_bbox else "unsupported",
        "figures": "unsupported",
        "javascript": "unsupported",
        "ocr": "approximated",
        "reading_order": "approximated",
        "tables": "normalized" if sink.saw_table else "unsupported",
        "text": "normalized",
    }
    unavailable = sorted(
        feature for feature, status in fidelity.items() if status == "unsupported"
    )
    anchors = sorted(sink.anchors, key=lambda item: item["id"])
    # present=false: none observed in the recorded response, not original-PDF proof.
    envelope = {
        "schema_version": SCHEMA_VERSION,
        "evidence_id": f"evd:document-ai:{processor_version}",
        "envelope_digest": "0" * 64,
        "original": {
            "media_type": "application/pdf",
            "byte_length": pdf_len,
            "sha256": pdf_digest,
        },
        "parser": {
            "identity": PARSER_IDENTITY,
            "processor": processor_version,
            "version": PRODUCT_VERSION,
            "attestation": "unavailable",
        },
        "source": {
            "locator": source_locator,
            "revision": {"state": "unavailable", "value": None},
        },
        "normalized": {"digest": normalized_digest(content), "content": content},
        "anchors": anchors,
        "assets": [],
        "relations": [],
        "fidelity": fidelity,
        "receipts": {
            "truncation": {
                "truncated": truncated,
                "omitted_bytes": omitted_bytes if truncated else 0,
                "omitted_anchor_count": sink.omitted_anchors if truncated else 0,
                "reason": "normalized-text-bound" if truncated else None,
            },
            "malformed_input": {"present": False, "reason": None},
            "unavailable": {
                "features": unavailable,
                "revision": True,
                "parser_attestation": True,
            },
        },
        "hazards": {
            "active_content": {"present": False, "kinds": []},
            "external_references": {"present": False, "count": 0},
        },
        "policy": {
            "classification": {"status": "unavailable", "value": None},
            "dlp": {"status": "not-evaluated", "findings": []},
            "public_boundary": {"outcome": "unavailable", "value": None},
        },
        "admission": {
            "status": "rejected",
            "packet": "ineligible",
            "query": "ineligible",
            "reasons": list(ENVELOPE_INELIGIBLE_REASONS),
        },
    }
    if not is_id(envelope["evidence_id"]):
        raise AdapterError("evidence identity cannot be attested")
    envelope["envelope_digest"] = envelope_digest(envelope)
    reason = classify(envelope)
    if reason is not None:
        raise AdapterError(f"envelope cannot be attested: {reason}")
    return envelope


def build_attestation(
    *,
    locator: str,
    parsed: dict[str, str],
    processor_version: str,
    account_ref: str,
    tenant: str,
    envelope: dict[str, Any],
    effective: str,
    attested: str,
) -> dict[str, Any]:
    filters = {"processor": processor_version}
    tenancy = {
        "tenant": tenant,
        "project": parsed["project"],
        "location": parsed["location"],
        "account_ref": account_ref,
        "perimeter": "unavailable",
    }
    record = {
        "schema_version": SCHEMA_VERSION,
        "attestation_id": f"att:google:document-ai:{parsed['processor']}",
        "attestation_digest": "0" * 64,
        "profile": {"identity": PROFILE_IDENTITY, "version": SCHEMA_VERSION},
        "product": {
            "name": "document-ai",
            "identity": PARSER_IDENTITY,
            "component": processor_version,
            "version": PRODUCT_VERSION,
        },
        "capability": {
            "requested": REQUESTED_CAPABILITY,
            "effective": effective,
            "attested": attested,
        },
        "tenancy": tenancy,
        "source": {
            "locator": locator,
            "filters": filters,
            "configured_digest": configured_digest(tenancy, locator, filters),
            "filter_digest": filter_digest(filters),
            "revision": {"state": "unavailable", "kind": "unavailable", "value": None},
        },
        "permission": {"status": "unavailable", "authorization_proof": False},
        "output": {
            "kind": ENVELOPE_KIND,
            "digest": envelope["envelope_digest"],
            "envelope_path": ENVELOPE_NAME,
            "normalized": None,
        },
        "checkpoint": {
            "id": f"ckpt:document-ai:{parsed['processor']}",
            "digest": None,
            "state": "unavailable",
            "revision_value": None,
        },
        "truncation": attestation_truncation(envelope),
        "cost": {"status": "unavailable", "quota": "unavailable"},
        "runtime": {"network": "not-invoked"},
        "policy": {
            "classification": {"status": "unavailable", "value": None},
            "dlp": {"status": "not-evaluated", "findings": []},
            "public_boundary": {"outcome": "unavailable", "value": None},
        },
        "failures": [],
        "admission": {
            "status": "admitted",
            "packet": "eligible",
            "query": "eligible",
            "reasons": [],
        },
    }
    if not is_id(record["attestation_id"]) or not is_id(record["checkpoint"]["id"]):
        raise AdapterError("attestation identity cannot be attested")
    path_error = envelope_path_error(record["output"]["envelope_path"], allow_leaf=True)
    if path_error is not None:
        raise AdapterError(f"output path is unsafe: {path_error}")
    record["failures"] = infer_failures(record)
    status, packet, query = admission_eligibility(record["failures"])
    record["admission"] = {
        "status": status,
        "packet": packet,
        "query": query,
        "reasons": list(record["failures"]),
    }
    record["attestation_digest"] = attestation_digest(record)
    reason = structural_reason(record, envelope=envelope)
    if reason is not None:
        raise AdapterError(f"attestation cannot be attested: {reason}")
    return record


def adapt(
    processor: object,
    process: object,
    pdf_len: int,
    pdf_digest: str,
    account_ref: str,
    tenant: str,
) -> tuple[dict[str, Any], dict[str, Any]]:
    if not account_ref.startswith(ACCOUNT_PREFIX) or not is_id(account_ref):
        raise AdapterError("account reference is not an opaque acct:opaque identity")
    if not isinstance(tenant, str) or not tenant.strip() or len(tenant) > 128:
        raise AdapterError("tenant is missing")
    locator, parsed = derive_locator(processor)
    document = document_of(process)
    sink = LayoutSink()
    sink.walk(layout_blocks(document), 1)
    if not sink.texts:
        raise AdapterError("documentLayout blocks contain no usable text")
    envelope = build_envelope(
        source_locator=f"sha256:{pdf_digest}",
        processor_version=parsed["processor_version"],
        pdf_len=pdf_len,
        pdf_digest=pdf_digest,
        sink=sink,
    )
    effective = "parsed" if sink.texts else "unavailable"
    attested = "parsed" if effective == "parsed" and locator else "unavailable"
    attestation = build_attestation(
        locator=locator,
        parsed=parsed,
        processor_version=parsed["processor_version"],
        account_ref=account_ref,
        tenant=tenant,
        envelope=envelope,
        effective=effective,
        attested=attested,
    )
    return envelope, attestation


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--processor-json", required=True, type=Path)
    parser.add_argument("--process-json", required=True, type=Path)
    parser.add_argument("--input-pdf", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--account-ref", required=True)
    parser.add_argument("--tenant", required=True)
    args = parser.parse_args(argv)
    try:
        processor = load_json(args.processor_json)
        process = load_json(args.process_json)
        pdf_len, pdf_digest = load_pdf(args.input_pdf)
        output_dir = args.output_dir.resolve()
        output_dir.mkdir(parents=True, exist_ok=True)
        envelope_path = (output_dir / ENVELOPE_NAME).resolve()
        attestation_path = (output_dir / ATTESTATION_NAME).resolve()
        if not contained(output_dir, envelope_path) or not contained(
            output_dir, attestation_path
        ):
            raise AdapterError("output paths escape the output directory")
        refuse_existing_outputs(envelope_path, attestation_path)
        envelope, attestation = adapt(
            processor,
            process,
            pdf_len,
            pdf_digest,
            args.account_ref,
            args.tenant,
        )
        write_json(envelope_path, envelope)
        write_json(attestation_path, attestation)
    except (AdapterError, ValueError, OSError) as error:
        print(f"FAIL document-ai recorded adapter: {error}", file=sys.stderr)
        return 1
    print(f"PASS document-ai recorded adapter: {envelope_path} {attestation_path}")
    return 0


def main() -> int:
    return run(sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
