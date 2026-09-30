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
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

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

/// Runs real git with no user or system config and no hooks.
fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", directory)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "BRAN test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "BRAN test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(["-c", "init.defaultBranch=main"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// The envelope without its registered-root `source_revisions` field.
fn without_revisions(envelope: &str) -> String {
    let start = envelope
        .find(",\"source_revisions\":[")
        .expect("source revisions");
    let end = start + envelope[start..].find(']').unwrap() + 1;
    format!("{}{}", &envelope[..start], &envelope[end..])
}

fn unavailable_revision(root: &str, reason: &str) -> String {
    format!(
        "{{\"root\":\"{root}\",\"kind\":\"git-head\",\"status\":\"unavailable\",\"ref\":null,\"value\":null,\"reason\":\"{reason}\"}}"
    )
}

/// A fresh canonical scratch directory for one test.
fn scratch(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("bran-deployment-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::canonicalize(&base).unwrap()
}

/// Like `run`, but gives up after `seconds`; `None` means the command hung.
fn run_within(
    cwd: &Path,
    registry: Option<&str>,
    args: &[&str],
    seconds: u64,
) -> Option<(i32, String)> {
    let mut command = Command::new(BRAN);
    command
        .env_clear()
        .current_dir(cwd)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(value) = registry {
        command.env(REGISTRY, value);
    }
    let mut child = command.spawn().expect("bran starts");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Some((output.status.code().unwrap_or(-1), text))
}

/// The readiness rule in docs/deployment-profile.md: exit 0, envelope status
/// `ok`, no failures, and no warning other than an unmatched readiness term or
/// a skipped in-repository symlink.
fn ready(code: i32, envelope: &str) -> bool {
    let Some(start) = envelope.find("\"warnings\":[") else {
        return false;
    };
    let rest = &envelope[start + "\"warnings\":[".len()..];
    let Some(end) = rest.find("],\"failures\":[]") else {
        return false;
    };
    let warnings = rest[..end].trim_matches('"');
    code == 0
        && envelope
            .starts_with("{\"schema_version\":\"1.0.0\",\"command\":\"query\",\"status\":\"ok\",")
        && (warnings.is_empty()
            || warnings.split("\",\"").all(|warning| {
                warning.starts_with("unmatched_query_terms:") || warning.starts_with("Symlink {")
            }))
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
    // Registered-root mode adds only the source revision; a root that is not
    // a git checkout is reported unavailable, never guessed.
    let not_git = format!(
        "\"source_revisions\":[{}]",
        unavailable_revision(repo_path, "not_a_git_checkout")
    );
    for envelope in [&output, &query, &packet] {
        assert!(envelope.contains(&not_git), "{envelope}");
    }
    for (envelope, args) in [
        (&output, vec!["check", repo_path, "bran-strict"]),
        (&query, vec!["query", repo_path, "ledger", "rotation"]),
        (&packet, vec!["packet", repo_path, "ledger", "rotation"]),
    ] {
        let (_, default_output) = run(&state, None, &args);
        assert!(!default_output.contains("source_revisions"));
        assert_eq!(without_revisions(envelope), default_output);
    }

    // A registered git checkout reports the commit its HEAD names, read from
    // git metadata without running git.
    let tracked = repos.join("tracked");
    policy_repository(&tracked, "Tracked ledger rotation");
    git(&tracked, &["init", "-q"]);
    git(&tracked, &["add", "."]);
    git(&tracked, &["commit", "-q", "-m", "tracked"]);
    let commit = git(&tracked, &["rev-parse", "HEAD"]);
    let tracked_path = tracked.to_str().unwrap();
    let tracked_registry = format!("{repo_path}:{tracked_path}");
    let attested = format!(
        "{{\"root\":\"{tracked_path}\",\"kind\":\"git-head\",\"status\":\"attested\",\"ref\":\"refs/heads/main\",\"value\":\"{commit}\",\"reason\":null}}"
    );
    let (code, output) = run(
        &state,
        Some(&tracked_registry),
        &["check", tracked_path, "bran-strict"],
    );
    assert_eq!(code, 0, "{output}");
    assert!(
        output.contains(&format!("\"source_revisions\":[{attested}]")),
        "{output}"
    );
    let (code, output) = run(
        &state,
        Some(&tracked_registry),
        &["query", repo_path, "--add-dir", tracked_path, "ledger"],
    );
    assert_eq!(code, 0, "{output}");
    let both = format!(
        "\"source_revisions\":[{},{attested}]",
        unavailable_revision(repo_path, "not_a_git_checkout")
    );
    assert!(output.contains(&both), "{output}");

    let rejected = |registry: Option<&str>, args: &[&str], code: i32, failure: &str| {
        let (actual, output) = run(&state, registry, args);
        // Name only the command: later arguments can carry the synthetic secret under test.
        let command = args.first().copied().unwrap_or_default();
        // Messages omit the output too: it derives from arguments that carry the secret.
        assert_eq!(actual, code, "{command} exited {actual}, expected {code}");
        assert!(
            output.contains(&format!("\"{failure}\"")),
            "{command}: output lacks failure {failure}"
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
    rejected(Some(""), &["smoke"], 3, "registered_roots_invalid");

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

// Review finding 5: an empty or invalid registry refuses every command, and a
// valid registry admits only check, query, and packet.
#[test]
fn registered_mode_admits_only_check_query_packet() {
    let base = scratch("allowlist");
    let repo = base.join("repo");
    policy_repository(&repo, "Ledger rotation");
    symlink(&repo, base.join("alias")).unwrap();
    let repo_path = repo.to_str().unwrap();
    let alias_path = base.join("alias").to_str().unwrap().to_owned();
    let commands: [&[&str]; 7] = [
        &["smoke"],
        &["help"],
        &["--help"],
        &["-h"],
        &["--version"],
        &["-V"],
        &["check", repo_path, "bran-strict"],
    ];
    for registry in ["", alias_path.as_str(), "relative"] {
        for args in commands {
            let (code, output) = run(&base, Some(registry), args);
            assert_eq!(code, 3, "{registry:?} {args:?}: {output}");
            assert!(output.contains("\"registered_roots_invalid\""), "{output}");
        }
    }
    for args in &commands[..6] {
        let (code, output) = run(&base, Some(repo_path), args);
        assert_eq!(code, 2, "{args:?}: {output}");
        assert!(
            output.contains("\"unavailable_in_registered_root_mode\""),
            "{output}"
        );
    }
    assert_eq!(run(&base, Some(repo_path), commands[6]).0, 0);
    // Default mode keeps every command.
    for args in &commands[..6] {
        assert_eq!(run(&base, None, args).0, 0, "{args:?}");
    }
    fs::remove_dir_all(base).unwrap();
}

// Review finding 6: error envelopes of an admitted root keep its source revision.
#[test]
fn error_envelopes_keep_source_revisions() {
    let base = scratch("error-provenance");
    let quota = base.join("quota");
    write(&quota.join(".bran/policy.yaml"), "schema_version: \"1\"\n");
    write(&quota.join(".git/HEAD"), &format!("{COMMIT}\n"));
    for index in 0..=10_000 {
        fs::write(quota.join(format!("f{index}")), "").unwrap();
    }
    let quota_path = quota.to_str().unwrap();
    let revision = format!(
        "\"provenance\":{{\"source_revisions\":[{{\"root\":\"{quota_path}\",\"kind\":\"git-head\",\"status\":\"attested\",\"ref\":null,\"value\":\"{COMMIT}\",\"reason\":null}}]}}"
    );
    for command in ["query", "packet"] {
        let (code, output) = run(&base, Some(quota_path), &[command, quota_path, "ledger"]);
        assert_eq!(code, 3, "{output}");
        assert!(output.contains("LimitExceeded"), "{output}");
        assert!(output.contains(&revision), "{command}: {output}");
    }
    let broken = base.join("broken-policy");
    write(&broken.join(".bran/policy.yaml"), "schema_version: \"9\"\n");
    let broken_path = broken.to_str().unwrap();
    let (code, output) = run(
        &base,
        Some(broken_path),
        &["check", broken_path, "bran-strict"],
    );
    assert_eq!(code, 2, "{output}");
    assert!(
        output.contains(&format!(
            "\"provenance\":{{\"source_revisions\":[{}]}}",
            unavailable_revision(broken_path, "not_a_git_checkout")
        )),
        "{output}"
    );
    fs::remove_dir_all(base).unwrap();
}

// Review finding 7: readiness needs the envelope, not only the exit code.
#[test]
fn readiness_rule_rejects_policy_and_quota_gaps() {
    let base = scratch("readiness");
    let good = base.join("good");
    policy_repository(&good, "Ledger rotation");
    symlink(base.join("elsewhere"), good.join("link.md")).unwrap();
    let plain = base.join("plain");
    fs::create_dir_all(&plain).unwrap();
    let oversized = base.join("oversized");
    policy_repository(&oversized, "Ledger rotation");
    fs::write(oversized.join("huge.txt"), vec![b'a'; 2 * 1024 * 1024]).unwrap();
    let quota = base.join("quota");
    write(&quota.join(".bran/policy.yaml"), "schema_version: \"1\"\n");
    for index in 0..=10_000 {
        fs::write(quota.join(format!("f{index}")), "").unwrap();
    }
    for (root, expected) in [
        (&good, true),
        (&plain, false),
        (&oversized, false),
        (&quota, false),
    ] {
        let root = root.to_str().unwrap();
        let (code, output) = run(&base, Some(root), &["query", root, "readiness"]);
        assert_eq!(
            ready(code, &output),
            expected,
            "{root}: exit {code} {output}"
        );
    }
    fs::remove_dir_all(base).unwrap();
}

// Review findings 2 and 3 through the CLI: an outside HEAD is not attested,
// and a FIFO HEAD does not block the request.
#[test]
fn registered_revision_is_confined_and_never_blocks() {
    let base = scratch("revision-safety");
    let repo = base.join("repo");
    policy_repository(&repo, "Ledger rotation");
    write(&base.join("outside-head"), &format!("{COMMIT}\n"));
    fs::create_dir_all(repo.join(".git")).unwrap();
    symlink(base.join("outside-head"), repo.join(".git/HEAD")).unwrap();
    let repo_path = repo.to_str().unwrap();
    let unsafe_head = format!(
        "\"source_revisions\":[{}]",
        unavailable_revision(repo_path, "git_metadata_unsafe")
    );
    let (code, output) = run(&base, Some(repo_path), &["query", repo_path, "ledger"]);
    assert_eq!(code, 0, "{output}");
    assert!(output.contains(&unsafe_head), "{output}");
    assert!(!output.contains(COMMIT), "{output}");

    fs::remove_file(repo.join(".git/HEAD")).unwrap();
    assert!(Command::new("mkfifo")
        .arg(repo.join(".git/HEAD"))
        .status()
        .unwrap()
        .success());
    let (code, output) = run_within(&base, Some(repo_path), &["query", repo_path, "ledger"], 10)
        .expect("registered query with a FIFO HEAD completes");
    assert_eq!(code, 0, "{output}");
    assert!(output.contains(&unsafe_head), "{output}");
    fs::remove_dir_all(base).unwrap();
}
