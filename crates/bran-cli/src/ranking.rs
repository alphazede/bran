//! Path-class tier configuration (`.bran/ranking.yaml`), copy-free
//! case-insensitive matching, and defining-file brief scoring (DES-04).

use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::path::Path;

/// Repository-relative path to the optional ranking configuration.
pub const RANKING_FILENAME: &str = ".bran/ranking.yaml";

/// Maximum accepted `.bran/ranking.yaml` size in bytes (64 KiB).
pub const MAX_RANKING_BYTES: usize = 64 * 1024;

/// Maximum number of body occurrences rewarded per query term.
pub const BODY_HIT_CAP: usize = 3;

const MAX_TIER: u8 = 9;

const WEIGHT_COVERAGE: u64 = 2_000;
const WEIGHT_ENTITY_COVERAGE: u64 = 3_000;
const WEIGHT_EXACT: u64 = 500;
const WEIGHT_FILENAME_SEG: u64 = 500;
const WEIGHT_FILENAME_SUB: u64 = 320;
const WEIGHT_PATH_SEG: u64 = 350;
const WEIGHT_PATH_SUB: u64 = 180;
const WEIGHT_FACT_WORD: u64 = 250;
const WEIGHT_FACT_SUB: u64 = 120;
const WEIGHT_PARENT_FACT: u64 = 30;
const WEIGHT_BODY_WORD: u64 = 25;
const WEIGHT_BODY_HIT: u64 = 10;
// Morphological evidence scores 3/4 of the same exact-token evidence.
const WEIGHT_FILENAME_STEM: u64 = WEIGHT_FILENAME_SEG * 3 / 4;
const WEIGHT_PATH_STEM: u64 = WEIGHT_PATH_SEG * 3 / 4;

/// English function and interrogative words that do not carry domain identity
/// when computing the brief defining-file score (coverage JSON stays unchanged).
const BRIEF_STOP_WORDS: &[&str] = &[
    "across", "after", "all", "also", "any", "are", "because", "been", "before", "being", "both",
    "can", "could", "did", "does", "each", "every", "had", "has", "have", "here", "how", "into",
    "its", "many", "may", "more", "most", "must", "often", "only", "onto", "other", "should",
    "some", "such", "than", "that", "their", "them", "then", "there", "these", "they", "those",
    "too", "unless", "until", "upon", "very", "was", "were", "what", "when", "where", "which",
    "who", "whom", "whose", "will", "would",
];

/// Returns true if `term` is a common English filler word ignored by brief
/// ranking when non-stop terms are present.
pub fn is_brief_stop_word(term: &str) -> bool {
    BRIEF_STOP_WORDS.contains(&term)
}

/// Conservative ASCII morphological stem for fallback matching in brief mode
/// (e.g. `tests` -> `test`, `pinned` -> `pin`, `indexed` -> `index`,
/// `studies` -> `study`). Applied to query words and document tokens alike, so
/// two words match morphologically when their normalized forms are equal.
/// Returns `None` when no safe stem exists.
pub fn ascii_stem(word: &str) -> Option<String> {
    if word.len() < 5 || !word.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    if let Some(base) = word.strip_suffix("ies") {
        return (base.len() >= 2).then(|| format!("{base}y"));
    }
    let stem = if word.ends_with("es")
        && (word.ends_with("shes")
            || word.ends_with("ches")
            || word.ends_with("sses")
            || word.ends_with("xes")
            || word.ends_with("zes"))
    {
        &word[..word.len() - 2]
    } else if word.ends_with('s')
        && !word.ends_with("ss")
        && !word.ends_with("us")
        && !word.ends_with("is")
    {
        &word[..word.len() - 1]
    } else if let Some(base) = word.strip_suffix("ed") {
        collapse_double_consonant(base)
    } else if let Some(base) = word.strip_suffix("ing") {
        if base.len() >= 4 {
            collapse_double_consonant(base)
        } else {
            return None;
        }
    } else {
        return None;
    };
    if stem.len() >= 3 && stem != word {
        Some(stem.to_owned())
    } else {
        None
    }
}

/// Normalized form of a query word for morphological fallback, or `None` when
/// no document token can match it morphologically without also containing the
/// word itself (the surface pass already covers that case).
pub fn morph_norm(word: &str) -> Option<String> {
    if word.len() < 3 || !word.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    match ascii_stem(word) {
        Some(stem) => Some(stem),
        // `study` also matches the token `studies`, which does not contain it.
        None => word.ends_with('y').then(|| word.to_owned()),
    }
}

/// True when `haystack` has an alphanumeric token whose normal form (the token
/// itself, or its `ascii_stem`) equals `norm`, ASCII case-insensitively. Whole tokens only: a stem never matches inside an
/// unrelated longer word.
pub fn has_morph_token(haystack: &str, norm: &str) -> bool {
    // Every token normalizing to `norm` contains `norm` minus a trailing `y`.
    let root = norm.strip_suffix('y').unwrap_or(norm);
    if root.is_empty() || !contains_ascii_ci(haystack, root) {
        return false;
    }
    let mut lower = String::new();
    haystack
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| token.len() >= root.len() && token.is_ascii())
        .any(|token| {
            lower.clear();
            lower.push_str(token);
            lower.make_ascii_lowercase();
            lower == norm || ascii_stem(&lower).as_deref() == Some(norm)
        })
}

