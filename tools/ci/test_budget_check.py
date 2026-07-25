#!/usr/bin/env python3
from __future__ import annotations
import ast, json, re, sys
from collections import Counter
from pathlib import Path
PHASE_ONE = ("P1-CLI", "P1-RELEASE", "P1-PUBLIC-BOUNDARY", "P1-PUBLIC-EXPORT",
             "P1-BUNDLE", "P1-PROFILES", "P1-CONFORMANCE")
REQUIRED_DIRECT_COMMANDS = {
    "P4-SEALED-RELEASE": ("run_release_seal", 'python3 "$script_dir/release_seal.py" --self-test', "full"),
}
FIELDS = {"stable_id", "phase", "slice", "runner", "name", "category",
          "classification", "source", "support_fixtures"}
RUNNERS = {"rust", "python", "shell", "golden", "fixture"}
RUST = re.compile(r"#\[test\]\s*(?:#\[[^\]]+\]\s*)*fn\s+([A-Za-z_]\w*)\s*\(")
PYTEST = re.compile(r"^def\s+(test_[A-Za-z_]\w*)\s*\(", re.M)
STABLE = re.compile(r"^P([1-6]|8)-([A-Za-z0-9]+(?:-[A-Za-z0-9]+)*)$")
SLUG = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
HIDDEN = re.compile(r"\b(?:self[-_ ]?test|probe|cases|case(?!-sensitive\b)|matrix|parameteri[sz]ed)\b", re.I)
IDENTIFIER = re.compile(r"[A-Za-z_]\w*")
PHASE_TWO_SCHEMA_BINDINGS = {
    "P2-METADATA": ("fixtures/source-metadata/metadata-report-v1.json",
                    "schemas/source-metadata-report.schema.json", ""),
    "P2-SCANNER": ("fixtures/scanner/scan-snapshot-v1.json",
                   "schemas/repository-scan-snapshot.schema.json", ""),
    "P2-GRAPH": ("fixtures/graph/knowledge-graph-v1.json",
                 "schemas/knowledge-graph.schema.json", ""),
    "P2-QUERIES": ("fixtures/queries/query-result-v1.json",
                   "schemas/graph-query-result.schema.json", ""),
    "P2-VIEW": ("fixtures/views/compiled-view-v1.json",
                "schemas/knowledge-view.schema.json", ""),
    "P2-PACKET-SQZ": ("fixtures/packets/context-packet-sqz-v1.json",
                      "schemas/context-packet-receipt.schema.json", "/packet_receipt"),
    "P2-PACKET-SQZ#sqz": ("fixtures/packets/context-packet-sqz-v1.json",
                          "schemas/sqz-receipt.schema.json", "/sqz_receipt"),
}
SCHEMA_BINDINGS = {
    **PHASE_TWO_SCHEMA_BINDINGS,
    "P8-SCHEMA-PARSERS": (
        "fixtures/graph/structural-graph-snapshot-v1.json",
        "schemas/structural-graph-snapshot.schema.json",
        "",
    ),
}
SCHEMA_KEYWORDS = {
    "$ref", "type", "properties", "required", "additionalProperties", "enum", "const",
    "minLength", "maxLength", "pattern", "minimum", "maximum", "minItems", "maxItems",
    "uniqueItems", "items", "oneOf", "allOf",
}
SCHEMA_ANNOTATIONS = {"$schema", "$id", "$defs", "title", "description"}


def pointer_part(value: object) -> str:
    return str(value).replace("~", "~0").replace("/", "~1")


