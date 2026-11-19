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
/// `repositories` -> `repositor`). Returns `None` when no safe stem exists.
pub fn ascii_stem(word: &str) -> Option<String> {
    if word.len() < 5 || !word.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let stem = if let Some(base) = word.strip_suffix("ies") {
        base
    } else if word.ends_with("es")
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
    } else if let Some(base) = word.strip_suffix('y') {
        if base.len() >= 5
            && !matches!(
                base.as_bytes().last(),
                Some(b'a' | b'e' | b'i' | b'o' | b'u')
            )
        {
            base
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
        for seg in raw.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." {
                return Err(format!("invalid path segment in glob pattern: {raw:?}"));
            }
            if seg == "**" {
                segments.push(SegmentPattern::DoubleStar);
            } else if seg.contains("**") {
                return Err(format!(
                    "'**' must appear as a whole path segment in glob pattern: {raw:?}"
                ));
            } else if seg.contains('*') {
                segments.push(SegmentPattern::Wildcard(seg.to_owned()));
            } else {
                segments.push(SegmentPattern::Literal(seg.to_owned()));
            }
        }
        Ok(Self { segments })
    }

    fn matches(&self, rel_path: &str) -> bool {
        let path_segs: Vec<&str> = rel_path.split('/').collect();
        let pat = &self.segments;
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
                if b == b'"' || b == b'\'' {
                    quote = Some(b);
                } else if b == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
                    return Ok(&line[..i]);
                }
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
                if b == b'"' || b == b'\'' {
                    quote = Some(b);
                } else if b == b',' {
                    let piece = inner[start..i].trim();
                    if piece.is_empty() {
                        return Err(format!("line {line_no}: empty inline list item"));
                    }
                    items.push(parse_yaml_scalar(piece, line_no)?);
                    start = i + 1;
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

impl RankingConfig {
    /// Load `.bran/ranking.yaml` from `root`. A missing file yields the default
    /// empty configuration (all paths tier 0). Any I/O, safety, or syntax error
    /// returns an error naming `.bran/ranking.yaml`.
    pub fn load(root: &Path) -> Result<Self, String> {
        let ranking_path = root.join(RANKING_FILENAME);
        match fs::symlink_metadata(&ranking_path) {
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(format!("{RANKING_FILENAME}: unreadable ({err})")),
        }
        let canonical_root = fs::canonicalize(root)
            .map_err(|err| format!("{RANKING_FILENAME}: invalid repository root ({err})"))?;
        let canonical_file = fs::canonicalize(&ranking_path)
            .map_err(|err| format!("{RANKING_FILENAME}: unreadable ({err})"))?;
        if !canonical_file.starts_with(&canonical_root) {
            return Err(format!("{RANKING_FILENAME}: path escapes repository root"));
        }
        let meta = fs::symlink_metadata(&canonical_file)
            .map_err(|err| format!("{RANKING_FILENAME}: unreadable ({err})"))?;
        if !meta.file_type().is_file() {
            return Err(format!("{RANKING_FILENAME}: expected a regular file"));
        }
        if meta.len() > MAX_RANKING_BYTES as u64 {
            return Err(format!(
                "{RANKING_FILENAME}: file exceeds {MAX_RANKING_BYTES} bytes"
            ));
        }
        let file = File::open(&canonical_file)
            .map_err(|err| format!("{RANKING_FILENAME}: unreadable ({err})"))?;
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
                let after_dash = trimmed.strip_prefix('-').unwrap().trim();
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
        for class in &self.classes {
            if class.globs.iter().any(|glob| glob.matches(rel_path)) {
                return (class.tier, Some(class.name.as_str()));
            }
        }
        (0, None)
    }
}

/// Copy-free ASCII case-insensitive substring check. Equivalent to
/// `haystack.to_ascii_lowercase().contains(needle)` when `needle` is already
/// ASCII-lowercase.
pub fn contains_ascii_ci(haystack: &str, needle: &str) -> bool {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() {
        return true;
    }
    if n.len() > h.len() {
        return false;
    }
    let first = n[0];
    let first_upper = first.to_ascii_uppercase();
    let limit = h.len() - n.len();
    if first == first_upper {
        for i in 0..=limit {
            if h[i] == first && h[i..i + n.len()].eq_ignore_ascii_case(n) {
                return true;
            }
        }
    } else {
        for i in 0..=limit {
            let b = h[i];
            if (b == first || b == first_upper) && h[i..i + n.len()].eq_ignore_ascii_case(n) {
                return true;
            }
        }
    }
    false
}

/// Copy-free count of non-overlapping ASCII case-insensitive occurrences of
/// `needle` in `haystack`, stopping as soon as `cap` matches are found.
pub fn count_ascii_ci_capped(haystack: &str, needle: &str, cap: usize) -> usize {
    if cap == 0 {
        return 0;
    }
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || n.len() > h.len() {
        return 0;
    }
    let first = n[0];
    let first_upper = first.to_ascii_uppercase();
    let limit = h.len() - n.len();
    let mut count = 0usize;
    let mut i = 0usize;
    while i <= limit {
        let b = h[i];
        if (b == first || b == first_upper) && h[i..i + n.len()].eq_ignore_ascii_case(n) {
            count += 1;
            if count >= cap {
                return count;
            }
            i += n.len();
        } else {
            i += 1;
        }
    }
    count
}

/// Copy-free check whether `haystack` has an alphanumeric token equal (ASCII
/// case-insensitively) to `needle`, matching `split(|c: char| !c.is_alphanumeric())`.
pub fn has_alnum_token_ascii_ci(haystack: &str, needle: &str) -> bool {
    haystack
        .split(|c: char| !c.is_alphanumeric())
        .any(|part| part.eq_ignore_ascii_case(needle))
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
    pub stem_path: bool,
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

/// Single integer brief score combining rarity-scaled term coverage, path and
/// filename precedence over body repeats, capped body hits, and tie-breakers.
pub fn compute_brief_score(
    hits: &[BriefUnitHit],
    rarity: &[u64],
    active: u8,
    canonical: u8,
    public_safe: u8,
    confidence: u8,
) -> u64 {
    let has_content_units = hits.iter().any(|h| !h.is_stop_word);
    let mut unit_sum = 0u64;
    for (hit, &weight) in hits.iter().zip(rarity.iter()) {
        if !hit.scored_match() || (has_content_units && hit.is_stop_word) {
            continue;
        }
        if !hit.matched() {
            // Morphological stem fallback: slightly below surface-form coverage
            // so exact word matches always win when coverage is otherwise tied.
            let stem_cov = (WEIGHT_COVERAGE * 3) / 4;
            let stem_loc = if hit.stem_path {
                WEIGHT_FILENAME_SUB
            } else {
                WEIGHT_BODY_HIT
            };
            unit_sum = unit_sum.saturating_add(weight.saturating_mul(stem_cov + stem_loc));
            continue;
        }
        let cov = if hit.is_entity {
            WEIGHT_ENTITY_COVERAGE
        } else {
            WEIGHT_COVERAGE
        };
        let exact_bonus = u64::from(hit.exact) * WEIGHT_EXACT;
        let path_bonus = if hit.filename_seg {
            WEIGHT_FILENAME_SEG
        } else if hit.filename_sub {
            WEIGHT_FILENAME_SUB
        } else if hit.path_seg {
            WEIGHT_PATH_SEG
        } else if hit.path_sub {
            WEIGHT_PATH_SUB
        } else {
            0
        };
        let fact_bonus = if hit.fact_word {
            WEIGHT_FACT_WORD
        } else if hit.fact_sub {
            WEIGHT_FACT_SUB
        } else {
            0
        };
        let parent_bonus = u64::from(hit.parent_fact) * WEIGHT_PARENT_FACT;
        let capped_body = u64::from(hit.body_hits.min(BODY_HIT_CAP as u8));
        let body_bonus =
            u64::from(hit.body_word) * WEIGHT_BODY_WORD + capped_body * WEIGHT_BODY_HIT;
        unit_sum = unit_sum.saturating_add(weight.saturating_mul(
            cov + exact_bonus + path_bonus + fact_bonus + parent_bonus + body_bonus,
        ));
    }
    let tie = u64::from(active) * 100
        + u64::from(canonical) * 50
        + u64::from(public_safe) * 20
        + u64::from(confidence);
    unit_sum.saturating_mul(1_000).saturating_add(tie)
}
