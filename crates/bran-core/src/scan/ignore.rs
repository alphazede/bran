//! Pure, deterministic BRAN ignore matching for repository scans.

use std::fmt;

pub const MAX_BRANIGNORE_BYTES: usize = 64 * 1024;
/// Backward-compatible size constant for callers that still recognize `.okfignore`.
pub const MAX_OKFIGNORE_BYTES: usize = MAX_BRANIGNORE_BYTES;
const BUILTIN_COMPONENTS: [&str; 8] = [
    ".git",
    ".bearing",
    "target",
    "node_modules",
    "vendor",
    "generated",
    "cache",
    "caches",
];

/// A malformed root ignore-file input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IgnoreDiagnostic {
    InputTooLarge { actual_bytes: usize },
    InvalidUtf8,
    InvalidRule { line: usize, reason: RuleError },
}

/// The exact reason an individual rule cannot be used safely.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuleError {
    EmptyPattern,
    AbsolutePath,
    Traversal,
    MultipleWildcards,
}

impl fmt::Display for IgnoreDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputTooLarge { actual_bytes } => write!(
                f,
                "ignore file is {actual_bytes} bytes; maximum is {MAX_BRANIGNORE_BYTES}"
            ),
            Self::InvalidUtf8 => write!(f, "ignore file must be valid UTF-8"),
            Self::InvalidRule { line, reason } => write!(f, "ignore file line {line}: {reason}"),
        }
    }
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::EmptyPattern => "empty pattern",
            Self::AbsolutePath => "absolute paths are not allowed",
            Self::Traversal => "dot or parent traversal components are not allowed",
            Self::MultipleWildcards => "at most one '*' wildcard is allowed",
        };
        f.write_str(text)
    }
}

impl std::error::Error for IgnoreDiagnostic {}

/// A root-relative matcher with built-in exclusions and parsed root rules.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IgnoreMatcher {
    rules: Vec<Rule>,
    git_rules: Vec<GitRule>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GitRule {
    rule: Rule,
    negated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Rule {
    pattern: String,
    anchored: bool,
    directory: bool,
    has_slash: bool,
}

impl IgnoreMatcher {
    /// Parse root `.branignore` bytes. This performs no filesystem access.
    pub fn from_branignore(bytes: &[u8]) -> Result<Self, IgnoreDiagnostic> {
        if bytes.len() > MAX_BRANIGNORE_BYTES {
            return Err(IgnoreDiagnostic::InputTooLarge {
                actual_bytes: bytes.len(),
            });
        }
        let text = std::str::from_utf8(bytes).map_err(|_| IgnoreDiagnostic::InvalidUtf8)?;
        let mut rules = Vec::new();
        for (offset, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            rules.push(
                Rule::parse(line).map_err(|reason| IgnoreDiagnostic::InvalidRule {
                    line: offset + 1,
                    reason,
                })?,
            );
        }
        Ok(Self {
            rules,
            git_rules: Vec::new(),
        })
    }

    /// Parse legacy root `.okfignore` bytes using the same bounded grammar.
    pub fn from_okfignore(bytes: &[u8]) -> Result<Self, IgnoreDiagnostic> {
        Self::from_branignore(bytes)
    }

    /// Add common root `.gitignore` rules without making repository discovery
    /// fail on syntax outside BRAN's deliberately small ignore grammar.
    pub fn add_gitignore(&mut self, bytes: &[u8]) {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return;
        };
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (negated, pattern) = line
                .strip_prefix('!')
                .map_or((false, line), |pattern| (true, pattern));
            if let Ok(rule) = Rule::parse(pattern) {
                self.git_rules.push(GitRule { rule, negated });
            }
        }
    }

    /// Return whether an already-safe, normalized root-relative path is ignored.
    /// This intentionally does not normalize or access the filesystem.
    pub fn is_ignored_relative(&self, relative_path: &str) -> bool {
        let path = relative_path.trim_end_matches('/');
        if path
            .split('/')
            .any(|component| BUILTIN_COMPONENTS.contains(&component))
        {
            return true;
        }
        if self.rules.iter().any(|rule| rule.matches(path)) {
            return true;
        }
        self.git_rules
            .iter()
            .filter(|rule| rule.rule.matches(path))
            .fold(false, |_, rule| !rule.negated)
    }
}

impl Rule {
    fn parse(input: &str) -> Result<Self, RuleError> {
        let normalized = input.replace('\\', "/");
        if normalized.starts_with("//") || looks_like_drive_path(&normalized) {
            return Err(RuleError::AbsolutePath);
        }
        let anchored = normalized.starts_with('/');
        let body = normalized.trim_start_matches('/');
        let directory = body.ends_with('/');
        let parts: Vec<_> = body.split('/').filter(|part| !part.is_empty()).collect();
        if parts.is_empty() {
            return Err(if anchored {
                RuleError::AbsolutePath
            } else {
                RuleError::EmptyPattern
            });
        }
        if parts.iter().any(|part| *part == "." || *part == "..") {
            return Err(RuleError::Traversal);
        }
        let pattern = parts.join("/");
        if pattern.matches('*').count() > 1 {
            return Err(RuleError::MultipleWildcards);
        }
        Ok(Self {
            has_slash: pattern.contains('/'),
            pattern,
            anchored,
            directory,
        })
    }

    fn matches(&self, path: &str) -> bool {
        if self.anchored {
            return self.matches_target(path);
        }
        if !self.has_slash {
            return path
                .split('/')
                .any(|component| self.matches_target(component));
        }
        self.matches_target(path)
            || path
                .match_indices('/')
                .any(|(index, _)| self.matches_target(&path[index + 1..]))
    }

    fn matches_target(&self, target: &str) -> bool {
        wildcard_match(&self.pattern, target)
            || (self.directory
                && target
                    .match_indices('/')
                    .any(|(index, _)| wildcard_match(&self.pattern, &target[..index])))
    }
}

fn looks_like_drive_path(path: &str) -> bool {
    path.as_bytes().get(1) == Some(&b':')
        && path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == value,
        Some((prefix, suffix)) => {
            value.starts_with(prefix)
                && value.ends_with(suffix)
                && value.len() >= prefix.len() + suffix.len()
        }
    }
}
