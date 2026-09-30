//! Shared OPC package intake for DOCX, XLSX, and PPTX.

use crate::canonical::{sha256_hex, Json};
use crate::xml::{self, Event};
use crate::{zip, Cancel, Limits, Refusal, RECEIPT_VERSION};
use bran_core::export::{validate_emitted_string, ExportError};
use std::collections::{BTreeMap, BTreeSet};

const CONTENT_TYPES: &str = "[content_types].xml";
const SIGNATURE: &str = "application/vnd.openxmlformats-package.digital-signature-xmlsignature+xml";
/// Lower-case content-type fragments of executable or macro-bearing parts.
const ACTIVE: [&str; 6] = [
    "vbaproject",
    "vbadata",
    "activex",
    "oleobject",
    "macrosheet",
    "macroenabled",
];
/// Lower-case content-type fragments of parts that reach outside the package.
const EXTERNAL: [&str; 1] = ["spreadsheetml.connections"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Part {
    pub name: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Relationship {
    pub source: String,
    pub id: String,
    pub kind: String,
    pub target: String,
    pub external: bool,
}

/// A validated package. Parts and relationships are sorted, so the canonical
/// bytes do not depend on ZIP order, timestamps, or compression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Package {
    pub parts: Vec<Part>,
    pub relationships: Vec<Relationship>,
    pub diagnostics: BTreeSet<&'static str>,
}

impl Package {
    /// Canonical inventory: content types and digests per part, resolved
    /// relationships, and receipt codes. The content-types stream itself is
    /// excluded because its ordering is a producer detail.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let text = |value: &str| Json::Str(value.to_owned());
        let parts = self.parts.iter().map(|part| {
            Json::Obj(BTreeMap::from([
                ("name".to_owned(), text(&part.name)),
                ("content_type".to_owned(), text(&part.content_type)),
                ("sha256".to_owned(), text(&sha256_hex(&part.data))),
                ("size".to_owned(), Json::Int(part.data.len() as i64)),
            ]))
        });
        let relationships = self.relationships.iter().map(|relationship| {
            Json::Obj(BTreeMap::from([
                ("source".to_owned(), text(&relationship.source)),
                ("id".to_owned(), text(&relationship.id)),
                ("type".to_owned(), text(&relationship.kind)),
                ("target".to_owned(), text(&relationship.target)),
                ("external".to_owned(), Json::Bool(relationship.external)),
            ]))
        });
        Json::Obj(BTreeMap::from([
            ("schema_version".to_owned(), text(RECEIPT_VERSION)),
            ("parts".to_owned(), Json::Arr(parts.collect())),
            (
                "relationships".to_owned(),
                Json::Arr(relationships.collect()),
            ),
            (
                "diagnostics".to_owned(),
                Json::Arr(self.diagnostics.iter().map(|code| text(code)).collect()),
            ),
        ]))
        .to_bytes()
    }
}

