//! Brief-mode deterministic candidate selection and optional caller-scoped advisor (DES-07).
//!
//! Reads advisor configuration exclusively from the file named by `BRAN_ADVISOR_CONFIG`
//! (never from the queried repository tree). Any missing configuration, policy check
//! failure, timeout, or invalid advisor output silently falls back to the deterministic
//! rule answer.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use bran_document::canonical::{sha256_hex, Json};

use crate::{one_line_reason, SourceRanking};

const ADVISOR_CONFIG_ENV: &str = "BRAN_ADVISOR_CONFIG";

// Deterministic rule threshold: keep the top result plus ranked results scoring
// at least 3/10 (0.30) of the top score, up to the caller's --top bound.
const RULE_RATIO_NUM: u128 = 3;
const RULE_RATIO_DEN: u128 = 10;
const SCORE_EXACT_WEIGHT: u128 = 1_000_000;

const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_STDOUT_BYTES: usize = 64 * 1024;
const MAX_TIMEOUT_MS: u64 = 600_000;

/// Provenance of the final brief selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SelectionSource {
    Rule,
    Advisor,
}

impl SelectionSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Advisor => "advisor",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum SendField {
    Question,
    Path,
    Reason,
    Rank,
}

#[derive(Debug)]
struct AdvisorConfig {
    advisor: PathBuf,
    sha256: String,
    timeout_ms: u64,
    send_fields: BTreeSet<SendField>,
    requires_env: Option<String>,
    pass_env: Vec<String>,
}

/// Apply the deterministic selection rule and, when caller-configured and valid,
/// refine the result order/subset via the external advisor executable.
pub(crate) fn apply(
    question: &str,
    ranked: Vec<SourceRanking>,
) -> (Vec<SourceRanking>, SelectionSource) {
    let rule_selected = rule_select(ranked);
    if rule_selected.is_empty() {
        return (rule_selected, SelectionSource::Rule);
    }
    if let Some(advised) = try_advise(question, &rule_selected) {
        return (advised, SelectionSource::Advisor);
    }
    (rule_selected, SelectionSource::Rule)
}

fn candidate_score(ranking: &SourceRanking) -> u128 {
    (ranking.exact_matches as u128)
        .saturating_mul(SCORE_EXACT_WEIGHT)
        .saturating_add(ranking.partial_matches as u128)
}

/// Keep the top candidate plus each following candidate whose score is at least
/// `RULE_RATIO_NUM / RULE_RATIO_DEN` times the top score.
fn rule_select(ranked: Vec<SourceRanking>) -> Vec<SourceRanking> {
    let Some(first) = ranked.first() else {
        return ranked;
    };
    let top_score = candidate_score(first);
    let mut kept = Vec::with_capacity(ranked.len());
    for (index, mut item) in ranked.into_iter().enumerate() {
        if index > 0
            && candidate_score(&item).saturating_mul(RULE_RATIO_DEN)
                < top_score.saturating_mul(RULE_RATIO_NUM)
        {
            continue;
        }
        item.rank = kept.len() + 1;
        kept.push(item);
    }
    kept
}

fn try_advise(question: &str, rule_selected: &[SourceRanking]) -> Option<Vec<SourceRanking>> {
    let config_path = std::env::var_os(ADVISOR_CONFIG_ENV).filter(|v| !v.is_empty())?;
    let config = load_config(Path::new(&config_path))?;
    if let Some(required) = &config.requires_env {
        if std::env::var_os(required).is_none_or(|v| v.is_empty()) {
            return None;
        }
    }
    verify_executable_digest(&config.advisor, &config.sha256)?;
    let stdin_bytes = build_request_json(question, rule_selected, &config.send_fields);
    let stdout_bytes = run_advisor(&config, stdin_bytes)?;
    apply_advisor_reply(&stdout_bytes, rule_selected)
}

fn load_config(path: &Path) -> Option<AdvisorConfig> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_CONFIG_BYTES {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    parse_config(&text)
}

fn strip_quotes(raw: &str) -> &str {
    let trimmed = raw.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

fn valid_env_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['=', '\0'])
}

fn valid_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn parse_inline_list(raw: &str) -> Option<Vec<String>> {
    let trimmed = raw.trim();
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?.trim();
    if inner.is_empty() {
        return Some(Vec::new());
    }
    let mut items = Vec::new();
    for part in inner.split(',') {
        let item = strip_quotes(part);
        if item.is_empty() {
            return None;
        }
        items.push(item.to_owned());
    }
    Some(items)
}

