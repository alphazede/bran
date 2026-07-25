//! Migration body-preservation validation (Slice 1.2-B).
//!
//! Compares original and migrated Markdown body bytes after removing
//! valid leading YAML frontmatter. Read-only; never writes a report
//! or repository file.

use crate::agent::result_store::ResultId;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

// --- limits ---

const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
const MAX_MANIFEST_ENTRIES: usize = 256;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_AGGREGATE_BYTES: u64 = 32 * 1024 * 1024;

type ParsedManifest = (u64, Vec<(String, String, Option<String>)>);

// --- public types ---

/// A single file pair in the body-preservation manifest.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct BodyEntry {
    pub original_path: String,
    pub migrated_path: String,
    /// Optional expected SHA-256 of the original stripped body.
    /// When present, must be exactly 64 lowercase hex characters.
    pub body_sha256: Option<String>,
}

/// Per-file body-preservation result.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct BodyFileResult {
    pub original_path: String,
    pub migrated_path: String,
    pub original_body_sha256: String,
    pub migrated_body_sha256: String,
    pub body_preserved: bool,
}

/// A single finding (error severity in this module).
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct BodyFinding {
    pub severity: String,
    pub code: String,
    pub path: String,
    pub message: String,
}

/// Top-level body-preservation validation report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BodyPreservedReport {
    pub schema_version: u64,
    pub manifest_path: String,
    pub files: Vec<BodyFileResult>,
    pub findings: Vec<BodyFinding>,
    pub passed: bool,
}

// --- errors ---

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MigrationError {
    ManifestTooLarge(u64),
    ManifestNotUtf8,
    ManifestInvalidJson(String),
    ManifestNotObject,
    ManifestMissingFiles,
    ManifestTooManyEntries(usize),
    ManifestEntryInvalid(usize, String),
    EmptyManifest,
    UnsafePath,
    InvalidManifestInput,
    ReadError,
    NotUtf8,
    MalformedFrontmatter,
}

// --- path safety ---

/// Detect Windows drive (e.g. `C:`) and UNC (`\\\\`) paths.
/// Platform-neutral: std `Component::Prefix` only fires on Windows.
fn is_windows_style_path(input: &str) -> bool {
    if input.len() >= 2 {
        let b = input.as_bytes();
        if b[0].is_ascii_alphabetic() && b[1] == b':' {
            return true;
        }
    }
    input.starts_with("\\\\")
}

/// Returns a safe display label. Redacts unsafe values.
fn safe_label(path: &str) -> String {
    if path.is_empty() || path.contains('\0') || is_windows_style_path(path) {
        return "[redacted unsafe path]".to_owned();
    }
    let p = Path::new(path);
    if p.is_absolute()
        || p.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return "[redacted unsafe path]".to_owned();
    }
    path.replace('\\', "/")
}

/// Validates a repo-relative path resolves to a regular non-symlink file under root.
/// Returns the canonical path. Never echoes unsafe values in errors.
fn safe_body_input(root: &Path, input: &str) -> Result<PathBuf, MigrationError> {
    if input.is_empty() || input.contains('\0') || is_windows_style_path(input) {
        return Err(MigrationError::UnsafePath);
    }
    let canonical_root = root.canonicalize().map_err(|_| MigrationError::ReadError)?;
    let path = Path::new(input);
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(MigrationError::UnsafePath);
    }
    let joined = canonical_root.join(path);
    let md = fs::symlink_metadata(&joined).map_err(|_| MigrationError::InvalidManifestInput)?;
    if md.file_type().is_symlink() || !md.file_type().is_file() {
        return Err(MigrationError::InvalidManifestInput);
    }
    let canonical = joined
        .canonicalize()
        .map_err(|_| MigrationError::ReadError)?;
    if !canonical.starts_with(&canonical_root) {
        return Err(MigrationError::InvalidManifestInput);
    }
    Ok(canonical)
}

// --- frontmatter stripping ---

/// Strips a valid leading YAML frontmatter block (delimited by `---` lines).
/// Returns `MalformedFrontmatter` if the opening delimiter is present but
/// no matching closing `---` line follows.
fn strip_frontmatter(bytes: &[u8]) -> Result<Vec<u8>, MigrationError> {
    if !bytes.starts_with(b"---\n") && !bytes.starts_with(b"---\r\n") {
        return Ok(bytes.to_vec());
    }
    let mut offset = 0_usize;
    for (idx, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
        offset += line.len();
        if idx > 0 && line.trim_ascii() == b"---" {
            return Ok(bytes[offset..].to_vec());
        }
    }
    Err(MigrationError::MalformedFrontmatter)
}

