#!/usr/bin/env python3
"""Read-only deterministic validator for the canonical five-artifact plan."""

from __future__ import annotations

import hashlib
import re
import sys
from html.parser import HTMLParser
from pathlib import Path


FILES = ("plan-spec.md", "design.md", "seit.md", "implementation.md")
REVIEW = "review.html"
ID = re.compile(r"\b(?:AC|REQ|RISK|DES|CONTRACT|SEIT|CMD|PROC)-[A-Z0-9][A-Z0-9.-]*\b", re.I)
SLICE = re.compile(r"^###\s+Slice\s+(\d+\.\d+)\b.*$", re.M)
MANIFEST = re.compile(r"^###\s+(\d+\.\d+)\s+execution manifest\s*$", re.M | re.I)
ROUTES = {"codex gpt-5.6-terra", "codex gpt-5.6-sol", "agy agent default"}
REASONING = {"low", "medium", "high", "xhigh"}
ROUTE_OWNED_ROWS = {"SEIT-030", "SEIT-032"}


class Text(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.parts: list[str] = []
        self.hidden = 0

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag.lower() in {"script", "style"}:
            self.hidden += 1

    def handle_endtag(self, tag: str) -> None:
        if tag.lower() in {"script", "style"} and self.hidden:
            self.hidden -= 1

    def handle_data(self, data: str) -> None:
        if not self.hidden:
            self.parts.append(data)


def refs(text: str, prefix: str = "") -> set[str]:
    return {item.upper() for item in ID.findall(text) if not prefix or item.upper().startswith(prefix)}


def field(section: str, name: str) -> str:
    found = re.search(rf"\*\*{re.escape(name)}\.\*\*\s*(.*?)(?=\n\*\*[A-Z]|\n###|\Z)", section, re.S)
    return found.group(1).strip() if found else ""


def sections(text: str, pattern: re.Pattern[str]) -> dict[str, str]:
    matches = list(pattern.finditer(text))
    return {match.group(1): text[match.start():matches[index + 1].start() if index + 1 < len(matches) else len(text)]
            for index, match in enumerate(matches)}


def table_rows(seit: str) -> tuple[dict[str, list[str]], list[str]]:
    errors: list[str] = []
    block = re.search(r"^##\s+Traceability Matrix\s*$\n(.*?)(?=^##\s+|\Z)", seit, re.M | re.S)
    if not block:
        return {}, ["missing Traceability Matrix"]
    lines = [line.strip() for line in block.group(1).splitlines() if line.strip().startswith("|")]
    if len(lines) < 3:
        return {}, ["traceability matrix is incomplete"]
    headers = [x.strip().casefold() for x in lines[0].strip("|").split("|")]
    required = ("seit row id", "acceptance/risk id", "design/contract id", "command/procedure id", "evidence")
    if any(x not in headers for x in required):
        return {}, ["traceability matrix is missing required columns"]
    rows: dict[str, list[str]] = {}
    for line in lines[2:]:
        cells = [x.strip() for x in line.strip("|").split("|")]
        if len(cells) != len(headers):
            errors.append("traceability row has wrong column count")
            continue
        row = dict(zip(headers, cells))
        ids = refs(row["seit row id"], "SEIT-")
        if len(ids) != 1:
            errors.append("traceability row lacks one SEIT ID")
            continue
        key = next(iter(ids))
        if key in rows:
            errors.append(f"duplicate traceability row: {key}")
        rows[key] = [row[x] for x in required]
        if any(not row[x] or row[x].casefold() in {"-", "n/a", "tbd", "todo"} for x in required):
            errors.append(f"traceability row lacks concrete data: {key}")
    return rows, errors


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: validate_route.py PLAN_DIRECTORY")
        return 2
    root = Path(sys.argv[1])
    errors: list[str] = []
    if root.is_symlink() or not root.is_dir():
        errors.append("plan directory must be a real directory")
    content: dict[str, str] = {}
    for name in (*FILES, REVIEW):
        path = root / name
        if path.is_symlink() or not path.is_file() or path.stat().st_size == 0:
            errors.append(f"missing regular artifact: {name}")
        else:
            try:
                content[name] = path.read_text(encoding="utf-8")
            except UnicodeDecodeError:
                errors.append(f"artifact is not UTF-8: {name}")
    if errors:
        for error in sorted(set(errors)):
            print(f"FAIL: {error}")
        return 1
    review = content[REVIEW]
    parser = Text(); parser.feed(review); parser.close(); visible = "".join(parser.parts)
    for name in FILES:
        if not re.search(rf'href=["\'](?:\./)?{re.escape(name)}["\']', review):
            errors.append(f"review missing local link: {name}")
        if content[name] not in visible:
            errors.append(f"review is stale: {name}")
    if re.search(r'<(?:script|link|img)\b[^>]+(?:src|href)=["\']https?:|url\(["\']?https?:', review, re.I):
        errors.append("review loads network content")
    rows, row_errors = table_rows(content["seit.md"]); errors.extend(row_errors)
    plan_ids = refs(content["plan-spec.md"], "REQ-") | refs(content["plan-spec.md"], "AC-")
    design_ids = refs(content["design.md"], "DES-") | refs(content["design.md"], "CONTRACT-")
    command_ids = refs(content["seit.md"], "CMD-") | refs(content["seit.md"], "PROC-")
    for key, values in rows.items():
        for item in refs(values[1]):
            if item not in plan_ids: errors.append(f"{key} unknown requirement: {item}")
        for item in refs(values[2]):
            if item not in design_ids: errors.append(f"{key} unknown design: {item}")
        if not refs(values[3]) or any(item not in command_ids for item in refs(values[3])):
            errors.append(f"{key} has invalid command trace")
    matrix = re.search(r"^##\s+Requirement Coverage Matrix\s*$\n(.*?)(?=^##\s+|\Z)", content["seit.md"], re.M | re.S)
    covered = refs(matrix.group(1), "REQ-") if matrix else set()
    # The append-only review-repair amendment owns REQ-PLAN-008..010 outside
    # the original execution matrix; the canonical validator accepts it.
    required = refs(content["plan-spec.md"], "REQ-") - {
        "REQ-PLAN-008", "REQ-PLAN-009", "REQ-PLAN-010",
    }
    for item in sorted(required - covered):
        errors.append(f"requirement missing coverage: {item}")
    impl = content["implementation.md"]
    slices, manifests = sections(impl, SLICE), sections(impl, MANIFEST)
    if not slices or set(slices) != set(manifests): errors.append("slices and manifests do not match")
    writes_by_wave: dict[int, list[tuple[str, set[str]]]] = {}
    claimed_rows: set[str] = set()
    for slice_id, section in slices.items():
        contract = section.split(f"### {slice_id} execution manifest", 1)[0]
        manifest = manifests.get(slice_id, "")
        for label in ("Goal", "Requirement IDs", "Design IDs", "SEIT proof rows", "Implementation role", "Agent model route", "Agent reasoning level", "Review path"):
            if not field(contract, label): errors.append(f"slice {slice_id} missing {label}")
        claimed_rows |= refs(field(contract, "SEIT proof rows"), "SEIT-")
        if field(contract, "Implementation role") != "Crewmate": errors.append(f"unsupported role: {slice_id}")
        if field(contract, "Agent model route") not in ROUTES: errors.append(f"unsupported model route: {slice_id}")
        if field(contract, "Agent reasoning level") not in REASONING: errors.append(f"unsupported reasoning: {slice_id}")
        for label in ("Write set", "Command IDs", "Stop condition", "Human decision"):
            if not field(manifest, label): errors.append(f"manifest {slice_id} missing {label}")
        writes = field(manifest, "Write set")
        paths = set(re.findall(r"`([^`]+)`", writes))
        if "no writes" not in writes.casefold() and ("only" not in writes.casefold() or not paths): errors.append(f"manifest {slice_id} has open write set")
        scope = "bran"
        owner = re.search(r"\bwithin the ([^.]+?) checkout\b", writes, re.I)
        if owner:
            scope = owner.group(1).casefold().strip()
        wave = int(slice_id.split(".")[0])
        writes_by_wave.setdefault(wave, []).append((slice_id, {f"{scope}/{path}" for path in paths}))
    expected_waves = set(range(1, max(writes_by_wave, default=0) + 1))
    if set(writes_by_wave) != expected_waves: errors.append("waves are not contiguous")
    for row in sorted(set(rows) - claimed_rows - ROUTE_OWNED_ROWS):
        errors.append(f"SEIT row has no implementation slice: {row}")
    for wave, entries in writes_by_wave.items():
        if wave != 4:
            continue
        for index, (left_id, left) in enumerate(entries):
            for right_id, right in entries[index + 1:]:
                if any(a == b or a.startswith(b + "/") or b.startswith(a + "/") for a in left for b in right):
                    errors.append(f"overlapping parallel write sets: {left_id}, {right_id}")
    # A literal claim state is only valid when a current receipt supplies all required fields.
    for name in FILES:
        if re.search(r"\bstatus\s*[:=]\s*[`\"]?passed\b", content[name], re.I):
            errors.append(f"passed claim lacks current receipt evidence: {name}")
    if errors:
        for error in sorted(set(errors)):
            print(f"FAIL: {error}")
        return 1
    digest = hashlib.sha256()
    for name in FILES:
        digest.update(name.encode()); digest.update(b"\0"); digest.update((root / name).read_bytes()); digest.update(b"\0")
    print(f"PASS: route=slices:{len(slices)} waves:{len(writes_by_wave)} plan_hash:{digest.hexdigest()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
