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

// Deterministic rule threshold: keep the top result plus ranked results scoring
// at least 3/10 (0.30) of the top score, as an ordered subsequence of the
// bounded shortlist before --top truncates the output.
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
    let stdin_bytes = build_request_json(question, rule_selected, &config.send_fields);
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

fn build_request_json(
    question: &str,
    rule_selected: &[SourceRanking],
    send_fields: &BTreeSet<SendField>,
) -> Vec<u8> {
    let is_multi_root = rule_selected.iter().any(|item| !item.bundle.is_empty());
    let mut root = BTreeMap::new();
    if send_fields.contains(&SendField::Question) {
        root.insert("question".to_owned(), Json::Str(question.to_owned()));
    }
    let options = rule_selected
        .iter()
        .map(|item| {
            let mut obj = BTreeMap::new();
            if is_multi_root {
                obj.insert("id".to_owned(), Json::Str(item.rank.to_string()));
                if !item.bundle.is_empty() && send_fields.contains(&SendField::Path) {
                    obj.insert("bundle".to_owned(), Json::Str(item.bundle.clone()));
                }
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
        let _ = child.wait();
    }
}