fn parse_config(text: &str) -> Option<AdvisorConfig> {
    let mut advisor: Option<PathBuf> = None;
    let mut sha256: Option<String> = None;
    let mut timeout_ms: Option<u64> = None;
    let mut send_fields: Option<BTreeSet<SendField>> = None;
    let mut requires_env: Option<String> = None;
    let mut pass_env: Option<Vec<String>> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, raw_value) = line.split_once(':')?;
        let key = key.trim();
        let value = raw_value.trim();
        match key {
            "advisor" => {
                if advisor.is_some() {
                    return None;
                }
                let unquoted = strip_quotes(value);
                if unquoted.is_empty() || unquoted.contains('\0') {
                    return None;
                }
                let path = PathBuf::from(unquoted);
                if !path.is_absolute() {
                    return None;
                }
                advisor = Some(path);
            }
            "sha256" => {
                if sha256.is_some() {
                    return None;
                }
                let hex = strip_quotes(value);
                if !valid_sha256_hex(hex) {
                    return None;
                }
                sha256 = Some(hex.to_owned());
            }
            "timeout_ms" => {
                if timeout_ms.is_some() {
                    return None;
                }
                let parsed = strip_quotes(value).parse::<u64>().ok()?;
                if parsed == 0 || parsed > MAX_TIMEOUT_MS {
                    return None;
                }
                timeout_ms = Some(parsed);
            }
            "send_fields" => {
                if send_fields.is_some() {
                    return None;
                }
                let items = parse_inline_list(value)?;
                if items.is_empty() {
                    return None;
                }
                let mut set = BTreeSet::new();
                for item in items {
                    let field = match item.as_str() {
                        "question" => SendField::Question,
                        "path" => SendField::Path,
                        "reason" => SendField::Reason,
                        "rank" => SendField::Rank,
                        _ => return None,
                    };
                    if !set.insert(field) {
                        return None;
                    }
                }
                send_fields = Some(set);
            }
            "requires_env" => {
                if requires_env.is_some() {
                    return None;
                }
                let name = strip_quotes(value);
                if !valid_env_name(name) {
                    return None;
                }
                requires_env = Some(name.to_owned());
            }
            "pass_env" => {
                if pass_env.is_some() {
                    return None;
                }
                let items = parse_inline_list(value)?;
                if items.iter().any(|name| !valid_env_name(name)) {
                    return None;
                }
                pass_env = Some(items);
            }
            _ => return None,
        }
    }

    Some(AdvisorConfig {
        advisor: advisor?,
        sha256: sha256?,
        timeout_ms: timeout_ms?,
        send_fields: send_fields?,
        requires_env,
        pass_env: pass_env.unwrap_or_default(),
    })
}

fn verify_executable_digest(path: &Path, expected_sha256: &str) -> Option<()> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_EXECUTABLE_BYTES {
        return None;
    }
    let file = File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(meta.len().min(MAX_EXECUTABLE_BYTES) as usize);
    file.take(MAX_EXECUTABLE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_EXECUTABLE_BYTES {
        return None;
    }
    (sha256_hex(&bytes) == expected_sha256).then_some(())
}

fn build_request_json(
    question: &str,
    rule_selected: &[SourceRanking],
    send_fields: &BTreeSet<SendField>,
) -> Vec<u8> {
    let mut root = BTreeMap::new();
    if send_fields.contains(&SendField::Question) {
        root.insert("question".to_owned(), Json::Str(question.to_owned()));
    }
    let options = rule_selected
        .iter()
        .map(|item| {
            let mut obj = BTreeMap::new();
            if send_fields.contains(&SendField::Path) {
                obj.insert("path".to_owned(), Json::Str(item.locator.clone()));
            }
            if send_fields.contains(&SendField::Reason) {
                obj.insert(
                    "reason".to_owned(),
                    Json::Str(one_line_reason(&item.match_reason)),
                );
            }
            if send_fields.contains(&SendField::Rank) {
                obj.insert("rank".to_owned(), Json::Int(item.rank as i64));
            }
            Json::Obj(obj)
        })
        .collect();
    root.insert("options".to_owned(), Json::Arr(options));
    Json::Obj(root).to_bytes()
}

fn run_advisor(config: &AdvisorConfig, stdin_bytes: Vec<u8>) -> Option<Vec<u8>> {
    let mut command = Command::new(&config.advisor);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_clear();
    for name in &config.pass_env {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }

    let deadline = Instant::now() + Duration::from_millis(config.timeout_ms);
    let mut child = command.spawn().ok()?;
    let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else {
        kill_and_reap(child);
        return None;
    };

    // Write stdin and drain bounded stdout on helper threads so neither blocks past deadline.
    thread::spawn(move || {
        let _ = stdin.write_all(&stdin_bytes);
        drop(stdin);
    });

    let (stdout_tx, stdout_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let res = (&mut stdout)
            .take((MAX_STDOUT_BYTES + 1) as u64)
            .read_to_end(&mut buf);
        drop(stdout);
        let outcome = match res {
            Ok(_) if buf.len() <= MAX_STDOUT_BYTES => Ok(buf),
            _ => Err(()),
        };
        let _ = stdout_tx.send(outcome);
    });

    let stdout_bytes = match recv_until(&stdout_rx, deadline) {
        Some(Ok(bytes)) => bytes,
        _ => {
            kill_and_reap(child);
            return None;
        }
    };

    match wait_exit_until(&mut child, deadline) {
        Some(status) if status.success() => Some(stdout_bytes),
        Some(_) => None,
        None => {
            kill_and_reap(child);
            None
        }
    }
}

fn recv_until<T>(rx: &mpsc::Receiver<T>, deadline: Instant) -> Option<T> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    rx.recv_timeout(remaining).ok()
}

fn wait_exit_until(child: &mut Child, deadline: Instant) -> Option<ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Err(_) => return None,
            Ok(None) => {}
        }
        let remaining = deadline.checked_duration_since(Instant::now())?;
        thread::sleep(remaining.min(Duration::from_millis(2)));
    }
}

fn kill_and_reap(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn apply_advisor_reply(
    stdout_bytes: &[u8],
    rule_selected: &[SourceRanking],
) -> Option<Vec<SourceRanking>> {
    let parsed = Json::parse(stdout_bytes)?;
    let Some(Json::Arr(path_values)) = parsed.get("paths") else {
        return None;
    };
    if path_values.is_empty() {
        return None;
    }

    let mut seen = BTreeSet::new();
    let mut advised = Vec::with_capacity(path_values.len());
    for value in path_values {
        let Json::Str(path) = value else {
            return None;
        };
        if !seen.insert(path.as_str()) {
            return None;
        }
        let found = rule_selected.iter().find(|item| &item.locator == path)?;
        let mut copy = found.clone();
        copy.rank = advised.len() + 1;
        advised.push(copy);
    }
    Some(advised)
}