/// True when `root` (ASCII, at least `min_len` bytes) occurs in `haystack`
/// starting at a sub-word boundary: a token start, a lower-to-upper camelCase
/// hump, or a letter/digit transition. Finds morphological matches inside
/// compound identifiers (`IndexBuilder`) but never mid-word (`Install`).
pub fn has_morph_subword(haystack: &str, root: &str, min_len: usize) -> bool {
    if root.len() < min_len || !root.is_ascii() {
        return false;
    }
    let h = haystack.as_bytes();
    let mut pos = 0usize;
    while let Some(idx) = find_ascii_ci_from(h, root.as_bytes(), pos) {
        let cur = h[idx];
        let starts_subword = idx == 0 || {
            let prev = h[idx - 1];
            (prev < 0x80 && !prev.is_ascii_alphanumeric())
                || ((prev.is_ascii_lowercase() || prev.is_ascii_digit())
                    && cur.is_ascii_uppercase())
                || prev.is_ascii_digit() != cur.is_ascii_digit()
        };
        if starts_subword {
            return true;
        }
        pos = idx + 1;
    }
    false
}

fn collapse_double_consonant(base: &str) -> &str {
    let bytes = base.as_bytes();
    if bytes.len() >= 4 {
        let last = bytes[bytes.len() - 1];
        let prev = bytes[bytes.len() - 2];
        if last == prev && !matches!(last, b'a' | b'e' | b'i' | b'o' | b'u' | b's' | b'l') {
            return &base[..base.len() - 1];
        }
    }
    base
}

/// Parsed, immutable path-class configuration from `.bran/ranking.yaml`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RankingConfig {
    classes: Vec<PathClass>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PathClass {
    name: String,
    tier: u8,
    globs: Vec<CompiledGlob>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CompiledGlob {
    segments: Vec<SegmentPattern>,
    min_segments: usize,
    has_double_star: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SegmentPattern {
    DoubleStar,
    Literal(String),
    Wildcard(String),
}

impl SegmentPattern {
    fn matches(&self, segment: &str) -> bool {
        match self {
            Self::DoubleStar => true,
            Self::Literal(lit) => segment == lit,
            Self::Wildcard(pat) => wildcard_match(pat.as_bytes(), segment.as_bytes()),
        }
    }
}

impl CompiledGlob {
    fn parse(raw: &str) -> Result<Self, String> {
        if raw.is_empty() || raw.starts_with('/') || raw.contains(['\0', '\r', '\n', '\\']) {
            return Err(format!("invalid glob pattern: {raw:?}"));
        }
        let mut segments = Vec::new();
        let mut min_segments = 0usize;
        let mut has_double_star = false;
        for seg in raw.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." {
                return Err(format!("invalid path segment in glob pattern: {raw:?}"));
            }
            if seg == "**" {
                has_double_star = true;
                if !matches!(segments.last(), Some(SegmentPattern::DoubleStar)) {
                    segments.push(SegmentPattern::DoubleStar);
                }
            } else if seg.contains("**") {
                return Err(format!(
                    "'**' must appear as a whole path segment in glob pattern: {raw:?}"
                ));
            } else if seg.contains('*') {
                min_segments += 1;
                segments.push(SegmentPattern::Wildcard(seg.to_owned()));
            } else {
                min_segments += 1;
                segments.push(SegmentPattern::Literal(seg.to_owned()));
            }
        }
        Ok(Self {
            segments,
            min_segments,
            has_double_star,
        })
    }

    fn matches_segments(&self, path_segs: &[&str]) -> bool {
        let pat = &self.segments;
        if (!self.has_double_star && path_segs.len() != pat.len())
            || path_segs.len() < self.min_segments
        {
            return false;
        }
        if let Some(SegmentPattern::Literal(first)) = pat.first() {
            if path_segs.first() != Some(&first.as_str()) {
                return false;
            }
        }
        if let Some(SegmentPattern::Literal(last)) = pat.last() {
            if path_segs.last() != Some(&last.as_str()) {
                return false;
            }
        }
        let (mut pi, mut si) = (0usize, 0usize);
        let mut star_pi: Option<usize> = None;
        let mut star_si = 0usize;
        while si < path_segs.len() {
            if pi < pat.len() {
                match &pat[pi] {
                    SegmentPattern::DoubleStar => {
                        star_pi = Some(pi);
                        star_si = si;
                        pi += 1;
                        continue;
                    }
                    other if other.matches(path_segs[si]) => {
                        pi += 1;
                        si += 1;
                        continue;
                    }
                    _ => {}
                }
            }
            if let Some(sp) = star_pi {
                pi = sp + 1;
                star_si += 1;
                si = star_si;
            } else {
                return false;
            }
        }
        while pi < pat.len() && matches!(pat[pi], SegmentPattern::DoubleStar) {
            pi += 1;
        }
        pi == pat.len()
    }
}

