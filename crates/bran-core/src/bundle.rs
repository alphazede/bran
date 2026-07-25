//! Immutable normalized bundle and document types for BRAN (Slice 1.2 Packet A).
//!
//! A bundle is a hierarchical collection of Markdown documents.
//! Concept documents carry YAML frontmatter. Only a non-empty `type` field is
//! required by the OKF v0.1 contract; unknown fields are tolerated and preserved.
//!
//! Parsing of Markdown/YAML is explicitly out of scope for this slice.
//! Parse success/failure is modeled explicitly via ParseStatus so that
//! profile validation can evaluate malformed fixtures without pretending
//! to parse YAML.
//!
//! All collections use BTree types. Normalization retains original source
//! evidence (source, body, raw frontmatter). Canonical serialization is
//! deterministic and byte-identical regardless of construction order.

use crate::schema::{write_canonical_yaml_value, YamlValue, BUNDLE_SCHEMA_VERSION};
use std::collections::BTreeMap;

/// Explicit parse status for a document's frontmatter.
/// Parsing is not performed by this slice; later adapters produce these states.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ParseStatus {
    /// Frontmatter parsed successfully (or was absent and treated as empty success).
    Ok,
    /// Frontmatter was malformed; reason is producer- or adapter-supplied and preserved.
    Malformed { reason: String },
}

impl ParseStatus {
    /// Returns true if this status indicates successful parse (or absence).
    pub fn is_ok(&self) -> bool {
        matches!(self, ParseStatus::Ok)
    }
}

/// Normalized frontmatter evidence.
/// Raw text is retained exactly to prove no silent rewrite occurred.
/// Parsed map is present only on successful parse; unknown keys are preserved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frontmatter {
    /// Exact original raw frontmatter block text (including delimiters if present),
    /// or empty string when no frontmatter block existed in source.
    raw: String,
    /// Normalized mapping when parse succeeded. None when absent or malformed.
    /// Keys and nested structures preserve every producer-defined field.
    parsed: Option<BTreeMap<String, YamlValue>>,
    /// Explicit parse outcome. Malformed keeps its reason for diagnostics.
    status: ParseStatus,
}

impl Frontmatter {
    /// Constructs an empty frontmatter (no block present, successful absence).
    pub fn empty() -> Self {
        Self {
            raw: String::new(),
            parsed: Some(BTreeMap::new()),
            status: ParseStatus::Ok,
        }
    }

    /// Constructs frontmatter from raw text and a successfully parsed map.
    /// The map may be empty or contain only unknown fields; `type` is not enforced here.
    pub fn from_parsed(raw: impl Into<String>, parsed: BTreeMap<String, YamlValue>) -> Self {
        Self {
            raw: raw.into(),
            parsed: Some(parsed),
            status: ParseStatus::Ok,
        }
    }

    /// Constructs an explicitly malformed frontmatter.
    /// Raw is retained for evidence; parsed is None.
    pub fn malformed(raw: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            raw: raw.into(),
            parsed: None,
            status: ParseStatus::Malformed {
                reason: reason.into(),
            },
        }
    }

    /// Raw frontmatter bytes exactly as in source (byte-for-byte, including delimiters).
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Parsed map if parse was successful; None for absent (empty) or malformed.
    pub fn parsed(&self) -> Option<&BTreeMap<String, YamlValue>> {
        self.parsed.as_ref()
    }

    /// Parse status (Ok or Malformed with reason).
    pub fn status(&self) -> &ParseStatus {
        &self.status
    }
}

/// A single document inside a bundle.
/// Source and body are retained verbatim. Frontmatter evidence is preserved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Doc {
    /// Relative path within the bundle, e.g. "concepts/foo.md" or "index.md".
    path: String,
    /// Full original source including any frontmatter delimiters and body.
    source: String,
    /// Markdown body after stripping any frontmatter block. May be empty.
    body: String,
    /// Frontmatter with raw + normalized evidence.
    frontmatter: Frontmatter,
}

impl Doc {
    /// Creates a new Doc. Callers supply already-separated source/body and frontmatter.
    /// This constructor does not parse; it models a normalized result.
    pub fn new(
        path: impl Into<String>,
        source: impl Into<String>,
        body: impl Into<String>,
        frontmatter: Frontmatter,
    ) -> Self {
        Self {
            path: path.into(),
            source: source.into(),
            body: body.into(),
            frontmatter,
        }
    }

