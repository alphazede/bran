//! Deterministic public-boundary checks for policy-declared bridge targets.

use crate::agent::result_store::ResultId;
use crate::policy::RepositoryPolicy;
use crate::profile::Diagnostic;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

const MAX_BRIDGE_BYTES: usize = 1024 * 1024;

/// Validate public bridge targets without echoing their content.
///
/// A finding is suppressed only when the native policy names the same path,
/// category, one-based line, and SHA-256 digest of the complete line.
pub fn validate_public_boundary(root: &Path, policy: &RepositoryPolicy) -> Vec<Diagnostic> {
    let Some(coverage) = policy.document_coverage.as_ref() else {
        return Vec::new();
    };
    let Some(boundary) = policy.public_boundary.as_ref() else {
        return Vec::new();
    };
    let canonical_root = match root.canonicalize() {
        Ok(path) => path,
        Err(_) => return vec![read_diagnostic("<root>")],
    };
    let mut diagnostics = Vec::new();
    for relative in &coverage.bridge_targets {
        let candidate = root.join(relative);
        let canonical = match candidate.canonicalize() {
            Ok(path) if path.starts_with(&canonical_root) => path,
            _ => {
                diagnostics.push(read_diagnostic(relative));
                continue;
            }
        };
        if fs::symlink_metadata(&candidate).is_err()
            || fs::symlink_metadata(&candidate).is_ok_and(|meta| meta.file_type().is_symlink())
        {
            diagnostics.push(read_diagnostic(relative));
            continue;
        }
        let bytes = match fs::read(&canonical) {
            Ok(bytes) if bytes.len() <= MAX_BRIDGE_BYTES => bytes,
            _ => {
                diagnostics.push(read_diagnostic(relative));
                continue;
            }
        };
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(_) => {
                diagnostics.push(read_diagnostic(relative));
                continue;
            }
        };
        for (offset, raw_line) in text.split('\n').enumerate() {
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            let line_number = offset + 1;
            let digest = ResultId::sha256(line.as_bytes()).value().to_owned();
            for code in finding_codes(line) {
                let allowed = boundary
                    .exact_allowlist
                    .get(relative)
                    .and_then(|codes| codes.get(code))
                    .and_then(|lines| lines.get(&line_number))
                    == Some(&digest);
                if !allowed {
                    diagnostics.push(Diagnostic {
                        path: relative.clone(),
                        code: code.to_owned(),
                        message: format!(
                            "public bridge contains unapproved {code} finding at line {line_number}"
                        ),
                    });
                }
            }
        }
    }
    diagnostics.sort();
    diagnostics.dedup();
    diagnostics
}

fn read_diagnostic(path: &str) -> Diagnostic {
    Diagnostic {
        path: path.to_owned(),
        code: "public-boundary-read".to_owned(),
        message: "public bridge target is unreadable, unsafe, oversized, or non-UTF-8".to_owned(),
    }
}

/// True when the line contains an absolute path rooted in a user home
/// directory, for any user name.
///
/// This deliberately matches by shape rather than by a fixed list of paths. A
/// hardcoded list only detects one machine's layout and publishes that layout
/// in source that ships publicly.
fn contains_home_path(line: &str) -> bool {
    const ROOTS: [(&str, char); 3] = [("/home/", '/'), ("/Users/", '/'), ("C:\\Users\\", '\\')];
    for (root, separator) in ROOTS {
        let mut rest = line;
        while let Some(start) = rest.find(root) {
            let after = &rest[start + root.len()..];
            let user_len = after
                .find(separator)
                .filter(|len| *len > 0 && after.len() > len + 1);
            if user_len.is_some() {
                return true;
            }
            rest = &rest[start + root.len()..];
        }
    }
    false
}

fn finding_codes(line: &str) -> BTreeSet<&'static str> {
    let lower = line.to_ascii_lowercase();
    let mut codes = BTreeSet::new();
    if contains_home_path(line) {
        codes.insert("private_home_path");
    }
    if line.contains("tools/agents/")
        || line.contains("docs/plans/")
        || line.contains(".codex/skills/")
    {
        codes.insert("internal_agent_path");
    }
    if lower.contains("prompt text")
        || lower.contains("scoring formula")
        || lower.contains("calibration value")
        || lower.contains("proprietary threshold")
    {
        codes.insert("proprietary_scoring_language");
    }
    if contains_private_network_literal(line) {
        codes.insert("private_network_literal");
    }
    if contains_credential_assignment(line) {
        codes.insert("credential_assignment");
    }
    codes
}

