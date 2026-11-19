//! Brief selection and ranking regressions, each built from a small synthetic
//! repository and run through the real CLI.
use bran_document::canonical::Json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const BRAN: &str = env!("CARGO_BIN_EXE_bran");
const LIMIT_CHARS: usize = 2000;

struct Repo(PathBuf);

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Repo {
    /// An empty repository with the fixture policy, then `files` written in.
    fn with(files: &[(&str, &str)]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "bran-brief-defects-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".bran")).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/recall/policy.yaml"),
            root.join(".bran/policy.yaml"),
        )
        .unwrap();
        let repo = Self(root);
        for (path, text) in files {
            repo.write(path, text);
        }
        repo
    }

    fn write(&self, path: &str, text: &str) {
        let target = self.0.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, text).unwrap();
    }

    /// Exit code and stdout followed by stderr.
    fn query(&self, extra: &[&str], question: &str) -> (i32, String) {
        let output = Command::new(BRAN)
            .env_remove("BRAN_REGISTERED_ROOTS")
            .env_remove("BRAN_ADVISOR_CONFIG")
            .current_dir(&self.0)
            .args(["query", self.0.to_str().unwrap(), "--brief"])
            .args(extra)
            .arg(question)
            .output()
            .expect("bran runs");
        let mut text = String::from_utf8(output.stdout).unwrap();
        text.push_str(&String::from_utf8(output.stderr).unwrap());
        (output.status.code().unwrap_or(-1), text)
    }

    fn paths(&self, extra: &[&str], question: &str) -> Vec<String> {
        let (code, stdout) = self.query(extra, question);
        assert_eq!(code, 0, "brief {question:?} exits 0: {stdout}");
        result_paths(&stdout)
    }
}

