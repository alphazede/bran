//! Brief-mode deterministic candidate selection and optional caller-scoped advisor (DES-07).
//!
//! Reads advisor configuration exclusively from the file named by `BRAN_ADVISOR_CONFIG`
//! (never from the queried repository tree). Any missing configuration, policy check
//! failure, timeout, or invalid advisor output silently falls back to the deterministic
//! rule answer.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bran_document::canonical::{sha256_hex, Json};

use crate::{one_line_reason, SourceRanking};

const ADVISOR_CONFIG_ENV: &str = "BRAN_ADVISOR_CONFIG";

// Deterministic rule threshold: keep the top result plus ranked results whose
// structural relevance is at least 3/10 (0.30) of the top's, as an ordered
// subsequence of the bounded shortlist before --top truncates the output.
const RULE_RATIO_NUM: u128 = 3;
const RULE_RATIO_DEN: u128 = 10;

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
    Excerpt,
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
    roots: &[&Path],
) -> (Vec<SourceRanking>, SelectionSource) {
    let rule_selected = rule_select(ranked);
    if rule_selected.is_empty() {
        return (rule_selected, SelectionSource::Rule);
    }
    if let Some(advised) = try_advise(question, &rule_selected, roots) {
        return (advised, SelectionSource::Advisor);
    }
    (rule_selected, SelectionSource::Rule)
}

