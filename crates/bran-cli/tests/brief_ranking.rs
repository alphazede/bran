//! Issue #60 (DES-04, AM-02, TC-09): `.bran/ranking.yaml` path-class tiers and
//! defining-file signals in `query --brief`, through the real CLI on synthetic
//! fixtures. Every query runs in a fresh copy so no earlier result store is scanned.
use bran_document::canonical::Json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const BRAN: &str = env!("CARGO_BIN_EXE_bran");
const LANTERN: &str = "rotate lantern keys";
const SCRIPT: &str = "tools/rotate-lantern-keys.sh";
// One query term in its path, every term repeated in its body.
const BODY_EVIDENCE: &str = "evidence/lantern/run-0042.md";
// Every query term in its path and repeated in its body: outscores the script.
const PATH_EVIDENCE: &str = "evidence/runs/rotate-lantern-keys-run-0043.md";

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
    /// A fresh copy of `fixtures/recall/<tree>` with the fixture policy and,
    /// when given, `ranking` as `.bran/ranking.yaml`.
    fn new(tree: &str, ranking: Option<&str>) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "bran-brief-ranking-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        copy_tree(&fixture_dir().join(tree), &root);
        fs::create_dir_all(root.join(".bran")).unwrap();
        fs::copy(
            fixture_dir().join("policy.yaml"),
            root.join(".bran/policy.yaml"),
        )
        .unwrap();
        if let Some(text) = ranking {
            fs::write(root.join(".bran/ranking.yaml"), text).unwrap();
        }
        Self(root)
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let output = Command::new(BRAN)
            .env_remove("BRAN_REGISTERED_ROOTS")
            .env_remove("BRAN_ADVISOR_CONFIG")
            .current_dir(&self.0)
            .args(args)
            .output()
            .expect("bran runs");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }

    /// `data.results[].path` of a brief query, in rank order.
    fn brief_paths(&self, question: &str) -> Vec<String> {
        let root = self.0.to_str().unwrap();
        let (code, stdout, stderr) = self.run(&["query", root, "--brief", question]);
        assert_eq!(code, 0, "brief {question:?} exits 0: {stdout}{stderr}");
        let envelope = Json::parse(stdout.as_bytes()).expect("brief output is JSON");
        let Some(Json::Arr(items)) = envelope.get("data").and_then(|d| d.get("results")) else {
            panic!("data.results is an array: {stdout}")
        };
        items
            .iter()
            .map(|item| match item.get("path") {
                Some(Json::Str(path)) => path.clone(),
                other => panic!("result path is a string, got {other:?}"),
            })
            .collect()
    }
}

fn canonical_ranking() -> String {
    fs::read_to_string(fixture_dir().join("ranking.yaml")).unwrap()
}

fn position(paths: &[String], wanted: &str) -> usize {
    paths
        .iter()
        .position(|p| p == wanted)
        .unwrap_or_else(|| panic!("{wanted} is in the brief results: {paths:?}"))
}

/// Every path of a higher tier sorts after every path of a lower tier.
fn assert_tiers_ordered(paths: &[String], tier: impl Fn(&str) -> u8) {
    for (i, earlier) in paths.iter().enumerate() {
        for later in &paths[i + 1..] {
            assert!(
                tier(earlier) <= tier(later),
                "{earlier} (tier {}) precedes {later} (tier {}): {paths:?}",
                tier(earlier),
                tier(later)
            );
        }
    }
}

#[test]
fn brief_defining_script_outranks_body_repeating_evidence_without_ranking_file() {
    // Missing ranking file: every path is tier 0, so the score alone decides.
    // Both files match all three terms; the script has all three in its file
    // name, the evidence one in its path and the rest only as body repeats.
    let repo = Repo::new("ranking-tree", None);
    let paths = repo.brief_paths(LANTERN);
    assert!(
        position(&paths, SCRIPT) < position(&paths, BODY_EVIDENCE),
        "a filename hit outweighs body repeats: {paths:?}"
    );
}

