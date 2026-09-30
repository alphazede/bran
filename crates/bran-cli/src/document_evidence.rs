//! Enterprise-document evidence for query and packet (issue #46).
//!
//! Discovery is extension-based under the requested root and mirrors the
//! repository scanner's walk rules (sorted entries, ignore files, no
//! symlinks, depth cap, root containment). Admitted anchors are reported in
//! an appended `document_evidence` member only: no document path ranks as a
//! Markdown source, and roots without document files produce byte-identical
//! output. Everything here is read-only and offline.

use bran_core::scan::{IgnoreMatcher, MAX_BRANIGNORE_BYTES};
use bran_document::admit::{self, Admitted, Derivation, EvidenceAnchor};
use bran_document::canonical::sha256_hex;
use bran_document::{Format, Limits};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::{json_escape, query_terms_and_entities};

/// Most document files admitted per root; the rest count as omitted.
const MAX_DOCUMENT_FILES: usize = 64;
/// Most anchor matches reported; the rest count as omitted.
const MAX_MATCHES: usize = 32;
/// Mirrors the scanner's depth cap.
const MAX_DEPTH: usize = 64;

/// The enterprise formats by filename extension (ASCII case-insensitive).
pub fn format_for_path(path: &str) -> Option<Format> {
    let extension = path.rsplit('.').next().unwrap_or_default();
    if !path.contains('.') || extension.is_empty() || extension.contains('/') {
        return None;
    }
    Format::ALL
        .into_iter()
        .find(|format| format.extension().eq_ignore_ascii_case(extension))
}

/// True for paths routed to document evidence instead of Markdown ranking.
pub fn is_document_path(locator: &str) -> bool {
    format_for_path(locator).is_some()
}

fn read_ignore_file(root: &Path, name: &str) -> Vec<u8> {
    let path = root.join(name);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(_) => return Vec::new(),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_BRANIGNORE_BYTES as u64
    {
        return Vec::new();
    }
    fs::read(&path).unwrap_or_default()
}

fn matcher_for(root: &Path) -> IgnoreMatcher {
    // The scanner already gated the query on these files; a concurrent change
    // falls back to no rules rather than failing the additive member.
    let native = read_ignore_file(root, ".branignore");
    let legacy = read_ignore_file(root, ".okfignore");
    let mut matcher = IgnoreMatcher::from_branignore(&native)
        .or_else(|_| IgnoreMatcher::from_okfignore(&legacy))
        .unwrap_or_default();
    matcher.add_gitignore(&read_ignore_file(root, ".gitignore"));
    matcher
}

fn join_relative(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    }
}

/// Sorted regular files with an enterprise extension, honouring ignores.
/// Returns the files (relative path plus format) and the over-cap remainder.
pub fn discover(root: &Path) -> (Vec<(String, Format)>, usize) {
    let canonical_root = match fs::canonicalize(root) {
        Ok(path) => path,
        Err(_) => return (Vec::new(), 0),
    };
    let matcher = matcher_for(&canonical_root);
    let mut found = Vec::new();
    let mut stack = vec![(canonical_root.clone(), String::new(), 0usize)];
    while let Some((directory, prefix, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        let current = match fs::symlink_metadata(&directory) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if current.file_type().is_symlink() {
            continue;
        }
        let canonical = match fs::canonicalize(&directory) {
            Ok(path) if path.starts_with(&canonical_root) => path,
            _ => continue,
        };
        let mut entries: Vec<PathBuf> = match fs::read_dir(&canonical) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .collect(),
            Err(_) => continue,
        };
        entries.sort();
        for path in entries {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let relative = join_relative(&prefix, name);
            if matcher.is_ignored_relative(&relative) {
                continue;
            }
            let file_type = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata.file_type(),
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            } else if file_type.is_dir() {
                stack.push((path, relative, depth + 1));
            } else if file_type.is_file() {
                if let Some(format) = format_for_path(&relative) {
                    found.push((relative, format));
                }
            }
        }
    }
    found.sort();
    let omitted = found.len().saturating_sub(MAX_DOCUMENT_FILES);
    found.truncate(MAX_DOCUMENT_FILES);
    (found, omitted)
}

fn read_document(path: &Path) -> Result<Vec<u8>, &'static str> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return Err("unreadable"),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("unreadable");
    }
    if metadata.len() > Limits::default().max_package_bytes {
        return Err("oversized");
    }
    fs::read(path).map_err(|_| "unreadable")
}

fn contained(root: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty() || relative.len() > 1024 {
        return None;
    }
    let candidate = root.join(relative);
    match fs::canonicalize(&candidate) {
        Ok(path) if path.starts_with(root) => Some(path),
        _ => None,
    }
}

