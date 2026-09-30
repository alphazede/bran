#!/usr/bin/env python3
"""Deployment-profile contract for issue #28 (docs/deployment-profile.md).

Offline and deterministic:
- workspace sources use no socket API and Cargo.lock holds no network crate
- bran-core keeps zero dependencies
- the systemd example keeps its isolation settings
- deploy/oci/build_image.py builds a reproducible image from a synthetic base
  and release archive, and `verify` rejects each contract violation
"""

from __future__ import annotations

import gzip
import hashlib
import importlib.util
import io
import json
import re
import sys
import tarfile
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SOCKET_API = re.compile(
    r"\b(?:std::net|unix::net|TcpListener|TcpStream|UdpSocket|UnixListener|UnixStream"
    r"|UnixDatagram|ToSocketAddrs|libc::(?:socket|connect|bind|listen|accept))\b")
NETWORK_CRATES = {
    "actix-server", "actix-web", "async-std", "axum", "curl", "curl-sys", "h2",
    "hickory-resolver", "hyper", "isahc", "native-tls", "openssl", "openssl-sys", "quinn",
    "reqwest", "rustls", "smol", "socket2", "surf", "tiny_http", "tokio", "tonic",
    "trust-dns-resolver", "tungstenite", "ureq", "warp",
}
UNIT = ROOT / "deploy/systemd/bran-check@.service"
UNIT_SETTINGS = {
    "ExecStart": "/usr/local/bin/bran check /srv/bran/repos/%i bran-strict",
    "Environment": "BRAN_REGISTERED_ROOTS=/srv/bran/repos/%i",
    "DynamicUser": "yes",
    "TemporaryFileSystem": "/srv/bran/repos:ro",
    "BindReadOnlyPaths": "/srv/bran/repos/%i",
    "ProtectSystem": "strict",
    "ProtectHome": "yes",
    "PrivateNetwork": "yes",
    "IPAddressDeny": "any",
    "RestrictAddressFamilies": "none",
    "NoNewPrivileges": "yes",
    "CapabilityBoundingSet": "",
    "AmbientCapabilities": "",
    "RestrictSUIDSGID": "yes",
    "TimeoutStartSec": "300",
}
UNIT_WRITABLE = {"ReadWritePaths", "StateDirectory", "CacheDirectory", "LogsDirectory",
                 "RuntimeDirectory", "ConfigurationDirectory", "BindPaths"}
SYNTHETIC_TAG = "bran-v0.0.0"
SYNTHETIC_ARCHIVE = f"{SYNTHETIC_TAG}-x86_64-unknown-linux-gnu.tar.gz"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def network_surface_errors() -> list[str]:
    errors = []
    for path in sorted((ROOT / "crates").rglob("*.rs")):
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if SOCKET_API.search(line):
                errors.append(f"socket API in {path.relative_to(ROOT)}:{number}")
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    crates = {package["name"] for package in lock.get("package", [])}
    errors += [f"network crate in Cargo.lock: {name}" for name in sorted(crates & NETWORK_CRATES)]
    core = tomllib.loads((ROOT / "crates/bran-core/Cargo.toml").read_text(encoding="utf-8"))
    errors += [f"bran-core must keep zero dependencies: [{key}]" for key in
               ("dependencies", "build-dependencies", "dev-dependencies", "target") if core.get(key)]
    return errors


def unit_errors() -> list[str]:
    settings: dict[str, list[str]] = {}
    section = ""
    for line in UNIT.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line.startswith("["):
            section = line
        elif line and not line.startswith("#") and section == "[Service]":
            key, _, value = line.partition("=")
            settings.setdefault(key, []).append(value)
    errors = [f"unit {key} must be exactly {value!r}" for key, value in UNIT_SETTINGS.items()
              if settings.get(key) != [value]]
    errors += [f"unit must not grant writable paths: {key}" for key in sorted(UNIT_WRITABLE & set(settings))]
    return errors