fn wildcard_match(pat: &[u8], text: &[u8]) -> bool {
    let (mut pi, mut si) = (0usize, 0usize);
    let mut star_pi: Option<usize> = None;
    let mut star_si = 0usize;
    while si < text.len() {
        if pi < pat.len() && pat[pi] == b'*' {
            star_pi = Some(pi);
            star_si = si;
            pi += 1;
        } else if pi < pat.len() && pat[pi] == text[si] {
            pi += 1;
            si += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_si += 1;
            si = star_si;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

#[derive(Default)]
struct ParsedClass {
    name: Option<String>,
    tier: Option<u8>,
    globs: Option<Vec<String>>,
}

impl ParsedClass {
    fn finish(self, line: usize) -> Result<PathClass, String> {
        let name = self
            .name
            .ok_or_else(|| format!("line {line}: class missing 'name'"))?;
        let tier = self
            .tier
            .ok_or_else(|| format!("line {line}: class '{name}' missing 'tier'"))?;
        let raw_globs = self
            .globs
            .ok_or_else(|| format!("line {line}: class '{name}' missing 'globs'"))?;
        if raw_globs.is_empty() {
            return Err(format!("line {line}: class '{name}' has empty 'globs'"));
        }
        let mut globs = Vec::with_capacity(raw_globs.len());
        for raw in raw_globs {
            globs.push(CompiledGlob::parse(&raw)?);
        }
        Ok(PathClass { name, tier, globs })
    }
}

fn strip_yaml_comment(line: &str) -> Result<&str, String> {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    // A quote opens a quoted scalar only where a scalar starts; inside a plain
    // scalar (`team's-docs`) it is an ordinary character.
    let mut scalar_start = true;
    let mut in_flow = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        let next_is_space = bytes.get(i + 1).is_none_or(u8::is_ascii_whitespace);
        match quote {
            Some(b'"') => {
                if b == b'\\' {
                    i += 2;
                    continue;
                }
                if b == b'"' {
                    quote = None;
                }
            }
            Some(b'\'') => {
                if b == b'\'' {
                    if bytes.get(i + 1) == Some(&b'\'') {
                        i += 2;
                        continue;
                    }
                    quote = None;
                }
            }
            Some(_) => unreachable!(),
            None => {
                if b == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
                    return Ok(&line[..i]);
                }
                if b.is_ascii_whitespace() {
                    i += 1;
                    continue;
                }
                if scalar_start && (b == b'"' || b == b'\'') {
                    quote = Some(b);
                } else if scalar_start && b == b'[' {
                    in_flow = true;
                } else if in_flow && b == b']' {
                    in_flow = false;
                }
                let line_start = bytes[..i].iter().all(u8::is_ascii_whitespace);
                scalar_start = (b == b':' && next_is_space)
                    || (b == b'-' && line_start && next_is_space)
                    || (in_flow && (b == b'[' || b == b','));
            }
        }
        i += 1;
    }
    if quote.is_some() {
        return Err("unclosed string quote".to_owned());
    }
    Ok(line)
}

fn parse_yaml_scalar(raw: &str, line_no: usize) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("line {line_no}: expected scalar value"));
    }
    if let Some(inner) = trimmed.strip_prefix('"') {
        let inner = inner
            .strip_suffix('"')
            .ok_or_else(|| format!("line {line_no}: unclosed double quote"))?;
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('\\') => out.push('\\'),
                    Some('"') => out.push('"'),
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => {
                        return Err(format!("line {line_no}: unsupported escape '\\{other}'"));
                    }
                    None => return Err(format!("line {line_no}: trailing backslash")),
                }
            } else {
                out.push(c);
            }
        }
        return Ok(out);
    }
    if let Some(inner) = trimmed.strip_prefix('\'') {
        let inner = inner
            .strip_suffix('\'')
            .ok_or_else(|| format!("line {line_no}: unclosed single quote"))?;
        return Ok(inner.replace("''", "'"));
    }
    Ok(trimmed.to_owned())
}

fn parse_inline_list(inner: &str, line_no: usize) -> Result<Vec<String>, String> {
    let mut items = Vec::new();
    let mut start = 0usize;
    let bytes = inner.as_bytes();
    let mut quote: Option<u8> = None;
    let mut item_start = true;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(b'"') => {
                if b == b'\\' {
                    i += 2;
                    continue;
                }
                if b == b'"' {
                    quote = None;
                }
            }
            Some(b'\'') => {
                if b == b'\'' {
                    if bytes.get(i + 1) == Some(&b'\'') {
                        i += 2;
                        continue;
                    }
                    quote = None;
                }
            }
            Some(_) => unreachable!(),
            None => {
                if item_start && (b == b'"' || b == b'\'') {
                    quote = Some(b);
                } else if b == b',' {
                    let piece = inner[start..i].trim();
                    if piece.is_empty() {
                        return Err(format!("line {line_no}: empty inline list item"));
                    }
                    items.push(parse_yaml_scalar(piece, line_no)?);
                    start = i + 1;
                    item_start = true;
                    i += 1;
                    continue;
                }
                if !b.is_ascii_whitespace() {
                    item_start = false;
                }
            }
        }
        i += 1;
    }
    if quote.is_some() {
        return Err(format!("line {line_no}: unclosed quote in inline list"));
    }
    let tail = inner[start..].trim();
    if !tail.is_empty() {
        items.push(parse_yaml_scalar(tail, line_no)?);
    }
    Ok(items)
}