/// Reads one document for inspection. `Ok` is the envelope JSON; `Err` is a
/// typed code that never reflects input bytes.
pub fn inspect(root: &Path, relative: &str) -> Result<String, &'static str> {
    let canonical_root = fs::canonicalize(root).map_err(|_| "unreadable")?;
    let Some(format) = format_for_path(relative) else {
        return Err("unsupported-format");
    };
    if admit::adapter(format).is_none() {
        return Err("unsupported-format");
    }
    let path = contained(&canonical_root, relative).ok_or("unreadable")?;
    let bytes = read_document(&path)?;
    admit::admit(&bytes, format, relative)
        .map(|admitted| String::from_utf8_lossy(&admitted.envelope.to_bytes()).into_owned())
        .map_err(|refusal| refusal.code())
}

fn column_letters(mut column: i64) -> String {
    let mut letters = Vec::new();
    while column > 0 {
        column -= 1;
        letters.push((b'A' + (column % 26) as u8) as char);
        column /= 26;
    }
    letters.iter().rev().collect()
}

/// The adapter-native locator a reader cites: `Sheet1!A1` for grids,
/// `page 2 block 3` for fixed layout, and the anchor id when the locator is
/// not one this path renders.
pub fn native_locator(anchor: &EvidenceAnchor) -> String {
    use bran_document::canonical::Json;
    let field = |key: &str| anchor.locator.get(key);
    let text = |key: &str| match field(key) {
        Some(Json::Str(value)) => Some(value.as_str()),
        _ => None,
    };
    let number = |key: &str| match field(key) {
        Some(Json::Int(value)) => Some(*value),
        _ => None,
    };
    match text("family") {
        Some("grid") => match (text("sheet"), number("row"), number("column")) {
            (Some(sheet), Some(row), Some(column))
                if row >= 1 && column >= 1 && !sheet.is_empty() =>
            {
                format!("{sheet}!{}{row}", column_letters(column))
            }
            _ => anchor.id.clone(),
        },
        Some("fixed-layout") => match (number("page"), number("block")) {
            (Some(page), Some(block)) if page >= 1 && block >= 1 => {
                format!("page {page} block {block}")
            }
            _ => anchor.id.clone(),
        },
        Some("flow") => match (text("section"), number("ordinal")) {
            (Some(section), Some(ordinal)) if ordinal >= 1 && !section.is_empty() => {
                format!("{section}#{ordinal}")
            }
            _ => anchor.id.clone(),
        },
        Some("presentation") => match (number("slide"), number("shape")) {
            (Some(slide), Some(shape)) if slide >= 1 && shape >= 1 => {
                format!("slide {slide} shape {shape}")
            }
            _ => anchor.id.clone(),
        },
        _ => anchor.id.clone(),
    }
}

struct Candidate<'a> {
    bundle: &'a str,
    path: &'a str,
    digest: &'a str,
    anchor: &'a EvidenceAnchor,
    score: usize,
}

/// Deterministic match order over admitted anchors: embedded anchors rank by
/// score, then bundle, path, and anchor id; OCR-derived anchors sort after
/// with no rank, so derived text is labelled and never byte-ranked.
fn rank_matches(candidates: &[Candidate<'_>]) -> (Vec<(usize, Option<usize>)>, usize) {
    let mut embedded: Vec<(usize, usize)> = Vec::new();
    let mut derived: Vec<usize> = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if candidate.score == 0 {
            continue;
        }
        if candidate.anchor.derivation == Derivation::Ocr {
            derived.push(index);
        } else {
            embedded.push((index, candidate.score));
        }
    }
    embedded.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| candidates[left.0].bundle.cmp(candidates[right.0].bundle))
            .then_with(|| candidates[left.0].path.cmp(candidates[right.0].path))
            .then_with(|| {
                candidates[left.0]
                    .anchor
                    .id
                    .cmp(&candidates[right.0].anchor.id)
            })
    });
    derived.sort_by(|left, right| {
        candidates[*left]
            .bundle
            .cmp(candidates[*right].bundle)
            .then_with(|| candidates[*left].path.cmp(candidates[*right].path))
            .then_with(|| {
                candidates[*left]
                    .anchor
                    .id
                    .cmp(&candidates[*right].anchor.id)
            })
    });
    let mut ordered: Vec<(usize, Option<usize>)> = embedded
        .into_iter()
        .enumerate()
        .map(|(position, (index, _))| (index, Some(position + 1)))
        .collect();
    ordered.extend(derived.into_iter().map(|index| (index, None)));
    let omitted = ordered.len().saturating_sub(MAX_MATCHES);
    ordered.truncate(MAX_MATCHES);
    (ordered, omitted)
}

fn term_score(text: &str, terms: &BTreeSet<String>) -> usize {
    let lowered = text.to_ascii_lowercase();
    terms
        .iter()
        .filter(|term| lowered.contains(term.as_str()))
        .count()
}