    /// Relative path within the bundle.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Full original source (byte-for-byte).
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Body after frontmatter (byte-for-byte).
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Frontmatter evidence (raw preserved exactly; no parsing performed here).
    pub fn frontmatter(&self) -> &Frontmatter {
        &self.frontmatter
    }

    /// Returns the kind of this document based on its path.
    /// Concept identity is the relative path with the trailing ".md" removed.
    /// index.md and log.md are reserved and distinguishable from concepts (at any hierarchy level).
    pub fn kind(&self) -> DocKind {
        let base = self.path.rsplit('/').next().unwrap_or(&self.path);
        if base == "index.md" {
            DocKind::Index
        } else if base == "log.md" {
            DocKind::Log
        } else {
            // Concept identity: strip a single trailing .md if present (keeps directory prefix).
            let identity = if let Some(stripped) = self.path.strip_suffix(".md") {
                stripped.to_owned()
            } else {
                self.path.clone()
            };
            DocKind::Concept { identity }
        }
    }

    /// Returns the concept identity if this doc is a concept, else None.
    pub fn concept_identity(&self) -> Option<&str> {
        let base = self.path.rsplit('/').next().unwrap_or(&self.path);
        if base == "index.md" || base == "log.md" {
            return None;
        }
        // Concept identity is relative path without trailing .md
        if let Some(stripped) = self.path.strip_suffix(".md") {
            Some(stripped)
        } else {
            Some(self.path.as_str())
        }
    }
}

/// Distinguishes document roles inside a bundle.
/// Concepts are identified by relative path without the ".md" suffix.
/// index.md and log.md are reserved kinds.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum DocKind {
    /// A concept document. Identity is the relative path without ".md".
    Concept { identity: String },
    /// Reserved root index document.
    Index,
    /// Reserved root log document.
    Log,
}

/// An immutable normalized bundle of documents.
/// Docs are stored under their path in a BTreeMap for stable canonical order.
/// Schema version is carried for deterministic serialization compatibility.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bundle {
    /// Normalization schema version. Serialized into canonical form.
    schema_version: String,
    /// Documents keyed by their relative path. BTreeMap guarantees order stability.
    docs: BTreeMap<String, Doc>,
}

impl Bundle {
    /// Creates an empty bundle with the stable schema version "1".
    pub fn new() -> Self {
        Self {
            schema_version: BUNDLE_SCHEMA_VERSION.to_owned(),
            docs: BTreeMap::new(),
        }
    }

    /// Constructs a bundle from documents.
    ///
    /// Duplicate paths are rejected with a typed error (deterministic: first duplicate
    /// encountered during iteration is reported). No public mutation after construction.
    /// A local BTreeMap is used only during construction.
    pub fn from_documents<I>(docs: I) -> Result<Self, DuplicatePathError>
    where
        I: IntoIterator<Item = Doc>,
    {
        let mut map: BTreeMap<String, Doc> = BTreeMap::new();
        for doc in docs {
            let p = doc.path.clone();
            if map.contains_key(&p) {
                return Err(DuplicatePathError { path: p });
            }
            map.insert(p, doc);
        }
        Ok(Self {
            schema_version: BUNDLE_SCHEMA_VERSION.to_owned(),
            docs: map,
        })
    }

    /// Returns a reference to a doc by path.
    pub fn get(&self, path: &str) -> Option<&Doc> {
        self.docs.get(path)
    }

    /// Returns the number of documents.
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// Returns true if the bundle contains no documents.
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Stable schema version for this canonical bundle envelope (always "1").
    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    /// Read-only access to the documents (BTreeMap for deterministic order).
    pub fn docs(&self) -> &BTreeMap<String, Doc> {
        &self.docs
    }

    /// Produces a deterministic canonical serialization.
    /// Output is byte-identical on repeated calls and independent of
    /// any prior insertion/Hash order because internal storage is BTree.
    /// Schema version is included. Unknown fields and raw evidence are preserved.
    pub fn to_canonical_form(&self) -> String {
        let mut out = String::new();
        out.push('{');
        // schema_version first for stable header shape
        out.push_str("\"schema_version\":");
        crate::schema::write_escaped_string_for_bundle(&mut out, &self.schema_version);
        out.push_str(",\"docs\":{");
        for (i, (path, doc)) in self.docs.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            crate::schema::write_escaped_string_for_bundle(&mut out, path);
            out.push(':');
            write_doc_canonical(&mut out, doc);
        }
        out.push_str("}}");
        out
    }
}