// --- minimal JSON parser ---

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn err(&self, msg: &str) -> MigrationError {
        MigrationError::ManifestInvalidJson(msg.to_owned())
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), MigrationError> {
        self.skip_ws();
        if self.pos >= self.bytes.len() || self.bytes[self.pos] != b {
            return Err(self.err(&format!("expected '{}'", b as char)));
        }
        self.pos += 1;
        Ok(())
    }

    fn parse_string(&mut self) -> Result<String, MigrationError> {
        self.expect(b'"')?;
        let mut buf: Vec<u8> = Vec::new();
        loop {
            if self.pos >= self.bytes.len() {
                return Err(self.err("unterminated string"));
            }
            let b = self.bytes[self.pos];
            self.pos += 1;
            if b == b'"' {
                return String::from_utf8(buf).map_err(|_| self.err("invalid UTF-8"));
            }
            if b < 0x20 {
                return Err(self.err("control char in string"));
            }
            if b == b'\\' {
                if self.pos >= self.bytes.len() {
                    return Err(self.err("unterminated escape"));
                }
                let esc = self.bytes[self.pos];
                self.pos += 1;
                match esc {
                    b'"' => buf.push(b'"'),
                    b'\\' => buf.push(b'\\'),
                    b'/' => buf.push(b'/'),
                    b'b' => buf.push(0x08),
                    b'f' => buf.push(0x0C),
                    b'n' => buf.push(b'\n'),
                    b'r' => buf.push(b'\r'),
                    b't' => buf.push(b'\t'),
                    b'u' => self.decode_u_escape(&mut buf)?,
                    _ => return Err(self.err("invalid escape")),
                }
            } else {
                buf.push(b);
            }
        }
    }

    fn decode_u_escape(&mut self, buf: &mut Vec<u8>) -> Result<(), MigrationError> {
        let cp = self.parse_hex_u32()?;
        if (0xD800..=0xDBFF).contains(&cp) {
            // high surrogate: expect \uXXXX low surrogate
            if self.pos + 1 < self.bytes.len()
                && self.bytes[self.pos] == b'\\'
                && self.bytes[self.pos + 1] == b'u'
            {
                self.pos += 2;
                let low = self.parse_hex_u32()?;
                if !(0xDC00..=0xDFFF).contains(&low) {
                    return Err(self.err("invalid surrogate pair"));
                }
                let ch = 0x10000 + (cp - 0xD800) * 0x400 + (low - 0xDC00);
                write_utf8(
                    buf,
                    char::from_u32(ch).ok_or_else(|| self.err("invalid code point"))?,
                );
            } else {
                return Err(self.err("lone high surrogate"));
            }
        } else if (0xDC00..=0xDFFF).contains(&cp) {
            return Err(self.err("lone low surrogate"));
        } else {
            write_utf8(
                buf,
                char::from_u32(cp).ok_or_else(|| self.err("invalid code point"))?,
            );
        }
        Ok(())
    }

    fn parse_hex_u32(&mut self) -> Result<u32, MigrationError> {
        if self.pos + 4 > self.bytes.len() {
            return Err(self.err("truncated \\u escape"));
        }
        let hex_str = std::str::from_utf8(&self.bytes[self.pos..self.pos + 4])
            .map_err(|_| self.err("invalid hex"))?;
        let val = u32::from_str_radix(hex_str, 16).map_err(|_| self.err("invalid hex"))?;
        self.pos += 4;
        Ok(val)
    }

    fn expect_comma_or_close(&mut self, close: u8) -> Result<(), MigrationError> {
        self.skip_ws();
        if self.pos >= self.bytes.len() {
            return Err(self.err("unterminated object/array"));
        }
        match self.bytes[self.pos] {
            b',' => {
                self.pos += 1;
                self.skip_ws();
                if self.pos < self.bytes.len() && self.bytes[self.pos] == close {
                    return Err(self.err("trailing comma"));
                }
                Ok(())
            }
            c if c == close => Ok(()),
            _ => Err(self.err("expected ',' or closing delimiter")),
        }
    }

    fn parse_u64(&mut self) -> Result<u64, MigrationError> {
        self.skip_ws();
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.err("expected number"));
        }
        let digits = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.err("invalid UTF-8"))?;
        if digits.len() > 1 && digits.starts_with('0') {
            return Err(self.err("leading zero in number"));
        }
        digits.parse().map_err(|_| self.err("number overflow"))
    }

    /// Parses the top-level manifest object into known fields.
    fn parse_manifest_object(&mut self) -> Result<ParsedManifest, MigrationError> {
        self.expect(b'{')?;
        let mut schema_version = 1_u64;
        let mut files: Option<Vec<(String, String, Option<String>)>> = None;
        let mut seen_sv = false;
        let mut seen_f = false;
        loop {
            self.skip_ws();
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b'}' {
                self.pos += 1;
                break;
            }
            let key = self.parse_string()?;
            self.expect(b':')?;
            match key.as_str() {
                "schema_version" => {
                    if seen_sv {
                        return Err(self.err("duplicate key 'schema_version'"));
                    }
                    seen_sv = true;
                    schema_version = self.parse_u64()?;
                }
                "files" => {
                    if seen_f {
                        return Err(self.err("duplicate key 'files'"));
                    }
                    seen_f = true;
                    files = Some(self.parse_files_array()?);
                }
                _ => {
                    self.skip_value()?;
                }
            }
            self.expect_comma_or_close(b'}')?;
        }
        // no trailing data
        self.skip_ws();
        if self.pos < self.bytes.len() {
            return Err(self.err("trailing data after root object"));
        }
        let files = files.ok_or(MigrationError::ManifestMissingFiles)?;
        Ok((schema_version, files))
    }

    fn parse_files_array(
        &mut self,
    ) -> Result<Vec<(String, String, Option<String>)>, MigrationError> {
        self.expect(b'[')?;
        let mut entries = Vec::new();
        loop {
            self.skip_ws();
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b']' {
                self.pos += 1;
                break;
            }
            entries.push(self.parse_file_entry()?);
            self.expect_comma_or_close(b']')?;
        }
        Ok(entries)
    }

    fn parse_file_entry(&mut self) -> Result<(String, String, Option<String>), MigrationError> {
        self.expect(b'{')?;
        let mut original_path = String::new();
        let mut migrated_path = String::new();
        let mut body_sha256: Option<String> = None;
        let mut seen_op = false;
        let mut seen_mp = false;
        let mut seen_bs = false;
        loop {
            self.skip_ws();
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b'}' {
                self.pos += 1;
                break;
            }
            let key = self.parse_string()?;
            self.expect(b':')?;
            match key.as_str() {
                "original_path" => {
                    if seen_op {
                        return Err(self.err("duplicate key 'original_path'"));
                    }
                    seen_op = true;
                    original_path = self.parse_string()?;
                }
                "migrated_path" => {
                    if seen_mp {
                        return Err(self.err("duplicate key 'migrated_path'"));
                    }
                    seen_mp = true;
                    migrated_path = self.parse_string()?;
                }
                "body_sha256" => {
                    if seen_bs {
                        return Err(self.err("duplicate key 'body_sha256'"));
                    }
                    seen_bs = true;
                    let v = self.parse_string()?;
                    body_sha256 = if v.is_empty() { None } else { Some(v) };
                }
                _ => {
                    self.skip_value()?;
                }
            }
            self.expect_comma_or_close(b'}')?;
        }
        Ok((original_path, migrated_path, body_sha256))
    }

    fn skip_value(&mut self) -> Result<(), MigrationError> {
        self.skip_ws();
        if self.pos >= self.bytes.len() {
            return Err(self.err("unexpected end"));
        }
        match self.bytes[self.pos] {
            b'"' => {
                self.parse_string()?;
            }
            b'{' => {
                self.pos += 1;
                let mut depth = 1_u32;
                while depth > 0 && self.pos < self.bytes.len() {
                    match self.bytes[self.pos] {
                        b'"' => {
                            self.pos += 1;
                            while self.pos < self.bytes.len() && self.bytes[self.pos] != b'"' {
                                if self.bytes[self.pos] == b'\\' {
                                    self.pos += 1;
                                }
                                self.pos += 1;
                            }
                            self.pos += 1; // skip closing quote
                        }
                        b'{' => {
                            depth += 1;
                            self.pos += 1;
                        }
                        b'}' => {
                            depth -= 1;
                            self.pos += 1;
                        }
                        _ => self.pos += 1,
                    }
                }
                if depth != 0 {
                    return Err(self.err("unterminated object"));
                }
            }
            b'[' => {
                self.pos += 1;
                let mut depth = 1_u32;
                while depth > 0 && self.pos < self.bytes.len() {
                    match self.bytes[self.pos] {
                        b'"' => {
                            self.pos += 1;
                            while self.pos < self.bytes.len() && self.bytes[self.pos] != b'"' {
                                if self.bytes[self.pos] == b'\\' {
                                    self.pos += 1;
                                }
                                self.pos += 1;
                            }
                            self.pos += 1;
                        }
                        b'[' => {
                            depth += 1;
                            self.pos += 1;
                        }
                        b']' => {
                            depth -= 1;
                            self.pos += 1;
                        }
                        _ => self.pos += 1,
                    }
                }
                if depth != 0 {
                    return Err(self.err("unterminated array"));
                }
            }
            b't' | b'f' | b'n' => {
                let kw_start = self.pos;
                while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_alphabetic() {
                    self.pos += 1;
                }
                let kw = std::str::from_utf8(&self.bytes[kw_start..self.pos])
                    .map_err(|_| self.err("invalid UTF-8"))?;
                if kw != "true" && kw != "false" && kw != "null" {
                    return Err(self.err("invalid literal"));
                }
            }
            b'-' | b'0'..=b'9' => {
                while self.pos < self.bytes.len()
                    && (self.bytes[self.pos].is_ascii_digit()
                        || self.bytes[self.pos] == b'.'
                        || self.bytes[self.pos] == b'e'
                        || self.bytes[self.pos] == b'E'
                        || self.bytes[self.pos] == b'-'
                        || self.bytes[self.pos] == b'+')
                {
                    self.pos += 1;
                }
            }
            _ => return Err(self.err("unexpected value")),
        }
        Ok(())
    }
}