/// Opens an untrusted OPC package. Nothing is fetched or executed; every
/// refusal is typed and nothing partial is returned.
pub fn open(bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Package, Refusal> {
    let mut lowered = BTreeSet::new();
    let mut raw = BTreeMap::new();
    for entry in zip::read(bytes, limits, cancel)? {
        // A ZIP directory entry is a name ending in '/' with no data.
        let (name, directory) = match entry.name.strip_suffix('/') {
            Some(directory) if entry.data.is_empty() => (directory.to_owned(), true),
            _ => (entry.name, false),
        };
        validate_part_name(&name)?;
        if !lowered.insert(name.to_ascii_lowercase()) {
            return Err(Refusal::DuplicatePart);
        }
        if !directory {
            raw.insert(name, entry.data);
        }
    }
    let types_name = raw
        .keys()
        .find(|name| name.to_ascii_lowercase() == CONTENT_TYPES)
        .cloned()
        .ok_or(Refusal::MalformedContainer)?;
    let types = raw.remove(&types_name).unwrap_or_default();
    let (defaults, overrides) = content_types(&types, limits, cancel)?;

    let mut parts = Vec::with_capacity(raw.len());
    for (name, data) in raw {
        let lower = name.to_ascii_lowercase();
        let extension = lower.rsplit_once('.').map(|(_, extension)| extension);
        let content_type = overrides
            .get(&format!("/{lower}"))
            .or_else(|| extension.and_then(|extension| defaults.get(extension)))
            .ok_or(Refusal::MalformedContainer)?;
        let content_type_lower = content_type.to_ascii_lowercase();
        if ACTIVE
            .iter()
            .any(|marker| content_type_lower.contains(marker))
        {
            return Err(Refusal::ActiveContent);
        }
        if EXTERNAL
            .iter()
            .any(|marker| content_type_lower.contains(marker))
        {
            return Err(Refusal::ExternalReference);
        }
        parts.push(Part {
            name,
            content_type: content_type.clone(),
            data,
        });
    }

    let mut diagnostics = BTreeSet::new();
    let mut relationships = Vec::new();
    for part in &parts {
        if let Some(source) = relationship_source(&part.name) {
            relationships.extend(read_relationships(
                &source,
                &part.data,
                &lowered,
                limits,
                cancel,
                &mut diagnostics,
            )?);
        }
        if part.content_type == SIGNATURE {
            diagnostics.insert("signature-not-verified");
        }
        match validate_emitted_string(&String::from_utf8_lossy(&part.data)) {
            Err(ExportError::DlpViolation(_)) => diagnostics.insert("dlp-findings"),
            Err(_) => diagnostics.insert("public-boundary-violation"),
            Ok(()) => false,
        };
    }
    relationships.sort();
    Ok(Package {
        parts,
        relationships,
        diagnostics,
    })
}

type ContentTypes = (BTreeMap<String, String>, BTreeMap<String, String>);

fn content_types(data: &[u8], limits: &Limits, cancel: &Cancel) -> Result<ContentTypes, Refusal> {
    let (mut defaults, mut overrides) = (BTreeMap::new(), BTreeMap::new());
    for event in xml::parse(data, limits, cancel)? {
        let Event::Open { name, attributes } = event else {
            continue;
        };
        let (key, table) = match name.as_str() {
            "Default" => ("Extension", &mut defaults),
            "Override" => ("PartName", &mut overrides),
            _ => continue,
        };
        let key = attribute(&attributes, key)?.to_ascii_lowercase();
        let content_type = attribute(&attributes, "ContentType")?.to_owned();
        if table.insert(key, content_type).is_some() {
            return Err(Refusal::MalformedContainer);
        }
    }
    Ok((defaults, overrides))
}

fn attribute<'a>(attributes: &'a [(String, String)], key: &str) -> Result<&'a str, Refusal> {
    attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
        .ok_or(Refusal::MalformedContainer)
}

/// `dir/_rels/name.rels` describes `dir/name`; `_rels/.rels` describes the package ("").
fn relationship_source(name: &str) -> Option<String> {
    let (directory, file) = name.rsplit_once('/').unwrap_or(("", name));
    // OPC names are case-insensitive, so `X.RELS` is a relationships part too.
    let stem = file.len().checked_sub(5)?;
    if !file.as_bytes()[stem..].eq_ignore_ascii_case(b".rels") {
        return None;
    }
    let file = &file[..stem]; // a char boundary: the five suffix bytes are ASCII
    let parent = if directory.eq_ignore_ascii_case("_rels") {
        ""
    } else {
        let (parent, rels) = directory.rsplit_once('/')?;
        if !rels.eq_ignore_ascii_case("_rels") {
            return None;
        }
        parent
    };
    Some(if parent.is_empty() {
        file.to_owned()
    } else {
        format!("{parent}/{file}")
    })
}

