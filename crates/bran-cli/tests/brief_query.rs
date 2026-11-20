//! Issue #60 (DES-01, DES-02, AM-05): `query --brief` and `get --full`
//! exercised through the real CLI on the synthetic recall fixture.
use bran_core::agent::receipt::InlineResult;
use bran_core::agent::result_store::ResultId;
mod support;

use bran_document::canonical::Json;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const BRAN: &str = env!("CARGO_BIN_EXE_bran");
const BRIEF_KEYS: [&str; 6] = [
    "coverage_basis",
    "query_coverage",
    "query_outcome",
    "result_id",
    "results",
    "selection",
];
const LIMIT_CHARS: usize = 2000;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/recall")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

struct Repo(PathBuf);

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Repo {
    fn empty(name: &str, policy: bool) -> Self {
        let root =
            std::env::temp_dir().join(format!("bran-brief-query-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        if policy {
            fs::create_dir_all(root.join(".bran")).unwrap();
            fs::copy(
                fixture_dir().join("policy.yaml"),
                root.join(".bran/policy.yaml"),
            )
            .unwrap();
        }
        Self(root)
    }

    /// The synthetic tree with `.bran/policy.yaml` installed, as a consumer has it.
    fn recall(name: &str, policy: bool) -> Self {
        let repo = Self::empty(name, policy);
        copy_tree(&fixture_dir().join("tree"), &repo.0);
        repo
    }

    /// AM-05: eight files whose exact paths alone exceed the 2,000-char budget.
    fn long_paths(name: &str) -> (Self, BTreeSet<String>) {
        let repo = Self::empty(name, true);
        let mut paths = BTreeSet::new();
        for n in 0..8 {
            let dir = (0..5)
                .map(|k| format!("segment-{n}-{k}-{}", "x".repeat(48)))
                .collect::<Vec<_>>()
                .join("/");
            let path = format!("long/{dir}/lantern-{n}.md");
            fs::create_dir_all(repo.0.join(format!("long/{dir}"))).unwrap();
            fs::write(
                repo.0.join(&path),
                format!("---\ntype: concept\ntitle: Lantern {n}\nokf_status: active\ntags: [developer]\n---\n\n# Lantern {n}\n\nThe lantern note {n}.\n"),
            )
            .unwrap();
            paths.insert(path);
        }
        (repo, paths)
    }

    fn git(&self, args: &[&str]) -> String {
        let mut command = Command::new("git");
        command
            .args([
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .current_dir(&self.0);
        let output = support::output(&mut command).expect("git runs");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8(output.stdout).unwrap()
    }

    /// A Git repository whose result store is ignored, as DES-02 requires of consumers.
    fn git_init(&self) {
        fs::write(self.0.join(".gitignore"), ".bran/results/\n").unwrap();
        self.git(&["init", "-q"]);
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", "fixture"]);
    }

    fn run(&self, args: &[&str]) -> (i32, String) {
        let (code, stdout, _) = self.run_all(args, &[]);
        (code, stdout)
    }

    fn run_all(&self, args: &[&str], envs: &[(&str, &str)]) -> (i32, String, String) {
        let mut cmd = Command::new(BRAN);
        cmd.env_remove("BRAN_REGISTERED_ROOTS")
            .env_remove("BRAN_ADVISOR_CONFIG")
            .env_remove("BRAN_TIMINGS")
            .current_dir(&self.0)
            .args(args);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let output = support::output(&mut cmd).expect("bran runs");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }

    fn brief(&self, extra: &[&str], question: &str) -> (Json, usize) {
        let root = self.0.to_str().unwrap();
        let mut args = vec!["query", root, "--brief"];
        args.extend_from_slice(extra);
        args.push(question);
        let (code, stdout) = self.run(&args);
        assert_eq!(code, 0, "brief query {question:?} exits 0: {stdout}");
        let chars = stdout.trim_end_matches('\n').chars().count();
        (
            Json::parse(stdout.as_bytes()).expect("brief output is JSON"),
            chars,
        )
    }
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

fn results(envelope: &Json) -> Vec<(i64, String, String)> {
    let Some(Json::Arr(items)) = data(envelope).get("results") else {
        panic!("data.results is an array: {envelope:?}")
    };
    items
        .iter()
        .map(|item| {
            assert_eq!(keys(item), BTreeSet::from(["path", "rank", "reason"]));
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

fn warnings(envelope: &Json) -> Vec<String> {
    match envelope.get("warnings") {
        Some(Json::Arr(items)) => items.iter().map(|w| text(Some(w)).to_owned()).collect(),
        _ => Vec::new(),
    }
}

fn questions() -> Vec<(String, String)> {
    fs::read_to_string(fixture_dir().join("questions.jsonl"))
        .unwrap()
        .lines()
        .map(|line| {
            let row = Json::parse(line.as_bytes()).expect("question line is JSON");
            (
                text(row.get("question")).to_owned(),
                text(row.get("expected")).to_owned(),
            )
        })
        .collect()
}

#[test]
fn brief_output_has_only_the_brief_shape_on_every_fixture_question() {
    let repo = Repo::recall("shape", true);
    let questions = questions();
    assert!((6..=10).contains(&questions.len()));
    for (question, _) in &questions {
        let (envelope, chars) = repo.brief(&[], question);
        assert_eq!(text(envelope.get("schema_version")), "1.0.0");
        assert_eq!(text(envelope.get("command")), "query");
        let data = data(&envelope);
        assert_eq!(keys(data), BTreeSet::from(BRIEF_KEYS), "{question:?}");
        assert_eq!(text(data.get("coverage_basis")), "sources");
        let selection = data.get("selection").unwrap();
        assert_eq!(keys(selection), BTreeSet::from(["source"]));
        assert!(!text(selection.get("source")).is_empty());
        let items = results(&envelope);
        assert!(items.len() <= 8, "{question:?}: default top is 8");
        for (index, (rank, _, reason)) in items.iter().enumerate() {
            assert_eq!(*rank, index as i64 + 1, "ranks are 1..n");
            assert!(!reason.is_empty() && !reason.contains('\n'));
            // Under budget the reason keeps the full match reason.
            assert!(
                reason.starts_with("exact:") || reason.starts_with("partial:"),
                "{question:?}: {reason:?}"
            );
        }
        assert!(chars <= LIMIT_CHARS, "{question:?}: {chars} chars");
    }
}

#[test]
fn brief_ranks_the_hand_derived_answer_first() {
    let repo = Repo::recall("order", true);
    // Hand-derived: the first eight fixture questions name their file in
    // path, title and body; question 9 is outranked by the archived note;
    // question 10 matches no term of its expected file.
    let questions = questions();
    for (question, expected) in &questions[..8] {
        let (envelope, _) = repo.brief(&[], question);
        assert_eq!(&results(&envelope)[0].1, expected, "{question:?}");
    }
    let (envelope, _) = repo.brief(&[], &questions[8].0);
    let paths: Vec<_> = results(&envelope).into_iter().map(|r| r.1).collect();
    assert_eq!(
        paths[..2],
        [
            "archive/old-watering-schedule.md",
            "docs/watering-schedule.md"
        ]
    );
    let (envelope, _) = repo.brief(&[], &questions[9].0);
    assert!(results(&envelope).iter().all(|r| r.1 != questions[9].1));
}

#[test]
fn brief_top_bounds_the_result_count() {
    let repo = Repo::recall("top", true);
    // All ten fixture files carry this tag with equal relevance, so all ten
    // pass the selection cut and only the default --top bounds the count.
    let (envelope, _) = repo.brief(&[], "developer");
    assert_eq!(results(&envelope).len(), 8);
    let (envelope, _) = repo.brief(&["--top", "2"], "weekly watering schedule");
    let paths: Vec<_> = results(&envelope).into_iter().map(|r| r.1).collect();
    assert_eq!(
        paths,
        [
            "docs/watering-schedule.md",
            "archive/old-watering-schedule.md"
        ]
    );
}

#[test]
fn brief_keeps_long_paths_exact_and_shortens_reasons_first() {
    let (repo, expected) = Repo::long_paths("long");
    assert!(expected.iter().all(|path| path.len() > 200));
    let path_chars: usize = expected.iter().map(String::len).sum();
    assert!(path_chars > LIMIT_CHARS);
    let (envelope, chars) = repo.brief(&[], "lantern");
    let items = results(&envelope);
    let paths: BTreeSet<_> = items.iter().map(|r| r.1.clone()).collect();
    assert!(!paths.is_empty(), "the top results survive");
    assert!(
        paths.is_subset(&expected),
        "paths are byte-equal, never elided"
    );
    let (full, _) = repo.brief(&["--top", "32"], "lantern");
    let complete = results(&full);
    assert_eq!(complete.len(), expected.len());
    for (kept, expected) in items.iter().zip(&complete) {
        assert_eq!(kept.1, expected.1, "only trailing results may be dropped");
    }
    for (_, _, reason) in &items {
        assert!(
            reason == "exact" || reason == "partial",
            "over budget, reason is cut to the match kind: {reason:?}"
        );
    }
    // Exact paths alone exceed the budget: whole results are dropped from the
    // end and the omission is reported, so the budget holds.
    assert!(chars < LIMIT_CHARS, "{chars} chars");
    let omitted = format!("brief_truncated: {} of 8 results omitted", 8 - paths.len());
    assert!(
        warnings(&envelope).iter().any(|w| w.starts_with(&omitted)),
        "{:?}",
        warnings(&envelope)
    );
}

#[test]
fn brief_zero_match_question_returns_empty_results_and_an_id() {
    let repo = Repo::recall("empty", true);
    let (envelope, _) = repo.brief(&[], "xylophone quasar");
    assert!(results(&envelope).is_empty());
    assert!(!text(data(&envelope).get("result_id")).is_empty());
}

#[test]
fn brief_without_bran_directory_reports_null_result_id() {
    let repo = Repo::recall("nostore", false);
    let (envelope, _) = repo.brief(&[], "valve relay wiring");
    assert_eq!(keys(data(&envelope)), BTreeSet::from(BRIEF_KEYS));
    assert_eq!(data(&envelope).get("result_id"), Some(&Json::Null));
    assert!(!warnings(&envelope).is_empty());
}

#[test]
fn brief_result_id_round_trips_through_get_without_dirtying_git() {
    let repo = Repo::recall("roundtrip", true);
    repo.git_init();
    let before = repo.git(&["status", "--porcelain"]);
    let question = "reset pump pressure alarm";
    let (envelope, _) = repo.brief(&[], question);
    let id = text(data(&envelope).get("result_id")).to_owned();
    let (again, _) = repo.brief(&[], question);
    assert_eq!(
        text(data(&again).get("result_id")),
        id,
        "same question, same ID"
    );
    assert_eq!(repo.git(&["status", "--porcelain"]), before);
    let (code, stdout) = repo.run(&["get", &id]);
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains(question),
        "stored record names the question"
    );
    assert!(stdout.contains("runbooks/pump-pressure-alarm.md"));
}

#[test]
fn get_full_returns_default_output_and_flags_a_stale_revision() {
    let repo = Repo::recall("full", true);
    repo.git_init();
    let (envelope, _) = repo.brief(&[], "frost threshold temperature");
    let id = text(data(&envelope).get("result_id")).to_owned();
    let full = |repo: &Repo| {
        let (code, stdout) = repo.run(&["get", &id, "--full"]);
        assert_eq!(code, 0, "{stdout}");
        Json::parse(stdout.as_bytes()).expect("get --full output is JSON")
    };
    let fresh = full(&repo);
    let Some(Json::Arr(rankings)) = data(&fresh).get("source_rankings") else {
        panic!("get --full carries the default source_rankings: {fresh:?}")
    };
    assert_eq!(text(rankings[0].get("locator")), "docs/frost-protection.md");
    assert!(!warnings(&fresh).iter().any(|w| w.starts_with("stale")));
    fs::write(
        repo.0.join("notes/later.md"),
        "---\ntype: concept\ntitle: Later\nokf_status: active\ntags: [developer]\n---\n\n# Later\n",
    )
    .unwrap();
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "later"]);
    let stale = full(&repo);
    assert!(
        warnings(&stale).iter().any(|w| w == "stale: true"),
        "{:?}",
        warnings(&stale)
    );
}

#[test]
fn get_unknown_result_id_fails() {
    let repo = Repo::recall("unknown", true);
    let (code, _) = repo.run(&["get", "not-a-result-id"]);
    assert_ne!(code, 0);
}

#[test]
fn default_query_keeps_its_data_shape() {
    let repo = Repo::recall("default", true);
    let (code, stdout) = repo.run(&["query", repo.0.to_str().unwrap(), "valve relay wiring"]);
    assert_eq!(code, 0);
    let envelope = Json::parse(stdout.as_bytes()).unwrap();
    assert_eq!(
        keys(data(&envelope)),
        BTreeSet::from([
            "actual_model_input_tokens",
            "candidate_source_bytes",
            "context_bytes_avoided",
            "estimated_tokens",
            "query",
            "query_coverage",
            "query_outcome",
            "root",
            "selected_locators",
            "selected_source_bytes",
            "source_rankings",
            "symbol_navigation",
            "token_estimate_method",
            "why_selected",
        ])
    );
}

#[test]
fn brief_bounds_scan_warnings_and_unmatched_terms_within_budget() {
    let repo = Repo::empty("bounded-warnings", true);
    for i in 0..8 {
        fs::write(
            repo.0.join(format!("doc-{i}.md")),
            format!(
                "---\ntype: concept\ntitle: Lantern {i}\nokf_status: active\ntags: [developer]\n---\n\n# Lantern {i}\n\nLantern calibration guide {i}.\n"
            ),
        )
        .unwrap();
    }
    fs::create_dir_all(repo.0.join("assets")).unwrap();
    for i in 0..200 {
        fs::write(
            repo.0.join(format!("assets/binary-artifact-{i:03}.png")),
            b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR",
        )
        .unwrap();
    }
    let unmatched: Vec<String> = (0..80).map(|i| format!("unmatchedterm{i:03}")).collect();
    let question = format!("lantern {}", unmatched.join(" "));
    let (code, stdout, _) = repo.run_all(
        &["query", repo.0.to_str().unwrap(), "--brief", &question],
        &[],
    );
    assert_eq!(code, 0, "{stdout}");
    let total_code_points = stdout.chars().count();
    assert!(
        total_code_points <= LIMIT_CHARS,
        "complete stdout must be <= {LIMIT_CHARS} code points, got {total_code_points}: {stdout}"
    );
    let envelope = Json::parse(stdout.as_bytes()).expect("brief output is JSON");
    assert_eq!(results(&envelope).len(), 8);
    let warns = warnings(&envelope);
    assert_eq!(warns.len(), 1, "expected single summary warning: {warns:?}");
    assert!(
        warns[0].contains("200") && warns[0].contains("bran get"),
        "expected bounded scan warning summary: {warns:?}"
    );

    let id = text(data(&envelope).get("result_id")).to_owned();
    let (full_code, full_stdout, _) = repo.run_all(&["get", &id, "--full"], &[]);
    assert_eq!(full_code, 0, "{full_stdout}");
    let full_env = Json::parse(full_stdout.as_bytes()).expect("full output is JSON");
    assert!(
        warnings(&full_env).len() >= 200,
        "full query via get --full should expose all scan warnings"
    );
}

#[test]
fn brief_and_stored_record_preserve_paths_over_graph_locator_limit() {
    let repo = Repo::empty("over-1024", true);
    let segments: Vec<String> = (0..11)
        .map(|k| format!("dir-segment-{k:02}-{}", "a".repeat(86)))
        .collect();
    let rel_dir = segments.join("/");
    let rel_path = format!("{rel_dir}/deep-calibration.md");
    assert!(
        rel_path.len() > 1024,
        "test path must exceed 1024 bytes, got {}",
        rel_path.len()
    );
    fs::create_dir_all(repo.0.join(&rel_dir)).unwrap();
    fs::write(
        repo.0.join(&rel_path),
        "# Deep Calibration\n\nUnique body token deepcalibrationsignal.\n",
    )
    .unwrap();

    let (envelope, _) = repo.brief(&[], "deepcalibrationsignal");
    let items = results(&envelope);
    assert_eq!(items.len(), 1, "{envelope:?}");
    assert_eq!(items[0].1, rel_path, "brief result path must be exact");

    let id = text(data(&envelope).get("result_id")).to_owned();
    let (code, get_stdout) = repo.run(&["get", &id]);
    assert_eq!(code, 0, "{get_stdout}");
    let get_env = Json::parse(get_stdout.as_bytes()).expect("get output is JSON");
    let answer_str = text(data(&get_env).get("answer"));
    let answer_json = Json::parse(answer_str.as_bytes()).expect("stored answer is JSON");
    let Some(Json::Arr(stored_results)) = answer_json.get("results") else {
        panic!("stored answer missing results: {answer_json:?}");
    };
    assert_eq!(
        text(stored_results[0].get("path")),
        rel_path,
        "stored answer must preserve exact >1024-byte path"
    );
}

#[test]
fn multi_root_brief_preserves_distinct_root_identity_and_emits_timings() {
    let root_a = Repo::empty("multi-dup-a", true);
    let root_b = Repo::empty("multi-dup-b", true);
    fs::write(
        root_a.0.join("lantern.md"),
        "---\ntype: concept\ntitle: Lantern Alpha\nokf_status: active\ntags: [developer]\n---\n\n# Lantern Alpha\n\nAlpha lantern calibration.\n",
    )
    .unwrap();
    fs::write(
        root_b.0.join("lantern.md"),
        "---\ntype: concept\ntitle: Lantern Beta\nokf_status: active\ntags: [developer]\n---\n\n# Lantern Beta\n\nBeta lantern wiring.\n",
    )
    .unwrap();

    let arg_a = root_a.0.to_str().unwrap();
    let arg_b = root_b.0.to_str().unwrap();
    let (code, stdout, stderr) = root_a.run_all(
        &["query", arg_a, "--add-dir", arg_b, "--brief", "lantern"],
        &[("BRAN_TIMINGS", "1")],
    );
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(
        stderr.contains("BRAN_TIMINGS step=total"),
        "multi-root brief must emit BRAN_TIMINGS lines: {stderr}"
    );

    let envelope = Json::parse(stdout.as_bytes()).expect("multi-root brief is JSON");
    let Some(Json::Arr(items)) = data(&envelope).get("results") else {
        panic!("missing results: {envelope:?}");
    };
    assert_eq!(items.len(), 2);
    let bundles: BTreeSet<&str> = items
        .iter()
        .map(|item| {
            assert_eq!(text(item.get("path")), "lantern.md");
            text(item.get("bundle"))
        })
        .collect();
    assert_eq!(bundles, BTreeSet::from([arg_a, arg_b]));

    let id = text(data(&envelope).get("result_id")).to_owned();
    let (get_code, get_stdout) = root_a.run(&["get", &id]);
    assert_eq!(get_code, 0, "{get_stdout}");
    let get_env = Json::parse(get_stdout.as_bytes()).expect("get output is JSON");
    let answer_json =
        Json::parse(text(data(&get_env).get("answer")).as_bytes()).expect("answer is JSON");
    let Some(Json::Arr(stored_results)) = answer_json.get("results") else {
        panic!("stored answer missing results: {answer_json:?}");
    };
    let stored_bundles: BTreeSet<&str> = stored_results
        .iter()
        .map(|item| text(item.get("bundle")))
        .collect();
    assert_eq!(stored_bundles, BTreeSet::from([arg_a, arg_b]));
}

#[test]
fn get_full_rejects_non_brief_inline_result_via_cli() {
    let repo = Repo::recall("nonbrief-full", true);
    let plain = InlineResult::new("plain answer", ["docs/valve-wiring.md"]).unwrap();
    let payload = plain.encode_canonical();
    let id = ResultId::sha256(&payload);
    let batch_id = ResultId::sha256(id.value().as_bytes());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let batch_dir = repo.0.join(format!(
        ".bran/results/batch-{now:020}-{}",
        batch_id.value()
    ));
    fs::create_dir_all(&batch_dir).unwrap();
    fs::write(batch_dir.join(id.value()), &payload).unwrap();

    let (code, _, stderr) = repo.run_all(&["get", &id.to_string(), "--full"], &[]);
    assert_ne!(code, 0, "non-brief result must reject --full");
    assert!(
        stderr.contains("full_query_unavailable"),
        "expected full_query_unavailable in stderr: {stderr}"
    );
}