struct Source<'a> {
    bundle: &'a str,
    path: String,
    format: Format,
    digest: String,
    admitted: Admitted,
}

/// The appended `document_evidence` member for `bundles` (one root, or every
/// root of a multi-root query). `None` when no document file was discovered,
/// which keeps output byte-identical for repositories without evidence.
pub fn document_evidence_json(
    bundles: &[(&str, &Path)],
    query_text: &str,
    excerpts: bool,
) -> Option<String> {
    let (terms, entities) = query_terms_and_entities(query_text);
    let mut query_terms = terms;
    query_terms.extend(entities);
    let multi = bundles.len() > 1;
    let mut sources: Vec<Source<'_>> = Vec::new();
    let mut refusals: Vec<(String, String, String)> = Vec::new();
    let mut unsupported: Vec<(String, String, String)> = Vec::new();
    let mut omitted_files = 0usize;
    for (bundle, root) in bundles {
        let canonical_root = fs::canonicalize(root).ok()?;
        let (files, omitted) = discover(&canonical_root);
        omitted_files += omitted;
        for (relative, format) in &files {
            if admit::adapter(*format).is_none() {
                unsupported.push((
                    (*bundle).to_owned(),
                    relative.clone(),
                    format.extension().to_owned(),
                ));
                continue;
            }
            let outcome = contained(&canonical_root, relative)
                .ok_or("unreadable")
                .and_then(|path| read_document(&path))
                .map_err(|code| code.to_owned())
                .and_then(|bytes| {
                    admit::admit(&bytes, *format, relative)
                        .map(|admitted| (sha256_hex(&bytes), admitted))
                        .map_err(|refusal| refusal.code().to_owned())
                });
            match outcome {
                Ok((digest, admitted)) => sources.push(Source {
                    bundle,
                    path: relative.clone(),
                    format: *format,
                    digest,
                    admitted,
                }),
                Err(code) => refusals.push(((*bundle).to_owned(), relative.clone(), code)),
            }
        }
    }
    sources.sort_by(|left, right| {
        left.bundle
            .cmp(right.bundle)
            .then_with(|| left.path.cmp(&right.path))
    });
    refusals.sort();
    unsupported.sort();
    if sources.is_empty() && refusals.is_empty() && unsupported.is_empty() {
        return None;
    }
    let mut candidates = Vec::new();
    for source in &sources {
        for anchor in &source.admitted.anchors {
            candidates.push(Candidate {
                bundle: source.bundle,
                path: &source.path,
                digest: &source.digest,
                anchor,
                score: term_score(&anchor.text, &query_terms),
            });
        }
    }
    let (ordered, omitted_matches) = rank_matches(&candidates);
    let mut sources_json = Vec::new();
    for source in &sources {
        let fidelity = String::from_utf8_lossy(&source.admitted.fidelity.to_bytes()).into_owned();
        sources_json.push(format!(
            "{{\"path\":\"{}\",\"source_type\":\"{}\",\"source_digest\":\"{}\",\"fidelity\":{},\"anchor_count\":{}}}",
            json_escape(&source.path),
            source.format.extension(),
            source.digest,
            fidelity,
            source.admitted.anchors.len()
        ));
    }
    let mut matches_json = Vec::new();
    for (index, rank) in &ordered {
        let candidate = &candidates[*index];
        let source = sources
            .iter()
            .find(|source| source.bundle == candidate.bundle && source.path == candidate.path)
            .expect("candidate source");
        let format = source.format;
        let fidelity = String::from_utf8_lossy(&source.admitted.fidelity.to_bytes()).into_owned();
        let bundle = if multi {
            format!("\"bundle\":\"{}\",", json_escape(candidate.bundle))
        } else {
            String::new()
        };
        let excerpt = if excerpts {
            format!(",\"excerpt\":\"{}\"", json_escape(&candidate.anchor.text))
        } else {
            String::new()
        };
        matches_json.push(format!(
            "{{{bundle}\"path\":\"{}\",\"source_type\":\"{}\",\"anchor\":\"{}\",\"native_locator\":\"{}\",\"source_digest\":\"{}\",\"fidelity\":{},\"derivation\":\"{}\",\"score\":{},\"rank\":{}{excerpt}}}",
            json_escape(candidate.path),
            format.extension(),
            json_escape(&candidate.anchor.id),
            json_escape(&native_locator(candidate.anchor)),
            candidate.digest,
            fidelity,
            candidate.anchor.derivation.as_str(),
            candidate.score,
            rank.map_or_else(|| "null".to_owned(), |rank| rank.to_string()),
        ));
    }
    let mut refusals_json = Vec::new();
    for (bundle, path, code) in &refusals {
        let bundle = if multi {
            format!("\"bundle\":\"{}\",", json_escape(bundle))
        } else {
            String::new()
        };
        refusals_json.push(format!(
            "{{{bundle}\"path\":\"{}\",\"code\":\"{}\"}}",
            json_escape(path),
            json_escape(code)
        ));
    }
    let mut unsupported_json = Vec::new();
    for (bundle, path, source_type) in &unsupported {
        let bundle = if multi {
            format!("\"bundle\":\"{}\",", json_escape(bundle))
        } else {
            String::new()
        };
        unsupported_json.push(format!(
            "{{{bundle}\"path\":\"{}\",\"source_type\":\"{}\"}}",
            json_escape(path),
            json_escape(source_type)
        ));
    }
    Some(format!(
        "{{\"sources\":[{}],\"matches\":[{}],\"refusals\":[{}],\"unsupported\":[{}],\"omitted_files\":{},\"omitted_matches\":{}}}",
        sources_json.join(","),
        matches_json.join(","),
        refusals_json.join(","),
        unsupported_json.join(","),
        omitted_files,
        omitted_matches
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bran_document::canonical::Json;
    use std::collections::BTreeMap;

    fn anchor(id: &str, text: &str, derivation: Derivation) -> EvidenceAnchor {
        EvidenceAnchor {
            id: id.to_owned(),
            role: "cell".to_owned(),
            locator: Json::Obj(BTreeMap::from([
                ("column".to_owned(), Json::Int(1)),
                ("family".to_owned(), Json::Str("grid".to_owned())),
                ("row".to_owned(), Json::Int(1)),
                ("sheet".to_owned(), Json::Str("Sheet1".to_owned())),
            ])),
            text: text.to_owned(),
            text_digest: sha256_hex(text.as_bytes()),
            derivation,
        }
    }

    #[test]
    fn derived_anchors_sort_last_without_rank() {
        let ocr = anchor("anc:ocr", "ledger total", Derivation::Ocr);
        let first = anchor("anc:b", "ledger total", Derivation::Embedded);
        let second = anchor("anc:a", "ledger", Derivation::Embedded);
        let digest = sha256_hex(b"ledger.xlsx");
        let candidates = vec![
            Candidate {
                bundle: ".",
                path: "scan.pdf",
                digest: &digest,
                anchor: &ocr,
                score: 2,
            },
            Candidate {
                bundle: ".",
                path: "ledger.xlsx",
                digest: &digest,
                anchor: &first,
                score: 2,
            },
            Candidate {
                bundle: ".",
                path: "ledger.xlsx",
                digest: &digest,
                anchor: &second,
                score: 1,
            },
        ];
        let (ordered, omitted) = rank_matches(&candidates);
        assert_eq!(omitted, 0);
        assert_eq!(ordered, vec![(1, Some(1)), (2, Some(2)), (0, None)]);
    }

    #[test]
    fn discovery_honours_ignores_and_sorts() {
        let root = std::env::temp_dir().join(format!("bran-doc-discovery-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join(".branignore"), "ignored/\n").unwrap();
        fs::create_dir_all(root.join("ignored")).unwrap();
        fs::write(root.join("ignored/hidden.xlsx"), b"x").unwrap();
        fs::write(root.join("sub/b.XLSX"), b"x").unwrap();
        fs::write(root.join("a.pdf"), b"x").unwrap();
        fs::write(root.join("notes.md"), b"x").unwrap();
        let (files, omitted) = discover(&root);
        assert_eq!(omitted, 0);
        assert_eq!(
            files
                .iter()
                .map(|(path, format)| (path.as_str(), format.extension()))
                .collect::<Vec<_>>(),
            vec![("a.pdf", "pdf"), ("sub/b.XLSX", "xlsx")]
        );
        assert!(is_document_path("sub/b.XLSX"));
        assert!(!is_document_path("notes.md"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn native_locators_render_per_family() {
        let base = anchor("anc:xlsx:s1:r1c28", "v", Derivation::Embedded);
        let mut locator = base.locator.clone();
        if let Json::Obj(map) = &mut locator {
            map.insert("sheet".to_owned(), Json::Str("Metrics".to_owned()));
            map.insert("column".to_owned(), Json::Int(28));
        }
        let grid = EvidenceAnchor {
            locator,
            ..base.clone()
        };
        assert_eq!(native_locator(&grid), "Metrics!AB1");
        let fixed = EvidenceAnchor {
            id: "pdf:p2:b3".to_owned(),
            locator: Json::Obj(BTreeMap::from([
                ("family".to_owned(), Json::Str("fixed-layout".to_owned())),
                ("page".to_owned(), Json::Int(2)),
                ("block".to_owned(), Json::Int(3)),
            ])),
            ..base.clone()
        };
        assert_eq!(native_locator(&fixed), "page 2 block 3");
        let broken = EvidenceAnchor {
            id: "anc:fallback".to_owned(),
            locator: Json::Null,
            ..base
        };
        assert_eq!(native_locator(&broken), "anc:fallback");
    }
}