// --- json helpers ---

fn write_utf8(buf: &mut Vec<u8>, c: char) {
    let mut tmp = [0u8; 4];
    let encoded = c.encode_utf8(&mut tmp);
    buf.extend_from_slice(encoded.as_bytes());
}

// --- sha256 helpers ---

fn hex_sha256(bytes: &[u8]) -> String {
    ResultId::sha256(bytes).value().to_owned()
}

fn is_valid_body_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

// --- main validation ---

fn read_error_finding(path: &str) -> BodyFinding {
    BodyFinding {
        severity: "error".to_owned(),
        code: "body_preserved_read_error".to_owned(),
        path: path.to_owned(),
        message: "manifest input could not be read safely".to_owned(),
    }
}

/// Public file-oriented wrapper around `validate_body_preservation`.
/// Canonicalizes root, validates manifest path as safe repo-relative
/// regular file, checks the 2 MiB limit before reading, reads at most
/// limit+1, and passes raw bytes with a safe label to the validator.
/// Never writes to the filesystem. Returns `ReadError` for any I/O
/// problem before the validator runs.
pub fn validate_body_preservation_from_path(
    root: &Path,
    manifest_path: &str,
) -> Result<BodyPreservedReport, MigrationError> {
    let canonical_root = root.canonicalize().map_err(|_| MigrationError::ReadError)?;
    let canonical_manifest = safe_body_input(&canonical_root, manifest_path)?;
    let metadata =
        fs::symlink_metadata(&canonical_manifest).map_err(|_| MigrationError::ReadError)?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(MigrationError::ManifestTooLarge(metadata.len()));
    }
    // ponytail: use Read::take to guard against file growth race after metadata
    let file = fs::File::open(&canonical_manifest).map_err(|_| MigrationError::ReadError)?;
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| MigrationError::ReadError)?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(MigrationError::ManifestTooLarge(bytes.len() as u64));
    }
    let safe_label_display = safe_label(manifest_path);
    validate_body_preservation(&canonical_root, &safe_label_display, &bytes)
}