fn apply_class_field(
    partial: &mut ParsedClass,
    text: &str,
    line_no: usize,
    key_indent: usize,
    in_globs_block: &mut bool,
    globs_indent: &mut Option<usize>,
) -> Result<(), String> {
    let Some((key, rest)) = text.split_once(':') else {
        return Err(format!("line {line_no}: expected 'key: value'"));
    };
    let key = key.trim();
    let val = rest.trim();
    match key {
        "name" => {
            if partial.name.is_some() {
                return Err(format!("line {line_no}: duplicate 'name'"));
            }
            let name = parse_yaml_scalar(val, line_no)?;
            if name.is_empty() {
                return Err(format!("line {line_no}: 'name' must not be empty"));
            }
            partial.name = Some(name);
        }
        "tier" => {
            if partial.tier.is_some() {
                return Err(format!("line {line_no}: duplicate 'tier'"));
            }
            let tier: u8 = val.parse().map_err(|_| {
                format!("line {line_no}: invalid 'tier' {val:?} (expected 0..={MAX_TIER})")
            })?;
            if tier > MAX_TIER {
                return Err(format!(
                    "line {line_no}: tier {tier} out of range (expected 0..={MAX_TIER})"
                ));
            }
            partial.tier = Some(tier);
        }
        "globs" => {
            if partial.globs.is_some() {
                return Err(format!("line {line_no}: duplicate 'globs'"));
            }
            if val.is_empty() {
                partial.globs = Some(Vec::new());
                *in_globs_block = true;
                *globs_indent = Some(key_indent);
            } else if val.starts_with('[') && val.ends_with(']') {
                let globs = parse_inline_list(&val[1..val.len() - 1], line_no)?;
                partial.globs = Some(globs);
            } else {
                return Err(format!("line {line_no}: 'globs' must be a YAML list"));
            }
        }
        other => {
            return Err(format!("line {line_no}: unknown class field {other:?}"));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

impl RankingConfig {
    /// Load `.bran/ranking.yaml` from `root`. A missing file yields the default
    /// empty configuration (all paths tier 0). Any I/O, safety, or syntax error
    /// returns an error naming `.bran/ranking.yaml`.
    pub fn load(root: &Path) -> Result<Self, String> {
        let bran_dir = root.join(".bran");
        let ranking_path = bran_dir.join("ranking.yaml");
        let ranking_meta = match fs::symlink_metadata(&ranking_path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(format!("{RANKING_FILENAME}: unreadable ({err})")),
        };
        let bran_meta = fs::symlink_metadata(&bran_dir)
            .map_err(|err| format!("{RANKING_FILENAME}: unreadable .bran ({err})"))?;
        if bran_meta.file_type().is_symlink() || !bran_meta.is_dir() {
            return Err(format!("{RANKING_FILENAME}: .bran must be a directory"));
        }
        if ranking_meta.file_type().is_symlink() || !ranking_meta.file_type().is_file() {
            return Err(format!("{RANKING_FILENAME}: expected a regular file"));
        }
        let canonical_root = fs::canonicalize(root)
            .map_err(|err| format!("{RANKING_FILENAME}: invalid repository root ({err})"))?;
        let canonical_bran = fs::canonicalize(&bran_dir)
            .map_err(|err| format!("{RANKING_FILENAME}: invalid .bran directory ({err})"))?;
        if !canonical_bran.starts_with(&canonical_root) {
            return Err(format!("{RANKING_FILENAME}: .bran escapes repository root"));
        }
        let canonical_file = fs::canonicalize(&ranking_path)
            .map_err(|err| format!("{RANKING_FILENAME}: unreadable ({err})"))?;
        if !canonical_file.starts_with(&canonical_bran) {
            return Err(format!("{RANKING_FILENAME}: path escapes .bran directory"));
        }
        let file = open_no_follow(&canonical_file)
            .map_err(|err| format!("{RANKING_FILENAME}: unreadable ({err})"))?;
        let meta = file
            .metadata()
            .map_err(|err| format!("{RANKING_FILENAME}: unreadable ({err})"))?;
        if !meta.file_type().is_file() {
            return Err(format!("{RANKING_FILENAME}: expected a regular file"));
        }
        if meta.len() > MAX_RANKING_BYTES as u64 {
            return Err(format!(
                "{RANKING_FILENAME}: file exceeds {MAX_RANKING_BYTES} bytes"
            ));
        }
        let mut bytes = Vec::new();
        file.take(MAX_RANKING_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|err| format!("{RANKING_FILENAME}: read error ({err})"))?;
        if bytes.len() > MAX_RANKING_BYTES {
            return Err(format!(
                "{RANKING_FILENAME}: file exceeds {MAX_RANKING_BYTES} bytes"
            ));
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| format!("{RANKING_FILENAME}: invalid UTF-8"))?;
        Self::parse_from_str(text).map_err(|msg| format!("{RANKING_FILENAME}: {msg}"))
    }

    /// Parse the restricted YAML subset for `.bran/ranking.yaml`.
    pub fn parse_from_str(text: &str) -> Result<Self, String> {
        let mut classes = Vec::new();
        let mut saw_classes_key = false;
        let mut classes_inline_empty = false;
        let mut current: Option<(usize, ParsedClass)> = None;
        let mut item_indent: Option<usize> = None;
        // Column of the current class's fields; every sibling key must align.
        let mut field_indent: Option<usize> = None;
        let mut in_globs_block = false;
        let mut globs_indent: Option<usize> = None;

        for (idx, raw_line) in text.lines().enumerate() {
            let line_no = idx + 1;
            if raw_line
                .chars()
                .take_while(|c| c.is_whitespace())
                .any(|c| c == '\t')
            {
                return Err(format!("line {line_no}: tabs are not allowed in YAML"));
            }
            let without_comment =
                strip_yaml_comment(raw_line).map_err(|msg| format!("line {line_no}: {msg}"))?;
            if without_comment.trim().is_empty() {
                continue;
            }
            let indent = without_comment.len() - without_comment.trim_start_matches(' ').len();
            let trimmed = without_comment.trim();

            if indent == 0 && !trimmed.starts_with('-') {
                if let Some((start_line, partial)) = current.take() {
                    classes.push(partial.finish(start_line)?);
                }
                in_globs_block = false;
                globs_indent = None;
                let Some((key, rest)) = trimmed.split_once(':') else {
                    return Err(format!("line {line_no}: expected 'key: value'"));
                };
                if key.trim() != "classes" {
                    return Err(format!(
                        "line {line_no}: unknown top-level key '{}'",
                        key.trim()
                    ));
                }
                if saw_classes_key {
                    return Err(format!("line {line_no}: duplicate 'classes' key"));
                }
                saw_classes_key = true;
                let rest = rest.trim();
                if rest == "[]" {
                    classes_inline_empty = true;
                } else if !rest.is_empty() {
                    return Err(format!("line {line_no}: 'classes' must be a list or '[]'"));
                }
                continue;
            }

            if !saw_classes_key || classes_inline_empty {
                return Err(format!("line {line_no}: unexpected content"));
            }

            if in_globs_block {
                let min_indent = globs_indent.unwrap_or(0);
                let deeper_than_item = item_indent.is_none_or(|item| indent > item);
                if indent >= min_indent && deeper_than_item && trimmed.starts_with("- ") {
                    let item = parse_yaml_scalar(trimmed[2..].trim(), line_no)?;
                    let (_, partial) = current
                        .as_mut()
                        .ok_or_else(|| format!("line {line_no}: glob outside class"))?;
                    partial.globs.get_or_insert_with(Vec::new).push(item);
                    continue;
                }
                if indent >= min_indent && deeper_than_item && trimmed == "-" {
                    return Err(format!("line {line_no}: empty glob item"));
                }
                in_globs_block = false;
                globs_indent = None;
            }

            let is_new_item = (trimmed.starts_with("- ") || trimmed == "-")
                && item_indent.is_none_or(|expected| indent == expected);
            if is_new_item {
                if let Some((start_line, partial)) = current.take() {
                    classes.push(partial.finish(start_line)?);
                }
                item_indent = Some(indent);
                let mut partial = ParsedClass::default();
                let after_dash_raw = trimmed.strip_prefix('-').unwrap();
                let after_dash = after_dash_raw.trim();
                field_indent = (!after_dash.is_empty())
                    .then(|| indent + 1 + after_dash_raw.len() - after_dash_raw.trim_start().len());
                if !after_dash.is_empty() {
                    apply_class_field(
                        &mut partial,
                        after_dash,
                        line_no,
                        indent + 2,
                        &mut in_globs_block,
                        &mut globs_indent,
                    )?;
                }
                current = Some((line_no, partial));
                continue;
            }

            if let Some((_, partial)) = current.as_mut() {
                if let Some(expected_item_indent) = item_indent {
                    if indent <= expected_item_indent {
                        return Err(format!("line {line_no}: invalid indentation"));
                    }
                }
                if *field_indent.get_or_insert(indent) != indent {
                    return Err(format!("line {line_no}: invalid indentation"));
                }
                apply_class_field(
                    partial,
                    trimmed,
                    line_no,
                    indent,
                    &mut in_globs_block,
                    &mut globs_indent,
                )?;
            } else {
                return Err(format!(
                    "line {line_no}: expected '-' list item under classes"
                ));
            }
        }

        if let Some((start_line, partial)) = current.take() {
            classes.push(partial.finish(start_line)?);
        }
        if !saw_classes_key {
            return Err("missing required top-level key 'classes'".to_owned());
        }
        Ok(Self { classes })
    }

    /// Return `(tier, Some(class_name))` for the first matching class in
    /// declaration order, or `(0, None)` when no class matches.
    pub fn classify(&self, rel_path: &str) -> (u8, Option<&str>) {
        if self.classes.is_empty() {
            return (0, None);
        }
        let mut stack = [""; 32];
        let mut count = 0usize;
        let mut overflow: Option<Vec<&str>> = None;
        for seg in rel_path.split('/') {
            if let Some(vec) = overflow.as_mut() {
                vec.push(seg);
            } else if count < stack.len() {
                stack[count] = seg;
                count += 1;
            } else {
                let mut vec = Vec::with_capacity(64);
                vec.extend_from_slice(&stack);
                vec.push(seg);
                overflow = Some(vec);
            }
        }
        let path_segs: &[&str] = overflow.as_deref().unwrap_or(&stack[..count]);
        for class in &self.classes {
            if class
                .globs
                .iter()
                .any(|glob| glob.matches_segments(path_segs))
            {
                return (class.tier, Some(class.name.as_str()));
            }
        }
        (0, None)
    }
}

#[cfg(target_endian = "little")]
#[inline]
fn next_hit_lane(mask: &mut u64) -> usize {
    let lane = (mask.trailing_zeros() >> 3) as usize;
    *mask &= *mask - 1;
    lane
}

#[cfg(target_endian = "big")]
#[inline]
fn next_hit_lane(mask: &mut u64) -> usize {
    let lane = (mask.leading_zeros() >> 3) as usize;
    *mask ^= 0x8000_0000_0000_0000u64 >> (lane * 8);
    lane
}

#[inline]
fn find_ascii_ci_from(h: &[u8], n: &[u8], mut i: usize) -> Option<usize> {
    if n.is_empty() {
        return (i <= h.len()).then_some(i);
    }
    if i + n.len() > h.len() {
        return None;
    }
    let limit = h.len() - n.len();
    let scan_end = limit + 1;
    let first_lower = n[0].to_ascii_lowercase();
    let first_upper = n[0].to_ascii_uppercase();
    if first_lower == first_upper {
        let target = u64::from_ne_bytes([first_lower; 8]);
        while i + 8 <= scan_end {
            let w = u64::from_ne_bytes(h[i..i + 8].try_into().unwrap());
            let x = w ^ target;
            let mut hit_mask = x.wrapping_sub(0x0101_0101_0101_0101) & !x & 0x8080_8080_8080_8080;
            while hit_mask != 0 {
                let pos = i + next_hit_lane(&mut hit_mask);
                if h[pos..pos + n.len()].eq_ignore_ascii_case(n) {
                    return Some(pos);
                }
            }
            i += 8;
        }
        while i <= limit {
            if h[i] == first_lower && h[i..i + n.len()].eq_ignore_ascii_case(n) {
                return Some(i);
            }
            i += 1;
        }
    } else if first_lower.is_ascii_lowercase() {
        let mask = 0x2020_2020_2020_2020u64;
        let target = u64::from_ne_bytes([first_lower; 8]);
        while i + 8 <= scan_end {
            let w = u64::from_ne_bytes(h[i..i + 8].try_into().unwrap());
            let x = (w | mask) ^ target;
            let mut hit_mask = x.wrapping_sub(0x0101_0101_0101_0101) & !x & 0x8080_8080_8080_8080;
            while hit_mask != 0 {
                let pos = i + next_hit_lane(&mut hit_mask);
                if h[pos..pos + n.len()].eq_ignore_ascii_case(n) {
                    return Some(pos);
                }
            }
            i += 8;
        }
        while i <= limit {
            if (h[i] | 0x20) == first_lower && h[i..i + n.len()].eq_ignore_ascii_case(n) {
                return Some(i);
            }
            i += 1;
        }
    } else {
        while i <= limit {
            let b = h[i];
            if (b == first_lower || b == first_upper) && h[i..i + n.len()].eq_ignore_ascii_case(n) {
                return Some(i);
            }
            i += 1;
        }
    }
    None
}

#[inline]
fn is_alnum_word_boundary(haystack: &str, start: usize, end: usize) -> bool {
    let left_ok = haystack[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_alphanumeric());
    left_ok
        && haystack[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric())
}

/// Copy-free ASCII case-insensitive substring check. Equivalent to
/// `haystack.to_ascii_lowercase().contains(needle)` when `needle` is already
/// ASCII-lowercase.
pub fn contains_ascii_ci(haystack: &str, needle: &str) -> bool {
    find_ascii_ci_from(haystack.as_bytes(), needle.as_bytes(), 0).is_some()
}

/// Precompiled Wu-Manber multi-pattern matcher for pre-lowercased text aligned
/// byte-for-byte with the original body text. Processes patterns in batches of
/// 32 so queries of any size are supported without caps.
pub struct BodyMultiMatcher<'a> {
    batches: Vec<WuManberBatch<'a>>,
}

struct WuManberBatch<'a> {
    patterns: Vec<(&'a [u8], bool)>,
    min_len: usize,
    shift: [u8; 4096],
    buckets: [u32; 4096],
    fallback: bool,
}

#[inline]
fn wm_hash2(b0: u8, b1: u8) -> usize {
    (((b0 as usize) << 5) ^ (b1 as usize)) & 4095
}

impl<'a> BodyMultiMatcher<'a> {
    pub fn new(specs: &[(&'a str, bool)]) -> Self {
        let mut batches = Vec::new();
        for chunk in specs.chunks(32) {
            let patterns: Vec<(&[u8], bool)> = chunk
                .iter()
                .map(|&(s, check_word)| {
                    let word_ok =
                        check_word && !s.is_empty() && s.chars().all(char::is_alphanumeric);
                    (s.as_bytes(), word_ok)
                })
                .collect();
            let raw_min = patterns.iter().map(|(p, _)| p.len()).min().unwrap_or(0);
            if raw_min < 2 {
                batches.push(WuManberBatch {
                    patterns,
                    min_len: raw_min,
                    shift: [0; 4096],
                    buckets: [0; 4096],
                    fallback: true,
                });
                continue;
            }
            let min_len = raw_min.min(256);
            let default_shift = (min_len - 1) as u8;
            let mut shift = [default_shift; 4096];
            let mut buckets = [0u32; 4096];
            for (idx, &(pat, _)) in patterns.iter().enumerate() {
                for j in 0..=min_len - 2 {
                    let hv = wm_hash2(pat[j], pat[j + 1]);
                    let d = (min_len - 2 - j) as u8;
                    if d < shift[hv] {
                        shift[hv] = d;
                    }
                    if d == 0 {
                        buckets[hv] |= 1u32 << idx;
                    }
                }
            }
            batches.push(WuManberBatch {
                patterns,
                min_len,
                shift,
                buckets,
                fallback: false,
            });
        }
        Self { batches }
    }

    pub fn match_body(
        &self,
        body: &str,
        body_lower: &str,
        cap: usize,
        counts: &mut [u8],
        words: &mut [bool],
    ) {
        if cap == 0 {
            return;
        }
        let text = body_lower.as_bytes();
        let end = text.len();
        let mut offset = 0usize;
        for batch in &self.batches {
            let batch_len = batch.patterns.len();
            if batch.fallback {
                for (i, &(pat, check_word)) in batch.patterns.iter().enumerate() {
                    let pat_str = std::str::from_utf8(pat).unwrap_or("");
                    let (c, w) = count_and_word_lower(body, body_lower, pat_str, cap);
                    counts[offset + i] = c as u8;
                    words[offset + i] = if check_word { w } else { c > 0 };
                }
                offset += batch_len;
                continue;
            }
            let min_len = batch.min_len;
            if end < min_len {
                offset += batch_len;
                continue;
            }
            let mut active_mask: u32 = if batch_len == 32 {
                u32::MAX
            } else {
                (1u32 << batch_len) - 1
            };
            let mut next_allowed = [0usize; 32];
            let mut local_counts = [0usize; 32];
            let mut local_words = [false; 32];
            let mut pos = min_len - 2;
            while pos + 1 < end {
                let hv = wm_hash2(text[pos], text[pos + 1]);
                let s = batch.shift[hv] as usize;
                if s > 0 {
                    pos += s;
                    continue;
                }
                let start = pos + 2 - min_len;
                let mut cand = batch.buckets[hv] & active_mask;
                while cand != 0 {
                    let bit = cand.trailing_zeros() as usize;
                    cand &= cand - 1;
                    let (pat, check_word) = batch.patterns[bit];
                    let pat_end = start + pat.len();
                    if start >= next_allowed[bit] && pat_end <= end && &text[start..pat_end] == pat
                    {
                        if local_counts[bit] < cap {
                            local_counts[bit] += 1;
                        }
                        if !local_words[bit]
                            && (!check_word || is_alnum_word_boundary(body, start, pat_end))
                        {
                            local_words[bit] = true;
                        }
                        next_allowed[bit] = pat_end;
                        if local_counts[bit] >= cap && local_words[bit] {
                            active_mask &= !(1u32 << bit);
                            if active_mask == 0 {
                                pos = end;
                                break;
                            }
                        }
                    }
                }
                pos += 1;
            }
            for i in 0..batch_len {
                counts[offset + i] = local_counts[i] as u8;
                words[offset + i] = local_words[i];
            }
            offset += batch_len;
        }
    }
}

/// Single-pass count (capped at `cap`) and whole-alphanumeric-token check on a
/// pre-lowercased copy `haystack_lower` aligned byte-for-byte with `haystack`.
pub fn count_and_word_lower(
    haystack: &str,
    haystack_lower: &str,
    needle: &str,
    cap: usize,
) -> (usize, bool) {
    if cap == 0 || needle.is_empty() {
        return (0, false);
    }
    let check_word = needle.is_ascii() && needle.bytes().all(|b| b.is_ascii_alphanumeric());
    let mut count = 0usize;
    let mut is_word = false;
    let mut pos = 0usize;
    while let Some(rel) = haystack_lower[pos..].find(needle) {
        let idx = pos + rel;
        let end = idx + needle.len();
        if count < cap {
            count += 1;
        }
        if check_word && !is_word && is_alnum_word_boundary(haystack, idx, end) {
            is_word = true;
        }
        if count >= cap && (!check_word || is_word) {
            break;
        }
        pos = end;
    }
    if !check_word && count > 0 {
        is_word = has_alnum_token_ascii_ci(haystack, needle);
    }
    (count, is_word)
}

/// Copy-free check whether `haystack` has an alphanumeric token equal (ASCII
/// case-insensitively) to `needle`, matching `split(|c: char| !c.is_alphanumeric())`.
pub fn has_alnum_token_ascii_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() || !needle.is_ascii() {
        return haystack
            .split(|c: char| !c.is_alphanumeric())
            .any(|part| part.eq_ignore_ascii_case(needle));
    }
    if !needle.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return false;
    }
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    let mut pos = 0usize;
    while let Some(idx) = find_ascii_ci_from(h, n, pos) {
        let end = idx + n.len();
        if is_alnum_word_boundary(haystack, idx, end) {
            return true;
        }
        pos = idx + haystack[idx..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// Per-unit match signals collected for one document without allocating.
#[derive(Clone, Copy, Debug, Default)]
pub struct BriefUnitHit {
    pub is_entity: bool,
    pub is_stop_word: bool,
    pub exact: bool,
    pub filename_seg: bool,
    pub filename_sub: bool,
    pub path_seg: bool,
    pub path_sub: bool,
    pub fact_word: bool,
    pub fact_sub: bool,
    pub parent_fact: bool,
    pub body_word: bool,
    pub body_hits: u8,
    /// Morphological whole-token match in the locator (any segment).
    pub stem_path: bool,
    /// Morphological whole-token match in the filename segment.
    pub stem_filename: bool,
    pub stem_body: bool,
}

impl BriefUnitHit {
    pub fn matched(self) -> bool {
        self.exact
            || self.filename_seg
            || self.filename_sub
            || self.path_seg
            || self.path_sub
            || self.fact_word
            || self.fact_sub
            || self.parent_fact
            || self.body_hits > 0
    }

    pub fn scored_match(self) -> bool {
        self.matched() || self.stem_path || self.stem_body
    }
}

/// Sparse per-unit match record retained only for query units that scored.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScoredUnitHit {
    pub unit_index: usize,
    pub hit: BriefUnitHit,
}

/// Deterministic integer IDF-like rarity weights from document frequency in the
/// same scan. Larger values mean rarer terms across the repository.
pub fn term_rarity_weights(total_docs: usize, doc_freqs: &[usize]) -> Vec<u64> {
    let numer = (total_docs as u64).saturating_add(1);
    doc_freqs
        .iter()
        .map(|&df| {
            let denom = (df as u64).saturating_add(1).min(numer);
            let ratio_q16 = (numer << 16) / denom;
            let k = u64::from(ratio_q16.ilog2().saturating_sub(16));
            let frac = ((ratio_q16 >> k) - 65_536) >> 10;
            8 + k * 96 + (frac * 3) / 2
        })
        .collect()
}

/// Brief relevance key, compared lexicographically (larger is better):
/// `(structural, rarity, detail, document traits)`. Each component is a full
/// `u64`, so no component saturates into the next at realistic query sizes.
pub type BriefScore = (u64, u64, u64, u64);

/// Brief score prioritizing term coverage and path/filename precedence, using
/// rarity only to order ties within equal coverage and structural match class,
/// followed by capped body repeats and document traits.
pub fn compute_brief_score(
    hits: &[ScoredUnitHit],
    rarity: &[u64],
    has_content_units: bool,
    active: u8,
    canonical: u8,
    public_safe: u8,
    confidence: u8,
) -> BriefScore {
    let mut structural_base = 0u64;
    let mut rarity_sum = 0u64;
    let mut detail_bonus = 0u64;
    for scored in hits {
        let hit = scored.hit;
        if !hit.scored_match() || (has_content_units && hit.is_stop_word) {
            continue;
        }
        let weight = rarity.get(scored.unit_index).copied().unwrap_or(0);
        let surface = hit.matched();
        let full_cov = if hit.is_entity {
            WEIGHT_ENTITY_COVERAGE
        } else {
            WEIGHT_COVERAGE
        };
        // Stem-only coverage sits below surface coverage so exact word
        // matches win when coverage is otherwise tied.
        let (cov, unit_rarity) = if surface {
            (full_cov, weight)
        } else {
            ((full_cov * 3) / 4, (weight * 3) / 4)
        };
        let exact_bonus = u64::from(hit.exact) * WEIGHT_EXACT;
        // The strongest applicable location signal counts, never a weaker
        // coincident one.
        let path_bonus = [
            (hit.filename_seg, WEIGHT_FILENAME_SEG),
            (hit.filename_sub, WEIGHT_FILENAME_SUB),
            (hit.path_seg, WEIGHT_PATH_SEG),
            (hit.path_sub, WEIGHT_PATH_SUB),
            (hit.stem_filename, WEIGHT_FILENAME_STEM),
            (hit.stem_path, WEIGHT_PATH_STEM),
        ]
        .into_iter()
        .filter_map(|(on, w)| on.then_some(w))
        .max()
        .unwrap_or(0);
        let fact_bonus = if hit.fact_word {
            WEIGHT_FACT_WORD
        } else if hit.fact_sub {
            WEIGHT_FACT_SUB
        } else {
            0
        };
        structural_base = structural_base.saturating_add(cov + exact_bonus + path_bonus);
        rarity_sum = rarity_sum.saturating_add(unit_rarity);
        let parent_bonus = u64::from(hit.parent_fact) * WEIGHT_PARENT_FACT;
        let capped_body = u64::from(hit.body_hits.min(BODY_HIT_CAP as u8));
        let body_bonus = if hit.body_hits == 0 && hit.stem_body {
            WEIGHT_BODY_HIT
        } else {
            u64::from(hit.body_word) * WEIGHT_BODY_WORD + capped_body * WEIGHT_BODY_HIT
        };
        detail_bonus = detail_bonus.saturating_add(fact_bonus + parent_bonus + body_bonus);
    }
    let tie = u64::from(active) * 100
        + u64::from(canonical) * 50
        + u64::from(public_safe) * 20
        + u64::from(confidence);
    (structural_base, rarity_sum, detail_bonus, tie)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rarity_orders_only_within_equal_coverage_and_path_class() {
        let rarity = [20, 900];
        let common_filename = compute_brief_score(
            &[ScoredUnitHit {
                unit_index: 0,
                hit: BriefUnitHit {
                    filename_seg: true,
                    ..Default::default()
                },
            }],
            &rarity,
            true,
            3,
            1,
            1,
            100,
        );
        let rare_body = compute_brief_score(
            &[ScoredUnitHit {
                unit_index: 1,
                hit: BriefUnitHit {
                    body_hits: 3,
                    body_word: true,
                    ..Default::default()
                },
            }],
            &rarity,
            true,
            3,
            1,
            1,
            100,
        );
        let common_body = compute_brief_score(
            &[ScoredUnitHit {
                unit_index: 0,
                hit: BriefUnitHit {
                    body_hits: 3,
                    body_word: true,
                    ..Default::default()
                },
            }],
            &rarity,
            true,
            3,
            1,
            1,
            100,
        );
        assert!(common_filename > rare_body);
        assert!(rare_body > common_body);
    }

    #[test]
    fn sparse_hits_respect_stop_word_flag_when_content_units_exist() {
        let rarity = [100, 100];
        let stop_only_hit = [ScoredUnitHit {
            unit_index: 0,
            hit: BriefUnitHit {
                is_stop_word: true,
                body_hits: 2,
                body_word: true,
                ..Default::default()
            },
        }];
        let with_content = compute_brief_score(&stop_only_hit, &rarity, true, 0, 0, 0, 0);
        let only_stops = compute_brief_score(&stop_only_hit, &rarity, false, 0, 0, 0, 0);
        assert_eq!(with_content, (0, 0, 0, 0));
        assert!(only_stops > (0, 0, 0, 0));
    }

    #[test]
    fn parser_handles_non_ascii_and_apostrophes_inside_plain_scalars() {
        let cfg = RankingConfig::parse_from_str(
            "classes:\n  - name: caf\u{e9}'s-d\u{f6}cs \u{2013} team's\n    tier: 1\n    globs: [\"**/*.txt\", d\u{e9}j\u{e0}'s/**]\n",
        )
        .unwrap();
        assert_eq!(
            cfg.classify("d\u{e9}j\u{e0}'s/x.md"),
            (1, Some("caf\u{e9}'s-d\u{f6}cs \u{2013} team's"))
        );
    }

    #[test]
    fn glob_classification_preserves_first_match_and_deep_paths() {
        let cfg = RankingConfig::parse_from_str(
            "classes:\n  - name: first\n    tier: 3\n    globs:\n      - \"a/**/**/c/*.md\"\n  - name: fallback\n    tier: 1\n    globs:\n      - \"**/*.md\"\n",
        )
        .unwrap();
        assert_eq!(cfg.classify("a/b/c/file.md"), (3, Some("first")));
        assert_eq!(cfg.classify("a/c/file.md"), (3, Some("first")));
        let deep = format!("{}/file.md", vec!["seg"; 40].join("/"));
        assert_eq!(cfg.classify(&deep), (1, Some("fallback")));
        assert_eq!(cfg.classify("a/b/c/file.txt"), (0, None));
    }

    #[test]
    fn body_multi_matcher_matches_single_pattern_counts_and_word_boundaries() {
        let body =
            "Check checking CHECK. Route-map and routed; frozen, guard, fallback! CafÉ café.";
        let lower = body.to_ascii_lowercase();
        let mut specs: Vec<(String, bool)> = vec![
            ("check".to_owned(), true),
            ("route".to_owned(), true),
            ("frozen".to_owned(), true),
            ("guard".to_owned(), true),
            ("fallback".to_owned(), true),
            ("café".to_owned(), true),
            ("route-map".to_owned(), false),
        ];
        for i in 0..30 {
            specs.push((format!("extra{i}"), true));
        }
        let spec_refs: Vec<(&str, bool)> = specs.iter().map(|(s, w)| (s.as_str(), *w)).collect();
        let matcher = BodyMultiMatcher::new(&spec_refs);
        let mut counts = vec![0u8; specs.len()];
        let mut words = vec![false; specs.len()];
        matcher.match_body(body, &lower, BODY_HIT_CAP, &mut counts, &mut words);

        for (idx, (pat, check_word)) in spec_refs.iter().enumerate() {
            let (expected_count, expected_word) =
                count_and_word_lower(body, &lower, pat, BODY_HIT_CAP);
            assert_eq!(counts[idx] as usize, expected_count, "count for {pat}");
            if *check_word {
                assert_eq!(words[idx], expected_word, "word for {pat}");
            } else {
                assert_eq!(words[idx], expected_count > 0, "entity for {pat}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_dangling_and_directory_symlinks() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "bran-ranking-symlink-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".bran")).unwrap();
        std::os::unix::fs::symlink("/nonexistent/ranking.yaml", root.join(".bran/ranking.yaml"))
            .unwrap();
        let err = RankingConfig::load(&root).unwrap_err();
        assert!(err.contains(RANKING_FILENAME), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