#[test]
fn brief_ranking_tier_keeps_demoted_evidence_below_every_tier_zero_result() {
    let repo = Repo::new("ranking-tree", Some(&canonical_ranking()));
    let paths = repo.brief_paths(LANTERN);
    position(&paths, PATH_EVIDENCE);
    position(&paths, BODY_EVIDENCE);
    assert_eq!(paths[0], SCRIPT, "the only tier-0 match leads: {paths:?}");
    assert_tiers_ordered(&paths, |p| u8::from(p.starts_with("evidence/")));
}

#[test]
fn brief_ranking_tier_puts_defining_doc_above_archived_note() {
    // Fixture question 9: without a ranking file the archived note leads
    // (brief_query.rs); a tier-1 `archive/**` class must put it below every
    // tier-0 match, so the expected defining doc leads.
    let repo = Repo::new("tree", Some(&canonical_ranking()));
    let paths = repo.brief_paths("earlier watering table");
    assert_eq!(paths[0], "docs/watering-schedule.md", "{paths:?}");
    position(&paths, "archive/old-watering-schedule.md");
    assert_tiers_ordered(&paths, |p| u8::from(p.starts_with("archive/")));
}

#[test]
fn brief_ranking_first_matching_class_wins_and_higher_tiers_sort_later() {
    // First match: run-0042 is tier 2, run-0043 tier 1, the script tier 0.
    // Were the last match used, run-0043 would be tier 0 and lead on score.
    let ranking = "classes:\n  - name: lantern-runs\n    tier: 2\n    globs:\n      - \"evidence/lantern/**\"\n  - name: evidence\n    tier: 1\n    globs:\n      - \"evidence/**\"\n  - name: run-logs\n    tier: 0\n    globs:\n      - \"evidence/runs/*.md\"\n";
    let repo = Repo::new("ranking-tree", Some(ranking));
    let paths = repo.brief_paths(LANTERN);
    let at = |p: &str| position(&paths, p);
    assert_eq!(at(SCRIPT), 0, "{paths:?}");
    assert!(at(PATH_EVIDENCE) < at(BODY_EVIDENCE), "{paths:?}");
    assert_tiers_ordered(&paths, |p| match p {
        BODY_EVIDENCE => 2,
        PATH_EVIDENCE => 1,
        _ => 0,
    });
}

#[test]
fn default_query_output_ignores_the_ranking_file() {
    // DES-04: the tier and the new score are brief-only; default values stay.
    let default_data = |ranking: Option<&str>| {
        let repo = Repo::new("ranking-tree", ranking);
        let root = repo.0.to_str().unwrap().to_owned();
        let (code, stdout, stderr) = repo.run(&["query", &root, LANTERN]);
        assert_eq!(code, 0, "default query exits 0: {stdout}{stderr}");
        let envelope = Json::parse(stdout.as_bytes()).expect("default output is JSON");
        let data = envelope.get("data").expect("envelope has data");
        (
            data.get("source_rankings").cloned(),
            data.get("selected_locators").cloned(),
        )
    };
    let without = default_data(None);
    assert!(without.0.is_some(), "default output has source_rankings");
    assert_eq!(default_data(Some(&canonical_ranking())), without);
}

#[test]
fn brief_query_with_malformed_ranking_file_fails_naming_the_file() {
    // A tier outside 0-9 is malformed; the loader's error path reports it
    // instead of silently ranking without the declared classes.
    let ranking =
        "classes:\n  - name: evidence\n    tier: 12\n    globs:\n      - \"evidence/**\"\n";
    let repo = Repo::new("ranking-tree", Some(ranking));
    let root = repo.0.to_str().unwrap().to_owned();
    let (code, stdout, stderr) = repo.run(&["query", &root, "--brief", LANTERN]);
    assert_ne!(code, 0, "malformed ranking file fails: {stdout}");
    assert!(
        format!("{stdout}{stderr}").contains(".bran/ranking.yaml"),
        "the failure names the file: {stdout}{stderr}"
    );
}
