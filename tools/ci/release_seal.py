#!/usr/bin/env python3
"""Strict local release seal verification; never signs, publishes, or uses network."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import shutil
import stat
import subprocess
import sys
import tempfile
from contextlib import redirect_stdout
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

import release_contract_check as contract


def bran_root() -> Path:
    return Path(__file__).resolve().parents[2]


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    try:
        with path.open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                hasher.update(chunk)
    except OSError as error:
        raise ValueError(f"read error for {path.name}: {error}") from None
    return hasher.hexdigest()


def git_state(root: Path, tag: str) -> tuple[str, str | None, bool | None, str | None]:
    try:
        head = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
        tagged = subprocess.check_output(
            ["git", "-C", str(root), "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}"],
            text=True, stderr=subprocess.DEVNULL,
        ).strip()
        dirty = bool(subprocess.check_output(
            ["git", "-C", str(root), "status", "--porcelain", "--untracked-files=no"], text=True
        ).strip())
        lock_path = (bran_root() / "Cargo.lock").relative_to(root).as_posix()
        tagged_lock = subprocess.check_output(
            ["git", "-C", str(root), "show", f"refs/tags/{tag}:{lock_path}"],
            stderr=subprocess.DEVNULL,
        )
        return head, tagged, dirty, hashlib.sha256(tagged_lock).hexdigest()
    except (OSError, subprocess.CalledProcessError, ValueError):
        return "", None, None, None


def verify_signature(
    sums: Path,
    signature: Path,
    *,
    _which: Callable[[str], str | None] = shutil.which,
    _run: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> tuple[str, str]:
    """Return the verified fingerprint and signature time."""
    gpg = _which("gpg")
    if not gpg:
        raise ValueError("signature verification unavailable: gpg is not installed")
    try:
        result = _run(
            [
                gpg,
                "--batch",
                "--no-auto-key-retrieve",
                "--auto-key-locate",
                "clear",
                "--status-fd",
                "1",
                "--verify",
                str(signature),
                str(sums),
            ],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
    except OSError as error:
        raise ValueError(f"signature verification unavailable: cannot run gpg: {error}") from error
    if result.returncode:
        detail = result.stderr.strip() or "no VALIDSIG status"
        raise ValueError(f"signature verification unavailable: {detail}")

    valid = [line.split() for line in result.stdout.splitlines() if line.startswith("[GNUPG:] VALIDSIG ")]
    if len(valid) != 1 or len(valid[0]) < 5:
        raise ValueError("signature verification unavailable: expected exactly one complete VALIDSIG status")
    fingerprint, creation_date, creation_epoch = valid[0][2:5]
    fingerprint = fingerprint.lower()
    if not contract.is_fingerprint(fingerprint):
        raise ValueError("signature verification unavailable: gpg returned an invalid signer fingerprint")
    try:
        epoch = int(creation_epoch)
        if epoch < 0:
            raise ValueError
        signed_at = datetime.fromtimestamp(epoch, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        parsed_date = datetime.strptime(creation_date, "%Y-%m-%d").strftime("%Y-%m-%d")
    except (OSError, OverflowError, ValueError) as error:
        raise ValueError("signature verification unavailable: gpg returned an invalid signature time") from error
    if parsed_date != creation_date or creation_date != signed_at[:10]:
        raise ValueError("signature verification unavailable: gpg signature date and epoch disagree")
    return fingerprint, signed_at


def archives(tag: str) -> tuple[str, ...]:
    # The semantic oracle owns the exact seven-asset contract; five are archives.
    return contract.expected_asset_names(tag)[:5]


def require_archives(tag: str, dist: Path) -> tuple[str, ...]:
    names = archives(tag)
    for name in names:
        p = dist / name
        if p.is_symlink():
            raise ValueError(f"symlink not permitted for {name}")
    present = {
        path.name
        for path in dist.iterdir()
        if (path.is_file() or path.is_symlink()) and path.name.endswith((".tar.gz", ".zip"))
    }
    missing = sorted(set(names) - present)
    extra = sorted(present - set(names))
    if missing or extra:
        parts = ([f"missing platform archives: {missing}"] if missing else []) + (
            [f"extra platform archives: {extra}"] if extra else []
        )
        raise ValueError("; ".join(parts))
    return names


def expected_sums(names: tuple[str, ...], dist: Path) -> str:
    return "".join(f"{digest(dist / name)}  {name}\n" for name in sorted(names))


def require_sums(names: tuple[str, ...], dist: Path, create: bool) -> Path:
    sums = dist / "SHA256SUMS"
    if sums.is_symlink():
        raise ValueError("symlink not permitted for SHA256SUMS")
    expected = expected_sums(names, dist)
    if not sums.exists() and create:
        sums.write_text(expected, encoding="utf-8")
    if not sums.is_file():
        raise ValueError("missing required asset: SHA256SUMS")
    if sums.read_text(encoding="utf-8") != expected:
        raise ValueError("SHA256SUMS does not match exactly the five platform archives")
    return sums


def _ensure_real_file_for_read(p: Path, label: str) -> bytes:
    """Small helper: reject symlink/non-regular/missing before read; accurate ValueError, never traceback."""
    if p.is_symlink():
        raise ValueError(f"symlink not permitted for {label}")
    if not p.is_file():
        raise ValueError(f"missing required asset: {label}")
    try:
        return p.read_bytes()
    except OSError as e:
        raise ValueError(f"read error for {label}: {e}") from None


def snapshot(path: Path, label: str) -> tuple[int, str]:
    """Return a streaming (size, sha256) snapshot of one non-symlink file."""
    try:
        if path.is_symlink():
            raise ValueError(f"symlink not permitted for {label}")
        before = path.stat()
        if not stat.S_ISREG(before.st_mode):
            raise ValueError(f"missing required asset: {label}")
        value = digest(path)
        if path.is_symlink() or path.stat().st_size != before.st_size:
            raise ValueError(f"{label} changed during verification")
    except OSError as error:
        raise ValueError(f"read error for {label}: {error}") from None
    return before.st_size, value


def expected_manifest(
    tag: str,
    head: str,
    lock_digest: str,
    names: tuple[str, ...],
    dist: Path,
    sums: Path,
    signature: Path,
    signer: str,
    signed_at: str,
    _archive_digests: dict[str, str] | None = None,
    _sums_digest: str | None = None,
    _sig_digest: str | None = None,
) -> dict:
    get_arch = (lambda n: _archive_digests[n]) if _archive_digests is not None else (lambda n: digest(dist / n))
    sums_d = _sums_digest if _sums_digest is not None else digest(sums)
    sig_d = _sig_digest if _sig_digest is not None else digest(signature)
    media = lambda name: "application/zip" if name.endswith(".zip") else "application/gzip"
    assets = [{"name": name, "url": f"{contract.RELEASE_BASE}/{tag}/{name}", "sha256": get_arch(name), "media_type": media(name)} for name in names]
    assets += [
        {"name": "SHA256SUMS", "url": f"{contract.RELEASE_BASE}/{tag}/SHA256SUMS", "sha256": sums_d, "media_type": "text/plain"},
        {"name": "SHA256SUMS.sig", "url": f"{contract.RELEASE_BASE}/{tag}/SHA256SUMS.sig", "sha256": sig_d, "media_type": "application/pgp-signature"},
    ]
    return {
        "schema_version": contract.SCHEMA_VERSION, "tag": tag, "repository": contract.REPOSITORY,
        "source_commit": head, "lockfile_sha256": lock_digest, "immutable": True,
        "manifest_asset": "bran-release-manifest.json", "assets": assets,
        "checksums": {"asset": "SHA256SUMS", "algorithm": "sha256", "sha256": sums_d},
        "signature": {"asset": "SHA256SUMS.sig", "format": "openpgp", "key_fingerprint": signer,
                      "signed_at": signed_at},
        "provenance": {"format": "https://slsa.dev/provenance/v1", "predicate_type": "https://slsa.dev/provenance/v1",
                       "source_repository": contract.REPOSITORY, "source_commit": head, "lockfile_sha256": lock_digest,
                       "build_type": "https://alphazede.dev/bran/build/v1"},
    }


def seal(tag: str, dist: Path, dry_run: bool, required_fingerprint: str | None,
         _git: Callable[[Path, str], tuple[str, str | None, bool | None, str | None]] = git_state,
         _proof: Callable[[Path, Path], tuple[str, str]] = verify_signature) -> int:
    if not contract.TAG_PATTERN.fullmatch(tag):
        print(f"FAIL invalid tag (must be bran-vX.Y.Z): {tag}")
        return 1
    if not dist.is_dir():
        print(f"FAIL --dist must be an existing directory: {dist}")
        return 1
    lock = bran_root() / "Cargo.lock"
    if not lock.is_file():
        print(f"FAIL required lockfile is missing: {lock}")
        return 1
    try:
        lock_digest = digest(lock)
    except ValueError as error:
        print(f"FAIL {error}")
        return 1
    head, tagged, dirty, tagged_lock_digest = _git(bran_root().parent, tag)
    if not contract.is_git_sha(head):
        print("FAIL git evidence unavailable: cannot determine HEAD")
        return 1
    if tagged != head or dirty is not False or tagged_lock_digest != lock_digest:
        print(
            "FAIL exact release required: "
            f"clean={dirty is False} tag_equals_head={tagged == head} "
            f"lock_matches_tag={tagged_lock_digest == lock_digest}"
        )
        return 1
    manifest_path = dist / "bran-release-manifest.json"
    sig_path = dist / "SHA256SUMS.sig"
    if dry_run:
        if manifest_path.exists() or manifest_path.is_symlink():
            print("FAIL dry-run unsigned rejects bran-release-manifest.json final-state input")
            return 1
        if sig_path.exists() or sig_path.is_symlink():
            print("FAIL dry-run unsigned rejects SHA256SUMS.sig final-state input")
            return 1
    try:
        names = require_archives(tag, dist)
        sums = require_sums(names, dist, create=dry_run)
    except (OSError, UnicodeDecodeError, ValueError) as error:
        print(f"FAIL {error}")
        return 1

    if dry_run:
        try:
            archive_snapshots = {name: snapshot(dist / name, name) for name in names}
            sums_snapshot = snapshot(sums, "SHA256SUMS")
            evidence = {
                "archives": [{"name": name, "sha256": archive_snapshots[name][1]} for name in sorted(names)],
                "checksums": {"asset": "SHA256SUMS", "sha256": sums_snapshot[1]},
                "final_manifest": "unavailable",
                "kind": "bran-release-unsigned-evidence",
                "lockfile_sha256": lock_digest,
                "publication": "unavailable",
                "signature": "unavailable",
                "source_commit": head,
                "tag": tag,
            }
            path = dist / "bran-release-evidence.unsigned.json"
            path.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        except (OSError, TypeError, ValueError) as error:
            print(f"FAIL dry-run evidence unavailable: {error}")
            return 1
        print("DRY-RUN UNSIGNED: signature, final manifest, and publication unavailable")
        print(f"wrote non-final evidence: {path.name}")
        return 0

    if not required_fingerprint or not contract.is_fingerprint(required_fingerprint.lower()):
        print("FAIL real mode requires a valid --fingerprint policy value")
        return 1
    try:
        manifest_blob = _ensure_real_file_for_read(manifest_path, "bran-release-manifest.json")
        sig_snapshot = snapshot(sig_path, "SHA256SUMS.sig")
    except ValueError as error:
        print(f"FAIL {error}")
        return 1
    # Freeze large archives as streaming (size, digest), not in-memory bytes.
    try:
        archive_snapshots = {name: snapshot(dist / name, name) for name in names}
        sums_blob = _ensure_real_file_for_read(sums, "SHA256SUMS")
    except (OSError, ValueError) as error:
        print(f"FAIL {error}")
        return 1
    # Frozen digests from the exact release evidence.
    arch_digests = {name: value[1] for name, value in archive_snapshots.items()}
    sums_dig_frozen = hashlib.sha256(sums_blob).hexdigest()
    sig_dig_frozen = sig_snapshot[1]
    try:
        signer, signed_at = _proof(sums, sig_path)
        if signer != required_fingerprint.lower():
            raise ValueError(
                f"verified signer fingerprint mismatch: actual={signer} "
                f"required={required_fingerprint.lower()}"
            )
    except (OSError, ValueError) as error:
        print(f"FAIL {error}")
        return 1

    # recheck every input against frozen snapshot; mutation/read error -> controlled FAIL
    def _recheck_all() -> None:
        for name, frozen in archive_snapshots.items():
            if snapshot(dist / name, name) != frozen:
                raise ValueError(f"{name} changed during verification")
        if _ensure_real_file_for_read(sums, "SHA256SUMS") != sums_blob:
            raise ValueError("SHA256SUMS changed during verification")
        if snapshot(sig_path, "SHA256SUMS.sig") != sig_snapshot:
            raise ValueError("SHA256SUMS.sig changed during verification")
        if _ensure_real_file_for_read(manifest_path, "bran-release-manifest.json") != manifest_blob:
            raise ValueError("bran-release-manifest.json changed during verification")

    try:
        _recheck_all()
    except (OSError, ValueError) as error:
        print(f"FAIL {error}")
        return 1

    try:
        manifest = json.loads(manifest_blob)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        print(f"FAIL invalid final manifest: {error}")
        return 1
    errors = contract.validate_manifest(manifest)
    if errors:
        print("FAIL semantic oracle rejected final manifest: " + "; ".join(errors))
        return 1
    # build expected manifest from frozen digests (byte-immutable)
    expected = expected_manifest(
        tag, head, lock_digest, names, dist, sums, sig_path, signer, signed_at,
        _archive_digests=arch_digests,
        _sums_digest=sums_dig_frozen,
        _sig_digest=sig_dig_frozen,
    )
    manifest_fields = {key: value for key, value in manifest.items() if key != "assets"}
    expected_fields = {key: value for key, value in expected.items() if key != "assets"}
    manifest_assets = {asset["name"]: asset for asset in manifest["assets"]}
    expected_assets = {asset["name"]: asset for asset in expected["assets"]}
    if manifest_fields != expected_fields or manifest_assets != expected_assets:
        print("FAIL final manifest does not match locally verified release evidence")
        return 1
    # recheck every input before PASS
    try:
        _recheck_all()
    except (OSError, ValueError) as error:
        print(f"FAIL {error}")
        return 1
    print("PASS sealed release and existing final manifest verified locally; publication remains excluded")
    return 0


def test_p4_sealed_release() -> None:
    """The single named P4 journey covers dry-run and strict refusal paths."""
    print("=== P4-SEALED-RELEASE self-test ===")
    tag, fingerprint, head = "bran-v4.2.0", "0123456789abcdef0123456789abcdef01234567", "a" * 40
    signed_at = "2026-01-02T03:04:05Z"
    lock_digest = digest(bran_root() / "Cargo.lock")
    good_git = lambda _root, _tag: (head, head, False, lock_digest)
    good_proof = lambda _sums, _signature: (fingerprint, signed_at)

    def expect_rejection(label: str, action: Callable[[], int]) -> None:
        with redirect_stdout(io.StringIO()):
            assert action() == 1
        print(f"EXPECTED-REJECTION {label}")

    commands: list[list[str]] = []

    def fake_gpg(command: list[str], **_kwargs: object) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        status = f"[GNUPG:] VALIDSIG {fingerprint.upper()} 2026-01-02 1767323045 0 4 0 1 10 00\n"
        return subprocess.CompletedProcess(command, 0, status, "")

    verified = verify_signature(
        Path("SHA256SUMS"),
        Path("SHA256SUMS.sig"),
        _which=lambda _name: "/usr/bin/gpg",
        _run=fake_gpg,
    )
    assert verified == (fingerprint, signed_at)
    assert commands == [[
        "/usr/bin/gpg", "--batch", "--no-auto-key-retrieve", "--auto-key-locate", "clear",
        "--status-fd", "1", "--verify", "SHA256SUMS.sig", "SHA256SUMS",
    ]]
    try:
        verify_signature(Path("SHA256SUMS"), Path("SHA256SUMS.sig"), _which=lambda _name: None)
        raise AssertionError("missing gpg was accepted")
    except ValueError as error:
        assert "gpg is not installed" in str(error)

    with tempfile.TemporaryDirectory() as tmp:
        dist = Path(tmp)
        for index, name in enumerate(archives(tag)):
            (dist / name).write_bytes(f"artifact-{index}".encode())
        missing = dist / archives(tag)[0]
        missing.unlink()
        expect_rejection("missing archive", lambda: seal(tag, dist, True, None, good_git, good_proof))
        missing.write_bytes(b"artifact-0")
        extra = dist / f"{tag}-unsupported.tar.gz"
        extra.symlink_to(archives(tag)[1])
        expect_rejection("unexpected archive symlink", lambda: seal(tag, dist, True, None, good_git, good_proof))
        extra.unlink()
        expect_rejection("wrong tag", lambda: seal(tag, dist, True, None, lambda *_: (head, "b" * 40, False, lock_digest), good_proof))
        expect_rejection("dirty tree", lambda: seal(tag, dist, True, None, lambda *_: (head, head, True, lock_digest), good_proof))
        expect_rejection("wrong lock", lambda: seal(tag, dist, True, None, lambda *_: (head, head, False, "0" * 64), good_proof))
        assert seal(tag, dist, True, None, good_git, good_proof) == 0
        sums = dist / "SHA256SUMS"
        assert sums.read_text(encoding="utf-8") == expected_sums(archives(tag), dist)
        evidence = dist / "bran-release-evidence.unsigned.json"
        first = evidence.read_bytes()
        assert seal(tag, dist, True, None, good_git, good_proof) == 0 and evidence.read_bytes() == first
        assert not (dist / "bran-release-manifest.json").exists()
        signature = dist / "SHA256SUMS.sig"
        signature.write_bytes(b"final-state signature")
        expect_rejection("signature-only dry-run", lambda: seal(tag, dist, True, None, good_git, good_proof))
        signature.unlink()
        sums.write_text("drift\n", encoding="utf-8")
        expect_rejection("checksum drift", lambda: seal(tag, dist, True, None, good_git, good_proof))
        sums.write_text(expected_sums(archives(tag), dist), encoding="utf-8")
        signature.write_bytes(b"not accepted by the injected verifier alone")
        manifest_path = dist / "bran-release-manifest.json"
        expect_rejection("missing manifest", lambda: seal(tag, dist, False, fingerprint, good_git, good_proof))
        manifest = expected_manifest(
            tag, head, lock_digest, archives(tag), dist, sums, signature, fingerprint, signed_at
        )
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        expect_rejection("unverified signature", lambda: seal(tag, dist, False, fingerprint, good_git, lambda *_: (_ for _ in ()).throw(ValueError("signature verification unavailable: no key"))))
        expect_rejection("wrong signer", lambda: seal(tag, dist, False, fingerprint, good_git, lambda *_: ("f" * 40, signed_at)))
        original_manifest = manifest_path.read_bytes()
        assert seal(tag, dist, False, fingerprint, good_git, good_proof) == 0
        assert manifest_path.read_bytes() == original_manifest
        original_sums = sums.read_bytes()
        def mutating_proof(_sums: Path, _signature: Path) -> tuple[str, str]:
            sums.write_bytes(original_sums + b"drift")
            return fingerprint, signed_at
        expect_rejection("mutated supporting asset", lambda: seal(tag, dist, False, fingerprint, good_git, mutating_proof))
        sums.write_bytes(original_sums)
        symlink_archive = dist / archives(tag)[0]
        saved_archive = dist / "saved-archive"
        symlink_archive.rename(saved_archive)
        symlink_archive.symlink_to(saved_archive.name)
        expect_rejection("symlink archive", lambda: seal(tag, dist, False, fingerprint, good_git, good_proof))
        symlink_archive.unlink()
        saved_archive.rename(symlink_archive)
        expect_rejection("dry-run final-state inputs", lambda: seal(tag, dist, True, None, good_git, good_proof))
        assert manifest_path.read_bytes() == original_manifest
        manifest["assets"][0]["sha256"] = "0" * 64
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        tampered_manifest = manifest_path.read_bytes()
        expect_rejection("tampered manifest", lambda: seal(tag, dist, False, fingerprint, good_git, good_proof))
        assert manifest_path.read_bytes() == tampered_manifest
    print("=== P4-SEALED-RELEASE self-test PASS ===")


def main() -> int:
    if sys.argv[1:] == ["--self-test"]:
        try:
            test_p4_sealed_release()
            return 0
        except (AssertionError, OSError, ValueError) as error:
            print(f"FAIL P4-SEALED-RELEASE: {error}")
            return 1
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", required=True)
    parser.add_argument("--dist", required=True, type=Path)
    parser.add_argument("--dry-run-unsigned", action="store_true")
    parser.add_argument("--fingerprint")
    args = parser.parse_args()
    return seal(args.tag, args.dist.resolve(), args.dry_run_unsigned, args.fingerprint)


if __name__ == "__main__":
    sys.exit(main())