fn result_paths(stdout: &str) -> Vec<String> {
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

/// Row 1: the selection cut measures the same relevance as the sort, so an
/// exact title field on the leader no longer drops a filename-and-body match.
#[test]
fn selection_cut_uses_the_sort_relevance_measure() {
    let repo = Repo::with(&[
        ("opal.md", "---\ntitle: opal\n---\nopal\n"),
        ("opal-guide.txt", "opal opal opal\n"),
    ]);
    assert_eq!(repo.paths(&[], "opal"), ["opal.md", "opal-guide.txt"]);
}

/// Row 2: an ignored word alone does not admit a document when the query has
/// a content word; a stop-word-only query still matches as before.
#[test]
fn ignored_word_alone_does_not_admit_a_document() {
    let files = [("incidental.txt", "which\n"), ("other.txt", "misc\n")];
    // Separate repositories: a stored brief result is itself scannable text.
    assert!(Repo::with(&files).paths(&[], "which quuxneedle").is_empty());
    assert_eq!(Repo::with(&files).paths(&[], "which"), ["incidental.txt"]);
}

/// Row 3: the strongest location weight counts; a weaker filename substring
/// does not lower a whole directory-token match, a stronger one still wins.
#[test]
fn strongest_location_weight_wins_over_coincident_filename_substring() {
    let repo = Repo::with(&[("a/port/airport.txt", ""), ("z/port/memo.txt", "")]);
    assert_eq!(
        repo.paths(&[], "port"),
        ["a/port/airport.txt", "z/port/memo.txt"]
    );
    repo.write("m/port/port.txt", "");
    assert_eq!(repo.paths(&[], "port")[0], "m/port/port.txt");
}

/// Row 4: filename morphology counts even when the body has the literal word.
/// Under the fielded scorer the filename field outweighs a lone body mention
/// enough that the body-only file may fall under the selection cut.
#[test]
fn filename_stem_counts_independently_of_body_hits() {
    let repo = Repo::with(&[("a.txt", "indexed\n"), ("index.txt", "indexed\n")]);
    assert_eq!(repo.paths(&[], "indexed")[0], "index.txt");
}

/// Row 4: query words and document tokens normalize the same way.
#[test]
fn morphology_normalizes_query_and_tokens_symmetrically() {
    let repo = Repo::with(&[("manual.txt", "study\n")]);
    assert_eq!(repo.paths(&[], "studies"), ["manual.txt"]);
    let repo = Repo::with(&[("manual.txt", "studies\n")]);
    assert_eq!(repo.paths(&[], "study"), ["manual.txt"]);
}

/// Row 4: a stem never matches inside an unrelated longer word.
#[test]
fn stem_does_not_match_inside_an_unrelated_word() {
    let repo = Repo::with(&[("setup.txt", "Install the tool\n")]);
    assert!(repo.paths(&[], "stalled").is_empty());
}

/// Row 4: a stem match scores below the exact token in the same location.
#[test]
fn stem_match_scores_below_exact_token() {
    let repo = Repo::with(&[("index.txt", ""), ("indexed.txt", "")]);
    assert_eq!(repo.paths(&[], "indexed"), ["indexed.txt", "index.txt"]);
}

/// Row 5: score components do not saturate into equality, so the document
/// covering one more term still ranks first on a very long query.
#[test]
fn wide_score_components_keep_ordering_at_saturation() {
    let terms: Vec<String> = (0..1400).map(|i| format!("z{i:04}")).collect();
    let repeat = |words: &[String]| {
        words
            .iter()
            .flat_map(|w| [w.as_str(); 3])
            .collect::<Vec<_>>()
            .join(" ")
    };
    let repo = Repo::with(&[
        ("alpha.txt", &repeat(&terms[..1399])),
        ("zulu.txt", &repeat(&terms)),
    ]);
    for (i, term) in terms.iter().take(100).enumerate() {
        repo.write(&format!("distractor{i:03}.txt"), term);
    }
    let paths = repo.paths(&["--top", "2"], &terms.join(" "));
    assert_eq!(paths, ["zulu.txt", "alpha.txt"]);
}

/// Row 6 (trust boundary): the ranking parser rejects a key indented under a
/// scalar sibling instead of silently accepting it.
#[test]
fn ranking_parser_rejects_misaligned_sibling_key() {
    let repo = Repo::with(&[("widget.txt", "widget\n")]);
    repo.write(
        ".bran/ranking.yaml",
        "classes:\n  - name: nested\n        tier: 1\n    globs: [\"**/*.txt\"]\n",
    );
    let (code, stdout) = repo.query(&[], "widget");
    assert_eq!(code, 3, "misaligned key is rejected: {stdout}");
    assert!(stdout.contains("invalid indentation"), "{stdout}");
}

/// Row 6: an apostrophe inside a plain scalar is an ordinary character.
#[test]
fn ranking_parser_accepts_apostrophe_in_plain_scalar() {
    let repo = Repo::with(&[("widget.txt", "widget\n")]);
    repo.write(
        ".bran/ranking.yaml",
        "classes:\n  - name: team's-docs\n    tier: 1\n    globs: [\"**/*.txt\"]\n",
    );
    let (code, stdout) = repo.query(&[], "widget");
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("[team's-docs]"), "{stdout}");
}

/// Row 7: brief output stays within its character budget, keeping whole
/// results from the top and reporting how many were omitted.
#[test]
fn brief_output_respects_budget_and_reports_truncation() {
    let stem = format!("very-{}", "long-".repeat(15));
    let files: Vec<(String, &str)> = (1..=32)
        .map(|i| (format!("{stem}{i:02}.txt"), "unicorn\n"))
        .collect();
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), *t)).collect();
    let repo = Repo::with(&refs);
    let (code, stdout) = repo.query(&["--top", "32"], "unicorn");
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.chars().count() <= LIMIT_CHARS,
        "{} chars: {stdout}",
        stdout.chars().count()
    );
    let paths = result_paths(&stdout);
    assert!(!paths.is_empty() && paths.len() < 32, "{paths:?}");
    let expected: Vec<String> = (1..=paths.len())
        .map(|i| format!("{stem}{i:02}.txt"))
        .collect();
    assert_eq!(paths, expected, "whole results from the top");
    let omitted = format!(
        "brief_truncated: {} of 32 results omitted",
        32 - paths.len()
    );
    assert!(stdout.contains(&omitted), "{stdout}");
}

