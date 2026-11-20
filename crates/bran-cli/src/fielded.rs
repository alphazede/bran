//! Fielded lexical scorer for `bran query --brief`.
//!
//! Every brief query tokenizes the scanned documents into four fields
//! (filename, enclosing path, body, short description), stems query and
//! document words the same way, and scores each document with BM25-style
//! saturation per field. Nothing is persisted: the index is rebuilt from the
//! scan of the query, so a changed, added or removed file always counts.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use bran_core::graph::{GraphInput, NodeRole};
use bran_core::scan::ScanSnapshot;
use bran_document::canonical::{sha256_hex, Json};

use crate::stem::{agent_verb, stem, EXCEPTIONS};
use crate::{
    brief_source_excluded, node_ranking_traits, query_identifier_runs, query_terms_and_entities,
    ranking, snapshot_long_path_map, SourceRanking, RANKED_FACT_KEYS,
};

/// Optional per-file descriptions, one JSON object per line.
pub(crate) const DESCRIPTIONS_FILENAME: &str = ".bran/descriptions.jsonl";
const MAX_DESCRIPTIONS_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DESCRIPTION_LINE_BYTES: usize = 4096;
const MAX_DESCRIPTION_BYTES: usize = 2048;
const MAX_DESCRIPTION_ENTRIES: usize = 50_000;

/// Field order: filename without extension, enclosing path, body, description.
const FIELD_NAMES: [&str; 4] = ["filename", "path", "body", "description"];
const FIELD_WEIGHTS: [f64; 4] = [3.0, 1.0, 1.0, 1.5];
/// BM25 length normalization `b` per field.
const LENGTH_NORMALIZATION: [f64; 4] = [0.2, 0.2, 0.75, 0.6];
const K1: f64 = 1.2;
/// Weight of the document-trait tie value (status, canonical, public, confidence).
const TRAIT_WEIGHT: f64 = 0.5 / 400.0;
/// Test files gain this much for a question asking about tests and lose it otherwise.
const TEST_ROLE_WEIGHT: f64 = 4.0;
/// A referring source supports a value file when its score exceeds this share of the leader.
const REFERENCE_SUPPORT: f64 = 0.4;
const REFERENCE_TRANSFER: f64 = 0.75;
const VALUE_PEAK_SHARE: f64 = 0.25;
const SCHEDULE_PEAK_SHARE: f64 = 0.5;
const MAX_REFERENCE_LINE_BYTES: usize = 3000;
/// Lower tiers are tokenized only when the leading tier cannot fill this shortlist.
const SHORTLIST: usize = 32;
const SCORE_SCALE: f64 = 1_000_000.0;
const SCORE_OFFSET: f64 = 1_000.0;

const STOP_WORDS: &[&str] = &[
    "a", "about", "across", "after", "all", "also", "an", "and", "any", "are", "as", "at", "be",
    "because", "been", "before", "being", "both", "by", "can", "could", "did", "do", "does",
    "each", "every", "for", "from", "had", "has", "have", "here", "how", "if", "in", "into", "is",
    "it", "its", "many", "may", "more", "most", "must", "not", "of", "often", "on", "only", "onto",
    "or", "other", "per", "should", "some", "such", "than", "that", "the", "their", "them", "then",
    "there", "these", "they", "this", "those", "to", "too", "unless", "until", "upon", "use",
    "using", "very", "via", "was", "were", "what", "when", "where", "which", "who", "whom",
    "whose", "will", "with", "would",
];

fn is_stop(word: &[u8]) -> bool {
    word.len() <= 7
        && STOP_WORDS
            .binary_search_by(|s| s.as_bytes().cmp(word))
            .is_ok()
}

/// Calls `f` with each lowercase token: ASCII letter runs split at a
/// lowercase-to-uppercase change, and digit runs. Everything else separates.
fn for_each_token(text: &[u8], buf: &mut Vec<u8>, mut f: impl FnMut(&[u8], bool)) {
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        if c.is_ascii_digit() {
            let start = i;
            while i < text.len() && text[i].is_ascii_digit() {
                i += 1;
            }
            f(&text[start..i], true);
        } else if c.is_ascii_alphabetic() {
            buf.clear();
            let mut prev_lower = false;
            while i < text.len() && text[i].is_ascii_alphabetic() {
                let d = text[i];
                if prev_lower && d.is_ascii_uppercase() {
                    break;
                }
                prev_lower = d.is_ascii_lowercase();
                buf.push(d.to_ascii_lowercase());
                i += 1;
            }
            f(buf, false);
        } else {
            i += 1;
        }
    }
}

/// Indexed terms of `text`: tokens longer than one character, stop words
/// dropped, letters stemmed.
fn terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for_each_token(text.as_bytes(), &mut Vec::new(), |token, digits| {
        if token.len() > 1 && (digits || !is_stop(token)) {
            let token = std::str::from_utf8(token).unwrap_or_default();
            out.push(if digits {
                token.to_owned()
            } else {
                stem(token)
            });
        }
    });
    out
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Final path component without its last extension, and that extension.
fn split_name(locator: &str) -> (&str, &str) {
    let file = locator.rsplit('/').next().unwrap_or(locator);
    match file.rfind('.') {
        Some(dot) if dot > 0 => (&file[..dot], &file[dot + 1..]),
        _ => (file, ""),
    }
}

