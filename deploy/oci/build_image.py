#!/usr/bin/env python3
"""Build and verify BRAN's stage-1 OCI image (docs/deployment-profile.md).

`build` layers the exact release binary onto a base image layout pinned by
manifest digest. `verify` checks an image layout against the deployment
contract. Standard library only; the same inputs give the same image digest.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
INDEX_MEDIA = "application/vnd.oci.image.index.v1+json"
MANIFEST_MEDIA = "application/vnd.oci.image.manifest.v1+json"
BASE_MANIFEST_MEDIA = {MANIFEST_MEDIA, "application/vnd.docker.distribution.manifest.v2+json"}
CONFIG_MEDIA = "application/vnd.oci.image.config.v1+json"
LAYER_TAR = "application/vnd.oci.image.layer.v1.tar"
GZIP_LAYERS = {"application/vnd.oci.image.layer.v1.tar+gzip",
               "application/vnd.docker.image.rootfs.diff.tar.gzip"}
PLATFORMS = {"x86_64-unknown-linux-gnu": "amd64", "aarch64-unknown-linux-gnu": "arm64"}
USER = "65532:65532"
ENTRYPOINT = ["/usr/local/bin/bran"]
CMD = ["smoke"]
WORKDIR = "/"
# Only PATH, plus an empty registry so the image fails closed until the
# operator registers a root.
ENV = ["PATH=/usr/local/bin:/usr/bin:/bin", "BRAN_REGISTERED_ROOTS="]
BINARY = "usr/local/bin/bran"
DOCS = "usr/share/doc/bran"
INVENTORY = f"{DOCS}/dependency-licenses.json"
LICENSE_FILES = ("LICENSE-APACHE", "LICENSE-MIT")
DIRECTORIES = ("usr", "usr/local", "usr/local/bin", "usr/share", "usr/share/doc", DOCS)
ALLOWED_LICENSES = {"MIT", "Apache-2.0"}
MAX_BINARY_BYTES = 64 * 1024 * 1024
EPOCH = "1970-01-01T00:00:00Z"
LABELS = (
    "org.opencontainers.image.version",
    "org.opencontainers.image.revision",
    "org.opencontainers.image.source",
    "org.opencontainers.image.licenses",
    "org.opencontainers.image.base.digest",
    "dev.alphazede.bran.target",
    "dev.alphazede.bran.release-archive.sha256",
    "dev.alphazede.bran.binary.sha256",
    "dev.alphazede.bran.lockfile.sha256",
)


class ContractError(Exception):
    """An input or image that breaks the deployment contract."""


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def descriptor(media: str, data: bytes) -> dict:
    return {"mediaType": media, "digest": f"sha256:{sha256(data)}", "size": len(data)}


def blob(layout: Path, digest: object, size: object = None) -> bytes:
    algorithm, _, hexdigest = str(digest).partition(":")
    if algorithm != "sha256" or len(hexdigest) != 64 or set(hexdigest) - set("0123456789abcdef"):
        raise ContractError(f"unsupported digest {digest}")
    data = (layout / "blobs" / "sha256" / hexdigest).read_bytes()
    if sha256(data) != hexdigest or (size is not None and size != len(data)):
        raise ContractError(f"blob digest or size mismatch: {digest}")
    return data


def load_image(layout: Path) -> tuple[str, dict, dict]:
    """The single indexed manifest digest, the manifest, and its config."""
    manifests = json.loads((layout / "index.json").read_bytes()).get("manifests")
    if not isinstance(manifests, list) or len(manifests) != 1:
        raise ContractError("image layout must index exactly one manifest")
    entry = manifests[0]
    manifest = json.loads(blob(layout, entry.get("digest"), entry.get("size")))
    if manifest.get("mediaType", entry.get("mediaType")) not in BASE_MANIFEST_MEDIA:
        raise ContractError("indexed entry is not a single-platform image manifest")
    config = json.loads(blob(layout, manifest["config"]["digest"], manifest["config"]["size"]))
    return entry["digest"], manifest, config


def layer_members(media: str, data: bytes) -> tuple[str, list[tarfile.TarInfo], tarfile.TarFile]:
    """The uncompressed layer digest (diff_id) and its members."""
    if media in GZIP_LAYERS:
        data = gzip.decompress(data)
    elif media != LAYER_TAR:
        raise ContractError(f"unsupported layer media type {media}")
    archive = tarfile.open(fileobj=io.BytesIO(data), mode="r:")
    return f"sha256:{sha256(data)}", archive.getmembers(), archive


def license_allowed(expression: str) -> bool:
    """True when the SPDX expression can be satisfied with MIT or Apache-2.0."""
    if not expression or "(" in expression or ")" in expression:
        return False
    alternatives = expression.replace("/", " OR ").split(" OR ")
    return any(all(part.strip() in ALLOWED_LICENSES for part in choice.split(" AND "))
               for choice in alternatives)


def dependency_licenses(target: str) -> bytes:
    """Name, version, and licence of every normal dependency of bran-cli."""
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--offline", "--format-version", "1",
         "--filter-platform", target],
        cwd=ROOT, capture_output=True, check=True)
    metadata = json.loads(result.stdout)
    packages = {item["id"]: item for item in metadata["packages"]}
    nodes = {item["id"]: item for item in metadata["resolve"]["nodes"]}
    pending = [key for key, item in packages.items()
               if item["name"] == "bran-cli" and item["source"] is None]
    seen: set[str] = set()
    while pending:
        current = pending.pop()
        if current not in seen:
            seen.add(current)
            pending.extend(dep["pkg"] for dep in nodes[current]["deps"]
                           if any(kind["kind"] is None for kind in dep["dep_kinds"]))
    return canonical(sorted(
        ({"name": packages[key]["name"], "version": packages[key]["version"],
          "license": packages[key]["license"] or ""} for key in seen),
        key=lambda item: (item["name"], item["version"])))


def release_binary(archive: Path, release: dict) -> tuple[str, str, bytes]:
    """The target, archive digest, and binary of a release-manifest asset."""
    data = archive.read_bytes()
    assets = [item for item in release.get("assets", []) if item.get("name") == archive.name]
    if len(assets) != 1 or assets[0].get("sha256") != sha256(data):
        raise ContractError("release archive does not match its release-manifest digest")
    prefix, suffix = f"{release['tag']}-", ".tar.gz"
    target = archive.name[len(prefix):-len(suffix)]
    if not archive.name.startswith(prefix) or not archive.name.endswith(suffix) or target not in PLATFORMS:
        raise ContractError("release archive is not a Linux release target")
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as release_tar:
        members = release_tar.getmembers()
        if (len(members) != 1 or members[0].name != "bran" or not members[0].isfile()
                or members[0].size > MAX_BINARY_BYTES):
            raise ContractError("release archive must hold exactly one regular file named bran")
        return target, sha256(data), release_tar.extractfile(members[0]).read()


def expected_layer(binary: bytes, inventory: bytes) -> list[tuple[str, int, bytes | None]]:
    files = [(BINARY, 0o555, binary), (INVENTORY, 0o444, inventory)]
    files += [(f"{DOCS}/{name}", 0o444, (ROOT / name).read_bytes()) for name in LICENSE_FILES]
    return sorted([(name, 0o755, None) for name in DIRECTORIES] + files, key=lambda item: item[0])


def write_layer(entries: list[tuple[str, int, bytes | None]]) -> bytes:
    """An uncompressed USTAR layer with fixed owners and timestamps."""
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.USTAR_FORMAT) as layer:
        for name, mode, content in entries:
            info = tarfile.TarInfo(name)
            info.mode, info.mtime, info.uid, info.gid, info.uname, info.gname = mode, 0, 0, 0, "", ""
            if content is None:
                info.type = tarfile.DIRTYPE
                layer.addfile(info)
            else:
                info.size = len(content)
                layer.addfile(info, io.BytesIO(content))
    return buffer.getvalue()


def write_layout(out: Path, blobs: list[bytes], manifest: bytes, platform: dict, name: str) -> str:
    (out / "blobs" / "sha256").mkdir(parents=True)
    for data in [*blobs, manifest]:
        (out / "blobs" / "sha256" / sha256(data)).write_bytes(data)
    entry = {**descriptor(MANIFEST_MEDIA, manifest), "platform": platform,
             "annotations": {"org.opencontainers.image.ref.name": name}}
    (out / "oci-layout").write_bytes(canonical({"imageLayoutVersion": "1.0.0"}))
    (out / "index.json").write_bytes(canonical(
        {"schemaVersion": 2, "mediaType": INDEX_MEDIA, "manifests": [entry]}))
    return entry["digest"]


def build(archive: Path, release_manifest: Path, base_layout: Path, base_digest: str,
          out: Path, inventory: bytes | None = None) -> str:
    """Write the image layout to `out` (which must not exist); return its digest."""
    release = json.loads(release_manifest.read_bytes())
    target, archive_sha, binary = release_binary(archive, release)
    if sha256((ROOT / "Cargo.lock").read_bytes()) != release.get("lockfile_sha256"):
        raise ContractError("Cargo.lock does not match the release lockfile digest")
    digest, base_manifest, base_config = load_image(base_layout)
    if digest != base_digest:
        raise ContractError("base image manifest digest does not match the pin")
    platform = {"architecture": PLATFORMS[target], "os": "linux"}
    if {key: base_config.get(key) for key in platform} != platform:
        raise ContractError("base image platform does not match the release target")
    base_layers = [blob(base_layout, item["digest"], item["size"]) for item in base_manifest["layers"]]
    if [layer_members(item["mediaType"], data)[0] for item, data in
            zip(base_manifest["layers"], base_layers)] != base_config["rootfs"]["diff_ids"]:
        raise ContractError("base image diff_ids do not match its layers")
    layer = write_layer(expected_layer(binary, inventory or dependency_licenses(target)))
    version = release["tag"].removeprefix("bran-v")
    labels = dict(zip(LABELS, (
        version, release["source_commit"], f"https://github.com/{release['repository']}",
        "MIT OR Apache-2.0", digest, target, archive_sha, sha256(binary),
        release["lockfile_sha256"])))
    config = canonical({
        **platform,
        "created": EPOCH,
        "config": {"User": USER, "Env": ENV, "Entrypoint": ENTRYPOINT, "Cmd": CMD,
                   "WorkingDir": WORKDIR, "Labels": labels},
        "rootfs": {"type": "layers",
                   "diff_ids": [*base_config["rootfs"]["diff_ids"], f"sha256:{sha256(layer)}"]},
        "history": [*base_config.get("history", []),
                    {"created": EPOCH, "created_by": "deploy/oci/build_image.py",
                     "comment": f"bran {version}"}],
    })
    manifest = canonical({
        "schemaVersion": 2, "mediaType": MANIFEST_MEDIA,
        "config": descriptor(CONFIG_MEDIA, config),
        "layers": [*({key: item[key] for key in ("mediaType", "digest", "size")}
                     for item in base_manifest["layers"]), descriptor(LAYER_TAR, layer)],
    })
    return write_layout(out, [*base_layers, layer, config], manifest, platform, version)


def member_problems(member: tarfile.TarInfo) -> list[str]:
    problems = []
    parts = member.name.split("/")
    if member.name.startswith("/") or ".." in parts:
        problems.append(f"unsafe member name {member.name}")
    if member.mode & 0o6000:
        problems.append(f"setuid or setgid file {member.name}")
    if any(key.endswith("security.capability") for key in member.pax_headers):
        problems.append(f"file capability on {member.name}")
    if member.ischr() or member.isblk():
        problems.append(f"device node {member.name}")
    if (not member.issym() and member.mode & 0o002
            and not (member.isdir() and member.mode & 0o1000)):
        problems.append(f"world-writable path {member.name}")
    return problems


def verify(layout: Path) -> list[str]:
    """Every deployment-contract problem in the image layout; empty when it passes."""
    try:
        if json.loads((layout / "oci-layout").read_bytes()) != {"imageLayoutVersion": "1.0.0"}:
            return ["oci-layout version must be 1.0.0"]
        _, manifest, config = load_image(layout)
        layers = manifest["layers"]
        diff_ids = config["rootfs"]["diff_ids"]
    except (OSError, ValueError, KeyError, TypeError, ContractError) as error:
        return [f"unreadable image layout: {error}"]
    settings = config.get("config") or {}
    problems = [f"{key} must be {value!r}" for key, value in
                (("User", USER), ("Entrypoint", ENTRYPOINT), ("Cmd", CMD),
                 ("WorkingDir", WORKDIR), ("Env", ENV)) if settings.get(key) != value]
    problems += [f"{key} must be absent" for key in ("ExposedPorts", "Volumes") if settings.get(key)]
    labels = settings.get("Labels") or {}
    problems += [f"label {key} is missing" for key in LABELS if not labels.get(key)]
    if len(layers) != len(diff_ids) or not layers:
        return problems + ["layers and diff_ids differ"]
    last = None
    for item, diff_id in zip(layers, diff_ids):
        try:
            actual, members, archive = layer_members(
                item["mediaType"], blob(layout, item["digest"], item["size"]))
        except (OSError, KeyError, tarfile.TarError, ContractError) as error:
            problems.append(f"unreadable layer: {error}")
            last = None
            continue
        if actual != diff_id:
            problems.append(f"diff_id mismatch for {item['digest']}")
        for member in members:
            problems += member_problems(member)
        last = (item, members, archive)
    if last is None:
        return problems + ["BRAN layer is unreadable"]
    item, members, archive = last
    if item["mediaType"] != LAYER_TAR:
        return problems + ["BRAN layer must be an uncompressed tar"]
    observed = [(member.name, member.mode, member.type, member.uid, member.gid, member.mtime)
                for member in members]
    content = {member.name: archive.extractfile(member).read() for member in members if member.isfile()}
    binary, inventory = content.get(BINARY, b""), content.get(INVENTORY, b"[]")
    expected = [(name, mode, tarfile.REGTYPE if data is not None else tarfile.DIRTYPE, 0, 0, 0)
                for name, mode, data in expected_layer(binary, inventory)]
    if observed != expected:
        problems.append("BRAN layer entries, modes, owners, or timestamps differ from the contract")
    if labels.get("dev.alphazede.bran.binary.sha256") != sha256(binary):
        problems.append("BRAN binary does not match its digest label")
    try:
        dependencies = json.loads(inventory)
        problems += [f"dependency licence not allowed: {entry['name']} {entry['license']}"
                     for entry in dependencies if not license_allowed(entry["license"])]
        if not dependencies:
            problems.append("dependency licence inventory is empty")
    except (ValueError, TypeError, KeyError):
        problems.append("dependency licence inventory is malformed")
    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build_command = commands.add_parser("build")
    for option in ("--release-archive", "--release-manifest", "--base-layout", "--out"):
        build_command.add_argument(option, type=Path, required=True)
    build_command.add_argument("--base-manifest-digest", required=True)
    verify_command = commands.add_parser("verify")
    verify_command.add_argument("layout", type=Path)
    arguments = parser.parse_args()
    if arguments.command == "verify":
        problems = verify(arguments.layout)
        for problem in problems:
            print(f"FAIL {problem}")
        if not problems:
            print("PASS image layout meets the deployment contract")
        return 1 if problems else 0
    if arguments.out.exists():
        print("FAIL output path already exists")
        return 1
    try:
        digest = build(arguments.release_archive, arguments.release_manifest,
                       arguments.base_layout, arguments.base_manifest_digest, arguments.out)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError, ContractError) as error:
        # The output did not exist before this run, so only partial output is removed.
        shutil.rmtree(arguments.out, ignore_errors=True)
        print(f"FAIL {error}")
        return 1
    print(digest)
    return 0


if __name__ == "__main__":
    sys.exit(main())