def load_builder():
    spec = importlib.util.spec_from_file_location("build_image", ROOT / "deploy/oci/build_image.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def tar_bytes(members: list[tuple[tarfile.TarInfo, bytes | None]]) -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for info, content in members:
            archive.addfile(info, io.BytesIO(content) if content is not None else None)
    return buffer.getvalue()


def member(name: str, mode: int, content: bytes | None = None, kind: bytes = tarfile.REGTYPE,
           pax: dict[str, str] | None = None) -> tuple[tarfile.TarInfo, bytes | None]:
    info = tarfile.TarInfo(name)
    info.mode, info.type, info.pax_headers = mode, kind, pax or {}
    if content is not None:
        info.size = len(content)
    return info, content


def write_blob(layout: Path, data: bytes) -> dict:
    (layout / "blobs/sha256").mkdir(parents=True, exist_ok=True)
    (layout / "blobs/sha256" / sha256(data)).write_bytes(data)
    return {"digest": f"sha256:{sha256(data)}", "size": len(data)}


def write_index(layout: Path, manifest: dict) -> str:
    data = json.dumps(manifest, sort_keys=True).encode()
    entry = {"mediaType": "application/vnd.oci.image.manifest.v1+json", **write_blob(layout, data)}
    (layout / "oci-layout").write_text('{"imageLayoutVersion":"1.0.0"}')
    (layout / "index.json").write_text(json.dumps({"schemaVersion": 2, "manifests": [entry]}))
    return entry["digest"]


def synthetic_base(layout: Path, extra: list | None = None, architecture: str = "amd64") -> str:
    """A one-layer gzip base with a passwd entry, a sticky /tmp, and `extra` members."""
    raw = tar_bytes([
        member("etc", 0o755, kind=tarfile.DIRTYPE),
        member("etc/passwd", 0o644, b"nonroot:x:65532:65532::/nonexistent:/sbin/nologin\n"),
        member("tmp", 0o1777, kind=tarfile.DIRTYPE),
        *(extra or []),
    ])
    layer = gzip.compress(raw, mtime=0)
    config = json.dumps({
        "architecture": architecture, "os": "linux",
        "config": {"User": "0", "Env": ["PATH=/bin"], "ExposedPorts": {"80/tcp": {}}},
        "rootfs": {"type": "layers", "diff_ids": [f"sha256:{sha256(raw)}"]},
        "history": [{"created_by": "synthetic base"}],
    }).encode()
    return write_index(layout, {
        "schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", **write_blob(layout, config)},
        "layers": [{"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                    **write_blob(layout, layer)}],
    })


def synthetic_release(directory: Path, members: list | None = None,
                      lockfile: str | None = None, archive_sha: str | None = None) -> tuple[Path, Path]:
    """A release archive holding a stand-in `bran` plus its release manifest."""
    raw = tar_bytes(members or [member("bran", 0o755, b"\x7fELF synthetic bran stand-in\n")])
    archive = directory / SYNTHETIC_ARCHIVE
    archive.write_bytes(gzip.compress(raw, mtime=0))
    manifest = directory / "bran-release-manifest.json"
    manifest.write_text(json.dumps({
        "tag": SYNTHETIC_TAG, "repository": "alphazede/bran", "source_commit": "0" * 40,
        "lockfile_sha256": lockfile or sha256((ROOT / "Cargo.lock").read_bytes()),
        "assets": [{"name": SYNTHETIC_ARCHIVE, "sha256": archive_sha or sha256(archive.read_bytes())}],
    }))
    return archive, manifest


def reseal(layout: Path, change_config=None, layer_entries=None) -> None:
    """Rewrite config and/or the last layer, then re-digest manifest and index."""
    index = json.loads((layout / "index.json").read_bytes())
    manifest = json.loads((layout / "blobs/sha256" / index["manifests"][0]["digest"][7:]).read_bytes())
    config = json.loads((layout / "blobs/sha256" / manifest["config"]["digest"][7:]).read_bytes())
    if layer_entries is not None:
        layer = tar_bytes(layer_entries)
        manifest["layers"][-1] = {"mediaType": "application/vnd.oci.image.layer.v1.tar",
                                  **write_blob(layout, layer)}
        config["rootfs"]["diff_ids"][-1] = f"sha256:{sha256(layer)}"
    if change_config is not None:
        change_config(config["config"])
    manifest["config"] = {**manifest["config"], **write_blob(layout, json.dumps(config).encode())}
    write_index(layout, manifest)


def bran_layer_entries(layout: Path) -> list[tuple[tarfile.TarInfo, bytes | None]]:
    index = json.loads((layout / "index.json").read_bytes())
    manifest = json.loads((layout / "blobs/sha256" / index["manifests"][0]["digest"][7:]).read_bytes())
    data = (layout / "blobs/sha256" / manifest["layers"][-1]["digest"][7:]).read_bytes()
    archive = tarfile.open(fileobj=io.BytesIO(data), mode="r:")
    return [(item, archive.extractfile(item).read() if item.isfile() else None)
            for item in archive.getmembers()]


def tree(layout: Path) -> list[tuple[str, bytes]]:
    return sorted((str(path.relative_to(layout)), path.read_bytes())
                  for path in layout.rglob("*") if path.is_file())


def image_errors() -> list[str]:
    builder = load_builder()
    errors: list[str] = []
    with tempfile.TemporaryDirectory(prefix="bran-oci-") as scratch:
        work = Path(scratch)
        base = work / "base"
        base_digest = synthetic_base(base)
        archive, release = synthetic_release(work)

        first = builder.build(archive, release, base, base_digest, work / "first")
        second = builder.build(archive, release, base, base_digest, work / "second")
        if first != second or tree(work / "first") != tree(work / "second"):
            errors.append("image build is not reproducible")
        if problems := builder.verify(work / "first"):
            errors.append(f"reference image fails verify: {problems}")

        def rejected(label: str, layout: Path, expected: str) -> None:
            problems = builder.verify(layout)
            if not any(expected in problem for problem in problems):
                errors.append(f"verify accepted {label}: {problems}")

        def copy_of(name: str) -> Path:
            target = work / name
            for relative, data in tree(work / "first"):
                (target / relative).parent.mkdir(parents=True, exist_ok=True)
                (target / relative).write_bytes(data)
            return target

        env = list(builder.ENV)
        config_violations = [
            ("root user", lambda c: c.update(User="0"), "User must be"),
            ("exposed port", lambda c: c.update(ExposedPorts={"8080/tcp": {}}), "ExposedPorts must be absent"),
            ("shell entrypoint", lambda c: c.update(Entrypoint=["/bin/sh", "-c"]), "Entrypoint must be"),
            ("writable volume", lambda c: c.update(Volumes={"/var/lib/bran": {}}), "Volumes must be absent"),
            ("baked credential", lambda c: c.update(Env=[*env, "AWS_SECRET_ACCESS_KEY=x"]), "Env must be"),
            ("unpinned base", lambda c: c["Labels"].pop("org.opencontainers.image.base.digest"),
             "label org.opencontainers.image.base.digest is missing"),
        ]
        for number, (label, change, expected) in enumerate(config_violations):
            layout = copy_of(f"config-{number}")
            reseal(layout, change_config=change)
            rejected(label, layout, expected)

        entries = bran_layer_entries(work / "first")
        extra = [*entries, member("etc/bran.conf", 0o444, b"registered=/\n")]
        writable = bran_layer_entries(work / "first")
        writable[[info.name for info, _ in writable].index(builder.BINARY)][0].mode = 0o777
        for label, layer, expected in (
                ("extra file in the BRAN layer", extra, "BRAN layer entries"),
                ("world-writable binary", writable, "world-writable path usr/local/bin/bran")):
            layout = copy_of(label.replace(" ", "-"))
            reseal(layout, layer_entries=layer)
            rejected(label, layout, expected)

        tampered = copy_of("tampered")
        index = json.loads((tampered / "index.json").read_bytes())
        manifest = json.loads((tampered / "blobs/sha256" / index["manifests"][0]["digest"][7:]).read_bytes())
        layer_path = tampered / "blobs/sha256" / manifest["layers"][-1]["digest"][7:]
        data = bytearray(layer_path.read_bytes())
        data[600] ^= 1
        layer_path.write_bytes(bytes(data))
        rejected("tampered layer blob", tampered, "blob digest or size mismatch")

        inventory = json.dumps([{"name": "copyleft", "version": "1.0.0", "license": "GPL-3.0-only"}]).encode()
        builder.build(archive, release, base, base_digest, work / "copyleft", inventory)
        rejected("GPL dependency", work / "copyleft", "dependency licence not allowed: copyleft")

        base_violations = [
            ("setuid base file", member("usr/bin/su", 0o4755, b"su"), "setuid or setgid file usr/bin/su"),
            ("file capability", member("usr/bin/ping", 0o755, b"ping",
                                       pax={"SCHILY.xattr.security.capability": "cap"}),
             "file capability on usr/bin/ping"),
            ("device node", member("dev/mem", 0o600, kind=tarfile.CHRTYPE), "device node dev/mem"),
            ("world-writable base file", member("etc/shared", 0o666, b"x"),
             "world-writable path etc/shared"),
        ]
        for number, (label, extra_member, expected) in enumerate(base_violations):
            other_base = work / f"base-{number}"
            digest = synthetic_base(other_base, [extra_member])
            builder.build(archive, release, other_base, digest, work / f"image-{number}")
            rejected(label, work / f"image-{number}", expected)

        arm_base = work / "arm-base"
        arm_digest = synthetic_base(arm_base, architecture="arm64")
        refusals = [
            ("archive digest mismatch", lambda d: synthetic_release(d, archive_sha="0" * 64), base, base_digest),
            ("lockfile mismatch", lambda d: synthetic_release(d, lockfile="0" * 64), base, base_digest),
            ("two-member archive", lambda d: synthetic_release(d, members=[
                member("bran", 0o755, b"a"), member("extra", 0o644, b"b")]), base, base_digest),
            ("unpinned base digest", synthetic_release, base, "sha256:" + "0" * 64),
            ("platform mismatch", synthetic_release, arm_base, arm_digest),
        ]
        for number, (label, make_release, base_layout, digest) in enumerate(refusals):
            directory = work / f"refusal-{number}"
            directory.mkdir()
            try:
                builder.build(*make_release(directory), base_layout, digest, directory / "out")
            except builder.ContractError:
                continue
            errors.append(f"build accepted {label}")
    return errors


def main() -> int:
    errors = network_surface_errors() + unit_errors() + image_errors()
    for error in errors:
        print(f"FAIL deployment contract: {error}")
    if errors:
        return 1
    print("PASS deployment contract: no socket API or network crate, bran-core has zero "
          "dependencies, systemd isolation intact, reproducible OCI image verified")
    return 0


if __name__ == "__main__":
    sys.exit(main())