/// Filename field text: the name without its extension, plus the enclosing
/// directory's name for an `index` or `README` document, which describes that
/// directory. A code `index` module only re-exports and keeps its own name.
fn filename_words(locator: &str) -> String {
    let name = split_name(locator).0;
    let index = name.eq_ignore_ascii_case("index") || name.eq_ignore_ascii_case("readme");
    match locator.rsplit('/').nth(1) {
        Some(dir) if index && path_role(locator) == Role::Doc => format!("{dir} {name}"),
        _ => name.to_owned(),
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Role {
    Test,
    Doc,
    Config,
    Code,
}

/// Code extensions whose files count as tests when a path word says so.
const SCRIPT_EXTENSIONS: [&str; 15] = [
    "sh", "py", "js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "rs", "go", "rb", "cpp",
    "java",
];

fn path_role(locator: &str) -> Role {
    let ext = split_name(locator).1.to_ascii_lowercase();
    let ext = ext.as_str();
    if SCRIPT_EXTENSIONS.contains(&ext)
        && words(locator)
            .iter()
            .any(|w| matches!(w.as_str(), "test" | "tests" | "spec" | "specs"))
    {
        return Role::Test;
    }
    match ext {
        "md" | "rst" | "adoc" | "sdoc" | "html" => Role::Doc,
        "json" | "yaml" | "yml" | "toml" | "ini" | "conf" | "txt" | "timer" | "service" | "xml"
        | "csv" | "list" | "cfg" => Role::Config,
        _ => Role::Code,
    }
}

/// A path row (`/x`, `./x`, `~/x`, `C:\x`) or a `key: value` / `key = value` row.
fn is_list_row(row: &str) -> bool {
    let b = row.as_bytes();
    let rest = if b.starts_with(b"/") {
        Some(&row[1..])
    } else if b.starts_with(b"./") || b.starts_with(b"~/") {
        Some(&row[2..])
    } else if b.len() > 3 && b[0].is_ascii_alphabetic() && &b[1..3] == b":\\" {
        Some(&row[3..])
    } else {
        None
    };
    if rest.is_some_and(|r| !r.is_empty() && !r.contains(char::is_whitespace)) {
        return true;
    }
    let key_end = row
        .find(|c: char| c.is_whitespace() || c == ':' || c == '=')
        .unwrap_or(row.len());
    key_end > 0 && row[key_end..].trim_start().starts_with([':', '='])
}

/// Plain-text files count as data when at least half their meaningful lines
/// are path or key/value rows.
fn content_role(locator: &str, source: &str) -> Role {
    let role = path_role(locator);
    if role != Role::Config || !split_name(locator).1.eq_ignore_ascii_case("txt") {
        return role;
    }
    let rows: Vec<&str> = source
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with(['#', ';']))
        .collect();
    let list = rows.iter().filter(|r| is_list_row(r)).count();
    if !rows.is_empty() && 2 * list >= rows.len() {
        Role::Config
    } else {
        Role::Doc
    }
}

fn truncate_chars(s: &str, max: usize) -> &str {
    s.char_indices().nth(max).map_or(s, |(i, _)| &s[..i])
}

/// Leading title and metadata lines of a document, or leading comments of
/// code and data.
fn lead(locator: &str, source: &str) -> String {
    let lines = source.lines();
    if path_role(locator) == Role::Doc {
        const STARTS: [&str; 6] = [
            "# ",
            "title:",
            "purpose:",
            "description:",
            "aliases:",
            "tags:",
        ];
        lines
            .take(50)
            .filter(|s| STARTS.iter().any(|p| s.starts_with(p)))
            .take(4)
            .map(|s| truncate_chars(s, 230))
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        const STARTS: [&str; 5] = ["#", "//", "\"\"\"", "'''", ";"];
        lines
            .take(30)
            .filter(|s| STARTS.iter().any(|p| s.starts_with(p)))
            .take(12)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The query's lexical channel: sorted stems plus a two-letter prefix filter,
/// so most document tokens are rejected before stemming.
struct Query {
    stems: Vec<String>,
    /// Agent-noun verb form of each stem (`parser` and `parse` both give
    /// `pars`), sorted, with the stem index; used for filename words only.
    verbs: Vec<(String, u16)>,
    surface: BTreeSet<Vec<u8>>,
    pairs: Vec<bool>,
    unfiltered: bool,
}

fn pair_index(w: &[u8]) -> Option<usize> {
    (w.len() >= 2 && w[0].is_ascii_lowercase() && w[1].is_ascii_lowercase())
        .then(|| usize::from(w[0] - b'a') * 26 + usize::from(w[1] - b'a'))
}

impl Query {
    fn new(text: &str) -> Self {
        let stems: BTreeSet<String> = terms(text).into_iter().collect();
        let stems: Vec<String> = stems.into_iter().collect();
        let mut pairs = vec![false; 26 * 26];
        let mut unfiltered = false;
        for s in &stems {
            match pair_index(s.as_bytes()) {
                Some(i) => pairs[i] = true,
                None => unfiltered |= !s.as_bytes()[0].is_ascii_digit(),
            }
        }
        // A stem keeps its word's first two letters except for these forms.
        for (word, fixed) in EXCEPTIONS {
            if stems.iter().any(|s| s == fixed) {
                if let Some(i) = pair_index(word.as_bytes()) {
                    pairs[i] = true;
                }
            }
        }
        let mut surface = BTreeSet::new();
        for_each_token(text.as_bytes(), &mut Vec::new(), |t, _| {
            surface.insert(t.to_vec());
        });
        let mut verbs: Vec<(String, u16)> = stems
            .iter()
            .enumerate()
            .map(|(i, s)| (agent_verb(s), i as u16))
            .collect();
        verbs.sort();
        Self {
            stems,
            verbs,
            surface,
            pairs,
            unfiltered,
        }
    }

    fn may_match(&self, w: &[u8]) -> bool {
        self.unfiltered
            || pair_index(w).is_some_and(|i| self.pairs[i])
            // A vowel plus an inflection (`aing`) is the one stem that changes
            // its second letter.
            || (b"aeiouy".contains(&w[0])
                && matches!(&w[1..], b"ed" | b"edly" | b"ing" | b"ingly"))
    }

    /// Filename match through the agent-noun verb form, when the direct
    /// match failed: `parser` in the question meets `parse` in a filename and
    /// the reverse.
    fn match_verb(&self, token: &[u8]) -> Option<u16> {
        let text = std::str::from_utf8(token).ok()?;
        let verb = agent_verb(&stem(text));
        let i = self
            .verbs
            .partition_point(|(v, _)| v.as_str() < verb.as_str());
        self.verbs
            .get(i)
            .filter(|(v, _)| *v == verb)
            .map(|&(_, idx)| idx)
    }

    fn match_token(
        &self,
        token: &[u8],
        digits: bool,
        memo: &mut HashMap<Vec<u8>, Option<u16>>,
    ) -> Option<u16> {
        let find = |s: &str| {
            self.stems
                .binary_search_by(|x| x.as_str().cmp(s))
                .ok()
                .map(|i| i as u16)
        };
        let text = std::str::from_utf8(token).ok()?;
        if digits {
            return find(text);
        }
        if !self.may_match(token) {
            return None;
        }
        if let Some(&hit) = memo.get(token) {
            return hit;
        }
        let hit = find(&stem(text));
        memo.insert(token.to_vec(), hit);
        hit
    }
}

struct Doc<'a> {
    root: usize,
    node: &'a bran_core::graph::NodeInput,
    locator: &'a str,
    source: &'a str,
    tier: u8,
    class: Option<&'a str>,
    role: Role,
    description: Option<&'a str>,
}

/// Per-document token statistics for the query stems.
#[derive(Clone, Default)]
struct Stats {
    len: [u32; 4],
    /// `tf[field * stems + stem]`.
    tf: Vec<u32>,
    /// Query words equal to a whole filename token.
    literal_filename: u64,
}

fn analyze(doc: &Doc, q: &Query, memo: &mut HashMap<Vec<u8>, Option<u16>>) -> Stats {
    let n = q.stems.len();
    let mut stats = Stats {
        tf: vec![0; 4 * n],
        ..Stats::default()
    };
    let dir = doc.locator.rsplit_once('/').map_or("", |(d, _)| d);
    let mut description = lead(doc.locator, doc.source);
    if let Some(extra) = doc.description {
        description.push(' ');
        description.push_str(extra);
    }
    let name = filename_words(doc.locator);
    let fields: [&str; 4] = [&name, dir, doc.source, &description];
    let mut buf = Vec::new();
    for (k, text) in fields.iter().enumerate() {
        for_each_token(text.as_bytes(), &mut buf, |token, digits| {
            if token.len() < 2 || (!digits && is_stop(token)) {
                return;
            }
            stats.len[k] += 1;
            let hit = q.match_token(token, digits, memo).or_else(|| {
                (k == 0 && !digits && q.may_match(token))
                    .then(|| q.match_verb(token))
                    .flatten()
            });
            if let Some(i) = hit {
                stats.tf[k * n + usize::from(i)] += 1;
                if k == 0 && q.surface.contains(token) {
                    stats.literal_filename += 1;
                }
            }
        });
    }
    stats
}

/// Analyzes `docs` on up to eight threads; output order matches input order.
fn analyze_all(docs: &[&Doc], q: &Query) -> Vec<Stats> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 8);
    let chunk = docs.len().div_ceil(threads).max(64);
    std::thread::scope(|scope| {
        let handles: Vec<_> = docs
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    let mut memo = HashMap::new();
                    part.iter()
                        .map(|d| analyze(d, q, &mut memo))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("analysis thread"))
            .collect()
    })
}

