//! Stage-1 deployment profile (docs/deployment-profile.md): the real `bran`
//! binary in registered-root mode against a read-only synthetic repository.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

const BRAN: &str = env!("CARGO_BIN_EXE_bran");
const REGISTRY: &str = "BRAN_REGISTERED_ROOTS";
const CANARY: &str = "OUTSIDE-CANARY-7731";
const SECRET: &str = "SYNTHETIC-SECRET-4411";

/// Runs `bran` with a cleared environment; returns the exit code and the
/// envelope (stdout on success, stderr on error).
fn run(cwd: &Path, registry: Option<&str>, args: &[&str]) -> (i32, String) {
    let mut command = Command::new(BRAN);
    command.env_clear().current_dir(cwd).args(args);
    if let Some(value) = registry {
        command.env(REGISTRY, value);
    }
    let output = command.output().expect("bran runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.code().unwrap_or(-1), text)
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn policy_repository(root: &Path, title: &str) {
    write(&root.join(".bran/policy.yaml"), "schema_version: \"1\"\n");
    write(
        &root.join("docs/ledger-rotation.md"),
        &format!(
            "---\ntype: concept\ntitle: {title}\nokf_status: active\ntags:\n  - deployment\ntimestamp: 2026-09-29T00:00:00Z\nresource: test://ledger-rotation\npublic_boundary: safe\n---\nLedger rotation runs nightly and keeps seven generations. See [the ledger](../src/ledger.rs).\n"
        ),
    );
    write(
        &root.join("src/ledger.rs"),
        "pub fn rotate_ledger(generations: u32) -> u32 {\n    generations.min(7)\n}\n",
    );
}

/// Every path below `root` with its bytes (or link target), sorted.
fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let kind = fs::symlink_metadata(&path).unwrap().file_type();
            let bytes = if kind.is_symlink() {
                fs::read_link(&path)
                    .unwrap()
                    .into_os_string()
                    .into_encoded_bytes()
            } else if kind.is_dir() {
                pending.push(path.clone());
                Vec::new()
            } else {
                fs::read(&path).unwrap()
            };
            entries.push((path, bytes));
        }
    }
    entries.sort();
    entries
}

fn set_modes(root: &Path, directory: u32, file: u32) {
    for (path, _) in tree(root) {
        let kind = fs::symlink_metadata(&path).unwrap().file_type();
        if !kind.is_symlink() {
            let mode = if kind.is_dir() { directory } else { file };
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        }
    }
    fs::set_permissions(root, fs::Permissions::from_mode(directory)).unwrap();
}