/// Typed rejection for duplicate document paths during Bundle construction.
/// Behavior is deterministic (reports the path of the first duplicate seen in iterator order).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DuplicatePathError {
    pub path: String,
}

impl Default for Bundle {
    fn default() -> Self {
        Self::new()
    }
}

fn write_doc_canonical(out: &mut String, doc: &Doc) {
    out.push('{');
    // Emit in fixed key order for determinism: body, frontmatter, path, source
    out.push_str("\"body\":");
    crate::schema::write_escaped_string_for_bundle(out, &doc.body);
    out.push_str(",\"frontmatter\":");
    write_frontmatter_canonical(out, &doc.frontmatter);
    out.push_str(",\"path\":");
    crate::schema::write_escaped_string_for_bundle(out, &doc.path);
    out.push_str(",\"source\":");
    crate::schema::write_escaped_string_for_bundle(out, &doc.source);
    out.push('}');
}

fn write_frontmatter_canonical(out: &mut String, fm: &Frontmatter) {
    out.push('{');
    out.push_str("\"raw\":");
    crate::schema::write_escaped_string_for_bundle(out, &fm.raw);
    out.push_str(",\"parsed\":");
    match &fm.parsed {
        Some(map) => {
            // Serialize as object using canonical value writer for values.
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                crate::schema::write_escaped_string_for_bundle(out, k);
                out.push(':');
                write_canonical_yaml_value(out, v);
            }
            out.push('}');
        }
        None => out.push_str("null"),
    }
    out.push_str(",\"status\":");
    match &fm.status {
        ParseStatus::Ok => out.push_str("\"ok\""),
        ParseStatus::Malformed { reason } => {
            out.push_str("{\"malformed\":");
            crate::schema::write_escaped_string_for_bundle(out, reason);
            out.push('}');
        }
    }
    out.push('}');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::YamlValue;
    use std::collections::BTreeMap;

    #[test]
    fn p1_bundle() {
        let mut extension = BTreeMap::new();
        extension.insert("deep".to_owned(), YamlValue::Bool(true));
        extension.insert("number".to_owned(), YamlValue::Number("42".to_owned()));
        let mut parsed = BTreeMap::new();
        parsed.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        parsed.insert(
            "producer_extension".to_owned(),
            YamlValue::Mapping(extension),
        );
        let raw = "---\ntype: Concept\nproducer_extension: {deep: true, number: 42}\n---\n";
        let source = format!("{raw}Preserved body.");
        let document = Doc::new(
            "concepts/preserved.md",
            source.clone(),
            "Preserved body.",
            Frontmatter::from_parsed(raw, parsed),
        );
        let preserved = Bundle::from_documents([document.clone()]).expect("unique path");
        let reordered = Bundle::from_documents([document]).expect("unique path");

        assert_eq!(preserved.schema_version(), BUNDLE_SCHEMA_VERSION);
        assert_eq!(preserved.to_canonical_form(), reordered.to_canonical_form());
        assert!(preserved.to_canonical_form().contains("producer_extension"));
        let retained = preserved
            .get("concepts/preserved.md")
            .expect("document retained");
        assert_eq!(retained.source(), source);
        assert_eq!(retained.frontmatter().raw(), raw);

        // Direct assertions proving unambiguous typed encoding (no collision):
        // Number("42") must differ from producer Mapping{"__yaml_number__": String("42")}.
        let number42 = YamlValue::Number("42".to_owned());
        let mut prod_collide = BTreeMap::new();
        prod_collide.insert(
            "__yaml_number__".to_owned(),
            YamlValue::String("42".to_owned()),
        );
        let prod_map = YamlValue::Mapping(prod_collide);
        assert_ne!(number42, prod_map);
        let mut cnum = String::new();
        crate::schema::write_canonical_yaml_value(&mut cnum, &number42);
        let mut cmap = String::new();
        crate::schema::write_canonical_yaml_value(&mut cmap, &prod_map);
        assert_ne!(cnum, cmap);
        // typed number lexical value survives
        assert!(cnum.contains("\"42\""));
        assert!(cnum.contains("\"number\""));
        // unknown nested data survive (typed form)
        let canon = preserved.to_canonical_form();
        assert!(canon.contains("producer_extension"));
        assert!(canon.contains("deep"));
    }
}