fn has_evidence(stats: &Stats) -> bool {
    stats.tf.iter().any(|&tf| tf > 0)
}

struct Corpus {
    idf: Vec<f64>,
    avg: [f64; 4],
}

impl Corpus {
    /// Document frequency from bodies and mean field lengths over the leading tier.
    fn new(stats: &[&Stats], n: usize) -> Self {
        let docs = stats.len() as f64;
        let avg = std::array::from_fn(|k| {
            let total: u64 = stats.iter().map(|s| u64::from(s.len[k])).sum();
            if docs > 0.0 {
                total as f64 / docs
            } else {
                0.0
            }
        });
        let idf = (0..n)
            .map(|t| {
                let df = stats.iter().filter(|s| s.tf[2 * n + t] > 0).count() as f64;
                ((docs - df + 0.5) / (df + 0.5)).ln_1p()
            })
            .collect();
        Self { idf, avg }
    }

    fn lexical(&self, stats: &Stats) -> f64 {
        let n = self.idf.len();
        let mut score = 0.0;
        for t in 0..n {
            for k in 0..4 {
                let tf = f64::from(stats.tf[k * n + t]);
                if tf == 0.0 {
                    continue;
                }
                let b = LENGTH_NORMALIZATION[k];
                // A field absent in the leading tier still counts in lower tiers.
                let avg = if self.avg[k] == 0.0 { 1.0 } else { self.avg[k] };
                let norm = K1 * (1.0 - b + b * f64::from(stats.len[k]) / avg);
                score += self.idf[t] * FIELD_WEIGHTS[k] * (K1 + 1.0) * tf / (tf + norm);
            }
        }
        score
    }
}

/// Root inputs to one fielded ranking.
pub(crate) struct Root<'a> {
    pub bundle: &'a str,
    pub path: &'a Path,
    pub graph_input: &'a GraphInput,
    pub snapshot: &'a ScanSnapshot,
    pub ranking_config: &'a ranking::RankingConfig,
}

pub(crate) struct Ranked {
    pub rankings: Vec<SourceRanking>,
    pub matched_terms: BTreeSet<String>,
    pub entities: BTreeSet<String>,
    pub warnings: Vec<String>,
}

#[derive(Default)]
struct Intent {
    test: bool,
    value: bool,
    frequency: bool,
}

impl Intent {
    fn of(question: &str) -> Self {
        let w = words(question);
        let pair = |a: &[&str], b: &[&str]| {
            w.windows(2)
                .any(|p| a.contains(&p[0].as_str()) && b.contains(&p[1].as_str()))
        };
        let test = w
            .iter()
            .any(|x| matches!(x.as_str(), "test" | "tests" | "testing"));
        // Bare "which"/"what" also asks for files, so only explicit quantity,
        // frequency and set questions trigger the value-source rule.
        let asks_value = pair(&["how"], &["often", "frequently", "long", "many", "much"])
            || w.iter().any(|x| x == "frequency")
            || asks_set(&w);
        let code_request = pair(
            &["function", "command", "hook", "script", "manual", "guide"],
            &["is", "does", "implements"],
        );
        Self {
            test,
            value: asks_value && !test && !code_request,
            frequency: pair(&["how"], &["often", "frequently"])
                || w.iter().any(|x| x == "frequency"),
        }
    }
}

