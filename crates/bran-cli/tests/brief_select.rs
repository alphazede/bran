//! Issue #60 selection step (DES-07, AM-06, AM-07; TC-13, TC-14, TC-15):
//! the deterministic rule and the caller-configured advisor, exercised
//! through `bran query --brief` only.
//!
//! Advisor protocol pinned here:
//! - BRAN reads the advisor configuration only from the file named by the
//!   caller's `BRAN_ADVISOR_CONFIG`, never from the queried tree.
//! - Configuration lines: `advisor: <absolute path>`, `sha256: <lowercase hex
//!   of the executable bytes>`, `timeout_ms: <n>`, `send_fields: [..]` (subset
//!   of question, path, reason, rank, excerpt), optional `requires_env: <NAME>`,
//!   optional `pass_env: [NAME, ..]`.
//! - The child gets a cleared environment plus only the `pass_env` names that
//!   are set in BRAN's environment.
//! - Stdin: `{"question": .., "options": [{..}, ..]}`, one option per rule
//!   path in rule order (the whole rule shortlist, before `--top`), each
//!   carrying its `id` plus only the `send_fields` keys.
//! - Stdout: `{"paths": [..]}`, a reorder or subset (no duplicates, at least
//!   one) of the rule paths, or `{"scores": [{"id": .., "score": 0..1}, ..]}`,
//!   which orders by score descending (ties and unscored keep rule order,
//!   unscored last). Anything else, a non-zero exit, oversized
//!   stdout, or no answer within `timeout_ms` (child killed) keeps the rule
//!   answer with `selection.source` "rule", exit 0 and no warning in `data`.
mod support;

use bran_document::canonical::Json;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const BRAN: &str = env!("CARGO_BIN_EXE_bran");
const BRIEF_KEYS: [&str; 6] = [
    "coverage_basis",
    "query_coverage",
    "query_outcome",
    "result_id",
    "results",
    "selection",
];
const ALL_FIELDS: &str = "[question, path, reason, rank]";
// Three words: a weak one-word body mention scores below 0.30 of the strong
// documents, which match all three (two in path and title, one in body).
const QUESTION: &str = "heron lantern wick";

fn advisor_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/advisor/advisor.sh")
        .canonicalize()
        .expect("fixture advisor exists")
}

fn sha256_hex(path: &Path) -> String {
    bran_document::canonical::sha256_hex(&fs::read(path).unwrap())
}

fn doc(title: &str, body: &str) -> String {
    format!("---\ntype: concept\ntitle: {title}\nokf_status: active\ntags: [developer]\n---\n\n# {title}\n\n{body}\n")
}