fn strace_available(scratch: &Path) -> bool {
    Command::new("strace")
        .arg("-o")
        .arg(scratch.join("strace-check"))
        .arg("true")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn p8_deployment_profile() {
    let base = std::env::temp_dir().join(format!("bran-deployment-{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();
    let base = fs::canonicalize(&base).unwrap();
    let repos = base.join("repos");
    let repo = repos.join("synthetic");
    let other = repos.join("other");
    let state = base.join("state");
    policy_repository(&repo, "Ledger rotation");
    policy_repository(&other, "Other ledger rotation");
    write(
        &base.join("outside/notes.md"),
        &format!("{CANARY} ledger rotation\n"),
    );
    symlink(base.join("outside/notes.md"), repo.join("escape.md")).unwrap();
    symlink(&repo, repos.join("alias")).unwrap();
    fs::create_dir_all(&state).unwrap();
    set_modes(&repo, 0o555, 0o444);
    set_modes(&state, 0o555, 0o444);
    let before = tree(&repo);

    let repo_path = repo.to_str().unwrap();
    let other_path = other.to_str().unwrap();
    let alias_path = repos.join("alias").to_str().unwrap().to_owned();
    let registered = Some(repo_path);

    // Smoke: check, query, and packet against the registered read-only root.
    let (code, output) = run(&state, registered, &["check", repo_path, "bran-strict"]);
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("\"selected_passed\":true"), "{output}");
    let (code, query) = run(
        &state,
        registered,
        &["query", repo_path, "ledger", "rotation"],
    );
    assert_eq!(code, 0, "{query}");
    assert!(query.contains("\"locator\":\"src/ledger.rs\""), "{query}");
    let (code, packet) = run(
        &state,
        registered,
        &["packet", repo_path, "ledger", "rotation"],
    );
    assert_eq!(code, 0, "{packet}");
    for field in [
        "\"payload\":",
        "\"why_selected\":",
        "\"dlp_status\":\"passed\"",
    ] {
        assert!(packet.contains(field), "{field}: {packet}");
    }
    // In-repository symlinks never pull outside content in.
    assert!(!query.contains(CANARY) && !packet.contains(CANARY));
    // Registered-root mode changes no admitted result.
    let (_, default_packet) = run(&state, None, &["packet", repo_path, "ledger", "rotation"]);
    assert_eq!(packet, default_packet);

    let rejected = |registry: Option<&str>, args: &[&str], code: i32, failure: &str| {
        let (actual, output) = run(&state, registry, args);
        assert_eq!(actual, code, "{args:?}: {output}");
        assert!(
            output.contains(&format!("\"{failure}\"")),
            "{args:?}: {output}"
        );
        output
    };

    // Unregistered roots, traversal, alternative spellings, and symlink aliases.
    let traversal = format!("{repo_path}/../synthetic");
    let dot = format!("{repo_path}/.");
    let slash = format!("{repo_path}/");
    let secret_root = format!("{repo_path}?access_token={SECRET}");
    for root in [
        other_path,
        traversal.as_str(),
        dot.as_str(),
        slash.as_str(),
        alias_path.as_str(),
        "repos/synthetic",
        secret_root.as_str(),
    ] {
        for args in [
            vec!["check", root, "bran-strict"],
            vec!["query", root, "ledger"],
            vec!["packet", root, "ledger"],
        ] {
            let output = rejected(registered, &args, 2, "root_not_registered");
            assert!(!output.contains(root), "{output}");
        }
    }
    rejected(
        registered,
        &["query", repo_path, "--add-dir", other_path, "ledger"],
        2,
        "root_not_registered",
    );
    // Cross-namespace: a root registered to another process is not admitted here.
    rejected(
        Some(other_path),
        &["check", repo_path, "bran-strict"],
        2,
        "root_not_registered",
    );

    // The registry itself must be exact canonical directories; otherwise fail closed.
    let nested = format!("{repo_path}:relative");
    let missing = base.join("missing").to_str().unwrap().to_owned();
    for registry in [
        "",
        slash.as_str(),
        alias_path.as_str(),
        "repos/synthetic",
        &nested,
        &missing,
    ] {
        rejected(
            Some(registry),
            &["check", repo_path, "bran-strict"],
            3,
            "registered_roots_invalid",
        );
    }
    assert_eq!(run(&state, Some(""), &["smoke"]).0, 0);

    // Oversized requests and secret reflection.
    let at_limit = "ledger ".repeat(8192 / 7) + &"x".repeat(8192 % 7);
    assert_eq!(at_limit.len(), 8192);
    assert_eq!(
        run(&state, registered, &["query", repo_path, &at_limit]).0,
        0
    );
    let oversized = at_limit.clone() + "x";
    let secret = format!("ledger token={SECRET}");
    for command in ["query", "packet"] {
        rejected(
            registered,
            &[command, repo_path, &oversized],
            2,
            "request_oversized",
        );
        let output = rejected(
            registered,
            &[command, repo_path, &secret],
            2,
            "request_dlp_rejected",
        );
        assert!(!output.contains(SECRET), "{output}");
    }

    // Quota overflow: a registered repository over the scanner's 10,000-file
    // quota fails visibly, never as a partial success.
    let quota = repos.join("quota");
    write(&quota.join(".bran/policy.yaml"), "schema_version: \"1\"\n");
    for index in 0..=10_000 {
        fs::write(quota.join(format!("f{index}")), "").unwrap();
    }
    let quota_path = quota.to_str().unwrap();
    let both = format!("{repo_path}:{quota_path}");
    for command in ["query", "packet"] {
        let (code, output) = run(&state, Some(&both), &[command, quota_path, "ledger"]);
        assert_eq!(code, 3, "{output}");
        assert!(output.contains("LimitExceeded"), "{output}");
        assert!(output.contains("\"status\":\"error\""), "{output}");
    }

    // Commands that write, replace policy, or reach agents are unavailable.
    let result_id = "0".repeat(64);
    for args in [
        vec!["query", repo_path, "--record", "ledger"],
        vec!["check", "--policy-stdin", repo_path, "bran-strict"],
        vec!["maintain", "revalidate", repo_path],
        vec!["evidence", "summarize", repo_path],
        vec!["body-preserved", repo_path, "manifest.json"],
        vec!["-p", "--agent", "offline", "ledger"],
        vec!["get", result_id.as_str()],
        vec!["agents", "list"],
        vec!["doctor", "--agent"],
        vec!["tui"],
    ] {
        rejected(registered, &args, 2, "unavailable_in_registered_root_mode");
    }

    // Default mode is unchanged: no registry, no root restriction.
    let (code, output) = run(&state, None, &["check", other_path, "bran-strict"]);
    assert_eq!(code, 0, "{output}");

    // No listener and no network call, observed at the syscall boundary.
    if strace_available(&base) {
        let trace = base.join("network-trace");
        for args in [
            vec!["check", repo_path, "bran-strict"],
            vec!["query", repo_path, "ledger"],
            vec!["packet", repo_path, "ledger"],
        ] {
            let output = Command::new("strace")
                .args(["-f", "-qq", "-e", "trace=network", "-o"])
                .arg(&trace)
                .arg(BRAN)
                .args(&args)
                .env_clear()
                .env(REGISTRY, repo_path)
                .current_dir(&state)
                .output()
                .unwrap();
            assert!(output.status.success(), "{args:?}");
            let calls = fs::read_to_string(&trace).unwrap();
            assert!(
                calls.trim().is_empty(),
                "{args:?} network syscalls:\n{calls}"
            );
        }
    } else {
        eprintln!("network syscall trace unavailable: strace cannot run here");
    }

    // Nothing was written to the repository or the working directory.
    assert_eq!(tree(&repo), before);
    assert!(fs::read_dir(&state).unwrap().next().is_none());

    set_modes(&repo, 0o755, 0o644);
    set_modes(&state, 0o755, 0o644);
    fs::remove_dir_all(&base).unwrap();
}