/// A set question asks which members of a group apply: `which`, an optional
/// modifier, a plural noun and an auxiliary verb (`which owners may ...`,
/// `which repository owners may ...`). A singular `which <noun>` asks for one
/// file, and `which <plural> <verb>` (`which scripts remove ...`) asks for
/// the files that act, so neither is a set question.
fn asks_set(w: &[String]) -> bool {
    const AUXILIARIES: [&str; 13] = [
        "are", "were", "do", "does", "did", "may", "can", "could", "must", "should", "will",
        "would", "have",
    ];
    let plural = |x: &str| {
        x.len() > 3 && x.ends_with('s') && !["ss", "us", "is"].iter().any(|e| x.ends_with(e))
    };
    let at = |i: usize| w.get(i).map_or("", String::as_str);
    (0..w.len()).any(|i| {
        at(i) == "which"
            && (plural(at(i + 1)) && AUXILIARIES.contains(&at(i + 2))
                || plural(at(i + 2)) && AUXILIARIES.contains(&at(i + 3)))
    })
}

/// Ranks the brief corpus of `roots` for `question`, or `None` when the
/// question has no indexable word (the caller keeps its previous behaviour).
pub(crate) fn rank(roots: &[Root], question: &str) -> Option<Ranked> {
    let q = Query::new(question);
    if q.stems.is_empty() {
        return None;
    }
    let n = q.stems.len();
    let intent = Intent::of(question);
    let mut warnings = Vec::new();
    let descriptions: Vec<_> = roots
        .iter()
        .map(|root| load_descriptions(root.path, root.snapshot, &mut warnings))
        .collect();
    let mut docs = Vec::new();
    for (r, root) in roots.iter().enumerate() {
        let descriptions = &descriptions[r];
        let long_paths = snapshot_long_path_map(root.snapshot);
        for node in root
            .graph_input
            .nodes()
            .iter()
            .filter(|node| node.role() == NodeRole::Document)
        {
            let raw = node.provenance().locator();
            let locator = long_paths.get(raw).copied().unwrap_or(raw);
            if brief_source_excluded(locator) {
                continue;
            }
            let source = root
                .snapshot
                .entries
                .get(locator)
                .and_then(|e| std::str::from_utf8(&e.source).ok())
                .unwrap_or("");
            let (tier, class) = root.ranking_config.classify(locator);
            docs.push(Doc {
                root: r,
                node,
                locator,
                source,
                tier,
                class,
                role: content_role(locator, source),
                description: descriptions.get(locator).map(String::as_str),
            });
        }
    }
    let lead_tier = docs.iter().map(|d| d.tier).min().unwrap_or(0);
    let mut stats: Vec<Option<Stats>> = vec![None; docs.len()];
    let lead_docs: Vec<&Doc> = docs.iter().filter(|d| d.tier == lead_tier).collect();
    let lead_ids: Vec<usize> = (0..docs.len())
        .filter(|&j| docs[j].tier == lead_tier)
        .collect();
    for (j, s) in lead_ids.iter().zip(analyze_all(&lead_docs, &q)) {
        stats[*j] = Some(s);
    }
    let lead_stats: Vec<&Stats> = stats.iter().flatten().collect();
    let corpus = Corpus::new(&lead_stats, n);
    let admitted = lead_stats.iter().filter(|s| has_evidence(s)).count();
    let unseen = (0..n).any(|t| {
        !lead_stats
            .iter()
            .any(|s| (0..4).any(|k| s.tf[k * n + t] > 0))
    });
    if admitted < SHORTLIST || unseen {
        let rest_ids: Vec<usize> = (0..docs.len()).filter(|&j| stats[j].is_none()).collect();
        let rest: Vec<&Doc> = rest_ids.iter().map(|&j| &docs[j]).collect();
        for (j, s) in rest_ids.iter().zip(analyze_all(&rest, &q)) {
            stats[*j] = Some(s);
        }
    }

    let traits: Vec<_> = docs.iter().map(|d| node_ranking_traits(d.node)).collect();
    let tie: Vec<u64> = docs
        .iter()
        .zip(&traits)
        .map(|(d, t)| {
            u64::from(t.0) * 100
                + u64::from(t.1) * 50
                + u64::from(t.2) * 20
                + u64::from(d.node.confidence().value())
        })
        .collect();
    let lexical: Vec<f64> = stats
        .iter()
        .map(|s| s.as_ref().map_or(0.0, |s| corpus.lexical(s)))
        .collect();
    let mut eligible: Vec<bool> = stats
        .iter()
        .map(|s| s.as_ref().is_some_and(has_evidence))
        .collect();
    let mut score: Vec<f64> = (0..docs.len())
        .map(|j| {
            let role = if docs[j].role != Role::Test {
                0.0
            } else if intent.test {
                TEST_ROLE_WEIGHT
            } else {
                -TEST_ROLE_WEIGHT
            };
            lexical[j] + TRAIT_WEIGHT * tie[j].min(1023) as f64 + role
        })
        .collect();
    let mut referenced = vec![false; docs.len()];
    if intent.value && eligible.iter().any(|&e| e) {
        value_sources(
            &docs,
            &q,
            &intent,
            &mut score,
            &mut eligible,
            &mut referenced,
        );
    }

    let (terms_list, entities) = query_terms_and_entities(question);
    let mut rankings = Vec::new();
    for (j, doc) in docs.iter().enumerate() {
        if !eligible[j] {
            continue;
        }
        let s = stats[j].as_ref();
        let mut fields: Vec<&str> = (0..4)
            .filter(|&k| s.is_some_and(|s| (0..n).any(|t| s.tf[k * n + t] > 0)))
            .map(|k| FIELD_NAMES[k])
            .collect();
        if referenced[j] {
            fields.push("reference");
        }
        let exact = exact_units(doc, &terms_list, &entities);
        let mut reason = format!(
            "{}:{}",
            if exact > 0 { "exact" } else { "partial" },
            fields.join("+")
        );
        if let Some(class) = doc.class {
            reason.push_str(&format!(" [{class}]"));
        }
        let (active, canonical, public_safe, freshness) = traits[j].clone();
        rankings.push(SourceRanking {
            bundle: roots[doc.root].bundle.to_owned(),
            id: doc.node.id().clone(),
            locator: doc.locator.to_owned(),
            rank: 0,
            exact_matches: exact,
            partial_matches: s.map_or(0, |s| {
                (0..n)
                    .filter(|&t| (0..4).any(|k| s.tf[k * n + t] > 0))
                    .count()
            }),
            match_reason: reason,
            active,
            canonical,
            public_safe,
            confidence: doc.node.confidence().value(),
            freshness,
            brief_tier: doc.tier,
            brief_score: (
                (score[j].max(0.0) * SCORE_SCALE) as u64,
                ((score[j] + SCORE_OFFSET).max(0.0) * SCORE_SCALE) as u64,
                s.map_or(0, |s| s.literal_filename),
                tie[j],
            ),
        });
    }

    let seen: Vec<bool> = (0..n)
        .map(|t| {
            stats
                .iter()
                .flatten()
                .any(|s| (0..4).any(|k| s.tf[k * n + t] > 0))
        })
        .collect();
    let mut matched_terms = BTreeSet::new();
    for original in query_identifier_runs(question) {
        let term = original.to_ascii_lowercase();
        if !terms_list.contains(&term) {
            continue;
        }
        let stems = terms(original);
        let matched = if stems.is_empty() {
            !rankings.is_empty()
        } else {
            stems
                .iter()
                .all(|s| q.stems.binary_search(s).is_ok_and(|i| seen[i]))
        };
        if matched {
            matched_terms.insert(term);
        }
    }
    for entity in &entities {
        if docs.iter().any(|d| {
            ranking::contains_ascii_ci(d.locator, entity)
                || ranking::contains_ascii_ci(d.source, entity)
        }) {
            matched_terms.insert(entity.clone());
        }
    }
    Some(Ranked {
        rankings,
        matched_terms,
        entities,
        warnings,
    })
}

