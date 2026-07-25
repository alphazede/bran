#!/usr/bin/env python3
"""Create and verify BRAN's deterministic public repository snapshot."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any


CONFIG_PATH = "public-export.json"
RECEIPT_PATH = ".bran-export.json"
ALLOWED_MODES = {"100644", "100755"}
CONFIG_KEYS = {
    "schema_version",
    "source_repository",
    "public_repository",
    "public_remote",
    "allowed_files",
    "allowed_roots",
    "excluded_files",
    "excluded_roots",
}
REQUIRED_EXCLUDED_FILES = {
    ".bran/settings.conf",
    ".closeout.json",
    "AGENTS.md",
    "CLAUDE.md",
    CONFIG_PATH,
}
REQUIRED_EXCLUDED_ROOTS = {
    ".bran/cache",
    ".bran/results",
    ".bran/sessions",
    "docs/integrations/proposals",
    "docs/plans",
    "docs/submissions",
}
FORBIDDEN_PUBLIC_ROOTS = (
    ".bran/cache",
    ".bran/caches",
    ".bran/index",
    ".bran/report",
    ".bran/reports",
    ".bran/results",
    ".bran/session",
    ".bran/sessions",
    ".bran/snapshot",
    ".bran/snapshots",
    ".bran/validator",
    "docs/integrations/proposals",
    "docs/plans",
    "docs/submissions",
)


class ExportError(RuntimeError):
    """A closed failure at the private-to-public repository boundary."""


@dataclass(frozen=True)
class GitBlob:
    path: str
    mode: str
    object_id: str
    data: bytes


@dataclass(frozen=True)
class ExportConfig:
    source_repository: str
    public_repository: str
    public_remote: str
    allowed_files: frozenset[str]
    allowed_roots: tuple[str, ...]
    excluded_files: frozenset[str]
    excluded_roots: tuple[str, ...]


def run_git(root: Path, arguments: list[str], *, text: bool = False) -> bytes | str:
    try:
        result = subprocess.run(
            ["git", "-C", str(root), *arguments],
            check=True,
            capture_output=True,
            text=text,
        )
    except FileNotFoundError as exc:
        raise ExportError("git is required") from exc
    except subprocess.CalledProcessError as exc:
        stderr = exc.stderr if text else exc.stderr.decode("utf-8", "replace")
        raise ExportError(f"git {' '.join(arguments)} failed: {stderr.strip()}") from exc
    return result.stdout


def safe_path(value: object, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ExportError(f"{label} must be a non-empty string")
    try:
        value.encode("utf-8", "strict")
    except UnicodeError as exc:
        raise ExportError(f"{label} must be valid UTF-8") from exc
    path = PurePosixPath(value)
    if (
        path.is_absolute()
        or value != path.as_posix()
        or any(part in {"", ".", ".."} for part in path.parts)
        or "\\" in value
        or any(ord(character) < 32 or ord(character) == 127 for character in value)
    ):
        raise ExportError(f"{label} is not a safe normalized repository path: {value!r}")
    return value


def path_list(document: dict[str, Any], key: str) -> tuple[str, ...]:
    value = document.get(key)
    if not isinstance(value, list):
        raise ExportError(f"{CONFIG_PATH} {key} must be an array")
    paths = tuple(safe_path(item, f"{CONFIG_PATH} {key}") for item in value)
    if list(paths) != sorted(set(paths)):
        raise ExportError(f"{CONFIG_PATH} {key} must be sorted and unique")
    return paths


def parse_config(data: bytes) -> ExportConfig:
    try:
        document = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ExportError(f"{CONFIG_PATH} is not valid UTF-8 JSON: {exc}") from exc
    if not isinstance(document, dict) or set(document) != CONFIG_KEYS:
        raise ExportError(f"{CONFIG_PATH} must contain exactly the supported keys")
    if document.get("schema_version") != 1:
        raise ExportError(f"{CONFIG_PATH} schema_version must be 1")
    identities: list[str] = []
    for key in ("source_repository", "public_repository", "public_remote"):
        value = document.get(key)
        if not isinstance(value, str) or not value or any(ord(char) < 32 for char in value):
            raise ExportError(f"{CONFIG_PATH} {key} must be a non-empty printable string")
        identities.append(value)
    allowed_files = frozenset(path_list(document, "allowed_files"))
    allowed_roots = path_list(document, "allowed_roots")
    excluded_files = frozenset(path_list(document, "excluded_files"))
    excluded_roots = path_list(document, "excluded_roots")
    if REQUIRED_EXCLUDED_FILES - excluded_files:
        missing = ", ".join(sorted(REQUIRED_EXCLUDED_FILES - excluded_files))
        raise ExportError(f"{CONFIG_PATH} is missing required private files: {missing}")
    if REQUIRED_EXCLUDED_ROOTS - set(excluded_roots):
        missing = ", ".join(sorted(REQUIRED_EXCLUDED_ROOTS - set(excluded_roots)))
        raise ExportError(f"{CONFIG_PATH} is missing required private roots: {missing}")
    return ExportConfig(
        source_repository=identities[0],
        public_repository=identities[1],
        public_remote=identities[2],
        allowed_files=allowed_files,
        allowed_roots=allowed_roots,
        excluded_files=excluded_files,
        excluded_roots=excluded_roots,
    )


def resolve_commit(root: Path, reference: str) -> str:
    output = run_git(root, ["rev-parse", "--verify", f"{reference}^{{commit}}"], text=True)
    commit = output.strip()
    if len(commit) != 40 or any(char not in "0123456789abcdef" for char in commit):
        raise ExportError(f"resolved source commit is not a lowercase 40-hex Git SHA: {commit!r}")
    return commit


def read_tree(root: Path, commit: str) -> dict[str, GitBlob]:
    output = run_git(root, ["ls-tree", "-rz", "--full-tree", commit])
    blobs: dict[str, GitBlob] = {}
    for record in output.split(b"\0"):
        if not record:
            continue
        try:
            metadata, raw_path = record.split(b"\t", 1)
            mode_raw, kind_raw, object_raw = metadata.split(b" ", 2)
            path = raw_path.decode("utf-8", "strict")
            mode = mode_raw.decode("ascii", "strict")
            kind = kind_raw.decode("ascii", "strict")
            object_id = object_raw.decode("ascii", "strict")
        except (ValueError, UnicodeError) as exc:
            raise ExportError("Git tree contains an undecodable entry") from exc
        safe_path(path, "Git tree path")
        if kind != "blob" or mode not in ALLOWED_MODES:
            raise ExportError(f"unsafe Git entry {path}: type={kind} mode={mode}")
        data = run_git(root, ["cat-file", "blob", object_id])
        blobs[path] = GitBlob(path=path, mode=mode, object_id=object_id, data=data)
    return blobs


def matches_root(path: str, roots: tuple[str, ...]) -> bool:
    return any(path == root or path.startswith(f"{root}/") for root in roots)


def forbid_selected_path(path: str) -> None:
    if PurePosixPath(path).name in {"AGENTS.md", "CLAUDE.md"}:
        raise ExportError(f"agent instruction file selected for public export: {path}")
    if path.startswith(".bran/") and path != ".bran/policy.yaml":
        raise ExportError(f"local BRAN runtime path selected for public export: {path}")
    if matches_root(path, FORBIDDEN_PUBLIC_ROOTS):
        raise ExportError(f"private root selected for public export: {path}")


def select_public(tree: dict[str, GitBlob], config: ExportConfig) -> dict[str, GitBlob]:
    selected: dict[str, GitBlob] = {}
    for path, blob in sorted(tree.items()):
        allowed = path in config.allowed_files or matches_root(path, config.allowed_roots)
        excluded = path in config.excluded_files or matches_root(path, config.excluded_roots)
        if allowed == excluded:
            state = "both public and private" if allowed else "unclassified"
            raise ExportError(f"tracked path is {state}: {path}")
        if allowed:
            forbid_selected_path(path)
            if path == RECEIPT_PATH:
                raise ExportError(f"source repository may not track generated receipt {RECEIPT_PATH}")
            selected[path] = blob
    if ".bran/policy.yaml" not in selected:
        raise ExportError("public export must include .bran/policy.yaml")
    return selected


def receipt_bytes(config: ExportConfig, commit: str, selected: dict[str, GitBlob]) -> bytes:
    receipt = {
        "schema_version": 1,
        "source_repository": config.source_repository,
        "source_commit": commit,
        "public_repository": config.public_repository,
        "files": [
            {
                "path": blob.path,
                "mode": blob.mode,
                "bytes": len(blob.data),
                "sha256": hashlib.sha256(blob.data).hexdigest(),
            }
            for blob in selected.values()
        ],
    }
    return (json.dumps(receipt, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


def build_export(root: Path, reference: str) -> tuple[str, ExportConfig, dict[str, GitBlob], bytes]:
    commit = resolve_commit(root, reference)
    tree = read_tree(root, commit)
    config_blob = tree.get(CONFIG_PATH)
    if config_blob is None or config_blob.mode != "100644":
        raise ExportError(f"source commit must contain regular {CONFIG_PATH}")
    config = parse_config(config_blob.data)
    selected = select_public(tree, config)
    return commit, config, selected, receipt_bytes(config, commit, selected)


def require_clean(root: Path, label: str) -> None:
    output = run_git(root, ["status", "--porcelain=v1", "-z", "--untracked-files=all"])
    if output:
        raise ExportError(f"{label} checkout must be clean")


def write_snapshot(root: Path, output: Path, reference: str) -> tuple[str, int]:
    require_clean(root, "source")
    commit, _, selected, receipt = build_export(root, reference)
    if output.exists():
        if not output.is_dir() or any(output.iterdir()):
            raise ExportError(f"snapshot output must be absent or empty: {output}")
    else:
        output.mkdir(parents=True)
    for path, blob in selected.items():
        destination = output / Path(path)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(blob.data)
        destination.chmod(0o755 if blob.mode == "100755" else 0o644)
    (output / RECEIPT_PATH).write_bytes(receipt)
    (output / RECEIPT_PATH).chmod(0o644)
    return commit, len(selected) + 1


def public_entries(root: Path, commit: str) -> dict[str, tuple[str, bytes]]:
    return {path: (blob.mode, blob.data) for path, blob in read_tree(root, commit).items()}


def check_public(source: Path, public: Path, reference: str) -> tuple[str, int]:
    require_clean(source, "source")
    require_clean(public, "public")
    commit, config, selected, receipt = build_export(source, reference)
    public_commit = resolve_commit(public, "HEAD")
    remote = run_git(public, ["remote", "get-url", "origin"], text=True).strip()
    if remote != config.public_remote:
        raise ExportError(f"public origin must be {config.public_remote}, got {remote or '<empty>'}")
    expected = {path: (blob.mode, blob.data) for path, blob in selected.items()}
    expected[RECEIPT_PATH] = ("100644", receipt)
    actual = public_entries(public, public_commit)
    errors: list[str] = []
    for path in sorted(set(expected) - set(actual)):
        errors.append(f"missing public path: {path}")
    for path in sorted(set(actual) - set(expected)):
        errors.append(f"unexpected public path: {path}")
    for path in sorted(set(expected) & set(actual)):
        expected_mode, expected_data = expected[path]
        actual_mode, actual_data = actual[path]
        if actual_mode != expected_mode:
            errors.append(f"mode drift: {path} expected {expected_mode} got {actual_mode}")
        if actual_data != expected_data:
            errors.append(f"content drift: {path}")
    if errors:
        raise ExportError("public drift detected:\n  " + "\n  ".join(errors))
    return commit, len(expected)


def expect_export_error(action: Any, label: str) -> None:
    try:
        action()
    except ExportError:
        return
    raise AssertionError(f"expected closed export failure: {label}")


def git_ok(root: Path, arguments: list[str]) -> None:
    run_git(root, arguments)


def write_text(path: Path, value: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(value, encoding="utf-8")


def commit_all(root: Path, message: str) -> None:
    git_ok(root, ["add", "-A"])
    git_ok(root, ["commit", "-m", message])


def contract_config() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "source_repository": "example/bran-dev",
        "public_repository": "example/bran",
        "public_remote": "https://example.invalid/example/bran.git",
        "allowed_files": [".bran/policy.yaml", "README.md"],
        "allowed_roots": ["product"],
        "excluded_files": [
            ".bran/settings.conf",
            ".closeout.json",
            "AGENTS.md",
            "CLAUDE.md",
            "public-export.json",
        ],
        "excluded_roots": [
            ".bran/cache",
            ".bran/results",
            ".bran/sessions",
            "docs/integrations/proposals",
            "docs/plans",
            "docs/submissions",
        ],
    }


def run_contract_check() -> None:
    with tempfile.TemporaryDirectory(prefix="bran-export-") as temporary:
        base = Path(temporary)
        source = base / "source"
        public = base / "public"
        source.mkdir()
        git_ok(source, ["init", "-b", "main"])
        git_ok(source, ["config", "user.name", "BRAN Export Check"])
        git_ok(source, ["config", "user.email", "bran-export@example.invalid"])
        write_text(source / "README.md", "public product\n")
        write_text(source / ".bran/policy.yaml", 'schema_version: "1"\n')
        write_text(source / "product/data.txt", "deterministic\n")
        write_text(source / "AGENTS.md", "private instructions\n")
        write_text(source / ".bran/results/run.json", "private runtime\n")
        write_text(source / "docs/integrations/proposals/draft.md", "unpublished\n")
        write_text(source / CONFIG_PATH, json.dumps(contract_config(), indent=2) + "\n")
        commit_all(source, "seed export contract")

        nonempty = base / "nonempty"
        nonempty.mkdir()
        write_text(nonempty / "keep", "occupied\n")
        expect_export_error(lambda: write_snapshot(source, nonempty, "HEAD"), "nonempty output")

        commit, count = write_snapshot(source, public, "HEAD")
        assert count == 4
        assert len(commit) == 40
        assert not (public / "AGENTS.md").exists()
        assert not (public / ".bran/results/run.json").exists()
        assert not (public / "docs/integrations/proposals/draft.md").exists()

        git_ok(public, ["init", "-b", "main"])
        git_ok(public, ["config", "user.name", "BRAN Export Check"])
        git_ok(public, ["config", "user.email", "bran-export@example.invalid"])
        git_ok(public, ["remote", "add", "origin", contract_config()["public_remote"]])
        commit_all(public, "public snapshot")
        check_public(source, public, "HEAD")

        write_text(public / "README.md", "dirty\n")
        expect_export_error(lambda: check_public(source, public, "HEAD"), "dirty public checkout")
        git_ok(public, ["checkout", "--", "README.md"])

        write_text(public / "README.md", "committed drift\n")
        commit_all(public, "change public content")
        expect_export_error(lambda: check_public(source, public, "HEAD"), "public content drift")
        write_text(public / "README.md", "public product\n")
        commit_all(public, "restore public content")

        (public / "product/data.txt").chmod(0o755)
        commit_all(public, "change public mode")
        expect_export_error(lambda: check_public(source, public, "HEAD"), "public mode drift")
        (public / "product/data.txt").chmod(0o644)
        commit_all(public, "restore public mode")

        write_text(public / "extra.txt", "not exported\n")
        commit_all(public, "add extra public path")
        expect_export_error(lambda: check_public(source, public, "HEAD"), "extra public path")
        git_ok(public, ["rm", "extra.txt"])
        commit_all(public, "remove extra public path")

        git_ok(public, ["remote", "set-url", "origin", "https://example.invalid/wrong.git"])
        expect_export_error(lambda: check_public(source, public, "HEAD"), "wrong public remote")
        git_ok(public, ["remote", "set-url", "origin", contract_config()["public_remote"]])

        git_ok(public, ["rm", "README.md"])
        commit_all(public, "remove public path")
        expect_export_error(lambda: check_public(source, public, "HEAD"), "missing public path")

        link = source / "product/link"
        os.symlink("data.txt", link)
        commit_all(source, "add unsafe link")
        expect_export_error(lambda: build_export(source, "HEAD"), "symbolic link")
        link.unlink()
        commit_all(source, "remove unsafe link")
        write_text(source / "unknown.txt", "unclassified\n")
        commit_all(source, "add unclassified path")
        expect_export_error(lambda: build_export(source, "HEAD"), "unclassified path")


def source_root() -> Path:
    return Path(__file__).resolve().parents[2]


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command")
    snapshot = commands.add_parser("snapshot", help="write a new public snapshot")
    snapshot.add_argument("--output", required=True, type=Path)
    snapshot.add_argument("--source-ref", default="HEAD")
    check = commands.add_parser("check", help="fail if a public checkout drifts")
    check.add_argument("--public-dir", required=True, type=Path)
    check.add_argument("--source-ref", default="HEAD")
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    try:
        if arguments.command is None:
            run_contract_check()
            print("PASS P1-PUBLIC-EXPORT: deterministic snapshot and drift failures verified")
        elif arguments.command == "snapshot":
            commit, count = write_snapshot(source_root(), arguments.output.resolve(), arguments.source_ref)
            print(f"PASS public snapshot: source_commit={commit} files={count} output={arguments.output.resolve()}")
        elif arguments.command == "check":
            commit, count = check_public(source_root(), arguments.public_dir.resolve(), arguments.source_ref)
            print(f"PASS public drift guard: source_commit={commit} files={count} public={arguments.public_dir.resolve()}")
        else:
            raise ExportError(f"unsupported command: {arguments.command}")
    except (ExportError, AssertionError, OSError) as exc:
        print(f"FAIL public export: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