/// The structural relevance component of the brief sort key, so the cut
/// measures the same quantity the ranking orders by.
fn candidate_score(ranking: &SourceRanking) -> u128 {
    u128::from(ranking.brief_score.0)
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

fn try_advise(
    question: &str,
    rule_selected: &[SourceRanking],
    roots: &[&Path],
) -> Option<Vec<SourceRanking>> {
    let config_path = std::env::var_os(ADVISOR_CONFIG_ENV).filter(|v| !v.is_empty())?;
    let config = load_config(Path::new(&config_path), roots)?;
    if let Some(required) = &config.requires_env {
        if std::env::var_os(required).is_none_or(|v| v.is_empty()) {
            return None;
        }
    }
    let executable = open_and_verify_executable(&config.advisor, &config.sha256)?;
    let stdin_bytes = build_request_json(question, rule_selected, &config.send_fields, roots);
    let stdout_bytes = run_advisor(&config, executable, stdin_bytes)?;
    apply_advisor_reply(&stdout_bytes, rule_selected)
}

#[cfg(unix)]
fn open_readonly_nonblock(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_readonly_nonblock(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

#[cfg(unix)]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file_identity(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    true
}

fn canonical_parent_dir(path: &Path) -> Option<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let parent = abs.parent()?;
    fs::canonicalize(parent).ok()
}

fn load_config(path: &Path, roots: &[&Path]) -> Option<AdvisorConfig> {
    let file = open_readonly_nonblock(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_CONFIG_BYTES {
        return None;
    }

    let canonical_config = fs::canonicalize(path).ok()?;
    let canonical_meta = fs::metadata(&canonical_config).ok()?;
    if !same_file_identity(&meta, &canonical_meta) {
        return None;
    }
    let canonical_parent = canonical_parent_dir(path)?;
    for root in roots {
        let canonical_root = fs::canonicalize(root).ok()?;
        if canonical_config.starts_with(&canonical_root)
            || canonical_parent.starts_with(&canonical_root)
        {
            return None;
        }
    }

    let mut text = String::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() as u64 > MAX_CONFIG_BYTES {
        return None;
    }
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
                        "excerpt" => SendField::Excerpt,
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

#[cfg(unix)]
fn ensure_fd_above_stdio(file: File) -> std::io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
    if file.as_raw_fd() >= 3 {
        return Ok(file);
    }
    let old_fd = file.into_raw_fd();
    // SAFETY: `old_fd` is a valid open descriptor owned here and closed on all paths.
    let new_fd = unsafe { libc::fcntl(old_fd, libc::F_DUPFD_CLOEXEC, 3) };
    // SAFETY: `old_fd` is no longer referenced after `into_raw_fd()`.
    unsafe { libc::close(old_fd) };
    if new_fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `new_fd` is a newly duplicated open descriptor owned by this `File`.
    Ok(unsafe { File::from_raw_fd(new_fd) })
}

fn open_and_verify_executable(path: &Path, expected_sha256: &str) -> Option<File> {
    if !path.is_absolute() {
        return None;
    }
    let mut file = open_readonly_nonblock(path).ok()?;
    #[cfg(unix)]
    {
        file = ensure_fd_above_stdio(file).ok()?;
    }
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_EXECUTABLE_BYTES {
        return None;
    }
    let mut bytes = Vec::with_capacity(meta.len().min(MAX_EXECUTABLE_BYTES) as usize);
    (&file)
        .take(MAX_EXECUTABLE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_EXECUTABLE_BYTES {
        return None;
    }
    if sha256_hex(&bytes) != expected_sha256 {
        return None;
    }
    file.seek(SeekFrom::Start(0)).ok()?;
    Some(file)
}

const EXCERPT_MAX_BYTES: usize = 900;
// Leading lines fill the excerpt up to this many bytes; after that only lines matching a query term.
const EXCERPT_LEAD_BYTES: usize = 300;
const EXCERPT_READ_BYTES: u64 = 256 * 1024;

/// Leading non-empty lines up to `EXCERPT_LEAD_BYTES`, then the lines that contain a
/// query term, cut to `EXCERPT_MAX_BYTES` at a character boundary.
fn build_excerpt(text: &str, terms: &[String]) -> String {
    let mut out = String::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if out.len() >= EXCERPT_MAX_BYTES {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if out.len() < EXCERPT_LEAD_BYTES || terms.iter().any(|term| lower.contains(term)) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        }
    }
    let mut end = out.len().min(EXCERPT_MAX_BYTES);
    while !out.is_char_boundary(end) {
        end -= 1;
    }
    out.truncate(end);
    out
}

/// The excerpt of a regular file inside `root`; `None` (sent as empty) for anything else,
/// including text that is not valid UTF-8.
fn read_excerpt(root: &Path, locator: &str, terms: &[String]) -> Option<String> {
    let path = root.join(locator);
    let file = open_readonly_nonblock(&path).ok()?;
    let meta = file.metadata().ok()?;
    let canonical = fs::canonicalize(&path).ok()?;
    if !meta.is_file()
        || !canonical.starts_with(fs::canonicalize(root).ok()?)
        || !same_file_identity(&meta, &fs::metadata(&canonical).ok()?)
    {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(EXCERPT_READ_BYTES).read_to_end(&mut bytes).ok()?;
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        // shortcut: only the first EXCERPT_READ_BYTES are read, so matches past them are not seen.
        Err(e) if e.error_len().is_none() && meta.len() > bytes.len() as u64 => {
            std::str::from_utf8(&bytes[..e.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    Some(build_excerpt(text, terms))
}

fn candidate_excerpt(item: &SourceRanking, roots: &[&Path], terms: &[String]) -> String {
    let root = if item.bundle.is_empty() {
        roots.first()
    } else {
        roots.iter().find(|root| **root == Path::new(&item.bundle))
    };
    root.and_then(|root| read_excerpt(root, &item.locator, terms))
        .unwrap_or_default()
}

fn build_request_json(
    question: &str,
    rule_selected: &[SourceRanking],
    send_fields: &BTreeSet<SendField>,
    roots: &[&Path],
) -> Vec<u8> {
    let is_multi_root = rule_selected.iter().any(|item| !item.bundle.is_empty());
    let terms: Vec<String> = if send_fields.contains(&SendField::Excerpt) {
        let (terms, entities) = crate::query_terms_and_entities(question);
        terms.into_iter().chain(entities).collect()
    } else {
        Vec::new()
    };
    let mut root = BTreeMap::new();
    if send_fields.contains(&SendField::Question) {
        root.insert("question".to_owned(), Json::Str(question.to_owned()));
    }
    let options = rule_selected
        .iter()
        .map(|item| {
            let mut obj = BTreeMap::new();
            // Every option carries its id so an advisor can answer with per-id scores.
            obj.insert("id".to_owned(), Json::Str(item.rank.to_string()));
            if is_multi_root && !item.bundle.is_empty() && send_fields.contains(&SendField::Path) {
                obj.insert("bundle".to_owned(), Json::Str(item.bundle.clone()));
            }
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
            if send_fields.contains(&SendField::Excerpt) {
                obj.insert(
                    "excerpt".to_owned(),
                    Json::Str(candidate_excerpt(item, roots, &terms)),
                );
            }
            Json::Obj(obj)
        })
        .collect();
    root.insert("options".to_owned(), Json::Arr(options));
    Json::Obj(root).to_bytes()
}

fn build_verified_command(config: &AdvisorConfig, executable: &File) -> Command {
    #[cfg(target_os = "linux")]
    let mut command = {
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new(format!("/proc/self/fd/{}", executable.as_raw_fd()));
        cmd.arg0(&config.advisor);
        cmd
    };
    #[cfg(not(target_os = "linux"))]
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

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        #[cfg(target_os = "linux")]
        let raw_fd = {
            use std::os::fd::AsRawFd;
            executable.as_raw_fd()
        };
        // SAFETY: runs in the forked child prior to `exec` using async-signal-safe
        // `setpgid`/`fcntl`/`lseek` calls on valid open descriptors.
        unsafe {
            command.pre_exec(move || {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                #[cfg(target_os = "linux")]
                {
                    let flags = libc::fcntl(raw_fd, libc::F_GETFD);
                    if flags < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::fcntl(raw_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let _ = libc::lseek(raw_fd, 0, libc::SEEK_SET);
                }
                Ok(())
            });
        }
    }

    command
}

fn run_advisor(config: &AdvisorConfig, executable: File, stdin_bytes: Vec<u8>) -> Option<Vec<u8>> {
    let mut command = build_verified_command(config, &executable);
    let deadline = Instant::now() + Duration::from_millis(config.timeout_ms);
    let mut child = command.spawn().ok()?;
    drop(executable);

    let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else {
        kill_and_reap(child);
        return None;
    };

    // Write stdin and drain bounded stdout on helper threads so neither blocks past deadline.
    let (stdin_tx, stdin_rx) = mpsc::channel();
    let stdin_handle = thread::spawn(move || {
        let res = stdin.write_all(&stdin_bytes).and_then(|()| stdin.flush());
        drop(stdin);
        let _ = stdin_tx.send(res.map_err(|_| ()));
    });

    let (stdout_tx, stdout_rx) = mpsc::channel();
    let stdout_handle = thread::spawn(move || {
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
            join_io(stdin_handle, stdout_handle);
            return None;
        }
    };

    if !matches!(recv_until(&stdin_rx, deadline), Some(Ok(()))) {
        kill_and_reap(child);
        join_io(stdin_handle, stdout_handle);
        return None;
    }

    let status = match wait_exit_until(&mut child, deadline) {
        WaitOutcome::Exited(status) => status,
        WaitOutcome::TimedOutOrError => {
            kill_and_reap(child);
            join_io(stdin_handle, stdout_handle);
            return None;
        }
    };
    join_io(stdin_handle, stdout_handle);

    if status.success() && Instant::now() <= deadline {
        Some(stdout_bytes)
    } else {
        None
    }
}

fn join_io(stdin_handle: JoinHandle<()>, stdout_handle: JoinHandle<()>) {
    let _ = stdin_handle.join();
    let _ = stdout_handle.join();
}

fn recv_until<T>(rx: &mpsc::Receiver<T>, deadline: Instant) -> Option<T> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    rx.recv_timeout(remaining).ok()
}

enum WaitOutcome {
    Exited(ExitStatus),
    TimedOutOrError,
}

fn wait_exit_until(child: &mut Child, deadline: Instant) -> WaitOutcome {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if Instant::now() <= deadline {
                    WaitOutcome::Exited(status)
                } else {
                    WaitOutcome::TimedOutOrError
                };
            }
            Err(_) => return WaitOutcome::TimedOutOrError,
            Ok(None) => {}
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return WaitOutcome::TimedOutOrError;
        };
        thread::sleep(remaining.min(Duration::from_millis(2)));
    }
}

fn kill_and_reap(mut child: Child) {
    #[cfg(unix)]
    {
        let pgid = child.id() as libc::pid_t;
        if pgid > 0 {
            // SAFETY: `child` has not been reaped yet, so `pgid` still identifies
            // the process group created via `setpgid(0, 0)`.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn resolve_candidate_index(
    value: &Json,
    rule_selected: &[SourceRanking],
    is_multi_root: bool,
) -> Option<usize> {
    match value {
        Json::Str(token) => {
            let mut matched: Option<usize> = None;
            for (idx, item) in rule_selected.iter().enumerate() {
                let id_match = is_multi_root && item.rank.to_string() == *token;
                let path_match = &item.locator == token;
                if id_match || path_match {
                    if matched.is_some() {
                        return None;
                    }
                    matched = Some(idx);
                }
            }
            matched
        }
        Json::Obj(obj) if is_multi_root => {
            let Some(Json::Str(bundle)) = obj.get("bundle") else {
                return None;
            };
            let Some(Json::Str(path)) = obj.get("path") else {
                return None;
            };
            let mut matched: Option<usize> = None;
            for (idx, item) in rule_selected.iter().enumerate() {
                if &item.bundle == bundle && &item.locator == path {
                    if matched.is_some() {
                        return None;
                    }
                    matched = Some(idx);
                }
            }
            matched
        }
        _ => None,
    }
}

/// True when `token` matches the JSON number grammar.
fn is_json_number(token: &str) -> bool {
    let digits = |s: &str| s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let rest = token.strip_prefix('-').unwrap_or(token);
    let int_len = digits(rest);
    if int_len == 0 || (int_len > 1 && rest.starts_with('0')) {
        return false;
    }
    let mut rest = &rest[int_len..];
    if let Some(frac) = rest.strip_prefix('.') {
        let frac_len = digits(frac);
        if frac_len == 0 {
            return false;
        }
        rest = &frac[frac_len..];
    }
    if let Some(exp) = rest.strip_prefix(['e', 'E']) {
        let exp = exp.strip_prefix(['+', '-']).unwrap_or(exp);
        let exp_len = digits(exp);
        return exp_len > 0 && exp_len == exp.len();
    }
    rest.is_empty()
}

/// Give each number a temporary integer index for the integer-only JSON parser.
/// Keep the original lexemes for validation and ordering.
fn index_numbers(bytes: &[u8]) -> Option<(Vec<u8>, Vec<&str>)> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut numbers = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => {
                // '"' and '\\' never occur inside a multi-byte UTF-8 sequence.
                let start = at;
                at += 1;
                while at < bytes.len() && bytes[at] != b'"' {
                    at += if bytes[at] == b'\\' { 2 } else { 1 };
                }
                at = (at + 1).min(bytes.len());
                out.extend_from_slice(&bytes[start..at]);
            }
            b'-' | b'0'..=b'9' => {
                let start = at;
                while at < bytes.len()
                    && matches!(bytes[at], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                {
                    at += 1;
                }
                let token = std::str::from_utf8(&bytes[start..at]).ok()?;
                if !is_json_number(token) {
                    return None;
                }
                out.extend_from_slice(numbers.len().to_string().as_bytes());
                numbers.push(token);
            }
            byte => {
                out.push(byte);
                at += 1;
            }
        }
    }
    Some((out, numbers))
}

fn score_number(token: &str) -> Option<f64> {
    let number = token.parse::<f64>().ok()?;
    if !(0.0..=1.0).contains(&number) {
        return None;
    }
    // Check decimals that a float rounds onto an endpoint of the allowed range.
    if number == 0.0 || number == 1.0 {
        let mantissa = token.split(['e', 'E']).next()?;
        let mut significant = mantissa
            .bytes()
            .filter(u8::is_ascii_digit)
            .skip_while(|&digit| digit == b'0');
        if (number == 0.0 && token.starts_with('-') && significant.next().is_some())
            || (number == 1.0
                && significant.next() == Some(b'1')
                && significant.any(|digit| digit != b'0'))
        {
            return None;
        }
    }
    Some(number)
}

/// Orders the rule candidates by advisor score, descending; ties and unscored candidates
/// keep rule order, unscored after scored. Any invalid entry rejects the whole answer.
fn apply_scores(
    scores: &Json,
    numbers: &[&str],
    rule_selected: &[SourceRanking],
) -> Option<Vec<SourceRanking>> {
    let Json::Arr(entries) = scores else {
        return None;
    };
    if entries.is_empty() {
        return None;
    }
    let mut by_index = vec![None; rule_selected.len()];
    for entry in entries {
        let Json::Obj(map) = entry else {
            return None;
        };
        let (Some(Json::Str(id)), Some(Json::Int(number_index)), 2) =
            (map.get("id"), map.get("score"), map.len())
        else {
            return None;
        };
        let score = score_number(numbers.get(usize::try_from(*number_index).ok()?)?)?;
        let idx = rule_selected
            .iter()
            .position(|item| item.rank.to_string() == *id)?;
        if by_index[idx].replace(score).is_some() {
            return None;
        }
    }
    let mut order: Vec<usize> = (0..rule_selected.len()).collect();
    order.sort_by(|&left, &right| {
        by_index[right]
            .partial_cmp(&by_index[left])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Some(
        order
            .into_iter()
            .enumerate()
            .map(|(position, idx)| {
                let mut copy = rule_selected[idx].clone();
                copy.rank = position + 1;
                copy
            })
            .collect(),
    )
}

fn apply_advisor_reply(
    stdout_bytes: &[u8],
    rule_selected: &[SourceRanking],
) -> Option<Vec<SourceRanking>> {
    let (indexed, numbers) = index_numbers(stdout_bytes)?;
    let parsed = Json::parse_unique(&indexed)?;
    if let Some(scores) = parsed.get("scores") {
        return match parsed.get("paths") {
            Some(_) => None,
            None => apply_scores(scores, &numbers, rule_selected),
        };
    }
    let Some(Json::Arr(path_values)) = parsed.get("paths") else {
        return None;
    };
    if path_values.is_empty() {
        return None;
    }

    let is_multi_root = rule_selected.iter().any(|item| !item.bundle.is_empty());
    let mut seen = BTreeSet::new();
    let mut advised = Vec::with_capacity(path_values.len());
    for value in path_values {
        let idx = resolve_candidate_index(value, rule_selected, is_multi_root)?;
        if !seen.insert(idx) {
            return None;
        }
        let mut copy = rule_selected[idx].clone();
        copy.rank = advised.len() + 1;
        advised.push(copy);
    }
    Some(advised)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    #[cfg(target_os = "linux")]
    fn verified_executable_runs_original_bytes_after_path_replacement() {
        let dir = std::env::temp_dir().join(format!("bran-select-toctou-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let script_path = dir.join("advisor.sh");
        let poison_marker = dir.join("poison-ran");
        let original = b"#!/bin/sh\ncat >/dev/null\nprintf '{\"paths\":[\"a.md\"]}'\n";
        fs::write(&script_path, original).unwrap();
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

        let digest = sha256_hex(original);
        let verified_fd = open_and_verify_executable(&script_path, &digest)
            .expect("original script passes digest");

        // Atomically replace the file at `script_path` after verification and before spawn.
        let replacement_path = dir.join("advisor.sh.new");
        let replacement = format!(
            "#!/bin/sh\ncat >/dev/null\ntouch '{}'\nprintf '{{\"paths\":[\"b.md\"]}}'\n",
            poison_marker.display()
        );
        fs::write(&replacement_path, replacement).unwrap();
        fs::set_permissions(&replacement_path, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&replacement_path, &script_path).unwrap();

        let config = AdvisorConfig {
            advisor: script_path.clone(),
            sha256: digest,
            timeout_ms: 5_000,
            send_fields: BTreeSet::from([SendField::Path]),
            requires_env: None,
            pass_env: Vec::new(),
        };
        let stdout = run_advisor(&config, verified_fd, b"{}".to_vec())
            .expect("verified fd runs despite path replacement");
        assert!(!poison_marker.exists(), "replacement script must not run");
        assert_eq!(stdout, br#"{"paths":["a.md"]}"#);

        let _ = fs::remove_dir_all(&dir);
    }

    fn candidate(rank: usize) -> SourceRanking {
        SourceRanking {
            bundle: String::new(),
            id: crate::NodeId::parse(format!("n{rank}")).unwrap(),
            locator: format!("doc-{rank}.md"),
            rank,
            exact_matches: 1,
            partial_matches: 0,
            match_reason: String::new(),
            active: 0,
            canonical: 0,
            public_safe: 0,
            confidence: 0,
            freshness: String::new(),
            brief_tier: 0,
            brief_score: Default::default(),
        }
    }

    fn scored(reply: &str, count: usize) -> Option<Vec<String>> {
        let rule: Vec<SourceRanking> = (1..=count).map(candidate).collect();
        let advised = apply_advisor_reply(reply.as_bytes(), &rule)?;
        for (index, item) in advised.iter().enumerate() {
            assert_eq!(item.rank, index + 1, "ranks follow the advised order");
        }
        Some(advised.into_iter().map(|item| item.locator).collect())
    }

    #[test]
    fn scores_order_descending_with_rule_order_for_ties_and_unscored() {
        let reply =
            r#"{"scores":[{"id":"3","score":0.5},{"id":"1","score":0.5},{"id":"4","score":0.9}]}"#;
        assert_eq!(
            scored(reply, 5).unwrap(),
            ["doc-4.md", "doc-1.md", "doc-3.md", "doc-2.md", "doc-5.md"],
            "descending score, ties and unscored keep rule order, unscored last"
        );
        let reply = r#"{"scores":[{"score":1,"id":"2"},{"id":"1","score":0}]}"#;
        assert_eq!(scored(reply, 2).unwrap(), ["doc-2.md", "doc-1.md"]);
        let reply = r#"{"scores":[{"id":"2","score":1e-1},{"id":"1","score":5E-2}]}"#;
        assert_eq!(scored(reply, 2).unwrap(), ["doc-2.md", "doc-1.md"]);
        let reply = r#"{"scores":[{"id":"1","score":0.5},{"id":"2","score":0.5000000001}]}"#;
        assert_eq!(scored(reply, 2).unwrap(), ["doc-2.md", "doc-1.md"]);
        let reply = r#"{"paths":["doc-2.md"]}"#;
        assert_eq!(
            scored(reply, 2).unwrap(),
            ["doc-2.md"],
            "path replies unchanged"
        );
    }

    #[test]
    fn invalid_score_replies_reject_the_whole_answer() {
        for reply in [
            r#"{"scores":[{"id":"9","score":0.5}]}"#,
            r#"{"scores":[{"id":"0","score":0.5}]}"#,
            r#"{"scores":[{"id":"1","score":0.5},{"id":"1","score":0.4}]}"#,
            r#"{"scores":[{"id":"1","score":1.5}]}"#,
            r#"{"scores":[{"id":"1","score":-0.1}]}"#,
            r#"{"scores":[{"id":"1","score":1.0000000001}]}"#,
            r#"{"scores":[{"id":"1","score":-0.0000000001}]}"#,
            r#"{"scores":[{"id":"1","score":1.00000000000000001}]}"#,
            r#"{"scores":[{"id":"1","score":-1e-999}]}"#,
            r#"{"scores":[{"id":"999","\u0069d":"1","score":0.5}]}"#,
            r#"{"paths":["unknown.md"],"paths":["doc-1.md"]}"#,
            r#"{"scores":[{"id":"1","score":2}]}"#,
            r#"{"scores":[{"id":"1","score":1e999}]}"#,
            r#"{"scores":[{"id":"1","score":NaN}]}"#,
            r#"{"scores":[{"id":"1","score":Infinity}]}"#,
            r#"{"scores":[{"id":"1","score":"0.5"}]}"#,
            r#"{"scores":[{"id":"1","score":null}]}"#,
            r#"{"scores":[{"id":"1"}]}"#,
            r#"{"scores":[{"id":1,"score":0.5}]}"#,
            r#"{"scores":[{"id":"1","score":0.5,"note":"x"}]}"#,
            r#"{"scores":[{"id":"1","score":.5}]}"#,
            r#"{"scores":[{"id":"1","score":01}]}"#,
            r#"{"scores":[{"id":"1","score":1.}]}"#,
            r#"{"scores":[{"id":"1","score":0.5e}]}"#,
            r#"{"scores":[{"id":"1","score":+0.5}]}"#,
            r#"{"scores":[]}"#,
            r#"{"scores":{"1":0.5}}"#,
            r#"{"scores":[0.5]}"#,
            r#"{"scores":[{"id":"1","score":0.5}],"paths":["doc-1.md"]}"#,
            r#"{"paths":["doc-1.md"],"scores":[{"id":"1","score":99999999999}]}"#,
            r#"{"scores":[{"id":"1","score":0.5}]"#,
        ] {
            assert_eq!(scored(reply, 3), None, "rejected: {reply}");
        }
    }

    #[test]
    fn advisor_number_parsing_leaves_strings_untouched() {
        let reply = r#"{"paths":["a\"1.5.md"]}"#;
        let mut rule = [candidate(1), candidate(2)];
        rule[1].locator = r#"a"1.5.md"#.to_owned();
        let advised = apply_advisor_reply(reply.as_bytes(), &rule).unwrap();
        assert_eq!(advised.len(), 1);
        assert_eq!(advised[0].locator, rule[1].locator);
        let reply = r#"{"note":"a\"1.5","scores":[{"id":"2","score":0.5}]}"#;
        assert_eq!(
            apply_advisor_reply(reply.as_bytes(), &rule).unwrap()[0].locator,
            rule[1].locator
        );
    }

    #[test]
    fn excerpt_keeps_leading_lines_then_matching_lines_within_bound() {
        let terms = vec!["lantern".to_owned()];
        let text = "# Title\n\n  intro line  \nfiller\nmore filler\nthe Lantern room\n";
        assert_eq!(
            build_excerpt(text, &terms),
            "# Title\nintro line\nfiller\nmore filler\nthe Lantern room"
        );

        let lead = "x".repeat(EXCERPT_LEAD_BYTES);
        let text = format!("{lead}\nfiller\nlantern one\nfiller\nlantern two\n");
        assert_eq!(
            build_excerpt(&text, &terms),
            format!("{lead}\nlantern one\nlantern two"),
            "past the leading bytes only matching lines are kept"
        );

        // A two-byte character straddles the byte bound: the cut lands before it.
        let text = format!("{}é lantern", "a".repeat(EXCERPT_MAX_BYTES - 1));
        let excerpt = build_excerpt(&text, &terms);
        assert_eq!(excerpt, "a".repeat(EXCERPT_MAX_BYTES - 1));
        let long = "lantern é ".repeat(500);
        let excerpt = build_excerpt(&long, &terms);
        assert!(excerpt.len() <= EXCERPT_MAX_BYTES && excerpt.len() > EXCERPT_MAX_BYTES - 4);
    }

    #[test]
    #[cfg(unix)]
    fn read_excerpt_refuses_non_utf8_escapes_and_non_files() {
        let dir = std::env::temp_dir().join(format!("bran-select-excerpt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let root = dir.join("tree");
        fs::create_dir_all(&root).unwrap();
        let terms = vec!["lantern".to_owned()];
        fs::write(root.join("good.md"), "lantern\n").unwrap();
        fs::write(root.join("binary.md"), b"lantern \xff\xfe\n").unwrap();
        fs::write(dir.join("outside.md"), "lantern secret\n").unwrap();
        std::os::unix::fs::symlink(dir.join("outside.md"), root.join("link.md")).unwrap();
        fs::create_dir_all(root.join("sub.md")).unwrap();

        assert_eq!(
            read_excerpt(&root, "good.md", &terms).as_deref(),
            Some("lantern")
        );
        assert_eq!(
            read_excerpt(&root, "binary.md", &terms),
            None,
            "invalid UTF-8"
        );
        assert_eq!(
            read_excerpt(&root, "link.md", &terms),
            None,
            "symlink out of root"
        );
        assert_eq!(
            read_excerpt(&root, "../outside.md", &terms),
            None,
            "path out of root"
        );
        assert_eq!(read_excerpt(&root, "sub.md", &terms), None, "not a file");
        assert_eq!(read_excerpt(&root, "missing.md", &terms), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn excerpt_accepts_a_cut_character_only_when_more_bytes_follow() {
        let dir =
            std::env::temp_dir().join(format!("bran-select-excerpt-cut-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("notes.md");
        let mut bytes = vec![b'a'; EXCERPT_READ_BYTES as usize - 1];
        bytes.push(0xc3);
        fs::write(&path, &bytes).unwrap();
        let incomplete = read_excerpt(&dir, "notes.md", &[]);
        bytes.push(0xa9);
        fs::write(&path, &bytes).unwrap();
        let split = read_excerpt(&dir, "notes.md", &[]);
        fs::remove_dir_all(dir).unwrap();
        assert_eq!(incomplete, None);
        assert_eq!(split, Some("a".repeat(EXCERPT_MAX_BYTES)));
    }

    #[test]
    #[cfg(unix)]
    fn late_exit_past_deadline_is_rejected_even_if_child_exited_zero() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn sh");
        // Wait until the child is definitely finished, then check against an expired deadline.
        thread::sleep(Duration::from_millis(30));
        let expired = Instant::now() - Duration::from_millis(5);
        assert!(matches!(
            wait_exit_until(&mut child, expired),
            WaitOutcome::TimedOutOrError
        ));
        if child.try_wait().unwrap().is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}