/// Identity-level matches: entity units present, or a query word equal to
/// the whole locator or a whole ranked metadata value.
fn exact_units(doc: &Doc, terms: &BTreeSet<String>, entities: &BTreeSet<String>) -> usize {
    let entity_hits = entities
        .iter()
        .filter(|e| {
            ranking::contains_ascii_ci(doc.locator, e) || ranking::contains_ascii_ci(doc.source, e)
        })
        .count();
    let term_hits = terms
        .iter()
        .filter(|t| {
            doc.locator.eq_ignore_ascii_case(t)
                || RANKED_FACT_KEYS.iter().any(|key| {
                    doc.node
                        .facts()
                        .values(key)
                        .is_some_and(|vs| vs.iter().any(|v| v.eq_ignore_ascii_case(t)))
                })
        })
        .count();
    entity_hits + term_hits
}

/// Value-source rule: for a "how many/how often/how long" question, a configuration
/// or list file that a relevant document or code file names gains part of
/// that source's score, and may enter the ranking through that reference.
fn value_sources(
    docs: &[Doc],
    q: &Query,
    intent: &Intent,
    score: &mut [f64],
    eligible: &mut [bool],
    referenced: &mut [bool],
) {
    let peak = (0..docs.len())
        .filter(|&j| eligible[j])
        .map(|j| score[j])
        .fold(f64::NEG_INFINITY, f64::max);
    let resolver = Resolver::new(docs);
    let mut refer = vec![0.0f64; docs.len()];
    for (j, doc) in docs.iter().enumerate() {
        if !eligible[j] || !matches!(doc.role, Role::Doc | Role::Code) {
            continue;
        }
        for dest in resolver.references(j, doc) {
            if docs[dest].role == Role::Config {
                refer[dest] = refer[dest].max(score[j]);
            }
        }
    }
    for (j, doc) in docs.iter().enumerate() {
        if doc.role != Role::Config {
            continue;
        }
        let strong = refer[j] > REFERENCE_SUPPORT * peak;
        if !eligible[j] && !strong {
            continue;
        }
        let timer = intent.frequency && has_schedule(doc.source);
        if !(strong || timer || key_hit(doc.source, q)) {
            continue;
        }
        score[j] += (REFERENCE_TRANSFER * (refer[j] - score[j])).max(0.0)
            + VALUE_PEAK_SHARE * peak
            + if timer {
                SCHEDULE_PEAK_SHARE * peak
            } else {
                0.0
            };
        eligible[j] = true;
        referenced[j] |= strong;
    }
}

/// Resolves file references inside one root: relative to the referring file,
/// then to the root, then by a unique path suffix.
struct Resolver<'a> {
    ids: HashMap<(usize, &'a str), usize>,
    /// Lowercase locator of each document.
    lower: Vec<String>,
    /// Documents by root and lowercase file name; keys stay linear in path length.
    // shortcut: a lookup scans every file with the same name, upgrade to a suffix trie if one name repeats thousands of times.
    names: HashMap<(usize, String), Vec<usize>>,
}

impl<'a> Resolver<'a> {
    fn new(docs: &'a [Doc]) -> Self {
        let mut ids = HashMap::new();
        let mut names: HashMap<(usize, String), Vec<usize>> = HashMap::new();
        let lower: Vec<String> = docs
            .iter()
            .map(|d| d.locator.to_ascii_lowercase())
            .collect();
        for (j, d) in docs.iter().enumerate() {
            ids.insert((d.root, d.locator), j);
            let name = lower[j].rsplit('/').next().unwrap_or_default();
            names.entry((d.root, name.to_owned())).or_default().push(j);
        }
        Self { ids, lower, names }
    }