/// Validates body preservation for a migration manifest.
///
/// `manifest_label` is a safe display label (sanitized internally).
/// `manifest_json` is the raw JSON bytes (max 2 MiB).
///
/// Returns a report with deterministically ordered files and findings.
/// Never writes to the filesystem.
pub fn validate_body_preservation(
    root: &Path,
    manifest_label: &str,
    manifest_json: &[u8],
) -> Result<BodyPreservedReport, MigrationError> {
    let safe_manifest_label = safe_label(manifest_label);
    let canonical_root = root.canonicalize().map_err(|_| MigrationError::ReadError)?;

    if manifest_json.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(MigrationError::ManifestTooLarge(manifest_json.len() as u64));
    }
    if manifest_json.is_empty() {
        return Err(MigrationError::EmptyManifest);
    }

    let (schema_version, raw_entries) = JsonParser::new(manifest_json).parse_manifest_object()?;

    if raw_entries.is_empty() {
        return Err(MigrationError::EmptyManifest);
    }
    if raw_entries.len() > MAX_MANIFEST_ENTRIES {
        return Err(MigrationError::ManifestTooManyEntries(raw_entries.len()));
    }

    let mut findings: Vec<BodyFinding> = Vec::new();
    let mut file_results: Vec<BodyFileResult> = Vec::new();
    let mut aggregate_bytes: u64 = 0;

    for (original_path, migrated_path, body_sha256) in &raw_entries {
        let orig_label = safe_label(original_path);
        let mig_label = safe_label(migrated_path);
        let mut skip = false;

        // validate optional body_sha256 format
        if let Some(ref expected) = body_sha256 {
            if !is_valid_body_sha256(expected) {
                findings.push(BodyFinding {
                    severity: "error".to_owned(),
                    code: "invalid_body_sha256".to_owned(),
                    path: orig_label.clone(),
                    message: "body_sha256 must be exactly 64 lowercase hex characters".to_owned(),
                });
                skip = true;
            }
        }

        if skip {
            continue;
        }

        // validate paths
        let (orig_canonical, mig_canonical) = match (
            safe_body_input(&canonical_root, original_path),
            safe_body_input(&canonical_root, migrated_path),
        ) {
            (Ok(o), Ok(m)) => (o, m),
            _ => {
                findings.push(BodyFinding {
                    severity: "error".to_owned(),
                    code: "body_preserved_read_error".to_owned(),
                    path: safe_manifest_label.clone(),
                    message: "manifest input could not be read safely".to_owned(),
                });
                continue;
            }
        };

        // check file sizes and aggregate limit
        let mut size_ok = true;
        for path in [&orig_canonical, &mig_canonical] {
            let size = match fs::metadata(path) {
                Ok(md) => md.len(),
                Err(_) => {
                    size_ok = false;
                    break;
                }
            };
            if size > MAX_FILE_BYTES {
                size_ok = false;
                break;
            }
            aggregate_bytes = aggregate_bytes.saturating_add(size);
            if aggregate_bytes > MAX_AGGREGATE_BYTES {
                size_ok = false;
                break;
            }
        }
        if !size_ok {
            findings.push(BodyFinding {
                severity: "error".to_owned(),
                code: "body_preserved_read_error".to_owned(),
                path: safe_manifest_label.clone(),
                message: "manifest input could not be read safely".to_owned(),
            });
            continue;
        }

        // read both files
        let orig_bytes = match fs::read(&orig_canonical) {
            Ok(b) => b,
            Err(_) => {
                findings.push(read_error_finding(&safe_manifest_label));
                continue;
            }
        };
        let mig_bytes = match fs::read(&mig_canonical) {
            Ok(b) => b,
            Err(_) => {
                findings.push(read_error_finding(&safe_manifest_label));
                continue;
            }
        };

        // UTF-8 check
        if std::str::from_utf8(&orig_bytes).is_err() || std::str::from_utf8(&mig_bytes).is_err() {
            findings.push(read_error_finding(&safe_manifest_label));
            continue;
        }

        let orig_body = strip_frontmatter(&orig_bytes)?;
        let mig_body = strip_frontmatter(&mig_bytes)?;

        let orig_hash = hex_sha256(&orig_body);
        let mig_hash = hex_sha256(&mig_body);

        if let Some(ref expected) = body_sha256 {
            if *expected != orig_hash {
                findings.push(BodyFinding {
                    severity: "error".to_owned(),
                    code: "original_body_hash_mismatch".to_owned(),
                    path: orig_label.clone(),
                    message: "original body hash does not match manifest".to_owned(),
                });
            }
        }

        let preserved = orig_body == mig_body;
        if !preserved {
            findings.push(BodyFinding {
                severity: "error".to_owned(),
                code: "body_changed".to_owned(),
                path: mig_label.clone(),
                message: "body content differs after frontmatter is removed".to_owned(),
            });
        }

        file_results.push(BodyFileResult {
            original_path: orig_label,
            migrated_path: mig_label,
            original_body_sha256: orig_hash,
            migrated_body_sha256: mig_hash,
            body_preserved: preserved,
        });
    }

    // deterministic ordering
    file_results.sort();
    findings.sort();

    Ok(BodyPreservedReport {
        schema_version,
        manifest_path: safe_manifest_label,
        files: file_results,
        passed: findings.is_empty(),
        findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    static FIXTURE_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn setup_fixture() -> PathBuf {
        let n = FIXTURE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("bran-mig-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn write_file(dir: &Path, name: &str, content: &[u8]) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
    }

    #[test]
    fn identical_bodies_different_frontmatter_passes() {
        let root = setup_fixture();
        write_file(&root, "orig.md", b"---\ntitle: A\n---\n# Hello\n");
        write_file(
            &root,
            "mig.md",
            b"---\ntitle: B\ndate: 2024\n---\n# Hello\n",
        );
        let manifest = r#"{"files":[{"original_path":"orig.md","migrated_path":"mig.md"}]}"#;
        let report = validate_body_preservation(&root, "test", manifest.as_bytes()).unwrap();
        assert!(report.passed);
        assert_eq!(report.files.len(), 1);
        assert!(report.files[0].body_preserved);
        assert_eq!(
            report.files[0].original_body_sha256,
            report.files[0].migrated_body_sha256
        );
    }

    #[test]
    fn correct_body_sha256_passes() {
        let root = setup_fixture();
        write_file(&root, "orig.md", b"---\ntitle: A\n---\nbody content\n");
        write_file(&root, "mig.md", b"---\ndifferent\n---\nbody content\n");
        let body_hash = hex_sha256(b"body content\n");
        let manifest = format!(
            r#"{{"files":[{{"original_path":"orig.md","migrated_path":"mig.md","body_sha256":"{}"}}]}}"#,
            body_hash
        );
        let report = validate_body_preservation(&root, "test", manifest.as_bytes()).unwrap();
        assert!(report.passed);
        assert_eq!(report.files[0].original_body_sha256, body_hash);
    }

    #[test]
    fn permuted_entries_produce_identical_ordered_results() {
        let root = setup_fixture();
        write_file(&root, "a.md", b"# A\n");
        write_file(&root, "b.md", b"# B\n");
        write_file(&root, "ma.md", b"# A\n");
        write_file(&root, "mb.md", b"# B\n");

        let manifests = [
            r#"{"files":[{"original_path":"a.md","migrated_path":"ma.md"},{"original_path":"b.md","migrated_path":"mb.md"}]}"#,
            r#"{"files":[{"original_path":"b.md","migrated_path":"mb.md"},{"original_path":"a.md","migrated_path":"ma.md"}]}"#,
        ];
        let reports: Vec<_> = manifests
            .iter()
            .map(|m| validate_body_preservation(&root, "test", m.as_bytes()).unwrap())
            .collect();
        assert_eq!(reports[0].files, reports[1].files);
        assert_eq!(reports[0].findings, reports[1].findings);
    }

    #[test]
    fn changed_body_fails() {
        let root = setup_fixture();
        write_file(&root, "orig.md", b"# Hello\n");
        write_file(&root, "mig.md", b"# World\n");
        let manifest = r#"{"files":[{"original_path":"orig.md","migrated_path":"mig.md"}]}"#;
        let report = validate_body_preservation(&root, "test", manifest.as_bytes()).unwrap();
        assert!(!report.passed);
        assert!(!report.files[0].body_preserved);
        assert!(report.findings.iter().any(|f| f.code == "body_changed"));
    }

    #[test]
    fn wrong_digest_fails() {
        let root = setup_fixture();
        write_file(&root, "orig.md", b"body\n");
        write_file(&root, "mig.md", b"body\n");
        let manifest = r#"{"files":[{"original_path":"orig.md","migrated_path":"mig.md","body_sha256":"0000000000000000000000000000000000000000000000000000000000000000"}]}"#;
        let report = validate_body_preservation(&root, "test", manifest.as_bytes()).unwrap();
        assert!(!report.passed);
        assert!(report
            .findings
            .iter()
            .any(|f| f.code == "original_body_hash_mismatch"));
    }

    #[test]
    fn malformed_sha256_fails() {
        let root = setup_fixture();
        write_file(&root, "orig.md", b"body\n");
        write_file(&root, "mig.md", b"body\n");
        // uppercase hex is invalid
        let manifest = r#"{"files":[{"original_path":"orig.md","migrated_path":"mig.md","body_sha256":"ABCD000000000000000000000000000000000000000000000000000000000000"}]}"#;
        let report = validate_body_preservation(&root, "test", manifest.as_bytes()).unwrap();
        assert!(!report.passed);
        assert!(report
            .findings
            .iter()
            .any(|f| f.code == "invalid_body_sha256"));
    }

    #[test]
    fn empty_sha256_treated_as_absent() {
        let root = setup_fixture();
        write_file(&root, "orig.md", b"body\n");
        write_file(&root, "mig.md", b"body\n");
        let manifest =
            r#"{"files":[{"original_path":"orig.md","migrated_path":"mig.md","body_sha256":""}]}"#;
        let report = validate_body_preservation(&root, "test", manifest.as_bytes()).unwrap();
        assert!(report.passed);
    }

    #[test]
    fn empty_manifest_rejected() {
        let root = setup_fixture();
        assert!(matches!(
            validate_body_preservation(&root, "test", b"{}"),
            Err(MigrationError::ManifestMissingFiles)
        ));
        assert!(matches!(
            validate_body_preservation(&root, "test", b"{\"files\":[]}"),
            Err(MigrationError::EmptyManifest)
        ));
    }

    #[test]
    fn too_many_entries_rejected() {
        let root = setup_fixture();
        let mut entries = String::from("{\"files\":[");
        for i in 0..257 {
            if i > 0 {
                entries.push(',');
            }
            entries.push_str(&format!(
                "{{\"original_path\":\"a{}.md\",\"migrated_path\":\"m{}.md\"}}",
                i, i
            ));
        }
        entries.push_str("]}");
        assert!(matches!(
            validate_body_preservation(&root, "test", entries.as_bytes()),
            Err(MigrationError::ManifestTooManyEntries(257))
        ));
    }

    #[test]
    fn oversized_manifest_rejected() {
        let root = setup_fixture();
        let big = vec![b' '; (MAX_MANIFEST_BYTES as usize) + 1];
        assert!(matches!(
            validate_body_preservation(&root, "test", &big),
            Err(MigrationError::ManifestTooLarge(_))
        ));
    }

    #[test]
    fn missing_file_reports_error() {
        let root = setup_fixture();
        write_file(&root, "orig.md", b"body\n");
        let manifest = r#"{"files":[{"original_path":"orig.md","migrated_path":"missing.md"}]}"#;
        let report = validate_body_preservation(&root, "test", manifest.as_bytes()).unwrap();
        assert!(!report.passed);
        assert!(report
            .findings
            .iter()
            .any(|f| f.code == "body_preserved_read_error"));
    }

    #[test]
    fn unsafe_path_rejected_and_redacted() {
        let root = setup_fixture();
        let manifest = r#"{"files":[{"original_path":"orig.md","migrated_path":"../escape.md"}]}"#;
        let report = validate_body_preservation(&root, "test", manifest.as_bytes()).unwrap();
        assert!(!report.passed);
        // path must be redacted
        assert!(
            report.files.is_empty() || report.files[0].migrated_path == "[redacted unsafe path]"
        );
    }

    #[test]
    fn absolute_path_rejected() {
        let root = setup_fixture();
        let manifest = r#"{"files":[{"original_path":"orig.md","migrated_path":"/etc/passwd"}]}"#;
        let report = validate_body_preservation(&root, "test", manifest.as_bytes()).unwrap();
        assert!(!report.passed);
    }

    #[test]
    fn nul_path_rejected() {
        let root = setup_fixture();
        // Build JSON with embedded NUL byte in a string value
        let mut manifest =
            b"{\"files\":[{\"original_path\":\"orig.md\",\"migrated_path\":\"".to_vec();
        manifest.extend_from_slice(b"bad");
        manifest.push(0);
        manifest.extend_from_slice(b".md\"}]}");
        // NUL in JSON string is invalid JSON, so this is a parse error
        assert!(validate_body_preservation(&root, "test", &manifest).is_err());
    }

    #[test]
    fn unterminated_frontmatter_is_error() {
        let root = setup_fixture();
        // unterminated frontmatter: no closing ---
        write_file(&root, "orig.md", b"---\ntitle: A\nbody\n");
        write_file(&root, "mig.md", b"---\ntitle: A\nbody\n");
        let manifest = r#"{"files":[{"original_path":"orig.md","migrated_path":"mig.md"}]}"#;
        assert!(matches!(
            validate_body_preservation(&root, "test", manifest.as_bytes()),
            Err(MigrationError::MalformedFrontmatter)
        ));
    }

    // --- negative coverage (Slice 1.2-B remediation) ---

    #[test]
    fn symlink_input_rejected() {
        use std::os::unix::fs as unix_fs;
        let root = setup_fixture();
        write_file(&root, "real.md", b"body\n");
        let link_path = root.join("link.md");
        unix_fs::symlink("real.md", &link_path).unwrap();
        let manifest = r#"{"files":[{"original_path":"link.md","migrated_path":"real.md"}]}"#;
        let report = validate_body_preservation(&root, "test", manifest.as_bytes()).unwrap();
        assert!(!report.passed);
        assert!(report
            .findings
            .iter()
            .any(|f| f.code == "body_preserved_read_error"));
    }

    #[test]
    fn windows_drive_path_redacted() {
        // safe_label must redact C:\... style paths
        assert_eq!(safe_label("C:\\foo\\bar.md"), "[redacted unsafe path]");
        assert_eq!(safe_label("D:/stuff/file.md"), "[redacted unsafe path]");
    }

    #[test]
    fn unc_path_redacted() {
        // UNC \\server\share must be redacted
        assert_eq!(
            safe_label("\\\\server\\share\\file.md"),
            "[redacted unsafe path]"
        );
    }

    #[test]
    fn per_file_over_2mib_rejected() {
        let root = setup_fixture();
        // sparse file: create a large logical file without writing data
        let big_path = root.join("big.md");
        let f = fs::File::create(&big_path).unwrap();
        f.set_len(MAX_FILE_BYTES + 1).unwrap();
        drop(f);
        // small valid partner file
        write_file(&root, "small.md", b"body\n");
        let manifest = r#"{"files":[{"original_path":"big.md","migrated_path":"small.md"}]}"#;
        let report = validate_body_preservation(&root, "test", manifest.as_bytes()).unwrap();
        assert!(!report.passed);
        assert!(report
            .findings
            .iter()
            .any(|f| f.code == "body_preserved_read_error"));
    }

    #[test]
    fn aggregate_over_32mib_rejected() {
        let root = setup_fixture();
        // 18 files × 2 MiB = 36 MiB, referenced as 9 pairs
        // Each manifest entry contributes ~4 MiB (2 × 2 MiB), so 9 entries → 36 MiB > 32 MiB
        for i in 0..18_u32 {
            let p = root.join(format!("f{i}.md"));
            let f = fs::File::create(&p).unwrap();
            f.set_len(MAX_FILE_BYTES).unwrap();
        }
        let mut manifest = String::from(r#"{"schema_version":1,"files":["#);
        for i in 0..9_u32 {
            if i > 0 {
                manifest.push(',');
            }
            manifest.push_str(&format!(
                "{{\"original_path\":\"f{}.md\",\"migrated_path\":\"f{}.md\"}}",
                i * 2,
                i * 2 + 1
            ));
        }
        manifest.push_str("]}");
        let report = validate_body_preservation(&root, "test", manifest.as_bytes()).unwrap();
        assert!(!report.passed);
    }

    #[test]
    fn trailing_comma_json_rejected() {
        let root = setup_fixture();
        let manifest = r#"{"files":[{"original_path":"a.md","migrated_path":"b.md",}]}"#;
        assert!(validate_body_preservation(&root, "test", manifest.as_bytes()).is_err());
    }

    #[test]
    fn missing_comma_json_rejected() {
        let root = setup_fixture();
        let manifest = r#"{"files":[{"original_path":"a.md","migrated_path":"b.md"}{"original_path":"c.md","migrated_path":"d.md"}]}"#;
        assert!(validate_body_preservation(&root, "test", manifest.as_bytes()).is_err());
    }

    #[test]
    fn duplicate_keys_json_rejected() {
        let root = setup_fixture();
        let manifest =
            r#"{"files":[{"original_path":"a.md","migrated_path":"b.md","original_path":"c.md"}]}"#;
        assert!(validate_body_preservation(&root, "test", manifest.as_bytes()).is_err());
    }

    #[test]
    fn trailing_data_json_rejected() {
        let root = setup_fixture();
        let manifest = r#"{"files":[]} extra"#;
        assert!(validate_body_preservation(&root, "test", manifest.as_bytes()).is_err());
    }

    #[test]
    fn unterminated_container_rejected() {
        let root = setup_fixture();
        let manifest = r#"{"files":[{"original_path":"a.md","migrated_path":"b.md"}"#;
        assert!(validate_body_preservation(&root, "test", manifest.as_bytes()).is_err());
    }

    #[test]
    fn manifest_label_sanitized() {
        let root = setup_fixture();
        write_file(&root, "a.md", b"# A\n");
        write_file(&root, "b.md", b"# A\n");
        let manifest = r#"{"files":[{"original_path":"a.md","migrated_path":"b.md"}]}"#;
        // unsafe label should be sanitized in report
        let report =
            validate_body_preservation(&root, "../etc/passwd", manifest.as_bytes()).unwrap();
        assert_eq!(report.manifest_path, "[redacted unsafe path]");
    }

    // --- JsonParser::parse_string unit tests ---

    #[test]
    fn parse_string_positive() {
        let cases: &[(&[u8], &str)] = &[
            (b"\"repo.md\"", "repo.md"),
            (b"\"r\\u00e9po.md\"", "répo.md"),
            (b"\"\\uD83D\\uDE00\"", "😀"),
            (b"\"a\\\"b\"", "a\"b"),
            (b"\"a\\\\b\"", "a\\b"),
            (b"\"a\\bb\"", "a\x08b"),
            (b"\"a\\fb\"", "a\x0Cb"),
            (b"\"a\\nb\"", "a\nb"),
            (b"\"a\\rb\"", "a\rb"),
            (b"\"a\\tb\"", "a\tb"),
        ];
        for (input, expected) in cases {
            let got = JsonParser::new(input).parse_string().unwrap();
            assert_eq!(got, *expected);
        }
    }

    #[test]
    fn from_path_wrapper_passes_identical_bodies() {
        let root = setup_fixture();
        write_file(&root, "orig.md", b"---\ntitle: A\n---\n# Hello\n");
        write_file(&root, "mig.md", b"---\ntitle: B\n---\n# Hello\n");
        let manifest = r#"{"files":[{"original_path":"orig.md","migrated_path":"mig.md"}]}"#;
        write_file(&root, "bp.json", manifest.as_bytes());
        let report = validate_body_preservation_from_path(&root, "bp.json").unwrap();
        assert!(report.passed);
        assert_eq!(report.files.len(), 1);
        assert!(report.files[0].body_preserved);
    }

    #[test]
    fn from_path_rejects_oversized_manifest() {
        let root = setup_fixture();
        let big = vec![b' '; (MAX_MANIFEST_BYTES as usize) + 1];
        fs::write(root.join("big.json"), &big).unwrap();
        assert!(matches!(
            validate_body_preservation_from_path(&root, "big.json"),
            Err(MigrationError::ManifestTooLarge(_))
        ));
    }

    #[test]
    fn from_path_rejects_unsafe_path() {
        let root = setup_fixture();
        write_file(&root, "bp.json", b"{}");
        assert!(matches!(
            validate_body_preservation_from_path(&root, "../escape.json"),
            Err(MigrationError::UnsafePath)
        ));
    }

    #[test]
    fn parse_string_negative() {
        let cases: &[(&[u8], &str)] = &[
            (&[b'"', 0xFF, b'"'], "invalid raw UTF-8"),
            (b"\"\\uGGGG\"", "invalid hex in \\u"),
            (b"\"\\u12\"", "truncated \\u"),
            (b"\"\\uD800\"", "lone high surrogate"),
            (b"\"\\uDC00\"", "lone low surrogate"),
            (b"\"\\uD800\\u0041\"", "high surrogate + non-low"),
            (&[b'"', 0x01, b'"'], "unescaped control byte"),
        ];
        for (input, label) in cases {
            assert!(
                JsonParser::new(input).parse_string().is_err(),
                "expected error: {label}"
            );
        }
    }

    // --- 2.1-C remediation core tests ---

    #[test]
    fn from_path_exact_2mib_manifest_reaches_parser() {
        let root = setup_fixture();
        let exact = vec![b' '; MAX_MANIFEST_BYTES as usize];
        fs::write(root.join("exact.json"), &exact).unwrap();
        // reaches parser, not Oversized
        assert!(matches!(
            validate_body_preservation_from_path(&root, "exact.json"),
            Err(MigrationError::ManifestInvalidJson(_))
        ));
    }

    #[test]
    fn from_path_equals_direct_raw_validator_output() {
        let root = setup_fixture();
        write_file(&root, "orig.md", b"---\ntitle: A\n---\n# Hello\n");
        write_file(&root, "mig.md", b"---\ntitle: B\n---\n# Hello\n");
        let manifest = r#"{"files":[{"original_path":"orig.md","migrated_path":"mig.md"}]}"#;
        write_file(&root, "bp.json", manifest.as_bytes());
        let from_path = validate_body_preservation_from_path(&root, "bp.json").unwrap();
        let direct = validate_body_preservation(&root, "bp.json", manifest.as_bytes()).unwrap();
        assert_eq!(from_path.files, direct.files);
        assert_eq!(from_path.findings, direct.findings);
        assert_eq!(from_path.passed, direct.passed);
        assert_eq!(from_path.schema_version, direct.schema_version);
    }

    #[test]
    fn from_path_symlink_manifest_validation_error() {
        let root = setup_fixture();
        write_file(&root, "real.json", b"{\"files\":[]}");
        std::os::unix::fs::symlink("real.json", root.join("link.json")).unwrap();
        assert!(matches!(
            validate_body_preservation_from_path(&root, "link.json"),
            Err(MigrationError::InvalidManifestInput)
        ));
    }

    #[test]
    fn from_path_directory_manifest_validation_error() {
        let root = setup_fixture();
        fs::create_dir(root.join("subdir")).unwrap();
        assert!(matches!(
            validate_body_preservation_from_path(&root, "subdir"),
            Err(MigrationError::InvalidManifestInput)
        ));
    }

    #[test]
    fn from_path_missing_manifest_validation_error() {
        let root = setup_fixture();
        assert!(matches!(
            validate_body_preservation_from_path(&root, "nope.json"),
            Err(MigrationError::InvalidManifestInput)
        ));
    }

    #[test]
    fn from_path_containment_escape_validation_error() {
        let root = setup_fixture();
        // symlink inside root pointing outside
        let outside = std::env::temp_dir().join(format!("bran-escape-{}", std::process::id()));
        fs::write(&outside, b"{}").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape.json")).unwrap();
        assert!(matches!(
            validate_body_preservation_from_path(&root, "escape.json"),
            Err(MigrationError::InvalidManifestInput)
        ));
    }

    #[test]
    fn from_path_limit_plus_one_oversized() {
        let root = setup_fixture();
        let big = vec![b' '; (MAX_MANIFEST_BYTES as usize) + 1];
        fs::write(root.join("big.json"), &big).unwrap();
        assert!(matches!(
            validate_body_preservation_from_path(&root, "big.json"),
            Err(MigrationError::ManifestTooLarge(_))
        ));
    }

    #[test]
    fn from_path_windows_style_validation_error() {
        let root = setup_fixture();
        assert!(matches!(
            validate_body_preservation_from_path(&root, "C:\\foo\\bar.json"),
            Err(MigrationError::UnsafePath)
        ));
        assert!(matches!(
            validate_body_preservation_from_path(&root, "D:/stuff/file.json"),
            Err(MigrationError::UnsafePath)
        ));
    }

    #[test]
    fn from_path_unc_validation_error() {
        let root = setup_fixture();
        assert!(matches!(
            validate_body_preservation_from_path(&root, "\\\\server\\share\\file.json"),
            Err(MigrationError::UnsafePath)
        ));
    }

    #[test]
    fn from_path_nul_validation_error() {
        let root = setup_fixture();
        assert!(matches!(
            validate_body_preservation_from_path(&root, "bad\0.json"),
            Err(MigrationError::UnsafePath)
        ));
    }

    #[test]
    fn from_path_traversal_validation_error() {
        let root = setup_fixture();
        assert!(matches!(
            validate_body_preservation_from_path(&root, "../escape.json"),
            Err(MigrationError::UnsafePath)
        ));
    }
}