fn description_line(path: &str, text: &str, description: &str) -> String {
    format!(
        "{{\"path\":\"{path}\",\"sha256\":\"{}\",\"description\":\"{description}\"}}\n",
        bran_document::canonical::sha256_hex(text.as_bytes())
    )
}

/// Sidecar: a description adds words to its file only while the recorded
/// hash matches; a changed file drops it with a stale warning.
#[test]
fn description_sidecar_applies_only_while_hash_matches() {
    let repo = Repo::with(&[("a.txt", "alpha\n"), ("b.txt", "beta\n")]);
    repo.write(
        ".bran/descriptions.jsonl",
        &description_line("b.txt", "beta\n", "zephyr rotation"),
    );
    assert_eq!(repo.paths(&[], "zephyr"), ["b.txt"]);
    repo.write("b.txt", "beta changed\n");
    let (code, stdout) = repo.query(&[], "zephyr");
    assert_eq!(code, 0, "{stdout}");
    // The first query's stored result is scannable text and may match.
    assert!(
        !result_paths(&stdout).contains(&"b.txt".to_owned()),
        "{stdout}"
    );
    assert!(
        stdout.contains("descriptions_ignored: 0 rejected, 1 stale"),
        "{stdout}"
    );
}

/// Sidecar trust boundary: out-of-tree paths, bad hashes, extra keys and
/// malformed lines are rejected one by one without failing the query.
#[test]
fn description_sidecar_rejects_bad_lines_without_failing() {
    let repo = Repo::with(&[("a.txt", "alpha\n")]);
    let good = description_line("a.txt", "alpha\n", "quillwort");
    let hash = bran_document::canonical::sha256_hex(b"alpha\n");
    let sidecar = [
        description_line("../a.txt", "alpha\n", "outside"),
        description_line("/etc/passwd", "alpha\n", "absolute"),
        "{\"path\":\"a.txt\",\"sha256\":\"XYZ\",\"description\":\"badhash\"}\n".to_owned(),
        format!("{{\"path\":\"a.txt\",\"sha256\":\"{hash}\",\"description\":\"x\",\"extra\":1}}\n"),
        "not json at all\n".to_owned(),
        description_line("missing.txt", "alpha\n", "absent"),
        good,
    ]
    .concat();
    repo.write(".bran/descriptions.jsonl", &sidecar);
    let (code, stdout) = repo.query(&[], "quillwort");
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(result_paths(&stdout), ["a.txt"]);
    assert!(
        stdout.contains("descriptions_ignored: 6 rejected, 0 stale"),
        "{stdout}"
    );
    for word in ["outside", "absolute", "badhash", "absent"] {
        assert!(
            !repo.paths(&[], word).contains(&"a.txt".to_owned()),
            "{word}"
        );
    }
}

/// Sidecar trust boundary: an oversize file is ignored as a whole and the
/// query still answers from the repository.
#[test]
fn description_sidecar_oversize_is_ignored() {
    let repo = Repo::with(&[("a.txt", "alpha\n")]);
    let mut big = description_line("a.txt", "alpha\n", "quillwort");
    big.push_str(&" ".repeat(8 * 1024 * 1024));
    repo.write(".bran/descriptions.jsonl", &big);
    let (code, stdout) = repo.query(&[], "alpha quillwort");
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(result_paths(&stdout), ["a.txt"]);
    assert!(
        stdout.contains("descriptions_ignored: file too large"),
        "{stdout}"
    );
}