    fn resolve(&self, root: usize, from: &str, raw: &str) -> Option<usize> {
        let dir = from.rsplit_once('/').map_or("", |(d, _)| d);
        let local = normalize(&format!("{dir}/{raw}"));
        if let Some(&j) = local.as_deref().and_then(|l| self.ids.get(&(root, l))) {
            return Some(j);
        }
        if let Some(&j) = self.ids.get(&(root, raw)) {
            return Some(j);
        }
        // A suffix matches the whole locator or starts after a `/`; two matches are ambiguous.
        let raw = raw.to_ascii_lowercase();
        let name = raw.rsplit('/').next().unwrap_or_default();
        let mut hit = None;
        for &j in self.names.get(&(root, name.to_owned()))? {
            let l = &self.lower[j];
            let suffix = l
                .strip_suffix(raw.as_str())
                .is_some_and(|head| head.is_empty() || head.ends_with('/'));
            if suffix && hit.replace(j).is_some() {
                return None;
            }
        }
        hit
    }

    fn references(&self, j: usize, doc: &Doc) -> BTreeSet<usize> {
        let mut out = BTreeSet::new();
        for line in doc.source.lines() {
            if line.len() > MAX_REFERENCE_LINE_BYTES {
                continue;
            }
            let is_ref = |c: char| c.is_ascii_alphanumeric() || "_.+@-/".contains(c);
            for run in line.split(|c: char| !is_ref(c)) {
                if !run.contains(['/', '.']) {
                    continue;
                }
                let raw = run.trim_end_matches('.').trim_matches('/');
                let raw = raw.strip_prefix("./").unwrap_or(raw);
                if raw.len() < 4 {
                    continue;
                }
                if let Some(dest) = self.resolve(doc.root, doc.locator, raw) {
                    if dest != j {
                        out.insert(dest);
                    }
                }
            }
        }
        out
    }
}

fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

/// A recognizable schedule: a timer unit, a `schedule:`/`cron:` key or an
/// `OnCalendar=` setting near the start of the file.
fn has_schedule(source: &str) -> bool {
    let head = &source.as_bytes()[..source.len().min(2000)];
    let head = String::from_utf8_lossy(head).to_ascii_lowercase();
    head.lines().any(|line| {
        let t = line.trim_start();
        line.starts_with("[timer]") || t.starts_with("schedule:") || t.starts_with("cron:")
    }) || head.match_indices("oncalendar=").any(|(i, _)| {
        i == 0 || !head.as_bytes()[i - 1].is_ascii_alphanumeric() && head.as_bytes()[i - 1] != b'_'
    })
}

/// True when a structured key of the file (`key:` or `key =` at a line
/// start or after `{` or `,`) shares a stem with the query.
fn key_hit(source: &str, q: &Query) -> bool {
    let mut keys = String::new();
    for line in source.lines() {
        let b = line.as_bytes();
        let mut starts = vec![0];
        starts.extend(
            b.iter()
                .enumerate()
                .filter(|(_, &c)| c == b',' || c == b'{')
                .map(|(i, _)| i + 1),
        );
        for start in starts {
            let mut i = start;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                i += 1;
            }
            let key_start = i;
            if i >= b.len() || !(b[i].is_ascii_alphabetic() || b[i] == b'_') {
                continue;
            }
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b"_.-".contains(&b[i])) {
                i += 1;
            }
            let key_end = i;
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                i += 1;
            }
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b':' || b[i] == b'=') {
                keys.push_str(&line[key_start..key_end]);
                keys.push(' ');
            }
        }
    }
    terms(&keys)
        .iter()
        .any(|k| q.stems.binary_search(k).is_ok())
}

/// Loads `.bran/descriptions.jsonl`. Each line is
/// `{"path": "<repo-relative path>", "sha256": "<hex of file bytes>", "description": "<text>"}`.
/// A description applies only while its hash matches the scanned file;
/// malformed, oversize, out-of-tree, duplicate or stale lines are skipped
/// and counted in one warning. The query never fails on this file.
fn load_descriptions(
    root: &Path,
    snapshot: &ScanSnapshot,
    warnings: &mut Vec<String>,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let bytes = match read_sidecar(root) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return out,
        Err(reason) => {
            warnings.push(format!("descriptions_ignored: {reason}"));
            return out;
        }
    };
    let (mut rejected, mut stale) = (0usize, 0usize);
    let mut hashes = HashMap::new();
    for line in bytes.split(|&b| b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if out.len() + rejected + stale >= MAX_DESCRIPTION_ENTRIES {
            rejected += 1;
            continue;
        }
        match parse_description(line) {
            Some((path, sha256, text)) if !out.contains_key(&path) => {
                match snapshot.entries.get_key_value(&path) {
                    Some((key, entry)) => {
                        let hash = hashes
                            .entry(key)
                            .or_insert_with(|| sha256_hex(&entry.source));
                        if *hash == sha256 {
                            out.insert(path, text);
                        } else {
                            stale += 1;
                        }
                    }
                    None => rejected += 1,
                }
            }
            _ => rejected += 1,
        }
    }
    if rejected + stale > 0 {
        warnings.push(format!(
            "descriptions_ignored: {rejected} rejected, {stale} stale"
        ));
    }
    out
}

fn read_sidecar(root: &Path) -> Result<Option<Vec<u8>>, &'static str> {
    let file = match open_sidecar(root) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            #[cfg(unix)]
            if err.raw_os_error() == Some(libc::ELOOP) {
                return Err("not a regular file");
            }
            return Err(match err.kind() {
                std::io::ErrorKind::NotADirectory | std::io::ErrorKind::InvalidData => {
                    "not a regular file"
                }
                _ => "unreadable",
            });
        }
    };
    let meta = file.metadata().map_err(|_| "unreadable")?;
    if !meta.is_file() {
        return Err("not a regular file");
    }
    if meta.len() > MAX_DESCRIPTIONS_BYTES {
        return Err("file too large");
    }
    let mut bytes = Vec::new();
    file.take(MAX_DESCRIPTIONS_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "unreadable")?;
    if bytes.len() as u64 > MAX_DESCRIPTIONS_BYTES {
        return Err("file too large");
    }
    Ok(Some(bytes))
}

