//! BRAN release packager (xtask).
//!
//! Replaces the shell+python packaging that `tools/ci/build-release.sh` used
//! to inline, so the release path needs only cargo:
//!
//! ```text
//! cargo run -p xtask -- package --target <TRIPLE> --tag <TAG> --dist <DIR>
//! ```
//!
//! Builds the `bran` binary with `--locked` and writes the deterministic
//! archive described in `tools/ci/build-release.sh` and `docs/plans/`.

mod archive;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The exact five release targets. Anything else is rejected.
const TARGETS: [&str; 5] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

const WINDOWS_TARGET: &str = "x86_64-pc-windows-msvc";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("FAIL {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("package") => package(args),
        Some(other) => Err(format!("unknown command: {other}")),
        None => Err(usage()),
    }
}

fn usage() -> String {
    "usage: cargo run -p xtask -- package --target TRIPLE --tag TAG --dist DIR".to_string()
}

fn package(args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut target = None;
    let mut tag = None;
    let mut dist = None;
    let mut args = args.into_iter();
    while let Some(option) = args.next() {
        let value = match option.as_str() {
            "--target" | "--tag" | "--dist" => args
                .next()
                .ok_or_else(|| format!("missing value for {option}"))?,
            _ => return Err(format!("unknown option: {option}")),
        };
        match option.as_str() {
            "--target" => target = Some(value),
            "--tag" => tag = Some(value),
            _ => dist = Some(value),
        }
    }
    let target = target.ok_or("missing --target TRIPLE")?;
    let tag = tag.ok_or("missing --tag TAG")?;
    let dist = dist.ok_or("missing --dist DIR")?;

    if !TARGETS.contains(&target.as_str()) {
        return Err(format!(
            "unknown target: {target} (expected one of: {})",
            TARGETS.join(", ")
        ));
    }
    validate_tag(&tag)?;

    let archive_name = archive_name(&tag, &target);
    println!("BUILD target={target} tag={tag} artifact={archive_name}");

    let root = workspace_root()?;
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "--locked",
            "--target",
            target.as_str(),
            "--bin",
            "bran",
        ])
        .current_dir(&root)
        .status()
        .map_err(|error| format!("failed to run cargo build: {error}"))?;
    if !status.success() {
        return Err("cargo build failed".to_string());
    }

    let binary_name = if target == WINDOWS_TARGET {
        "bran.exe"
    } else {
        "bran"
    };
    let binary_path = root
        .join("target")
        .join(&target)
        .join("release")
        .join(binary_name);
    let binary = fs::read(&binary_path)
        .map_err(|error| format!("binary not found at {}: {error}", binary_path.display()))?;

    let dist = PathBuf::from(dist);
    let dist = if dist.is_absolute() {
        dist
    } else {
        env::current_dir()
            .map_err(|error| format!("cannot resolve --dist: {error}"))?
            .join(dist)
    };
    fs::create_dir_all(&dist).map_err(|error| format!("cannot create --dist dir: {error}"))?;
    let out_path = dist.join(&archive_name);
    let result = if target == WINDOWS_TARGET {
        archive::write_zip(&out_path, binary_name, &binary)
    } else {
        archive::write_tar_gz(&out_path, binary_name, &binary)
    };
    result.map_err(|error| format!("failed to write {}: {error}", out_path.display()))?;
    println!("CREATED {}", out_path.display());
    Ok(())
}

/// Rejects tags that could not be a safe single path component: the tag is
/// part of the artifact filename.
fn validate_tag(tag: &str) -> Result<(), String> {
    if tag.is_empty() || tag == "." || tag == ".." || tag.contains('/') || tag.contains('\\') {
        return Err(format!("invalid tag: {tag:?}"));
    }
    Ok(())
}

/// The workspace root is the parent of the xtask crate; cargo sets
/// CARGO_MANIFEST_DIR whenever the xtask is built, so this works no matter
/// which directory the caller runs `cargo run -p xtask` from.
fn workspace_root() -> Result<PathBuf, String> {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR")
        .ok_or("CARGO_MANIFEST_DIR is not set; run via `cargo run -p xtask`")?;
    let manifest_dir = PathBuf::from(manifest_dir);
    manifest_dir.parent().map(Path::to_path_buf).ok_or_else(|| {
        format!(
            "CARGO_MANIFEST_DIR has no parent: {}",
            manifest_dir.display()
        )
    })
}

fn archive_name(tag: &str, target: &str) -> String {
    if target == WINDOWS_TARGET {
        format!("{tag}-{target}.zip")
    } else {
        format!("{tag}-{target}.tar.gz")
    }
}
