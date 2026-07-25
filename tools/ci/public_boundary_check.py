#!/usr/bin/env python3
"""Verify that synthetic public-boundary canaries stay in their test fixture."""

from __future__ import annotations

import hashlib
import os
import stat
import subprocess
import sys
from pathlib import Path


MAX_TEXT_FILE_BYTES = 1024 * 1024
REJECTED_FIXTURE_RELATIVE = Path("fixtures/public-boundary/rejected/synthetic-canaries.txt")
PUBLIC_SAFE_FIXTURE_RELATIVE = Path("fixtures/public-boundary/public-safe/neutral-product-text.txt")
ALLOWED_BINARIES = {
    Path("assets/brand/bran-repository-raven.png"): (
        1974398,
        "4f6f1c4a4d82b6ef661d80602831c562c0d824daa36c009394c40dcdef1b4a2d",
    ),
}

# Keep the complete canaries out of this source file so the checker (which is itself
# scanned) does not mask an accidental literal copy here.
CANARIES = (
    "AZ_SYNTHETIC_" + "PUBLIC_" + "BOUNDARY_" + "CANARY_" + "ALPHA_7F3A",
    "AZ_SYNTHETIC_" + "PUBLIC_" + "BOUNDARY_" + "CANARY_" + "OMEGA_9C2D",
)


def bran_root() -> Path:
    return Path(__file__).resolve().parents[2]


def enumerate_phase1_public_surface_files(root: Path) -> list[Path]:
    """Enumerate every tracked or untracked/nonignored repository file.
    Uses exactly one fail-closed git ls-files call from repository root. Parses
    NUL-delimited bytes losslessly with os.fsdecode.
    Returned paths are guaranteed to be lexically under repo root (reject absolute
    or parent traversal paths). Raises on any failure.
    """
    try:
        result = subprocess.run(
            ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
            cwd=root,
            capture_output=True,
            check=True,
        )
        paths: list[Path] = []
        for entry in result.stdout.split(b"\0"):
            if not entry:
                continue
            rel_str = os.fsdecode(entry)
            rel = Path(rel_str)
            if rel.is_absolute() or any(part == ".." for part in rel.parts):
                raise RuntimeError(f"git ls-files produced escaping path (absolute/parent traversal): {rel_str}")
            candidate = root / rel
            try:
                candidate.relative_to(root)
            except ValueError:
                raise RuntimeError(f"git ls-files produced path outside repository: {rel_str}")
            paths.append(candidate)
        paths.sort(key=lambda p: str(p))
        return paths
    except subprocess.CalledProcessError as exc:
        if isinstance(exc.stderr, (bytes, bytearray)):
            stderr = exc.stderr.decode("utf-8", "replace").strip()
        else:
            stderr = str(exc.stderr or "").strip()
        raise RuntimeError(f"git ls-files failed (exit {exc.returncode}): {stderr}") from exc
    except FileNotFoundError as exc:
        raise RuntimeError("git command not found") from exc
    except Exception as exc:
        raise RuntimeError(f"enumeration failed: {exc}") from exc


def get_text_or_fail(path: Path) -> str:
    """Return bounded valid UTF-8 text, or raise ValueError describing closed failure.
    Fails closed (no silent skip) for: missing/non-regular/symlink, no read permission bits,
    over 1 MiB, read error, NUL/binary, invalid UTF-8.
    """
    try:
        st = path.lstat()
    except FileNotFoundError:
        raise ValueError("missing")
    except OSError as e:
        raise ValueError(f"unreadable ({e})")
    if stat.S_ISLNK(st.st_mode):
        raise ValueError("symlink")
    if not stat.S_ISREG(st.st_mode):
        raise ValueError("non-regular")
    if (st.st_mode & 0o444) == 0:
        raise ValueError("no read permission bits")
    if st.st_size > MAX_TEXT_FILE_BYTES:
        raise ValueError("over 1 MiB")
    try:
        data = path.read_bytes()
    except OSError as e:
        raise ValueError(f"read error: {e}")
    if b"\0" in data:
        raise ValueError("NUL/binary")
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        raise ValueError("invalid UTF-8")