#[cfg(unix)]
fn open_sidecar(root: &Path) -> std::io::Result<File> {
    open_sidecar_at(&open_bran_directory(root)?)
}

#[cfg(unix)]
fn open_bran_directory(root: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(root.join(".bran"))
}

#[cfg(unix)]
fn open_sidecar_at(directory: &File) -> std::io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};

    // SAFETY: the directory descriptor stays open and the filename is a C string.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            c"descriptions.jsonl".as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: openat returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(not(unix))]
fn open_sidecar(root: &Path) -> std::io::Result<File> {
    let path = root.join(DESCRIPTIONS_FILENAME);
    if !fs::symlink_metadata(root.join(".bran"))?.is_dir()
        || !fs::symlink_metadata(&path)?.is_file()
    {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    // shortcut: portable pathname checks cover static links. Use handle-relative
    // opens on this platform if the queried tree can change concurrently.
    File::open(path)
}

/// One sidecar line: exactly `path`, `sha256` and `description` strings,
/// with an in-tree relative path and a lowercase hex digest.
fn parse_description(line: &[u8]) -> Option<(String, String, String)> {
    if line.len() > MAX_DESCRIPTION_LINE_BYTES {
        return None;
    }
    let Json::Obj(map) = Json::parse_unique(line)? else {
        return None;
    };
    if map.len() != 3 {
        return None;
    }
    let text = |key: &str| match map.get(key) {
        Some(Json::Str(s)) => Some(s.clone()),
        _ => None,
    };
    let (path, sha256, description) = (text("path")?, text("sha256")?, text("description")?);
    let in_tree = !path.is_empty()
        && !path.contains(['\\', '\0'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    let hex = sha256.len() == 64
        && sha256
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    let sized = !description.trim().is_empty() && description.len() <= MAX_DESCRIPTION_BYTES;
    (in_tree && hex && sized).then_some((path, sha256, description))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_split_case_digits_and_separators() {
        assert_eq!(
            terms("parseHTTPServer v2_beta the run-books"),
            ["pars", "httpserver", "beta", "run", "book"]
        );
        assert_eq!(terms("which of the"), Vec::<String>::new());
        assert_eq!(terms("Studies 2024"), ["studi", "2024"]);
    }

    #[test]
    fn prefix_filter_never_hides_a_matching_token() {
        let q = Query::new("dying studies aed");
        let mut memo = HashMap::new();
        for word in ["dying", "die", "studi", "study", "studying", "aed", "aing"] {
            let expected = q.stems.binary_search(&stem(word)).ok().map(|i| i as u16);
            assert_eq!(
                q.match_token(word.as_bytes(), false, &mut memo),
                expected,
                "{word}"
            );
        }
    }

    #[test]
    fn list_rows_and_roles() {
        assert!(is_list_row("/etc/app.conf"));
        assert!(is_list_row("key = value"));
        assert!(is_list_row("name: x"));
        assert!(!is_list_row("a b: c"));
        assert!(!is_list_row("plain prose here"));
        assert!(content_role("x/list.txt", "/a\n/b\nnote\n") == Role::Config);
        assert!(content_role("x/notes.txt", "some words\nmore words\n") == Role::Doc);
        assert!(path_role("tests/run.sh") == Role::Test);
        assert!(path_role("docs/tests.md") == Role::Doc);
        assert!(path_role("src/dial.test.tsx") == Role::Test);
        assert!(path_role("src/Dial.tsx") == Role::Code);
    }

    #[test]
    fn value_intent_detection() {
        let i = Intent::of("How often does the backup run?");
        assert!(i.value && i.frequency && !i.test);
        assert!(!Intent::of("which tests cover backups").value);
        assert!(!Intent::of("which file holds the backup list").value);
        assert!(Intent::of("what is the backup frequency").value);
        assert!(!Intent::of("what script is used for backups").value);
        assert!(Intent::of("which owners may the tool write into").value);
        assert!(Intent::of("which repository owners may the tool write into").value);
        assert!(Intent::of("which repositories does the sync sweep").value);
        assert!(!Intent::of("which assertions keep imports out").value);
        // A plural ends in `s` (not `ss`, `us`, `is`) and has more than three letters.
        assert!(!Intent::of("which policy may apply").value);
        assert!(!Intent::of("which gas does the meter read").value);
        assert!(!Intent::of("which parser derives a match").value);
        assert!(!Intent::of("which unit schedules stalled checks").value);
        assert!(!Intent::of("which process runs the backup").value);
    }

    #[test]
    fn index_pages_take_the_directory_name() {
        assert_eq!(filename_words("docs/garnet/index.md"), "garnet index");
        assert_eq!(filename_words("docs/garnet/README.md"), "garnet README");
        assert_eq!(filename_words("README.md"), "README");
        assert_eq!(filename_words("docs/garnet/palette.md"), "palette");
        assert_eq!(filename_words("src/react/index.ts"), "index");
    }

    #[test]
    fn schedule_and_keys() {
        assert!(has_schedule("[Timer]\nOnCalendar=daily\n"));
        assert!(has_schedule("jobs:\n  schedule: '0 1 * * *'\n"));
        assert!(!has_schedule("reschedule: later\nxoncalendar=1"));
        let q = Query::new("retention days");
        assert!(key_hit("{\"retentionDays\": 3}", &q));
        assert!(key_hit("  retention = 4", &q));
        assert!(!key_hit("other: 1", &q));
    }

    #[test]
    fn description_line_validation() {
        let hash = "a".repeat(64);
        let ok = format!("{{\"path\":\"src/a.rs\",\"sha256\":\"{hash}\",\"description\":\"d\"}}");
        assert!(parse_description(ok.as_bytes()).is_some());
        for bad in [
            format!("{{\"path\":\"../a.rs\",\"sha256\":\"{hash}\",\"description\":\"d\"}}"),
            format!("{{\"path\":\"/etc/passwd\",\"sha256\":\"{hash}\",\"description\":\"d\"}}"),
            "{\"path\":\"a.rs\",\"sha256\":\"XYZ\",\"description\":\"d\"}".to_owned(),
            format!("{{\"path\":\"a.rs\",\"sha256\":\"{hash}\",\"description\":\"d\",\"x\":1}}"),
            format!(
                "{{\"path\":\"a.rs\",\"sha256\":\"{hash}\",\"description\":\"{}\"}}",
                "d".repeat(3000)
            ),
            "not json".to_owned(),
        ] {
            assert!(parse_description(bad.as_bytes()).is_none(), "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_rejects_linked_files_and_directories() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("bran-sidecar-links-{}", std::process::id()));
        let bran = root.join(".bran");
        fs::create_dir_all(&bran).unwrap();
        fs::write(root.join("descriptions.txt"), b"test").unwrap();
        symlink("../descriptions.txt", root.join(DESCRIPTIONS_FILENAME)).unwrap();
        let linked_file = read_sidecar(&root);
        fs::rename(&bran, root.join("alternate")).unwrap();
        symlink("missing", &bran).unwrap();
        let linked_directory = read_sidecar(&root);
        fs::remove_dir_all(root).unwrap();
        assert!(linked_file.is_err());
        assert!(linked_directory.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_open_stays_with_the_opened_directory() {
        let root = std::env::temp_dir().join(format!("bran-sidecar-dir-{}", std::process::id()));
        let bran = root.join(".bran");
        fs::create_dir_all(&bran).unwrap();
        fs::write(root.join(DESCRIPTIONS_FILENAME), "original").unwrap();
        let directory = open_bran_directory(&root).unwrap();
        fs::rename(&bran, root.join("old")).unwrap();
        fs::create_dir(&bran).unwrap();
        fs::write(root.join(DESCRIPTIONS_FILENAME), "replacement").unwrap();
        let mut contents = String::new();
        open_sidecar_at(&directory)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        fs::remove_dir_all(root).unwrap();
        assert_eq!(contents, "original");
    }

    /// Counts heap bytes this thread allocates, so parallel tests do not interfere.
    struct CountingAlloc;

    thread_local! {
        static ALLOCATED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    // SAFETY: forwards every call to the system allocator unchanged.
    unsafe impl std::alloc::GlobalAlloc for CountingAlloc {
        unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
            let _ = ALLOCATED.try_with(|a| a.set(a.get() + layout.size()));
            unsafe { std::alloc::System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
            unsafe { std::alloc::System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static COUNTING: CountingAlloc = CountingAlloc;

    fn allocated_during(f: impl FnOnce()) -> usize {
        let before = ALLOCATED.with(|a| a.get());
        f();
        ALLOCATED.with(|a| a.get()) - before
    }

    #[test]
    fn suffix_resolution_work_grows_linearly_with_path_depth() {
        let node = bran_core::graph::NodeInput::new(
            bran_core::graph::NodeId::parse("test.node").unwrap(),
            NodeRole::Document,
            bran_core::graph::Provenance::new("test", "test://node").unwrap(),
            bran_core::graph::Confidence::new(100).unwrap(),
        );
        fn config<'a>(
            node: &'a bran_core::graph::NodeInput,
            root: usize,
            locator: &'a str,
        ) -> Doc<'a> {
            Doc {
                root,
                node,
                locator,
                source: "",
                tier: 0,
                class: None,
                role: Role::Config,
                description: None,
            }
        }
        let doc = |root, locator| config(&node, root, locator);
        let deep = |depth: usize| {
            let dirs: Vec<String> = (0..depth).map(|i| format!("Dir{i:03}")).collect();
            format!("{}/app.conf", dirs.join("/"))
        };
        let measure = |locator: &str| {
            let docs = [config(&node, 0, locator)];
            let tail = locator.split_once('/').unwrap().1.to_ascii_lowercase();
            allocated_during(|| {
                let resolver = Resolver::new(&docs);
                assert_eq!(resolver.resolve(0, "other/readme.md", &tail), Some(0));
            })
        };
        // Eight times the depth: linear work grows about 8x, quadratic about 64x.
        let (shallow, deeper) = (measure(&deep(64)), measure(&deep(512)));
        eprintln!("allocated bytes: depth 64 = {shallow}, depth 512 = {deeper}");
        assert!(
            deeper < 12 * shallow,
            "8x depth grew allocation from {shallow} to {deeper} bytes"
        );

        // Same results: a unique suffix resolves case-insensitively, only at a
        // segment boundary, and a suffix shared by two files is rejected.
        let docs = [
            doc(0, "a/x/Shared/list.conf"),
            doc(0, "b/y/shared/list.conf"),
            doc(0, "c/Only.conf"),
            doc(1, "d/only.conf"),
        ];
        let resolver = Resolver::new(&docs);
        assert_eq!(resolver.resolve(0, "z/n.md", "only.conf"), Some(2));
        assert_eq!(resolver.resolve(0, "z/n.md", "C/ONLY.CONF"), Some(2));
        assert_eq!(resolver.resolve(1, "z/n.md", "only.conf"), Some(3));
        assert_eq!(resolver.resolve(0, "z/n.md", "nly.conf"), None);
        assert_eq!(resolver.resolve(0, "z/n.md", "e/f/c/only.conf"), None);
        assert_eq!(resolver.resolve(0, "z/n.md", "shared/list.conf"), None);
        assert_eq!(resolver.resolve(0, "z/n.md", "x/shared/list.conf"), Some(0));
        assert_eq!(
            resolver.resolve(0, "z/n.md", "a/x/Shared/list.conf"),
            Some(0)
        );
    }
}