def json_value(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def json_equal(left: object, right: object) -> bool:
    if isinstance(left, bool) or isinstance(right, bool):
        return type(left) is type(right) and left == right
    if (isinstance(left, (int, float)) and isinstance(right, (int, float))
            and not isinstance(left, bool) and not isinstance(right, bool)):
        return left == right
    return type(left) is type(right) and left == right


def resolve_pointer(document: object, pointer: str, label: str, errors: list[str]) -> object | None:
    if pointer == "":
        return document
    if not pointer.startswith("/"):
        errors.append(f"{label}: unsafe or non-local JSON pointer {pointer!r}")
        return None
    current = document
    for encoded in pointer[1:].split("/"):
        if "~" in re.sub(r"~[01]", "", encoded):
            errors.append(f"{label}: invalid JSON pointer escape in {pointer!r}")
            return None
        part = encoded.replace("~1", "/").replace("~0", "~")
        if isinstance(current, dict) and part in current:
            current = current[part]
        elif isinstance(current, list) and part.isascii() and part.isdigit() and int(part) < len(current):
            current = current[int(part)]
        else:
            errors.append(f"{label}: JSON pointer {pointer!r} does not resolve")
            return None
    return current


def audit_schema(schema: object, path: str, errors: list[str]) -> None:
    if not isinstance(schema, dict):
        errors.append(f"{path}: schema must be an object")
        return
    for key in schema:
        if key not in SCHEMA_KEYWORDS and key not in SCHEMA_ANNOTATIONS and not key.startswith("x-"):
            errors.append(f"{path}: unsupported schema keyword {key!r}")
    if "$ref" in schema and (not isinstance(schema["$ref"], str)
                              or not schema["$ref"].startswith("#/$defs/")):
        errors.append(f"{path}/$ref: only local #/$defs references are supported")
    if "type" in schema and schema["type"] not in {
            "object", "array", "string", "integer", "number", "boolean", "null"}:
        errors.append(f"{path}/type: unsupported JSON Schema type")
    if "required" in schema and (not isinstance(schema["required"], list)
                                  or not all(isinstance(item, str) for item in schema["required"])):
        errors.append(f"{path}/required: must be an array of strings")
    if "pattern" in schema:
        try:
            re.compile(schema["pattern"] if isinstance(schema["pattern"], str) else None)
        except (re.error, TypeError) as error:
            errors.append(f"{path}/pattern: invalid regular expression: {error}")
    for key in ("minLength", "maxLength", "minItems", "maxItems"):
        if key in schema and (type(schema[key]) is not int or schema[key] < 0):
            errors.append(f"{path}/{key}: must be a non-negative integer")
    for key in ("minimum", "maximum"):
        if key in schema and (not isinstance(schema[key], (int, float))
                              or isinstance(schema[key], bool)):
            errors.append(f"{path}/{key}: must be a number")
    if "uniqueItems" in schema and type(schema["uniqueItems"]) is not bool:
        errors.append(f"{path}/uniqueItems: must be boolean")
    if "additionalProperties" in schema and not isinstance(schema["additionalProperties"], (bool, dict)):
        errors.append(f"{path}/additionalProperties: must be boolean or a schema")
    if "$defs" in schema:
        if not isinstance(schema["$defs"], dict):
            errors.append(f"{path}/$defs: must be an object")
        else:
            for name, child in schema["$defs"].items():
                audit_schema(child, f"{path}/$defs/{pointer_part(name)}", errors)
    if "properties" in schema:
        if not isinstance(schema["properties"], dict):
            errors.append(f"{path}/properties: must be an object")
        else:
            for name, child in schema["properties"].items():
                audit_schema(child, f"{path}/properties/{pointer_part(name)}", errors)
    for key in ("items", "additionalProperties"):
        if isinstance(schema.get(key), dict):
            audit_schema(schema[key], f"{path}/{key}", errors)
    for key in ("oneOf", "allOf"):
        if key in schema and (not isinstance(schema[key], list) or not schema[key]):
            errors.append(f"{path}/{key}: must be a non-empty array")
        elif key in schema:
            for index, child in enumerate(schema[key]):
                audit_schema(child, f"{path}/{key}/{index}", errors)


def filter_shape(value: object) -> tuple[int, int]:
    maximum_depth, clauses = 0, 0
    pending = [(value, 1)]
    while pending:
        current, depth = pending.pop()
        if not isinstance(current, dict):
            continue
        maximum_depth = max(maximum_depth, depth)
        clauses += 1
        nested = current.get("clauses")
        if isinstance(nested, list):
            pending.extend((item, depth + 1) for item in reversed(nested))
        if "clause" in current:
            pending.append((current["clause"], depth + 1))
    return maximum_depth, clauses


def instance_type(value: object, expected: str) -> bool:
    return {
        "object": isinstance(value, dict),
        "array": isinstance(value, list),
        "string": isinstance(value, str),
        "integer": type(value) is int,
        "number": isinstance(value, (int, float)) and not isinstance(value, bool),
        "boolean": type(value) is bool,
        "null": value is None,
    }[expected]


def validate_instance(instance: object, schema: dict, document: dict,
                      path: str = "", depth: int = 0) -> list[str]:
    errors: list[str] = []
    shown = path or "/"
    if depth > 256:
        return [f"{shown}: validation nesting exceeds 256"]
    if "$ref" in schema:
        reference = schema["$ref"]
        target = resolve_pointer(document, reference[1:] if isinstance(reference, str)
                                 and reference.startswith("#") else str(reference),
                                 f"{shown}/$ref", errors)
        if not isinstance(target, dict):
            if target is not None:
                errors.append(f"{shown}/$ref: resolved value is not a schema")
            return errors
        errors.extend(validate_instance(instance, target, document, path, depth + 1))
    maximum_depth = schema.get("x-max-depth")
    maximum_clauses = schema.get("x-max-total-clauses")
    if maximum_depth is not None or maximum_clauses is not None:
        actual_depth, actual_clauses = filter_shape(instance)
        if type(maximum_depth) is not int or maximum_depth < 0:
            errors.append(f"{shown}: x-max-depth must be a non-negative integer")
        elif actual_depth > maximum_depth:
            errors.append(f"{shown}: filter depth {actual_depth} exceeds x-max-depth {maximum_depth}")
        if type(maximum_clauses) is not int or maximum_clauses < 0:
            errors.append(f"{shown}: x-max-total-clauses must be a non-negative integer")
        elif actual_clauses > maximum_clauses:
            errors.append(f"{shown}: filter clause count {actual_clauses} exceeds x-max-total-clauses {maximum_clauses}")
        if errors:
            return errors
    expected = schema.get("type")
    if isinstance(expected, str) and not instance_type(instance, expected):
        return [f"{shown}: expected {expected}, got {type(instance).__name__}"]
    if "const" in schema and not json_equal(instance, schema["const"]):
        errors.append(f"{shown}: value {json_value(instance)} does not equal const {json_value(schema['const'])}")
    if "enum" in schema:
        enum = schema["enum"]
        if not isinstance(enum, list) or not any(json_equal(instance, choice) for choice in enum):
            errors.append(f"{shown}: value {json_value(instance)} is not in enum")
    if isinstance(instance, dict):
        properties = schema.get("properties", {})
        required = schema.get("required", [])
        if isinstance(required, list):
            for name in required:
                if name not in instance:
                    errors.append(f"{shown}: missing required property {name!r}")
        if isinstance(properties, dict):
            for name, value in instance.items():
                child_path = f"{path}/{pointer_part(name)}"
                if name in properties and isinstance(properties[name], dict):
                    errors.extend(validate_instance(value, properties[name], document,
                                                    child_path, depth + 1))
                elif schema.get("additionalProperties") is False:
                    errors.append(f"{child_path}: additional property is not allowed")
                elif isinstance(schema.get("additionalProperties"), dict):
                    errors.extend(validate_instance(value, schema["additionalProperties"],
                                                    document, child_path, depth + 1))
    if isinstance(instance, str):
        if isinstance(schema.get("minLength"), int) and len(instance) < schema["minLength"]:
            errors.append(f"{shown}: string length {len(instance)} is below minLength {schema['minLength']}")
        if isinstance(schema.get("maxLength"), int) and len(instance) > schema["maxLength"]:
            errors.append(f"{shown}: string length {len(instance)} exceeds maxLength {schema['maxLength']}")
        if isinstance(schema.get("pattern"), str) and re.search(schema["pattern"], instance) is None:
            errors.append(f"{shown}: string does not match pattern {schema['pattern']!r}")
    if isinstance(instance, (int, float)) and not isinstance(instance, bool):
        if isinstance(schema.get("minimum"), (int, float)) and instance < schema["minimum"]:
            errors.append(f"{shown}: value {instance} is below minimum {schema['minimum']}")
        if isinstance(schema.get("maximum"), (int, float)) and instance > schema["maximum"]:
            errors.append(f"{shown}: value {instance} exceeds maximum {schema['maximum']}")
    if isinstance(instance, list):
        if isinstance(schema.get("minItems"), int) and len(instance) < schema["minItems"]:
            errors.append(f"{shown}: item count {len(instance)} is below minItems {schema['minItems']}")
        if isinstance(schema.get("maxItems"), int) and len(instance) > schema["maxItems"]:
            errors.append(f"{shown}: item count {len(instance)} exceeds maxItems {schema['maxItems']}")
        if schema.get("uniqueItems") is True:
            seen: set[str] = set()
            for index, item in enumerate(instance):
                canonical = json_value(item)
                if canonical in seen:
                    errors.append(f"{path}/{index}: duplicate item violates uniqueItems")
                seen.add(canonical)
        if isinstance(schema.get("items"), dict):
            for index, item in enumerate(instance):
                errors.extend(validate_instance(item, schema["items"], document,
                                                f"{path}/{index}", depth + 1))
    if isinstance(schema.get("allOf"), list):
        for branch in schema["allOf"]:
            if isinstance(branch, dict):
                errors.extend(validate_instance(instance, branch, document, path, depth + 1))
    if isinstance(schema.get("oneOf"), list):
        matches = sum(not validate_instance(instance, branch, document, path, depth + 1)
                      for branch in schema["oneOf"] if isinstance(branch, dict))
        if matches != 1:
            errors.append(f"{shown}: oneOf must match exactly one branch; matched {matches}")
    return errors
def fail(errors: list[str]) -> int:
    for error in errors:
        print(f"FAIL test budget: {error}")
    return 1
def text(path: Path, errors: list[str]) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        errors.append(f"cannot read {path}: {error}")
        return ""
def body(shell: str, name: str) -> str | None:
    match = re.search(rf"^{re.escape(name)}\(\) \{{\n(.*?)^\}}$", shell, re.M | re.S)
    return match.group(1) if match else None
def suspicious_identifier(value: str) -> bool:
    parts = value.lower().split("_")
    return (any(part in {"case", "cases", "matrix", "matrices", "probe", "probes",
                         "parameterized", "parameterised", "selftest", "selftests"}
                for part in parts)
            or any(parts[index:index + 2] in (["self", "test"], ["self", "tests"])
                   for index in range(len(parts) - 1)))
def rust_body(source: str, name: str) -> str | None:
    match = re.search(rf"#\[test\]\s*(?:#\[[^\]]+\]\s*)*fn\s+{re.escape(name)}\s*\([^)]*\)\s*\{{", source)
    if match is None:
        return None
    opening = match.end() - 1
    depth, quote, escaped, line_comment, block_comment = 0, None, False, False, 0
    index = opening
    while index < len(source):
        char = source[index]
        following = source[index:index + 2]
        if line_comment:
            line_comment = char != "\n"
        elif block_comment:
            if following == "/*": block_comment += 1; index += 1
            elif following == "*/": block_comment -= 1; index += 1
        elif quote:
            if escaped: escaped = False
            elif char == "\\": escaped = True
            elif char == quote: quote = None
        elif following == "//": line_comment = True; index += 1
        elif following == "/*": block_comment = 1; index += 1
        elif char in {'"', "'"}: quote = char
        elif char == "{": depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0: return source[opening + 1:index]
        index += 1
    return None
def hidden_source(runner: str, name: str, source: str, direct: bool) -> bool:
    if runner == "python":
        try:
            tree = ast.parse(source)
        except SyntaxError:
            return True
        if not direct:
            matches = [node for node in tree.body
                       if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
                       and node.name == name]
            if len(matches) != 1:
                return True
            tree = matches[0]
        identifiers: list[str] = []
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                identifiers.append(node.name)
            elif isinstance(node, ast.Name):
                identifiers.append(node.id)
            elif isinstance(node, ast.arg):
                identifiers.append(node.arg)
            elif isinstance(node, ast.Attribute):
                identifiers.append(node.attr)
        return any(suspicious_identifier(item) for item in identifiers)
    if runner == "shell":
        scoped = source if direct else body(source, name)
        if scoped is None:
            return True
        identifiers = re.findall(r"^\s*([A-Za-z_]\w*)\(\)|^\s*([A-Za-z_]\w*)=|\bfor\s+([A-Za-z_]\w*)\s+in|\$\{?([A-Za-z_]\w*)", scoped, re.M)
        return any(suspicious_identifier(item) for group in identifiers for item in group if item)
    return False
def local(root: Path, value: object) -> Path | None:
    if not isinstance(value, str) or not value or Path(value).is_absolute() or ".." in Path(value).parts:
        return None
    path = root / value
    return path if path.is_file() else None
def check_shell(root: Path, journeys: list[dict], errors: list[str]) -> None:
    shell = text(root / "tools/ci/check.sh", errors)
    functions = {match.group(1): match.group(2) for match in
                 re.finditer(r"^([A-Za-z_]\w*)\(\) \{\n(.*?)^\}$", shell, re.M | re.S)}
    executable = lambda value: [line.strip() for line in value.splitlines()
                                if line.strip() and not line.lstrip().startswith("#")]
    calls = {name: {line for line in executable(part) if line in functions}
             for name, part in functions.items()}
    dispatch_match = re.search(r'^case "\$gate_mode" in\n(.*?)^esac$', shell, re.M | re.S)
    dispatch: dict[str, set[str]] = {}
    if dispatch_match:
        for match in re.finditer(r"^\s*([a-z-]+)\)\n(.*?)(?=^\s*[a-z-]+\)|\Z)",
                                 dispatch_match.group(1), re.M | re.S):
            dispatch[match.group(1)] = {line for line in executable(match.group(2))
                                        if line in functions}
    expected_dispatch = {"test-budget": {"run_budget"}, "fast": {"run_fast"},
                         "conformance": {"run_conformance"},
                         "security": {"run_security"},
                         "performance": {"run_performance"},
                         "full": {"run_fast", "run_conformance", "run_security",
                                  "run_performance", "run_release_seal"}}
    if dispatch != expected_dispatch:
        errors.append("check.sh supported gate dispatch is missing or not exact")
    def reachable(roots: set[str]) -> set[str]:
        found, pending = set(), list(roots)
        while pending:
            current = pending.pop()
            if current in found or current not in functions: continue
            found.add(current); pending.extend(calls[current] - found)
        return found
    reachable_from_modes = reachable(set().union(*dispatch.values()) if dispatch else set())
    budget = 'python3 "$script_dir/test_budget_check.py" "$script_dir/test-budget.json"'
    global_commands = [line for line in executable(shell)
                       if re.match(r"^(?:python3|sh|bash)\s+", line)]
    all_commands = [(name, line) for name, part in functions.items()
                    for line in executable(part) if re.match(r"^(?:python3|sh|bash)\s+", line)]
    budget_sites = [(name, line) for name, line in all_commands if line == budget]
    if global_commands.count(budget) != 1 or budget_sites != [("run_budget", budget)]:
        errors.append("check.sh must execute the budget checker exactly once in run_budget")
    for name in ("run_fast", "run_conformance", "run_security", "run_performance"):
        if "run_budget" not in reachable({name}):
            errors.append(f"check.sh {name} must reach run_budget")
    expected = {budget}
    for journey in journeys:
        stable_id, runner, name, source = (journey[key] for key in ("stable_id", "runner", "name", "source"))
        if stable_id in REQUIRED_DIRECT_COMMANDS:
            owner, command, mode = REQUIRED_DIRECT_COMMANDS[stable_id]
            sites = [(site_owner, line) for site_owner, line in all_commands if line == command]
            allowed = reachable(dispatch.get(mode, set()))
            other_modes = set().union(*(roots for key, roots in dispatch.items() if key != mode))
            if (global_commands.count(command) != 1 or sites != [(owner, command)]
                    or owner not in allowed or owner in reachable(other_modes)):
                errors.append(f"check.sh must execute {stable_id} exactly once from only the {mode} gate")
            expected.add(command)
            continue
        if isinstance(runner, str) and runner in {"python", "shell"} and name == source:
            commands = ({f'python3 "$bran_root/{source}"'} if runner == "python"
                        else {f'sh "$bran_root/{source}"', f'bash "$bran_root/{source}"'})
            sites = [(owner, line) for owner, line in all_commands if line in commands]
            if sum(global_commands.count(command) for command in commands) != 1 or len(sites) != 1 or sites[0][0] not in reachable_from_modes:
                errors.append(f"check.sh must execute {source} exactly once from a supported gate mode")
            expected.update(commands)
    for line in global_commands:
        if line not in expected:
            errors.append(f"unregistered direct Python/shell journey or variant: {line}")
def check_fixtures(root: Path, owned: list[str], errors: list[str]) -> None:
    actual = {path.relative_to(root).as_posix() for path in (root / "fixtures").rglob("*") if path.is_file()}
    counts = Counter(owned)
    for fixture in sorted(actual ^ set(owned)):
        errors.append(f"{'unowned' if fixture in actual else 'missing'} fixture file: {fixture}")
    for fixture, count in sorted(counts.items()):
        if count != 1:
            errors.append(f"support fixture must have exactly one owner: {fixture}")
    for fixture in sorted(actual):
        if HIDDEN.search(fixture) or HIDDEN.search(text(root / fixture, errors)):
            errors.append(f"suspicious hidden test artifact: {fixture}")


def strict_json(path: Path, label: str, errors: list[str]) -> object | None:
    def object_without_duplicates(pairs: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate object key {key!r}")
            result[key] = value
        return result
    try:
        return json.loads(path.read_text(encoding="utf-8"),
                          object_pairs_hook=object_without_duplicates)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
        errors.append(f"{label}: invalid JSON: {error}")
        return None


def check_schema_bindings(root: Path, journeys: list[dict], errors: list[str]) -> None:
    phase_two = [item for item in journeys if item.get("phase") == 2]
    by_id = {item.get("stable_id"): item for item in phase_two
             if isinstance(item.get("stable_id"), str)}
    expected_ids = {key.split("#", 1)[0] for key in PHASE_TWO_SCHEMA_BINDINGS}
    actual_ids = set(by_id)
    for stable_id in sorted(expected_ids - actual_ids):
        errors.append(f"Phase 2 schema inventory is missing journey {stable_id}")
    for stable_id in sorted(actual_ids - expected_ids):
        errors.append(f"Phase 2 schema inventory has unmapped journey {stable_id}")
    actual_support = {item for journey in phase_two
                      for item in journey.get("support_fixtures", [])
                      if isinstance(item, str)}
    mapped_support = {fixture for fixture, _schema, _pointer in PHASE_TWO_SCHEMA_BINDINGS.values()}
    for fixture in sorted(actual_support ^ mapped_support):
        errors.append(f"Phase 2 fixture/schema mapping is not exact: {fixture}")
    all_by_id = {item.get("stable_id"): item for item in journeys
                 if isinstance(item.get("stable_id"), str)}
    expected_support = {}
    for mapping_id, (fixture_path, _schema_path, _pointer) in SCHEMA_BINDINGS.items():
        expected_support.setdefault(mapping_id.split("#", 1)[0], []).append(fixture_path)
    for stable_id, expected in sorted(expected_support.items()):
        journey = all_by_id.get(stable_id)
        if journey is None:
            errors.append(f"schema binding {stable_id} is missing bound journey")
        elif journey.get("support_fixtures") != sorted(set(expected)):
            errors.append(f"{stable_id} must own exactly its declared schema fixture: "
                          f"{', '.join(sorted(set(expected)))}")
    loaded_fixtures: dict[str, object | None] = {}
    for mapping_id, (fixture_path, schema_path, instance_pointer) in sorted(SCHEMA_BINDINGS.items()):
        label = f"{mapping_id} {fixture_path}{instance_pointer}"
        schema = strict_json(root / schema_path, f"{mapping_id} {schema_path}", errors)
        if schema is None:
            continue
        if not isinstance(schema, dict):
            errors.append(f"{mapping_id} {schema_path}: schema root must be an object")
            continue
        schema_errors: list[str] = []
        audit_schema(schema, f"{mapping_id} {schema_path}#", schema_errors)
        errors.extend(schema_errors)
        if schema_errors:
            continue
        if fixture_path not in loaded_fixtures:
            loaded_fixtures[fixture_path] = strict_json(root / fixture_path, fixture_path, errors)
        fixture = loaded_fixtures[fixture_path]
        if fixture is None:
            continue
        pointer_errors: list[str] = []
        instance = resolve_pointer(fixture, instance_pointer, label, pointer_errors)
        errors.extend(pointer_errors)
        if pointer_errors:
            continue
        version_schema = schema.get("properties", {}).get("schema_version")
        expected_version = version_schema.get("const") if isinstance(version_schema, dict) else None
        if not isinstance(expected_version, str) and type(expected_version) is not int:
            errors.append(f"{mapping_id} {schema_path}: schema_version must have a string or integer const")
            continue
        actual_version = instance.get("schema_version") if isinstance(instance, dict) else None
        if not json_equal(actual_version, expected_version):
            errors.append(f"{label}/schema_version: expected declared schema version "
                          f"{expected_version!r}, got {actual_version!r}")
        for validation_error in validate_instance(instance, schema, schema):
            errors.append(f"{label}{validation_error}")


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: test_budget_check.py PATH/TO/test-budget.json", file=sys.stderr)
        return 2
    errors: list[str] = []
    try:
        manifest = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        return fail([f"invalid manifest: {error}"])
    root = Path(__file__).resolve().parents[2]
    if not isinstance(manifest, dict):
        return fail(["manifest root must be an object"])
    if set(manifest) != {"schema_version", "journeys"}:
        errors.append("manifest must contain exactly schema_version and journeys")
    if manifest.get("schema_version") != 1:
        errors.append("manifest schema_version must be 1")
    journeys = manifest.get("journeys")
    if not isinstance(journeys, list):
        return fail(errors + ["journeys must be an ordered array"])
    ids, names = [], []
    rust, fixtures = Counter(), []
    phases = Counter()
    valid: list[dict] = []
    for index, journey in enumerate(journeys):
        label = f"journeys[{index}]"
        if not isinstance(journey, dict) or set(journey) != FIELDS:
            errors.append(f"{label} must contain exactly the required inventory fields")
            continue
        valid.append(journey)
        stable_id, phase, slice_name = (journey[key] for key in ("stable_id", "phase", "slice"))
        runner, name, source = (journey[key] for key in ("runner", "name", "source"))
        support = journey["support_fixtures"]
        if not isinstance(stable_id, str) or not (match := STABLE.fullmatch(stable_id)) or type(phase) is not int or match.group(1) != str(phase):
            errors.append(f"{label}.stable_id must be a P{{phase}}-prefixed deterministic slug")
        else:
            ids.append(stable_id)
        if not isinstance(name, str) or HIDDEN.search(name):
            errors.append(f"{label}.name must be an exact non-hidden name")
        else:
            names.append(name)
        if type(phase) is not int or phase not in {1, 2, 3, 4, 5, 6, 8}:
            errors.append(f"{label}.phase is unknown")
        else:
            phases[phase] += 1
        if not isinstance(slice_name, str) or not re.fullmatch(rf"{phase}\.[1-9]\d*", slice_name):
            errors.append(f"{label}.slice is unknown or belongs to another phase")
        if not isinstance(runner, str) or runner not in RUNNERS or not isinstance(journey["category"], str) or not SLUG.fullmatch(journey["category"]) or not isinstance(journey["classification"], str) or journey["classification"] not in {"new", "materially-expanded"}:
            errors.append(f"{label} has an unknown runner, category, or classification")
        source_path = local(root, source)
        if source_path is None:
            errors.append(f"{label}.source is missing or not a file")
        if not isinstance(support, list) or not all(isinstance(item, str) and item.startswith("fixtures/") and local(root, item) for item in support):
            errors.append(f"{label}.support_fixtures must be existing fixture paths")
        else:
            fixtures.extend(support)
        content = text(source_path, errors) if source_path else ""
        if runner == "rust" and isinstance(name, str):
            rust[name] += 1
            if RUST.findall(content).count(name) != 1: errors.append(f"{label} Rust name must resolve to exactly one #[test] in source")
        if runner == "python" and isinstance(name, str) and name != source and name not in PYTEST.findall(content):
            errors.append(f"{label} Python name must be a direct script or exact def test_* item")
        if runner == "shell" and isinstance(name, str) and name != source and body(content, name) is None:
            errors.append(f"{label} shell name is not an exact function")
        if isinstance(runner, str) and isinstance(name, str) and hidden_source(runner, name, content, name == source):
            errors.append(f"{label} source contains a suspicious hidden test multiplier")
        if isinstance(runner, str) and runner in {"golden", "fixture"} and (name != source or not isinstance(source, str) or not source.startswith("fixtures/") or source not in support):
            errors.append(f"{label} {runner} name/source must be its owned fixture")
    if [item.get("stable_id") for item in valid if item.get("phase") == 1] != list(PHASE_ONE):
        errors.append("Phase 1 must contain exactly the seven approved stable IDs in deterministic order")
    for values, what in ((ids, "stable IDs"), (names, "exact names")):
        if duplicates := sorted(item for item, count in Counter(values).items() if count > 1): errors.append(f"duplicate {what}: {', '.join(duplicates)}")
    later = [(item["phase"], item["slice"], item["stable_id"]) for item in valid if type(item["phase"]) is int and item["phase"] > 1]
    if later != sorted(later, key=lambda item: (item[0], item[1], item[2])): errors.append("later journeys must preserve deterministic phase, slice, and stable ID order")
    for name, count in sorted(rust.items()):
        if count != 1: errors.append(f"registered Rust journey must have exactly one owner: {name}")
    check_shell(root, valid, errors)
    check_fixtures(root, fixtures, errors)
    check_schema_bindings(root, valid, errors)
    if errors: return fail(errors)
    print(f"PASS test budget: Phase 1={phases[1]} Phase 2={phases[2]} journeys={len(journeys)}")
    return 0
if __name__ == "__main__":
    sys.exit(main())