def validate_allowed_binary(path: Path, expected: tuple[int, str]) -> None:
    """Fail closed unless an allowed binary has its authorized exact content."""
    try:
        st = path.lstat()
    except FileNotFoundError:
        raise ValueError("missing")
    except OSError as exc:
        raise ValueError(f"unreadable ({exc})")
    if stat.S_ISLNK(st.st_mode):
        raise ValueError("symlink")
    if not stat.S_ISREG(st.st_mode):
        raise ValueError("non-regular")
    size, expected_digest = expected
    if st.st_size != size:
        raise ValueError(f"unexpected size {st.st_size}")
    try:
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as exc:
        raise ValueError(f"read error: {exc}")
    if digest != expected_digest:
        raise ValueError("unexpected SHA-256")


def main() -> int:
    if len(sys.argv) > 1:
        print("usage: public_boundary_check.py", file=sys.stderr)
        print("FAIL unknown argument(s)")
        return 1

    try:
        repo = bran_root()
    except Exception as exc:
        print(f"FAIL failed to determine repo root: {exc}")
        return 1

    rejected_fixture = repo / REJECTED_FIXTURE_RELATIVE
    public_safe_fixture = repo / PUBLIC_SAFE_FIXTURE_RELATIVE

    try:
        enumerated = enumerate_phase1_public_surface_files(repo)
    except Exception as exc:
        print(f"FAIL failed to enumerate Phase 1 public surface files with git: {exc}")
        return 1

    # Validate public-safe fixture is clean (must be readable valid text)
    try:
        safe_text = get_text_or_fail(public_safe_fixture)
    except ValueError as e:
        print(f"FAIL public-safe fixture is not readable text: {public_safe_fixture} ({e})")
        return 1
    safe_matches = tuple(canary for canary in CANARIES if canary in safe_text)
    if safe_matches:
        print("FAIL public-safe fixture contains synthetic boundary canary")
        return 1

    # Validate rejected fixture contains both canaries (must be readable valid text)
    try:
        rej_text = get_text_or_fail(rejected_fixture)
    except ValueError as e:
        print(f"FAIL rejected fixture is not readable text: {rejected_fixture} ({e})")
        return 1
    rej_matches = tuple(canary for canary in CANARIES if canary in rej_text)
    if rej_matches != CANARIES:
        print("FAIL rejected test-only fixture does not contain every synthetic boundary canary")
        return 1

    # Scan every enumerated file except the single explicitly skipped rejected fixture.
    # All others must be valid; any closed-fail condition causes FAIL (no silent skips).
    # The checker itself is scanned.
    # Skip uses exact lexical repository-relative identity only (no resolve()).
    # This preserves fail-closed symlink rejection: a symlink alias cannot match lexical.
    scanned = 0
    allowed_skips = 0
    allowed_binaries = 0
    violations: list[tuple[Path, tuple[str, ...]]] = []
    for path in enumerated:
        try:
            rel = path.relative_to(repo)
        except ValueError:
            print(f"FAIL enumerated path not lexically under repo: {path}")
            return 1
        if rel == REJECTED_FIXTURE_RELATIVE:
            # explicit, observable skip of ONLY the designated fixture via lexical identity
            allowed_skips += 1
            continue
        if rel in ALLOWED_BINARIES:
            try:
                validate_allowed_binary(path, ALLOWED_BINARIES[rel])
            except ValueError as exc:
                print(f"FAIL {rel}: authorized binary {exc}")
                return 1
            allowed_binaries += 1
            continue
        try:
            text = get_text_or_fail(path)
        except ValueError as exc:
            try:
                rel = path.relative_to(repo)
            except ValueError:
                rel = path
            print(f"FAIL {rel}: {exc}")
            return 1
        matches = tuple(canary for canary in CANARIES if canary in text)
        scanned += 1
        if matches:
            violations.append((path, matches))

    if violations:
        print("FAIL synthetic public-boundary canary found outside rejected test fixture")
        for path, matches in violations:
            try:
                rel = path.relative_to(repo)
            except ValueError:
                rel = path
            print(f"  {rel}: {', '.join(matches)}")
        return 1

    if allowed_skips != 1:
        print(f"FAIL designated rejected fixture absent from Git enumeration or duplicated (observed count {allowed_skips})")
        return 1

    if allowed_binaries != len(ALLOWED_BINARIES):
        print(f"FAIL authorized binary absent from Git enumeration or duplicated (observed count {allowed_binaries})")
        return 1

    print(
        "PASS public boundary check: "
        f"scanned={scanned} allowed_skips={allowed_skips} allowed_binaries={allowed_binaries} "
        "public_safe_fixture=clean rejected_test_fixture=detected"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