fn contains_private_network_literal(line: &str) -> bool {
    line.split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .filter(|part| part.matches('.').count() == 3)
        .any(|part| {
            let octets: Vec<u8> = part
                .split('.')
                .map(str::parse::<u8>)
                .collect::<Result<_, _>>()
                .unwrap_or_default();
            octets.len() == 4
                && (octets[0] == 10
                    || (octets[0] == 192 && octets[1] == 168)
                    || (octets[0] == 172 && (16..=31).contains(&octets[1])))
        })
}

fn contains_credential_assignment(line: &str) -> bool {
    let keys = [
        "api_key", "apikey", "api-key", "token", "secret", "password",
    ];
    keys.iter().any(|key| {
        line.as_bytes()
            .windows(key.len())
            .enumerate()
            .filter(|(_, candidate)| candidate.eq_ignore_ascii_case(key.as_bytes()))
            .any(|(position, _)| credential_assignment_at(line, position, key.len()))
    })
}

fn credential_assignment_at(line: &str, position: usize, key_len: usize) -> bool {
    if position > 0 && is_identifier_byte(line.as_bytes()[position - 1]) {
        return false;
    }
    let inside_quote = inside_quoted_text(line, position);
    let quoted_key = is_quoted_credential_key(line, position, key_len);
    if inside_regex_literal(line, position) {
        return false;
    }

    let tail = &line[position + key_len + usize::from(quoted_key)..];
    let operator = tail.trim_start_matches(char::is_whitespace);
    let value = if let Some(value) = operator.strip_prefix(':') {
        value
    } else if let Some(value) = operator.strip_prefix('=') {
        let next = value.trim_start_matches(char::is_whitespace);
        if next.starts_with(['=', '>']) {
            return false;
        }
        value
    } else {
        return false;
    };

    if inside_quote && !quoted_key && !is_runtime_credential_reference(value) {
        return false;
    }

    contains_non_placeholder_value(value)
}

fn is_quoted_credential_key(line: &str, position: usize, key_len: usize) -> bool {
    let bytes = line.as_bytes();
    position > 0
        && position + key_len < bytes.len()
        && matches!(bytes[position - 1], b'\'' | b'"')
        && bytes[position - 1] == bytes[position + key_len]
}

fn is_runtime_credential_reference(value: &str) -> bool {
    let value = value.trim_start().to_ascii_lowercase();
    value.starts_with("process.env.")
        || value.starts_with("os.environ[")
        || value.starts_with("std::env::")
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'$')
}

fn inside_quoted_text(line: &str, position: usize) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for (offset, ch) in line.char_indices() {
        if offset >= position {
            break;
        }
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = quote.is_some();
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_none() && matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        }
    }
    quote.is_some()
}

fn inside_regex_literal(line: &str, position: usize) -> bool {
    let before = &line[..position];
    let Some(open) = before.rfind('/') else {
        return false;
    };
    if open > 0 && before.as_bytes()[open - 1] == b'\\' {
        return false;
    }
    let prefix = before[..open].trim_end();
    if !prefix.is_empty()
        && !prefix.ends_with(['=', '(', '[', '{', ',', ':', ';', '!', '&', '|', '?'])
    {
        return false;
    }
    line[position..].char_indices().any(|(offset, ch)| {
        ch == '/' && (offset == 0 || line.as_bytes()[position + offset - 1] != b'\\')
    })
}