/// A temporary query tree plus a sibling directory for caller-scoped files.
struct Repo {
    base: PathBuf,
    root: PathBuf,
    caller: PathBuf,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

impl Repo {
    /// Two equal strong matches (a tie the rule keeps) and two weak mentions
    /// far below half of the top score.
    fn new(name: &str) -> Self {
        let base =
            std::env::temp_dir().join(format!("bran-brief-select-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("tree");
        let caller = base.join("caller");
        fs::create_dir_all(root.join(".bran")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::create_dir_all(&caller).unwrap();
        // DES-02: consumers git-ignore the result store; without it later runs rank stored results.
        fs::write(root.join(".gitignore"), ".bran/results/\n").unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/recall/policy.yaml"),
            root.join(".bran/policy.yaml"),
        )
        .unwrap();
        let strong = "The heron lantern guides night boats. Heron lantern wicks need trimming; the heron lantern glass needs cleaning.";
        fs::write(
            root.join("docs/heron-lantern-north.md"),
            doc("Heron Lantern North", strong),
        )
        .unwrap();
        fs::write(
            root.join("docs/heron-lantern-south.md"),
            doc("Heron Lantern South", strong),
        )
        .unwrap();
        fs::write(
            root.join("notes/harbour-log.md"),
            doc(
                "Harbour Log",
                "Boats were counted at dawn. A heron was seen near the pier.",
            ),
        )
        .unwrap();
        fs::write(
            root.join("notes/supply-list.md"),
            doc("Supply List", "Rope, tar, paint and one spare lantern."),
        )
        .unwrap();
        Self { base, root, caller }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.caller.join(name)
    }

    /// Writes a caller-scoped advisor configuration outside the queried tree.
    fn config(&self, name: &str, lines: &[String]) -> PathBuf {
        let path = self.file(name);
        fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    fn reply(&self, body: &str) -> PathBuf {
        let path = self.file("reply.json");
        fs::write(&path, body).unwrap();
        path
    }

    fn command(&self, config: Option<&Path>, env: &[(&str, &Path)], extra: &[&str]) -> Command {
        let mut command = Command::new(BRAN);
        command
            .env_remove("BRAN_REGISTERED_ROOTS")
            .env_remove("BRAN_ADVISOR_CONFIG")
            .current_dir(&self.root)
            .args(["query", self.root.to_str().unwrap(), "--brief"])
            .args(extra)
            .arg(QUESTION);
        if let Some(config) = config {
            command.env("BRAN_ADVISOR_CONFIG", config);
        }
        for (key, value) in env {
            command.env(key, value);
        }
        command
    }

    /// Runs a brief query; asserts exit 0 and the brief data shape (no warning in data).
    fn brief_with(&self, mut command: Command) -> (Json, String) {
        let output = support::output(&mut command).expect("bran runs");
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "brief query exits 0: {stdout}{stderr}"
        );
        let envelope = Json::parse(stdout.as_bytes()).expect("brief output is JSON");
        assert_eq!(keys(data(&envelope)), BTreeSet::from(BRIEF_KEYS));
        (envelope, stdout + &stderr)
    }

    fn brief(&self, config: Option<&Path>, env: &[(&str, &Path)]) -> Json {
        self.brief_with(self.command(config, env, &[])).0
    }

    fn default_locators(&self) -> Vec<String> {
        let mut command = Command::new(BRAN);
        command
            .env_remove("BRAN_REGISTERED_ROOTS")
            .env_remove("BRAN_ADVISOR_CONFIG")
            .current_dir(&self.root)
            .args(["query", self.root.to_str().unwrap(), QUESTION]);
        let output = support::output(&mut command).expect("bran runs");
        let envelope = Json::parse(&output.stdout).expect("default output is JSON");
        let Some(Json::Arr(items)) = data(&envelope).get("source_rankings") else {
            panic!("default query has source_rankings")
        };
        items
            .iter()
            .map(|item| text(item.get("locator")).to_owned())
            .collect()
    }
}

fn advisor_lines(sha256: &str, timeout_ms: u64, fields: &str, pass_env: &[&str]) -> Vec<String> {
    vec![
        format!("advisor: {}", advisor_script().display()),
        format!("sha256: {sha256}"),
        format!("timeout_ms: {timeout_ms}"),
        format!("send_fields: {fields}"),
        format!("pass_env: [{}]", pass_env.join(", ")),
    ]
}

fn good_lines(pass_env: &[&str]) -> Vec<String> {
    advisor_lines(&sha256_hex(&advisor_script()), 5000, ALL_FIELDS, pass_env)
}

fn data(envelope: &Json) -> &Json {
    envelope.get("data").expect("envelope has data")
}

fn keys(value: &Json) -> BTreeSet<&str> {
    let Json::Obj(map) = value else {
        panic!("expected object, got {value:?}")
    };
    map.keys().map(String::as_str).collect()
}

fn text(value: Option<&Json>) -> &str {
    match value {
        Some(Json::Str(s)) => s,
        other => panic!("expected string, got {other:?}"),
    }
}

fn source(envelope: &Json) -> &str {
    text(
        data(envelope)
            .get("selection")
            .and_then(|s| s.get("source")),
    )
}

/// (rank, path, reason) for each result.
fn results(envelope: &Json) -> Vec<(i64, String, String)> {
    let Some(Json::Arr(items)) = data(envelope).get("results") else {
        panic!("data.results is an array: {envelope:?}")
    };
    items
        .iter()
        .map(|item| {
            let Some(Json::Int(rank)) = item.get("rank") else {
                panic!("rank is an integer")
            };
            (
                *rank,
                text(item.get("path")).to_owned(),
                text(item.get("reason")).to_owned(),
            )
        })
        .collect()
}

fn paths(envelope: &Json) -> Vec<String> {
    results(envelope).into_iter().map(|r| r.1).collect()
}

fn reply_json(paths: &[&str]) -> String {
    let quoted: Vec<String> = paths.iter().map(|p| format!("\"{p}\"")).collect();
    format!("{{\"paths\":[{}]}}", quoted.join(","))
}

const NORTH: &str = "docs/heron-lantern-north.md";
const SOUTH: &str = "docs/heron-lantern-south.md";

// TC-13: rule keeps the top result plus each next result with score >= ratio x top.
#[test]
fn rule_keeps_the_tied_strong_matches_and_drops_weak_mentions() {
    let repo = Repo::new("rule-ratio");
    let candidates = repo.default_locators();
    assert!(
        candidates.iter().any(|p| p.starts_with("notes/")),
        "weak documents are ranked candidates: {candidates:?}"
    );
    let envelope = repo.brief(None, &[]);
    assert_eq!(source(&envelope), "rule");
    let kept: BTreeSet<String> = paths(&envelope).into_iter().collect();
    assert_eq!(
        kept,
        BTreeSet::from([NORTH.to_owned(), SOUTH.to_owned()]),
        "rule keeps the tie and drops results below the ratio"
    );
}

// TC-13: output for --top N is a prefix of the rule answer (which itself is an ordered
// subsequence of the ranked shortlist across tiers), minimum 1, and runs are identical.
#[test]
fn rule_answer_is_a_deterministic_prefix_under_top() {
    let repo = Repo::new("rule-prefix");
    let (full, _) = repo.brief_with(repo.command(None, &[], &["--top", "32"]));
    let (again, _) = repo.brief_with(repo.command(None, &[], &["--top", "32"]));
    assert_eq!(data(&full), data(&again), "two runs give identical data");
    let all = paths(&full);
    assert!(!all.is_empty());
    for n in 1..=all.len() {
        let (top, _) = repo.brief_with(repo.command(None, &[], &["--top", &n.to_string()]));
        assert_eq!(paths(&top), all[..n].to_vec(), "--top {n} is a prefix");
        assert_eq!(source(&top), "rule");
    }
}

// TC-13 negative: an empty ranking is an empty selection, not an error.
#[test]
fn rule_with_no_candidates_answers_empty() {
    let repo = Repo::new("rule-empty");
    let mut command = Command::new(BRAN);
    command
        .env_remove("BRAN_REGISTERED_ROOTS")
        .env_remove("BRAN_ADVISOR_CONFIG")
        .current_dir(&repo.root)
        .args(["query", repo.root.to_str().unwrap(), "--brief", "zeppelin"]);
    let (envelope, _) = repo.brief_with(command);
    assert!(results(&envelope).is_empty());
    assert_eq!(source(&envelope), "rule");
}

// TC-14 valid_reorder: a reorder and a subset of the rule paths apply, source advisor.
#[test]
fn advisor_reorder_and_subset_of_rule_paths_apply() {
    let repo = Repo::new("advisor-valid");
    let rule = repo.brief(None, &[]);
    let rule_items = results(&rule);
    let config = repo.config(
        "advisor.yaml",
        &good_lines(&["ADVISOR_REPLY", "ADVISOR_MARKER"]),
    );
    let marker = repo.file("marker");

    let reply = repo.reply(&reply_json(&[SOUTH, NORTH]));
    let advised = repo.brief(
        Some(&config),
        &[("ADVISOR_REPLY", &reply), ("ADVISOR_MARKER", &marker)],
    );
    assert!(marker.exists(), "the caller's advisor ran");
    assert_eq!(source(&advised), "advisor");
    let items = results(&advised);
    assert_eq!(paths(&advised), vec![SOUTH.to_owned(), NORTH.to_owned()]);
    for (index, (rank, path, reason)) in items.iter().enumerate() {
        assert_eq!(*rank, index as i64 + 1, "ranks follow the advised order");
        let rule_reason = &rule_items.iter().find(|r| &r.1 == path).unwrap().2;
        assert_eq!(reason, rule_reason, "reason comes from the rule result");
    }

    let reply = repo.reply(&reply_json(&[NORTH]));
    let subset = repo.brief(Some(&config), &[("ADVISOR_REPLY", &reply)]);
    assert_eq!(source(&subset), "advisor");
    assert_eq!(paths(&subset), vec![NORTH.to_owned()]);
}

// TC-14 protocol: stdin carries the question and one option per rule path with send_fields only.
#[test]
fn advisor_request_carries_only_send_fields() {
    let repo = Repo::new("advisor-stdin");
    let rule_paths = paths(&repo.brief(None, &[]));
    let config = repo.config(
        "advisor.yaml",
        &advisor_lines(
            &sha256_hex(&advisor_script()),
            5000,
            "[question, path]",
            &["ADVISOR_STDIN", "ADVISOR_REPLY"],
        ),
    );
    let stdin = repo.file("stdin.json");
    let reply = repo.reply(&reply_json(&[NORTH]));
    repo.brief(
        Some(&config),
        &[("ADVISOR_STDIN", &stdin), ("ADVISOR_REPLY", &reply)],
    );
    let request = Json::parse(&fs::read(&stdin).expect("advisor read stdin")).unwrap();
    let options: Vec<String> = rule_paths
        .iter()
        .enumerate()
        .map(|(i, p)| format!("{{\"id\":\"{}\",\"path\":\"{p}\"}}", i + 1))
        .collect();
    let expected = format!(
        "{{\"question\":\"{QUESTION}\",\"options\":[{}]}}",
        options.join(",")
    );
    assert_eq!(request, Json::parse(expected.as_bytes()).unwrap());
}

/// Runs one fault configuration and asserts the rule answer is kept.
fn assert_rule_kept(repo: &Repo, rule: &Json, config: &Path, env: &[(&str, &Path)], what: &str) {
    let envelope = repo.brief(Some(config), env);
    assert_eq!(source(&envelope), "rule", "{what}: source stays rule");
    assert_eq!(
        data(&envelope).get("results"),
        data(rule).get("results"),
        "{what}: rule answer unchanged"
    );
}

// TC-14 malformed, nonzero_exit, path_outside_rule, oversized_stdout: rule answer kept.
#[test]
fn advisor_faults_keep_the_rule_answer() {
    let repo = Repo::new("advisor-faults");
    let rule = repo.brief(None, &[]);
    let config = repo.config(
        "advisor.yaml",
        &good_lines(&["ADVISOR_REPLY", "ADVISOR_MODE"]),
    );
    let good = reply_json(&[SOUTH, NORTH]);
    let bad_replies = [
        ("not json", "malformed stdout"),
        ("", "empty stdout"),
        ("{\"paths\":[]}", "empty paths"),
        ("{\"paths\":\"docs\"}", "paths not a list"),
        (&reply_json(&[NORTH, NORTH]) as &str, "duplicate path"),
        (
            &reply_json(&[SOUTH, "notes/harbour-log.md"]) as &str,
            "path outside the rule set",
        ),
        (
            &reply_json(&[SOUTH, "docs/invented.md"]) as &str,
            "invented path",
        ),
    ];
    for (body, what) in bad_replies {
        let reply = repo.reply(body);
        assert_rule_kept(&repo, &rule, &config, &[("ADVISOR_REPLY", &reply)], what);
    }
    let reply = repo.reply(&good);
    let exit_mode = PathBuf::from("exit");
    assert_rule_kept(
        &repo,
        &rule,
        &config,
        &[("ADVISOR_REPLY", &reply), ("ADVISOR_MODE", &exit_mode)],
        "non-zero exit with a valid reply",
    );
    let big_mode = PathBuf::from("big");
    assert_rule_kept(
        &repo,
        &rule,
        &config,
        &[("ADVISOR_REPLY", &reply), ("ADVISOR_MODE", &big_mode)],
        "valid reply padded past the stdout bound",
    );
}

// TC-14 sleep_past_budget: the query returns within timeout_ms + 2000 ms, the child
// process tree (both direct child and helper child) is killed, and the direct child is reaped.
#[test]
#[cfg(target_os = "linux")]
fn advisor_past_deadline_is_killed_and_rule_answers() {
    let repo = Repo::new("advisor-timeout");
    let rule = repo.brief(None, &[]);
    let baseline = (0..2)
        .map(|_| {
            let start = Instant::now();
            repo.brief(None, &[]);
            start.elapsed()
        })
        .max()
        .unwrap();
    let timeout_ms = 300;
    let config = repo.config(
        "advisor.yaml",
        &advisor_lines(
            &sha256_hex(&advisor_script()),
            timeout_ms,
            ALL_FIELDS,
            &["ADVISOR_MARKER", "ADVISOR_HELPER_MARKER", "ADVISOR_MODE"],
        ),
    );
    let marker = repo.file("marker");
    let helper_marker = repo.file("helper-marker");
    let sleep = PathBuf::from("sleep");
    let start = Instant::now();
    let envelope = repo.brief(
        Some(&config),
        &[
            ("ADVISOR_MARKER", &marker),
            ("ADVISOR_HELPER_MARKER", &helper_marker),
            ("ADVISOR_MODE", &sleep),
        ],
    );
    let elapsed = start.elapsed();
    assert_eq!(source(&envelope), "rule");
    assert_eq!(data(&envelope).get("results"), data(&rule).get("results"));
    assert!(
        elapsed <= baseline + Duration::from_millis(timeout_ms + 2000),
        "returned in {elapsed:?}; baseline {baseline:?}, timeout {timeout_ms} ms"
    );
    let pid = fs::read_to_string(&marker).expect("advisor started");
    let proc_dir = PathBuf::from(format!("/proc/{}", pid.trim()));
    assert!(
        !proc_dir.exists(),
        "direct advisor child {} should be reaped by bran",
        pid.trim()
    );
    let helper_pid = fs::read_to_string(&helper_marker).expect("advisor spawned helper child");
    let helper_stat = PathBuf::from(format!("/proc/{}/stat", helper_pid.trim()));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let alive = fs::read_to_string(&helper_stat)
            .map(|s| !s.contains(") Z "))
            .unwrap_or(false);
        if !alive {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "advisor helper child {} still running",
            helper_pid.trim()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

// TC-14 digest_mismatch, missing_executable, requires_env_unset, absent or unreadable config:
// nothing is spawned and the rule answers.
#[test]
fn advisor_not_spawned_without_digest_executable_or_required_variable() {
    let repo = Repo::new("advisor-nospawn");
    let rule = repo.brief(None, &[]);
    let marker = repo.file("marker");
    let reply = repo.reply(&reply_json(&[SOUTH, NORTH]));
    let env: [(&str, &Path); 2] = [("ADVISOR_MARKER", &marker), ("ADVISOR_REPLY", &reply)];
    let pass = ["ADVISOR_MARKER", "ADVISOR_REPLY"];

    let mismatch = repo.config(
        "mismatch.yaml",
        &advisor_lines(&"0".repeat(64), 5000, ALL_FIELDS, &pass),
    );
    assert_rule_kept(&repo, &rule, &mismatch, &env, "digest mismatch");
    assert!(!marker.exists(), "digest mismatch spawns nothing");

    let mut missing_lines = good_lines(&pass);
    missing_lines[0] = format!("advisor: {}", repo.file("no-such-advisor.sh").display());
    let missing = repo.config("missing.yaml", &missing_lines);
    assert_rule_kept(&repo, &rule, &missing, &env, "missing executable");

    let mut required_lines = good_lines(&pass);
    required_lines.push("requires_env: BRAN_FIXTURE_REQUIRED".to_owned());
    let required = repo.config("required.yaml", &required_lines);
    let mut command = repo.command(Some(&required), &env, &[]);
    command.env_remove("BRAN_FIXTURE_REQUIRED");
    let (envelope, _) = repo.brief_with(command);
    assert_eq!(source(&envelope), "rule");
    assert!(!marker.exists(), "requires_env unset spawns nothing");
    let mut command = repo.command(Some(&required), &env, &[]);
    command.env("BRAN_FIXTURE_REQUIRED", "1");
    let (envelope, _) = repo.brief_with(command);
    assert!(marker.exists(), "requires_env set: the advisor runs");
    assert_eq!(source(&envelope), "advisor");
    fs::remove_file(&marker).unwrap();

    let absent = repo.file("no-such-config.yaml");
    assert_rule_kept(&repo, &rule, &absent, &env, "unreadable config");
    assert!(!marker.exists(), "unreadable config spawns nothing");
}

// TC-14 pass_env_isolation: the child sees exactly the pass_env names set in the parent,
// and no variable value reaches stdout, stderr or the result record.
#[test]
#[cfg(target_os = "linux")]
fn advisor_environment_is_only_the_set_pass_env_names() {
    let repo = Repo::new("advisor-env");
    let config = repo.config(
        "advisor.yaml",
        &good_lines(&[
            "ADVISOR_ENV_OUT",
            "ADVISOR_REPLY",
            "BRAN_FIXTURE_PASSED",
            "BRAN_FIXTURE_NOT_SET",
        ]),
    );
    let names_file = repo.file("env-names");
    let reply = repo.reply(&reply_json(&[NORTH]));
    let passed = PathBuf::from("passed-value-7f3a");
    let decoy = PathBuf::from("decoy-value-91c2");
    let mut command = repo.command(
        Some(&config),
        &[
            ("ADVISOR_ENV_OUT", &names_file),
            ("ADVISOR_REPLY", &reply),
            ("BRAN_FIXTURE_PASSED", &passed),
            ("BRAN_FIXTURE_DECOY", &decoy),
        ],
        &[],
    );
    command.env_remove("BRAN_FIXTURE_NOT_SET");
    let (envelope, output) = repo.brief_with(command);
    assert_eq!(source(&envelope), "advisor");
    let names: BTreeSet<String> = fs::read_to_string(&names_file)
        .expect("advisor wrote its environment names")
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "ADVISOR_ENV_OUT".to_owned(),
            "ADVISOR_REPLY".to_owned(),
            "BRAN_FIXTURE_PASSED".to_owned(),
        ]),
        "no HOME, PATH, decoy or unset name reaches the child"
    );
    let mut record = String::new();
    for entry in fs::read_dir(repo.root.join(".bran/results")).expect("result store") {
        let path = entry.unwrap().path();
        if path.is_file() {
            record.push_str(&String::from_utf8_lossy(&fs::read(path).unwrap()));
        }
    }
    for value in ["passed-value-7f3a", "decoy-value-91c2"] {
        assert!(!output.contains(value), "{value} not in stdout or stderr");
        assert!(!record.contains(value), "{value} not in the result record");
    }
}

/// AM-07: a cloned tree carries advisor-shaped files naming the real fixture
/// advisor with its correct digest; only the caller's configuration counts.
fn plant_hostile_files(repo: &Repo) {
    let hostile = good_lines(&["ADVISOR_MARKER", "ADVISOR_REPLY"]).join("\n") + "\n";
    for name in [
        ".bran/selection.yaml",
        ".bran/advisor.yaml",
        ".bran/advisor.yml",
    ] {
        fs::write(repo.root.join(name), &hostile).unwrap();
    }
}

// TC-14 hostile_repository_unset and TC-15 run (d): in-tree advisor files never spawn.
#[test]
fn hostile_tree_advisor_files_never_spawn_without_caller_config() {
    let repo = Repo::new("hostile-unset");
    plant_hostile_files(&repo);
    let marker = repo.file("marker");
    let reply = repo.reply(&reply_json(&[SOUTH]));
    let envelope = repo.brief(
        None,
        &[("ADVISOR_MARKER", &marker), ("ADVISOR_REPLY", &reply)],
    );
    assert!(!marker.exists(), "no child process from in-tree files");
    assert_eq!(source(&envelope), "rule");
    assert_eq!(
        paths(&envelope).into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([NORTH.to_owned(), SOUTH.to_owned()])
    );
}

// TC-14 hostile_repository_with_caller_config: only the caller's advisor runs.
#[test]
fn hostile_tree_advisor_files_ignored_with_caller_config() {
    let repo = Repo::new("hostile-caller");
    plant_hostile_files(&repo);
    let config = repo.config(
        "caller-advisor.yaml",
        &good_lines(&["ADVISOR_STDIN", "ADVISOR_REPLY"]),
    );
    let marker = repo.file("marker");
    let stdin = repo.file("stdin.json");
    let reply = repo.reply(&reply_json(&[SOUTH, NORTH]));
    let envelope = repo.brief(
        Some(&config),
        &[
            ("ADVISOR_MARKER", &marker),
            ("ADVISOR_STDIN", &stdin),
            ("ADVISOR_REPLY", &reply),
        ],
    );
    assert!(stdin.exists(), "the caller's advisor ran");
    assert!(
        !marker.exists(),
        "the in-tree configuration (which passes ADVISOR_MARKER) did not apply"
    );
    assert_eq!(source(&envelope), "advisor");
    assert_eq!(paths(&envelope), vec![SOUTH.to_owned(), NORTH.to_owned()]);
}

// P4-R05 (TC-13): --top applies after the rule filters the bounded shortlist, so a weak
// tier-0 candidate between a strong tier-0 and strong tier-1 does not drop the tier-1 hit.
#[test]
fn rule_cross_tier_applies_filter_before_top() {
    let repo = Repo::new("cross-tier");
    fs::remove_file(repo.root.join(NORTH)).unwrap();
    fs::remove_file(repo.root.join(SOUTH)).unwrap();
    fs::create_dir_all(repo.root.join("archive")).unwrap();
    fs::write(
        repo.root.join(".bran/ranking.yaml"),
        "classes:\n  - name: archived\n    tier: 1\n    globs:\n      - \"archive/**\"\n",
    )
    .unwrap();
    let strong = "The heron lantern guides night boats. Heron lantern wicks need trimming; the heron lantern glass needs cleaning.";
    let primary = "docs/heron-lantern-primary.md";
    let archived = "archive/heron-lantern-old.md";
    fs::write(
        repo.root.join(primary),
        doc("Heron Lantern Primary", strong),
    )
    .unwrap();
    fs::write(repo.root.join(archived), doc("Heron Lantern Old", strong)).unwrap();

    let (top3, _) = repo.brief_with(repo.command(None, &[], &["--top", "3"]));
    let (top2, _) = repo.brief_with(repo.command(None, &[], &["--top", "2"]));
    let (top1, _) = repo.brief_with(repo.command(None, &[], &["--top", "1"]));
    assert_eq!(paths(&top3), vec![primary.to_owned(), archived.to_owned()]);
    assert_eq!(
        paths(&top2),
        vec![primary.to_owned(), archived.to_owned()],
        "--top 2 must keep both qualifying cross-tier results"
    );
    assert_eq!(paths(&top1), vec![primary.to_owned()]);
    assert_eq!(source(&top2), "rule");
}

// P4-R02 (DES-07, AM-07): config inside the primary root, inside an added root, or reached
// via a symlink pointing into a queried root must fall back silently without spawning.
#[test]
#[cfg(unix)]
fn advisor_config_inside_queried_tree_or_symlink_never_spawns() {
    let repo = Repo::new("config-inside-tree");
    let rule = repo.brief(None, &[]);
    let marker = repo.file("marker");
    let reply = repo.reply(&reply_json(&[SOUTH, NORTH]));
    let env: [(&str, &Path); 2] = [("ADVISOR_MARKER", &marker), ("ADVISOR_REPLY", &reply)];
    let lines = good_lines(&["ADVISOR_MARKER", "ADVISOR_REPLY"]).join("\n") + "\n";

    let in_primary = repo.root.join(".bran/caller-advisor.yaml");
    fs::write(&in_primary, &lines).unwrap();
    assert_rule_kept(
        &repo,
        &rule,
        &in_primary,
        &env,
        "config directly inside primary root",
    );
    assert!(!marker.exists(), "in-primary config must not spawn");

    let symlink_to_primary = repo.file("link-to-primary.yaml");
    std::os::unix::fs::symlink(&in_primary, &symlink_to_primary).unwrap();
    assert_rule_kept(
        &repo,
        &rule,
        &symlink_to_primary,
        &env,
        "symlinked config resolving inside primary root",
    );
    assert!(!marker.exists(), "symlink into primary root must not spawn");

    let added = Repo::new("config-inside-added");
    let in_added = added.root.join("advisor.yaml");
    fs::write(&in_added, &lines).unwrap();
    let symlink_to_added = repo.file("link-to-added.yaml");
    std::os::unix::fs::symlink(&in_added, &symlink_to_added).unwrap();

    for (cfg, what) in [
        (&in_added, "config directly inside added root"),
        (&symlink_to_added, "symlinked config inside added root"),
    ] {
        let mut command = Command::new(BRAN);
        command
            .env_remove("BRAN_REGISTERED_ROOTS")
            .env("BRAN_ADVISOR_CONFIG", cfg)
            .env("ADVISOR_MARKER", &marker)
            .env("ADVISOR_REPLY", &reply)
            .current_dir(&repo.root)
            .args([
                "query",
                repo.root.to_str().unwrap(),
                "--add-dir",
                added.root.to_str().unwrap(),
                "--brief",
                QUESTION,
            ]);
        let (envelope, _) = repo.brief_with(command);
        assert_eq!(source(&envelope), "rule", "{what}: source stays rule");
        assert!(!marker.exists(), "{what}: must not spawn");
    }
}

// P4-R04: an incomplete stdin request write (child closes stdin early while returning a valid
// reply and exit 0) must be rejected so BRAN keeps the rule answer.
#[test]
fn advisor_incomplete_stdin_write_keeps_rule_answer() {
    let repo = Repo::new("advisor-incomplete-stdin");
    let config = repo.config(
        "advisor.yaml",
        &good_lines(&["ADVISOR_REPLY", "ADVISOR_MODE"]),
    );
    let reply = repo.reply(&reply_json(&[SOUTH, NORTH]));
    let close_stdin = PathBuf::from("close_stdin");
    // Exceed the 64 KiB pipe buffer across multiple argv words (each below MAX_ARG_STRLEN)
    // so write_all cannot succeed before exec <&- closes stdin.
    let chunk = "lantern ".repeat(4_096);
    let mut rule_args = vec!["query", repo.root.to_str().unwrap(), "--brief", QUESTION];
    rule_args.extend(std::iter::repeat_n(chunk.as_str(), 4));

    let mut rule_cmd = Command::new(BRAN);
    rule_cmd
        .env_remove("BRAN_REGISTERED_ROOTS")
        .env_remove("BRAN_ADVISOR_CONFIG")
        .current_dir(&repo.root)
        .args(&rule_args);
    let (rule, _) = repo.brief_with(rule_cmd);

    let mut adv_cmd = Command::new(BRAN);
    adv_cmd
        .env_remove("BRAN_REGISTERED_ROOTS")
        .env("BRAN_ADVISOR_CONFIG", &config)
        .env("ADVISOR_REPLY", &reply)
        .env("ADVISOR_MODE", &close_stdin)
        .current_dir(&repo.root)
        .args(&rule_args);
    let (envelope, _) = repo.brief_with(adv_cmd);
    assert_eq!(source(&envelope), "rule");
    assert_eq!(data(&envelope).get("results"), data(&rule).get("results"));
}

// P4-R06: multi-root queries with identical relative paths send unambiguous candidate ids,
// fall back to rule on an ambiguous bare-path reply, and apply an unambiguous reorder.
#[test]
fn advisor_multi_root_identity_and_ambiguous_reply_fallback() {
    let root_a = Repo::new("multi-id-a");
    let root_b = Repo::new("multi-id-b");
    let doc_path = "docs/shared-heron-lantern.md";
    let strong = "The heron lantern guides night boats. Heron lantern wicks need trimming; the heron lantern glass needs cleaning. Extra heron lantern beacon.";
    for repo in [&root_a, &root_b] {
        fs::remove_file(repo.root.join(NORTH)).unwrap();
        fs::remove_file(repo.root.join(SOUTH)).unwrap();
        fs::write(repo.root.join(doc_path), doc("Shared Lantern", strong)).unwrap();
    }

    let arg_a = root_a.root.to_str().unwrap();
    let arg_b = root_b.root.to_str().unwrap();
    let config = root_a.config(
        "advisor.yaml",
        &good_lines(&["ADVISOR_STDIN", "ADVISOR_REPLY"]),
    );
    let stdin_file = root_a.file("stdin.json");
    let ambiguous_reply = root_a.reply(&reply_json(&[doc_path]));

    let mut cmd = Command::new(BRAN);
    cmd.env_remove("BRAN_REGISTERED_ROOTS")
        .env("BRAN_ADVISOR_CONFIG", &config)
        .env("ADVISOR_STDIN", &stdin_file)
        .env("ADVISOR_REPLY", &ambiguous_reply)
        .current_dir(&root_a.root)
        .args(["query", arg_a, "--add-dir", arg_b, "--brief", QUESTION]);
    let (ambiguous, _) = root_a.brief_with(cmd);
    assert_eq!(
        source(&ambiguous),
        "rule",
        "ambiguous bare path in multi-root must fall back to rule"
    );
    let Some(Json::Arr(ambig_results)) = data(&ambiguous).get("results") else {
        panic!("missing results");
    };
    assert_eq!(ambig_results.len(), 2);

    let req = Json::parse(&fs::read(&stdin_file).expect("advisor read stdin")).unwrap();
    let Some(Json::Arr(options)) = req.get("options") else {
        panic!("stdin options array");
    };
    assert_eq!(options.len(), 2);
    let ids: Vec<&str> = options.iter().map(|opt| text(opt.get("id"))).collect();
    let bundles: Vec<&str> = options.iter().map(|opt| text(opt.get("bundle"))).collect();
    assert_ne!(ids[0], ids[1], "candidate ids must be distinct");
    assert_eq!(bundles, vec![arg_a, arg_b]);

    let reordered_reply = root_a.reply(&reply_json(&[ids[1], ids[0]]));
    let mut cmd2 = Command::new(BRAN);
    cmd2.env_remove("BRAN_REGISTERED_ROOTS")
        .env("BRAN_ADVISOR_CONFIG", &config)
        .env("ADVISOR_REPLY", &reordered_reply)
        .current_dir(&root_a.root)
        .args(["query", arg_a, "--add-dir", arg_b, "--brief", QUESTION]);
    let (advised, _) = root_a.brief_with(cmd2);
    assert_eq!(source(&advised), "advisor");
    let Some(Json::Arr(adv_results)) = data(&advised).get("results") else {
        panic!("missing results");
    };
    assert_eq!(adv_results.len(), 2);
    assert_eq!(text(adv_results[0].get("bundle")), arg_b);
    assert_eq!(text(adv_results[1].get("bundle")), arg_a);
}

// P4-R09: independent test oracle at the exact 64 KiB stdout bound (accepted at 65,536
// bytes, rejected at 65,537 bytes even with valid JSON).
#[test]
fn advisor_stdout_byte_limit_accepts_at_bound_and_rejects_over_bound() {
    let repo = Repo::new("advisor-stdout-bound");
    let rule = repo.brief(None, &[]);
    let config = repo.config("advisor.yaml", &good_lines(&["ADVISOR_REPLY"]));
    let base = reply_json(&[SOUTH, NORTH]);
    const LIMIT: usize = 64 * 1024;
    assert!(base.len() < LIMIT);

    let at_limit = format!("{base}{}", " ".repeat(LIMIT - base.len()));
    assert_eq!(at_limit.len(), LIMIT);
    let reply_at = repo.reply(&at_limit);
    let accepted = repo.brief(Some(&config), &[("ADVISOR_REPLY", &reply_at)]);
    assert_eq!(source(&accepted), "advisor");
    assert_eq!(paths(&accepted), vec![SOUTH.to_owned(), NORTH.to_owned()]);

    let over_limit = format!("{base}{}", " ".repeat(LIMIT + 1 - base.len()));
    assert_eq!(over_limit.len(), LIMIT + 1);
    let reply_over = repo.reply(&over_limit);
    assert_rule_kept(
        &repo,
        &rule,
        &config,
        &[("ADVISOR_REPLY", &reply_over)],
        "valid JSON reply 1 byte over stdout limit",
    );
}

fn scores_json(scores: &[(&str, &str)]) -> String {
    let entries: Vec<String> = scores
        .iter()
        .map(|(id, score)| format!("{{\"id\":\"{id}\",\"score\":{score}}}"))
        .collect();
    format!("{{\"scores\":[{}]}}", entries.join(","))
}

fn request_options(stdin: &Path) -> Vec<Json> {
    let request = Json::parse(&fs::read(stdin).expect("advisor read stdin")).unwrap();
    let Some(Json::Arr(options)) = request.get("options") else {
        panic!("stdin options array: {request:?}")
    };
    options.clone()
}

/// Leading lines then query-term lines, joined by newlines (the excerpt rule for short files).
fn whole_file_excerpt(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap();
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines.join("\n")
}

// Slice D item 1: `excerpt` sends a bounded file excerpt per option, only when listed.
#[test]
fn advisor_excerpt_is_sent_only_when_listed() {
    let repo = Repo::new("advisor-excerpt");
    // A long candidate: past the leading lines only lines with query terms are kept.
    let mut body = String::from("The heron lantern guides night boats. Heron lantern wicks need trimming; the heron lantern glass needs cleaning.\n");
    for n in 0..40 {
        body.push_str(&format!(
            "\nUnrelated filler paragraph {n} about rope and tar.\n"
        ));
        body.push_str(&format!("\nHeron lantern note {n}: wick ñ é ü trimmed.\n"));
    }
    fs::write(repo.root.join(SOUTH), doc("Heron Lantern South", &body)).unwrap();
    let rule_paths = paths(&repo.brief(None, &[]));
    assert!(rule_paths.contains(&NORTH.to_owned()) && rule_paths.contains(&SOUTH.to_owned()));

    let stdin = repo.file("stdin.json");
    let reply = repo.reply(&reply_json(&[NORTH]));
    let env: [(&str, &Path); 2] = [("ADVISOR_STDIN", &stdin), ("ADVISOR_REPLY", &reply)];
    let with_excerpt = repo.config(
        "with.yaml",
        &advisor_lines(
            &sha256_hex(&advisor_script()),
            5000,
            "[question, path, excerpt]",
            &["ADVISOR_STDIN", "ADVISOR_REPLY"],
        ),
    );
    assert_eq!(source(&repo.brief(Some(&with_excerpt), &env)), "advisor");
    let options = request_options(&stdin);
    assert_eq!(options.len(), rule_paths.len());
    for option in &options {
        assert_eq!(
            keys(option),
            BTreeSet::from(["excerpt", "id", "path"]),
            "{option:?}"
        );
        let path = text(option.get("path"));
        let excerpt = text(option.get("excerpt"));
        assert!(excerpt.len() <= 900, "{path}: {} bytes", excerpt.len());
        if path == NORTH {
            assert_eq!(excerpt, whole_file_excerpt(&repo.root.join(NORTH)));
        } else if path == SOUTH {
            assert!(excerpt.starts_with("---\ntype: concept\ntitle: Heron Lantern South"));
            assert!(excerpt.contains("Heron lantern note 0: wick ñ é ü trimmed."));
            assert!(
                !excerpt.contains("filler paragraph 1 "),
                "non-matching lines dropped"
            );
            assert!(
                excerpt.len() > 850,
                "the bound is filled: {}",
                excerpt.len()
            );
        }
    }

    // Negative: without `excerpt` in send_fields no file text leaves the process.
    let without = repo.config(
        "without.yaml",
        &good_lines(&["ADVISOR_STDIN", "ADVISOR_REPLY"]),
    );
    assert_eq!(source(&repo.brief(Some(&without), &env)), "advisor");
    let raw = fs::read_to_string(&stdin).unwrap();
    assert!(!raw.contains("excerpt"), "no excerpt key: {raw}");
    assert!(!raw.contains("wicks need trimming"), "no file text: {raw}");
    assert!(!raw.contains("concept"), "no frontmatter text: {raw}");
}

// Slice D item 2: a per-id score answer orders by score, ties and unscored keep rule order.
#[test]
fn advisor_scores_order_the_rule_candidates() {
    let repo = Repo::new("advisor-scores");
    let rule = repo.brief(None, &[]);
    let rule_items = results(&rule);
    let rule_paths = paths(&rule);
    assert_eq!(rule_paths.len(), 2);
    let reversed = vec![rule_paths[1].clone(), rule_paths[0].clone()];
    let config = repo.config("advisor.yaml", &good_lines(&["ADVISOR_REPLY"]));

    for (scores, expected, what) in [
        (
            vec![("1", "0.25"), ("2", "0.9")],
            &reversed,
            "descending score",
        ),
        (
            vec![("2", "1"), ("1", "0")],
            &reversed,
            "integer bounds 0 and 1",
        ),
        (
            vec![("1", "0.5"), ("2", "0.5")],
            &rule_paths,
            "tie keeps rule order",
        ),
        (
            vec![("2", "0.1")],
            &reversed,
            "unscored candidates follow scored ones",
        ),
    ] {
        let reply = repo.reply(&scores_json(&scores));
        let advised = repo.brief(Some(&config), &[("ADVISOR_REPLY", &reply)]);
        assert_eq!(source(&advised), "advisor", "{what}");
        assert_eq!(&paths(&advised), expected, "{what}");
        for (index, (rank, path, reason)) in results(&advised).iter().enumerate() {
            assert_eq!(
                *rank,
                index as i64 + 1,
                "{what}: ranks follow the advised order"
            );
            let rule_reason = &rule_items.iter().find(|r| &r.1 == path).unwrap().2;
            assert_eq!(
                reason, rule_reason,
                "{what}: reason comes from the rule result"
            );
        }
    }
}

// Slice D item 2 negatives: every invalid score answer keeps the rule answer.
#[test]
fn advisor_score_faults_keep_the_rule_answer() {
    let repo = Repo::new("advisor-score-faults");
    let rule = repo.brief(None, &[]);
    let config = repo.config(
        "advisor.yaml",
        &good_lines(&["ADVISOR_REPLY", "ADVISOR_MODE"]),
    );
    let bad_replies = [
        (scores_json(&[("2", "0.9"), ("9", "0.5")]), "unknown id"),
        (scores_json(&[("2", "0.9"), ("2", "0.1")]), "duplicate id"),
        (scores_json(&[("2", "1.5")]), "score above 1"),
        (scores_json(&[("2", "-0.1")]), "negative score"),
        (scores_json(&[("2", "1e999")]), "non-finite score"),
        (scores_json(&[("2", "NaN")]), "NaN score"),
        (scores_json(&[("2", "\"0.9\"")]), "score as a string"),
        (scores_json(&[("2", ".9")]), "malformed number"),
        ("{\"scores\":[]}".to_owned(), "empty scores"),
        ("{\"scores\":{\"2\":0.9}}".to_owned(), "scores not a list"),
        (
            format!("{{\"paths\":[\"{NORTH}\"],\"scores\":[{{\"id\":\"2\",\"score\":0.9}}]}}"),
            "both paths and scores",
        ),
    ];
    for (body, what) in &bad_replies {
        let reply = repo.reply(body);
        assert_rule_kept(&repo, &rule, &config, &[("ADVISOR_REPLY", &reply)], what);
    }
    let good = scores_json(&[("2", "0.9"), ("1", "0.1")]);
    let reply = repo.reply(&good);
    assert_eq!(
        source(&repo.brief(Some(&config), &[("ADVISOR_REPLY", &reply)])),
        "advisor",
        "the control reply is accepted"
    );
    for mode in ["big", "exit"] {
        let mode_value = PathBuf::from(mode);
        assert_rule_kept(
            &repo,
            &rule,
            &config,
            &[("ADVISOR_REPLY", &reply), ("ADVISOR_MODE", &mode_value)],
            &format!("valid scores with mode {mode}"),
        );
    }
}

// Slice D item 3: the advisor sees the whole rule shortlist before --top truncates it
// (`select::apply` advises `rule_selected`; the callers truncate to --top afterwards),
// so it can promote a candidate into the returned set.
#[test]
fn advisor_sees_the_rule_shortlist_beyond_top() {
    let repo = Repo::new("advisor-shortlist");
    let (rule, _) = repo.brief_with(repo.command(None, &[], &["--top", "32"]));
    let rule_paths = paths(&rule);
    assert_eq!(rule_paths.len(), 2);
    let config = repo.config(
        "advisor.yaml",
        &good_lines(&["ADVISOR_STDIN", "ADVISOR_REPLY"]),
    );
    let stdin = repo.file("stdin.json");
    for body in [
        scores_json(&[("2", "0.8"), ("1", "0.2")]),
        reply_json(&[&rule_paths[1]]),
    ] {
        let reply = repo.reply(&body);
        let env: [(&str, &Path); 2] = [("ADVISOR_STDIN", &stdin), ("ADVISOR_REPLY", &reply)];
        let (advised, _) = repo.brief_with(repo.command(Some(&config), &env, &["--top", "1"]));
        let sent: Vec<String> = request_options(&stdin)
            .iter()
            .map(|o| text(o.get("path")).to_owned())
            .collect();
        assert_eq!(sent, rule_paths, "advisor receives the full rule shortlist");
        assert_eq!(source(&advised), "advisor");
        assert_eq!(paths(&advised), vec![rule_paths[1].clone()], "{body}");
    }
}