fn read_relationships(
    source: &str,
    data: &[u8],
    parts: &BTreeSet<String>,
    limits: &Limits,
    cancel: &Cancel,
    diagnostics: &mut BTreeSet<&'static str>,
) -> Result<Vec<Relationship>, Refusal> {
    let mut ids = BTreeSet::new();
    let mut relationships = Vec::new();
    for event in xml::parse(data, limits, cancel)? {
        let Event::Open { name, attributes } = event else {
            continue;
        };
        if name != "Relationship" {
            continue;
        }
        let id = attribute(&attributes, "Id")?.to_owned();
        let kind = attribute(&attributes, "Type")?.to_owned();
        let target = attribute(&attributes, "Target")?;
        if !ids.insert(id.clone()) {
            return Err(Refusal::MalformedContainer);
        }
        let external = match attribute(&attributes, "TargetMode") {
            Err(_) | Ok("Internal") => false,
            Ok("External") => true,
            Ok(_) => return Err(Refusal::MalformedContainer),
        };
        let target = if external {
            if !kind.ends_with("/hyperlink") {
                return Err(Refusal::ExternalReference);
            }
            diagnostics.insert("hyperlink-not-fetched");
            target.to_owned()
        } else {
            let resolved = resolve_target(source, target)?;
            // ponytail: exact case-insensitive lookup; percent-encoded targets that
            // name a decoded ZIP entry report as missing until an adapter needs them.
            if !parts.contains(&resolved.to_ascii_lowercase()) {
                diagnostics.insert("missing-relationship-target");
            }
            resolved
        };
        relationships.push(Relationship {
            source: source.to_owned(),
            id,
            kind,
            target,
            external,
        });
    }
    Ok(relationships)
}

/// Accepts a relative ZIP entry name that is a safe OPC part name.
pub fn validate_part_name(name: &str) -> Result<(), Refusal> {
    let lower = name.to_ascii_lowercase();
    let unsafe_segment = |segment: &str| {
        segment.is_empty() || segment == "." || segment == ".." || segment.ends_with('.')
    };
    if name.is_empty()
        || name.len() > 1024
        || name.starts_with('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
        || lower.contains("%2f")
        || lower.contains("%5c")
        || name.split('/').any(unsafe_segment)
    {
        return Err(Refusal::UnsafePartPath);
    }
    Ok(())
}

/// Resolves an internal relationship target against its source part. A
/// target that climbs above the package root is refused, never clamped.
pub fn resolve_target(source_part: &str, target: &str) -> Result<String, Refusal> {
    let target = target.split('#').next().unwrap_or_default();
    let (mut segments, rest): (Vec<&str>, &str) = match target.strip_prefix('/') {
        Some(absolute) => (Vec::new(), absolute),
        None => {
            let directory = source_part.rsplit_once('/').map(|(directory, _)| directory);
            (
                directory
                    .map(|d| d.split('/').collect())
                    .unwrap_or_default(),
                target,
            )
        }
    };
    for segment in rest.split('/') {
        match segment {
            "." => {}
            ".." => {
                segments.pop().ok_or(Refusal::UnsafePartPath)?;
            }
            segment => segments.push(segment),
        }
    }
    let resolved = segments.join("/");
    validate_part_name(&resolved)?;
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::relationship_source;

    #[test]
    fn relationship_source_maps_rels_parts() {
        assert_eq!(relationship_source("_rels/.rels").as_deref(), Some(""));
        assert_eq!(
            relationship_source("word/_rels/document.xml.rels").as_deref(),
            Some("word/document.xml")
        );
        assert_eq!(
            relationship_source("WORD/_RELS/Document.xml.RELS").as_deref(),
            Some("WORD/Document.xml")
        );
        assert_eq!(relationship_source("word/document.xml"), None);
        assert_eq!(relationship_source("word/other/x.rels"), None);
        // Multi-byte names must not split a character.
        assert_eq!(relationship_source("word/_rels/\u{e9}\u{e9}\u{e9}"), None);
        assert_eq!(relationship_source("\u{e9}"), None);
    }
}