fn contains_non_placeholder_value(value: &str) -> bool {
    let value = value.trim_start();
    if value.is_empty() {
        return false;
    }
    let token = if let Some(quote @ ('\'' | '"' | '`')) = value.chars().next() {
        value[quote.len_utf8()..]
            .split(quote)
            .next()
            .unwrap_or_default()
    } else {
        value
            .split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ';'))
            .next()
            .unwrap_or_default()
    };
    let token = token.trim();
    if token.starts_with("${") && token.ends_with('}') {
        return false;
    }
    let normalized = token
        .trim_matches(|ch: char| matches!(ch, '<' | '>' | '[' | ']' | '{' | '}'))
        .to_ascii_lowercase();
    let placeholders = [
        "null",
        "none",
        "undefined",
        "todo",
        "tbd",
        "changeme",
        "change-me",
        "placeholder",
        "redacted",
        "example",
        "dummy",
        "string",
        "number",
        "boolean",
        "unknown",
        "any",
    ];
    if normalized.is_empty()
        || placeholders.contains(&normalized.as_str())
        || normalized.starts_with("your-")
        || normalized.starts_with("your_")
        || normalized.starts_with("replace-")
        || normalized.starts_with("replace_")
        || normalized.chars().all(|ch| matches!(ch, 'x' | '*' | '-'))
    {
        return false;
    }

    token
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || "._%+/-".contains(*ch))
        .take(6)
        .count()
        >= 6
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{DocumentCoveragePolicy, PublicBoundaryPolicy};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn policy(path: &str, line: &str) -> RepositoryPolicy {
        let digest = ResultId::sha256(line.as_bytes()).value().to_owned();
        let exact = BTreeMap::from([(
            path.to_owned(),
            BTreeMap::from([(
                "private_home_path".to_owned(),
                BTreeMap::from([(1, digest)]),
            )]),
        )]);
        RepositoryPolicy {
            schema_version: "1".to_owned(),
            frontmatter: None,
            coverage: None,
            document_coverage: Some(DocumentCoveragePolicy {
                roots: vec![],
                native_bundle: vec![],
                canonical_documents: vec![],
                legacy_documents: vec![],
                excluded_documents: BTreeMap::new(),
                bridge_targets: vec![path.to_owned()],
            }),
            source_links: None,
            tags: None,
            status: None,
            public_boundary: Some(PublicBoundaryPolicy {
                values: vec![],
                path_allowlist: vec![],
                exact_allowlist: exact,
            }),
        }
    }

    #[test]
    fn exact_line_digest_allows_only_unchanged_finding() {
        let root = std::env::temp_dir().join(format!(
            "bran-boundary-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let path = "bridge.mjs";
        let approved = "const path = '/home/example-user/workspace/public';";
        fs::write(root.join(path), approved).unwrap();
        let policy = policy(path, approved);
        assert!(validate_public_boundary(&root, &policy).is_empty());

        fs::write(
            root.join(path),
            "const path = '/home/example-user/workspace/private';",
        )
        .unwrap();
        let findings = validate_public_boundary(&root, &policy);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, "private_home_path");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn private_home_path_is_detected_for_any_user_and_no_layout_is_hardcoded() {
        for line in [
            "const path = '/home/anyone/projects/x';",
            "const path = '/home/other-user/Downloads/y';",
            "const path = '/Users/someone/Library/z';",
            "const path = 'C:\\Users\\someone\\AppData';",
        ] {
            assert!(
                finding_codes(line).contains("private_home_path"),
                "expected private_home_path for {line}"
            );
        }
        // A bare home root with no path below it is not a private path leak.
        for line in ["/home/", "/home/user", "see /Users/ for details"] {
            assert!(
                !finding_codes(line).contains("private_home_path"),
                "unexpected private_home_path for {line}"
            );
        }
    }

    #[test]
    fn credential_assignment_requires_exact_key_operator_and_real_value() {
        for line in [
            "const password = 'hunter2';",
            r#"{"password": "hunter2"}"#,
            "'token': 'abcdef123',",
            "token: abcdef123,",
            "api_key = os.environ['API_KEY']",
            "secret = vault.production.secret",
            "apiKey: process.env.AZS_API_KEY,",
        ] {
            assert!(
                contains_credential_assignment(line),
                "expected credential assignment: {line}"
            );
        }
    }

    #[test]
    fn credential_assignment_rejects_non_assignments_and_placeholders() {
        for line in [
            "const passwordPattern = /password: [A-Za-z0-9]{20}/;",
            "const tokenizer = buildTokenizer();",
            "const pattern = /token: [A-Za-z0-9]{20}/;",
            "throw new Error(\"password: must contain at least six characters\");",
            "throw new Error(\"password: credentials-are-required\");",
            "if (token === expected) return true;",
            "if (token == expected) return true;",
            "if (token !== expected) return true;",
            "if (token >= expected) return true;",
            "tokens => tokens.length",
            "const tokenLabel = formatToken(); const value = abcdef123;",
            "token = ''",
            "token = 'changeme'",
            "api_key: '${AZS_API_KEY}',",
            "password: null,",
            "token: string;",
            "secret = '<redacted>'",
            "api_key = 'your-api-key'",
            "apiKey = placeholder",
        ] {
            assert!(
                !contains_credential_assignment(line),
                "unexpected credential assignment: {line}"
            );
        }
    }

    #[test]
    fn sports_bridge_credential_lines_keep_exact_classification() {
        let placeholder = "AZS_API_KEY: '${AZS_API_KEY}',";
        assert!(
            !finding_codes(placeholder).contains("credential_assignment"),
            "Sports interpolation placeholder must not be a credential finding"
        );

        let runtime_assignment =
            "    'const azs = createAzsClient({ apiKey: process.env.AZS_API_KEY });',";
        assert!(
            finding_codes(runtime_assignment).contains("credential_assignment"),
            "Sports runtime assignment must retain its exact-allowlist finding"
        );
    }
}
