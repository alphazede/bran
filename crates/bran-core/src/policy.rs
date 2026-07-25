//! Versioned BRAN repository-policy model, loader, and diagnostics (Slice 1.1).
//!
//! Parses `.bran/policy.yaml` into an immutable `RepositoryPolicy` value.
//! Rejects unsupported versions, unknown fields (top-level and nested),
//! malformed YAML, oversized input, unsafe paths, duplicate paths, and
//! overlapping document classifications with typed deterministic diagnostics.
//! No dependencies beyond std.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Component, Path};

/// Well-known policy filename relative to repository root.
pub const POLICY_FILENAME: &str = ".bran/policy.yaml";

/// Supported native policy schema version.
pub const SUPPORTED_SCHEMA_VERSION: &str = "1";

/// Maximum policy file size in bytes (64 KiB).
pub const MAX_POLICY_BYTES: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// Typed diagnostics
// ---------------------------------------------------------------------------

/// Every reason a policy load can fail. Deterministic, no allocation ambiguity.
/// Diagnostics never include raw input lines, bodies, or arbitrary scalar values.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum PolicyError {
    /// The `.bran/policy.yaml` path contains traversal or is otherwise unsafe.
    UnsafePath { path: String },
    /// I/O error reading the policy file.
    Io { message: String },
    /// YAML syntax is not parseable by the BRAN policy subset.
    MalformedYaml { line: usize, message: String },
    /// The policy file exists but does not declare `schema_version`.
    MissingSchemaVersion,
    /// `schema_version` is present but not equal to the single supported value.
    UnsupportedVersion { found: String },
    /// A top-level mapping key is not recognised by this schema version.
    UnknownField { field: String },
    /// A nested key inside a known section is not recognised.
    UnknownNestedField { parent: String, field: String },
    /// YAML parsed successfully, but a known field has the wrong schema shape.
    InvalidFieldShape { field: String, expected: String },
    /// Policy input exceeds `MAX_POLICY_BYTES`.
    Oversized { size: usize, max: usize },
    /// `canonical_docs_frontmatter_state` is not `legacy_baseline` or `strict`.
    /// The found value is intentionally not stored or displayed (non-echo).
    InvalidMigrationState,
    /// `coverage` contains a class outside the native vocabulary.
    InvalidCoverageClass,
    /// An exclusion reason in `excluded_documents` is empty or whitespace-only.
    EmptyExclusionReason,
    /// A document path is absolute, contains parent traversal, NUL,
    /// drive/UNC, or is empty. The path is intentionally not stored or
    /// displayed (non-echo).
    UnsafeDocumentPath,
    /// A mapping contains a duplicate key.
    DuplicateKey { line: usize, key: String },
    /// A document path appears more than once within the same list.
    DuplicateDocumentPath { path: String },
    /// A document path is classified in more than one coverage bucket.
    OverlappingClassification {
        path: String,
        first: String,
        second: String,
    },
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsafePath { path } => write!(f, "unsafe policy path: {path}"),
            Self::Io { message } => write!(f, "policy I/O error: {message}"),
            Self::MalformedYaml { line, message } => {
                write!(f, "malformed YAML at line {line}: {message}")
            }
            Self::MissingSchemaVersion => {
                write!(f, "policy missing required field: schema_version")
            }
            Self::UnsupportedVersion { found } => {
                write!(
                    f,
                    "unsupported policy schema_version: {found} (supported: {SUPPORTED_SCHEMA_VERSION})"
                )
            }
            Self::UnknownField { field } => {
                write!(f, "unknown policy field: {field}")
            }
            Self::UnknownNestedField { parent, field } => {
                write!(f, "unknown field in {parent}: {field}")
            }
            Self::InvalidFieldShape { field, expected } => {
                write!(f, "invalid policy field {field}: expected {expected}")
            }
            Self::Oversized { size, max } => {
                write!(
                    f,
                    "policy input is {size} bytes, exceeds maximum of {max} bytes"
                )
            }
            Self::InvalidMigrationState => {
                write!(
                    f,
                    "canonical_docs_frontmatter_state must be legacy_baseline or strict"
                )
            }
            Self::InvalidCoverageClass => {
                write!(
                    f,
                    "coverage entries must be canonical, legacy, excluded, or unclassified"
                )
            }
            Self::EmptyExclusionReason => {
                write!(f, "excluded_documents reason must not be empty")
            }
            Self::UnsafeDocumentPath => {
                write!(
                    f,
                    "document path must be repository-relative, nonempty, with no absolute, drive/UNC, parent traversal, or NUL"
                )
            }
            Self::DuplicateKey { line, key } => {
                write!(f, "duplicate key at line {line}: {key}")
            }
            Self::DuplicateDocumentPath { path } => {
                write!(f, "duplicate document path within a list: {path}")
            }
            Self::OverlappingClassification {
                path,
                first,
                second,
            } => {
                write!(
                    f,
                    "document path classified in both {first} and {second}: {path}"
                )
            }
        }
    }
}

impl Error for PolicyError {}

// ---------------------------------------------------------------------------
// Typed DES-1 policy sub-structures
// ---------------------------------------------------------------------------

/// Frontmatter policy: required, allowed, optional fields, preserved canonical
/// keys, and canonical-document migration state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrontmatterPolicy {
    pub required: Vec<String>,
    pub optional: Vec<String>,
    pub allowed: Vec<String>,
    pub preserved_canonical_keys: Vec<String>,
    pub canonical_docs_frontmatter_state: Option<String>,
}

/// Source-links policy: required prefix for source URLs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLinksPolicy {
    pub require_prefix: String,
}

/// Tag membership policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TagsPolicy {
    pub allowed: Vec<String>,
}

/// Document status policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusPolicy {
    pub allowed: Vec<String>,
}

/// Public/private boundary classification policy.
pub type ExactBoundaryAllowlist = BTreeMap<String, BTreeMap<String, BTreeMap<usize, String>>>;

/// Public/private boundary classification policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicBoundaryPolicy {
    pub values: Vec<String>,
    /// Repository-relative paths allowed to cross the public boundary.
    pub path_allowlist: Vec<String>,
    /// Exact one-based line digests permitted for deterministic DLP findings.
    pub exact_allowlist: ExactBoundaryAllowlist,
}

/// Document coverage classification with explicit document lists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentCoveragePolicy {
    pub roots: Vec<String>,
    pub native_bundle: Vec<String>,
    pub canonical_documents: Vec<String>,
    pub legacy_documents: Vec<String>,
    /// Repository-relative path → nonempty exclusion reason.
    pub excluded_documents: BTreeMap<String, String>,
    pub bridge_targets: Vec<String>,
}

// ---------------------------------------------------------------------------
// Immutable policy value
// ---------------------------------------------------------------------------

/// Parsed, validated, immutable repository policy.
///
/// Created by [`RepositoryPolicy::load`] or [`RepositoryPolicy::parse_from_string`]
/// and guaranteed to satisfy all Slice 1.1 contracts before construction completes.
/// The struct is `Clone` so that consumers (validator, scanner, adapter) can share it
/// without synchronisation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryPolicy {
    /// Exact schema version string (always `SUPPORTED_SCHEMA_VERSION`).
    pub schema_version: String,
    /// Frontmatter field requirements and migration state.
    pub frontmatter: Option<FrontmatterPolicy>,
    /// Coverage classes recognised by the validator.
    pub coverage: Option<Vec<String>>,
    /// Document coverage classification.
    pub document_coverage: Option<DocumentCoveragePolicy>,
    /// Source-link prefix policy.
    pub source_links: Option<SourceLinksPolicy>,
    /// Tag membership policy.
    pub tags: Option<TagsPolicy>,
    /// Document status policy.
    pub status: Option<StatusPolicy>,
    /// Public/private boundary policy.
    pub public_boundary: Option<PublicBoundaryPolicy>,
}

impl RepositoryPolicy {
    /// Load `.bran/policy.yaml` from the repository root.
    ///
    /// Delegates to [`parse_from_string`] after path-safety and I/O checks.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyError`] for every detectable contract violation
    /// (unsafe path, I/O, oversized, malformed YAML, missing/unsupported version,
    /// unknown top-level/nested fields, invalid migration state, unsafe/duplicate/
    /// overlapping document paths, empty exclusion reasons). Never returns a
    /// partially-initialised policy.
    pub fn load(root: &Path) -> Result<Self, PolicyError> {
        let policy_path = root.join(POLICY_FILENAME);

        // --- path safety ---
        if policy_path.components().any(|c| c == Component::ParentDir) {
            return Err(PolicyError::UnsafePath {
                path: policy_path.to_string_lossy().into_owned(),
            });
        }

        // --- size check via metadata (reject oversized before reading) ---
        let metadata = fs::metadata(&policy_path).map_err(|e| PolicyError::Io {
            message: e.to_string(),
        })?;
        let file_size = metadata.len() as usize;
        if file_size > MAX_POLICY_BYTES {
            return Err(PolicyError::Oversized {
                size: file_size,
                max: MAX_POLICY_BYTES,
            });
        }

        let raw = fs::read_to_string(&policy_path).map_err(|e| PolicyError::Io {
            message: e.to_string(),
        })?;

        Self::parse_from_string(&raw)
    }

    /// Parse and validate a policy from a string.
    ///
    /// Enforces `MAX_POLICY_BYTES`, then delegates to the YAML parser and
    /// typed extractors. Returns a fully validated [`RepositoryPolicy`].
    pub fn parse_from_string(raw: &str) -> Result<Self, PolicyError> {
        if raw.len() > MAX_POLICY_BYTES {
            return Err(PolicyError::Oversized {
                size: raw.len(),
                max: MAX_POLICY_BYTES,
            });
        }

        let parsed = parse_policy_yaml(raw)?;
        Self::from_parsed(parsed)
    }

    /// Build a [`RepositoryPolicy`] from a parsed token map.
    fn from_parsed(parsed: ParsedPolicy) -> Result<Self, PolicyError> {
        // --- schema_version required ---
        let version = match parsed.map.get("schema_version") {
            Some(YamlValue::Scalar(v)) => v.clone(),
            Some(_) => return Err(PolicyError::MissingSchemaVersion),
            None => return Err(PolicyError::MissingSchemaVersion),
        };

        if version != SUPPORTED_SCHEMA_VERSION {
            return Err(PolicyError::UnsupportedVersion { found: version });
        }

        // --- unknown top-level field detection ---
        let known: BTreeSet<&str> = known_top_level_keys();
        for key in parsed.map.keys() {
            if !known.contains(key.as_str()) {
                return Err(PolicyError::UnknownField { field: key.clone() });
            }
        }

        // --- extract typed fields ---
        let frontmatter = extract_frontmatter(&parsed)?;
        let coverage = extract_coverage(&parsed)?;
        let document_coverage = extract_document_coverage(&parsed)?;
        let source_links = extract_source_links(&parsed)?;
        let tags = extract_tags(&parsed)?;
        let status = extract_status(&parsed)?;
        let public_boundary = extract_public_boundary(&parsed)?;

        // --- cross-cutting path validations ---
        if let Some(ref dc) = document_coverage {
            validate_document_coverage_paths(dc)?;
        }
        if let Some(ref pb) = public_boundary {
            validate_path_list(&pb.path_allowlist, "path_allowlist")?;
            if !pb.exact_allowlist.is_empty() {
                let bridges: BTreeSet<&str> = document_coverage
                    .as_ref()
                    .map(|coverage| coverage.bridge_targets.iter().map(String::as_str).collect())
                    .unwrap_or_default();
                if pb
                    .exact_allowlist
                    .keys()
                    .any(|path| !bridges.contains(path.as_str()))
                {
                    return Err(PolicyError::MalformedYaml {
                        line: 0,
                        message: "public_boundary exact allowlist paths must be bridge targets"
                            .into(),
                    });
                }
            }
        }

        Ok(Self {
            schema_version: version,
            frontmatter,
            coverage,
            document_coverage,
            source_links,
            tags,
            status,
            public_boundary,
        })
    }

    /// Return a shared reference to this already-normalised policy.
    ///
    /// Normalisation is a load-time property; every `RepositoryPolicy` is
    /// immutable after construction so `normalize()` is an identity alias.
    pub fn normalize(&self) -> &Self {
        self
    }
}

// ---------------------------------------------------------------------------
// Known fields for Slice 1.1 (DES-1 contract surface)
// ---------------------------------------------------------------------------

fn known_top_level_keys() -> BTreeSet<&'static str> {
    [
        "schema_version",
        "frontmatter",
        "coverage",
        "document_coverage",
        "source_links",
        "tags",
        "status",
        "public_boundary",
    ]
    .into_iter()
    .collect()
}

fn invalid_field_shape(field: impl Into<String>, expected: &str) -> PolicyError {
    PolicyError::InvalidFieldShape {
        field: field.into(),
        expected: expected.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Path validation helpers
// ---------------------------------------------------------------------------

/// Validate a single repository-relative path: nonempty, no absolute prefix,
/// no parent-traversal components.
fn validate_repo_path(path: &str) -> Result<(), PolicyError> {
    if path.is_empty() {
        return Err(PolicyError::UnsafeDocumentPath);
    }
    // NUL bytes are never valid in a path.
    if path.contains('\0') {
        return Err(PolicyError::UnsafeDocumentPath);
    }
    // Windows drive-letter absolute (C:\..., D:/...).
    if path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':' {
        return Err(PolicyError::UnsafeDocumentPath);
    }
    // UNC (\\server\share...).
    if path.starts_with('\\') {
        return Err(PolicyError::UnsafeDocumentPath);
    }
    // Unix absolute.
    if path.starts_with('/') {
        return Err(PolicyError::UnsafeDocumentPath);
    }
    let p = Path::new(path);
    if p.components().any(|c| c == Component::ParentDir) {
        return Err(PolicyError::UnsafeDocumentPath);
    }
    Ok(())
}

/// Validate every path in a string list: each path is safe, and there are
/// no duplicates within the list.
fn validate_path_list(paths: &[String], _list_name: &str) -> Result<(), PolicyError> {
    let mut seen = BTreeSet::new();
    for p in paths {
        validate_repo_path(p)?;
        if !seen.insert(p.as_str()) {
            return Err(PolicyError::DuplicateDocumentPath { path: p.clone() });
        }
    }
    Ok(())
}

/// Validate every key in a BTreeMap as a safe path, and every value is
/// nonempty. No duplicate keys possible (BTreeMap).
fn validate_excluded_documents(map: &BTreeMap<String, String>) -> Result<(), PolicyError> {
    for (path, reason) in map {
        validate_repo_path(path)?;
        if reason.trim().is_empty() {
            return Err(PolicyError::EmptyExclusionReason);
        }
    }
    Ok(())
}

/// Validate that no path appears in more than one classification bucket.
/// Roots are exempt: they are scan roots and may contain classified documents.
fn validate_no_overlapping_classifications(dc: &DocumentCoveragePolicy) -> Result<(), PolicyError> {
    // Track which set each path belongs to (first assignment wins for error message).
    let mut assigned: BTreeMap<&str, &str> = BTreeMap::new();

    for p in &dc.native_bundle {
        if let Some(existing) = assigned.get(p.as_str()) {
            return Err(PolicyError::OverlappingClassification {
                path: p.clone(),
                first: existing.to_string(),
                second: "native_bundle".to_string(),
            });
        }
        assigned.insert(p.as_str(), "native_bundle");
    }
    for p in &dc.canonical_documents {
        if let Some(existing) = assigned.get(p.as_str()) {
            return Err(PolicyError::OverlappingClassification {
                path: p.clone(),
                first: existing.to_string(),
                second: "canonical_documents".to_string(),
            });
        }
        assigned.insert(p.as_str(), "canonical_documents");
    }
    for p in &dc.legacy_documents {
        if let Some(existing) = assigned.get(p.as_str()) {
            return Err(PolicyError::OverlappingClassification {
                path: p.clone(),
                first: existing.to_string(),
                second: "legacy_documents".to_string(),
            });
        }
        assigned.insert(p.as_str(), "legacy_documents");
    }
    for p in &dc.bridge_targets {
        if let Some(existing) = assigned.get(p.as_str()) {
            return Err(PolicyError::OverlappingClassification {
                path: p.clone(),
                first: existing.to_string(),
                second: "bridge_targets".to_string(),
            });
        }
        assigned.insert(p.as_str(), "bridge_targets");
    }

    // excluded_documents keys
    for p in dc.excluded_documents.keys() {
        if let Some(existing) = assigned.get(p.as_str()) {
            return Err(PolicyError::OverlappingClassification {
                path: p.clone(),
                first: existing.to_string(),
                second: "excluded_documents".to_string(),
            });
        }
        assigned.insert(p.as_str(), "excluded_documents");
    }

    Ok(())
}

/// Run all document-coverage path validations.
fn validate_document_coverage_paths(dc: &DocumentCoveragePolicy) -> Result<(), PolicyError> {
    validate_path_list(&dc.roots, "roots")?;
    validate_path_list(&dc.native_bundle, "native_bundle")?;
    validate_path_list(&dc.canonical_documents, "canonical_documents")?;
    validate_path_list(&dc.legacy_documents, "legacy_documents")?;
    validate_path_list(&dc.bridge_targets, "bridge_targets")?;
    validate_excluded_documents(&dc.excluded_documents)?;
    validate_no_overlapping_classifications(dc)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Typed extractors — each consumes a known section, validates shape,
// and rejects unknown nested keys.
// ---------------------------------------------------------------------------

fn extract_frontmatter(parsed: &ParsedPolicy) -> Result<Option<FrontmatterPolicy>, PolicyError> {
    let node = match parsed.map.get("frontmatter") {
        Some(YamlValue::Map(m)) => m,
        Some(_) => return Err(invalid_field_shape("frontmatter", "mapping")),
        None => return Ok(None),
    };

    const FRONTMATTER_KEYS: &[&str] = &[
        "required",
        "optional",
        "allowed",
        "preserved_canonical_keys",
        "canonical_docs_frontmatter_state",
    ];
    for key in node.keys() {
        if !FRONTMATTER_KEYS.contains(&key.as_str()) {
            return Err(PolicyError::UnknownNestedField {
                parent: "frontmatter".into(),
                field: key.clone(),
            });
        }
    }

    let required = node
        .get("required")
        .map(|v| match v {
            YamlValue::Seq(s) => Ok(s.clone()),
            _ => Err(invalid_field_shape("frontmatter.required", "sequence")),
        })
        .transpose()?
        .unwrap_or_default();

    let optional = node
        .get("optional")
        .map(|v| match v {
            YamlValue::Seq(s) => Ok(s.clone()),
            _ => Err(invalid_field_shape("frontmatter.optional", "sequence")),
        })
        .transpose()?
        .unwrap_or_default();

    let allowed = node
        .get("allowed")
        .map(|v| match v {
            YamlValue::Seq(s) => Ok(s.clone()),
            _ => Err(invalid_field_shape("frontmatter.allowed", "sequence")),
        })
        .transpose()?
        .unwrap_or_default();

    let preserved_canonical_keys = node
        .get("preserved_canonical_keys")
        .map(|v| match v {
            YamlValue::Seq(s) => Ok(s.clone()),
            _ => Err(invalid_field_shape(
                "frontmatter.preserved_canonical_keys",
                "sequence",
            )),
        })
        .transpose()?
        .unwrap_or_default();

    let canonical_docs_frontmatter_state = node
        .get("canonical_docs_frontmatter_state")
        .map(|v| match v {
            YamlValue::Scalar(s) => {
                let s = s.clone();
                if s != "legacy_baseline" && s != "strict" {
                    return Err(PolicyError::InvalidMigrationState);
                }
                Ok(s)
            }
            _ => Err(invalid_field_shape(
                "frontmatter.canonical_docs_frontmatter_state",
                "string",
            )),
        })
        .transpose()?;

    Ok(Some(FrontmatterPolicy {
        required,
        optional,
        allowed,
        preserved_canonical_keys,
        canonical_docs_frontmatter_state,
    }))
}

fn extract_coverage(parsed: &ParsedPolicy) -> Result<Option<Vec<String>>, PolicyError> {
    match parsed.map.get("coverage") {
        Some(YamlValue::Seq(s)) => {
            if s.iter().any(|value| {
                !matches!(
                    value.as_str(),
                    "canonical" | "legacy" | "excluded" | "unclassified"
                )
            }) {
                return Err(PolicyError::InvalidCoverageClass);
            }
            Ok(Some(s.clone()))
        }
        Some(_) => Err(invalid_field_shape("coverage", "sequence")),
        None => Ok(None),
    }
}

fn extract_document_coverage(
    parsed: &ParsedPolicy,
) -> Result<Option<DocumentCoveragePolicy>, PolicyError> {
    let node = match parsed.map.get("document_coverage") {
        Some(YamlValue::Map(m)) => m,
        Some(_) => return Err(invalid_field_shape("document_coverage", "mapping")),
        None => return Ok(None),
    };

    const DOC_COVERAGE_KEYS: &[&str] = &[
        "roots",
        "native_bundle",
        "canonical_documents",
        "legacy_documents",
        "excluded_documents",
        "bridge_targets",
    ];
    for key in node.keys() {
        if !DOC_COVERAGE_KEYS.contains(&key.as_str()) {
            return Err(PolicyError::UnknownNestedField {
                parent: "document_coverage".into(),
                field: key.clone(),
            });
        }
    }

    let roots = extract_string_seq(node, "roots")?.unwrap_or_default();
    let native_bundle = extract_string_seq(node, "native_bundle")?.unwrap_or_default();
    let canonical_documents = extract_string_seq(node, "canonical_documents")?.unwrap_or_default();
    let legacy_documents = extract_string_seq(node, "legacy_documents")?.unwrap_or_default();
    let bridge_targets = extract_string_seq(node, "bridge_targets")?.unwrap_or_default();

    let excluded_documents = match node.get("excluded_documents") {
        Some(YamlValue::Map(m)) => {
            let mut result = BTreeMap::new();
            for (k, v) in m {
                let reason = match v {
                    YamlValue::Scalar(s) => s.clone(),
                    _ => {
                        return Err(invalid_field_shape(
                            format!("document_coverage.excluded_documents.{k}"),
                            "string",
                        ))
                    }
                };
                result.insert(k.clone(), reason);
            }
            result
        }
        Some(_) => {
            return Err(invalid_field_shape(
                "document_coverage.excluded_documents",
                "mapping",
            ))
        }
        None => BTreeMap::new(),
    };

    Ok(Some(DocumentCoveragePolicy {
        roots,
        native_bundle,
        canonical_documents,
        legacy_documents,
        excluded_documents,
        bridge_targets,
    }))
}

fn extract_source_links(parsed: &ParsedPolicy) -> Result<Option<SourceLinksPolicy>, PolicyError> {
    let node = match parsed.map.get("source_links") {
        Some(YamlValue::Map(m)) => m,
        Some(_) => return Err(invalid_field_shape("source_links", "mapping")),
        None => return Ok(None),
    };

    const SOURCE_LINKS_KEYS: &[&str] = &["require_prefix"];
    for key in node.keys() {
        if !SOURCE_LINKS_KEYS.contains(&key.as_str()) {
            return Err(PolicyError::UnknownNestedField {
                parent: "source_links".into(),
                field: key.clone(),
            });
        }
    }

    let require_prefix = node
        .get("require_prefix")
        .map(|v| match v {
            YamlValue::Scalar(s) => Ok(s.clone()),
            _ => Err(invalid_field_shape("source_links.require_prefix", "string")),
        })
        .transpose()?
        .unwrap_or_default();

    Ok(Some(SourceLinksPolicy { require_prefix }))
}

fn extract_tags(parsed: &ParsedPolicy) -> Result<Option<TagsPolicy>, PolicyError> {
    let node = match parsed.map.get("tags") {
        Some(YamlValue::Map(m)) => m,
        Some(_) => return Err(invalid_field_shape("tags", "mapping")),
        None => return Ok(None),
    };

    const TAGS_KEYS: &[&str] = &["allowed"];
    for key in node.keys() {
        if !TAGS_KEYS.contains(&key.as_str()) {
            return Err(PolicyError::UnknownNestedField {
                parent: "tags".into(),
                field: key.clone(),
            });
        }
    }

    let allowed = node
        .get("allowed")
        .map(|v| match v {
            YamlValue::Seq(s) => Ok(s.clone()),
            _ => Err(invalid_field_shape("tags.allowed", "sequence")),
        })
        .transpose()?
        .unwrap_or_default();

    Ok(Some(TagsPolicy { allowed }))
}

fn extract_status(parsed: &ParsedPolicy) -> Result<Option<StatusPolicy>, PolicyError> {
    let node = match parsed.map.get("status") {
        Some(YamlValue::Map(m)) => m,
        Some(_) => return Err(invalid_field_shape("status", "mapping")),
        None => return Ok(None),
    };

    const STATUS_KEYS: &[&str] = &["allowed"];
    for key in node.keys() {
        if !STATUS_KEYS.contains(&key.as_str()) {
            return Err(PolicyError::UnknownNestedField {
                parent: "status".into(),
                field: key.clone(),
            });
        }
    }

    let allowed = node
        .get("allowed")
        .map(|v| match v {
            YamlValue::Seq(s) => Ok(s.clone()),
            _ => Err(invalid_field_shape("status.allowed", "sequence")),
        })
        .transpose()?
        .unwrap_or_default();

    Ok(Some(StatusPolicy { allowed }))
}

fn extract_public_boundary(
    parsed: &ParsedPolicy,
) -> Result<Option<PublicBoundaryPolicy>, PolicyError> {
    let node = match parsed.map.get("public_boundary") {
        Some(YamlValue::Map(m)) => m,
        Some(_) => return Err(invalid_field_shape("public_boundary", "mapping")),
        None => return Ok(None),
    };

    const PUBLIC_BOUNDARY_KEYS: &[&str] = &["values", "path_allowlist", "exact_allowlist"];
    for key in node.keys() {
        if !PUBLIC_BOUNDARY_KEYS.contains(&key.as_str()) {
            return Err(PolicyError::UnknownNestedField {
                parent: "public_boundary".into(),
                field: key.clone(),
            });
        }
    }

    let values = node
        .get("values")
        .map(|v| match v {
            YamlValue::Seq(s) => Ok(s.clone()),
            _ => Err(invalid_field_shape("public_boundary.values", "sequence")),
        })
        .transpose()?
        .unwrap_or_default();

    let path_allowlist = node
        .get("path_allowlist")
        .map(|v| match v {
            YamlValue::Seq(s) => Ok(s.clone()),
            _ => Err(invalid_field_shape(
                "public_boundary.path_allowlist",
                "sequence",
            )),
        })
        .transpose()?
        .unwrap_or_default();

    let exact_allowlist = match node.get("exact_allowlist") {
        None => BTreeMap::new(),
        Some(YamlValue::Map(paths)) => parse_boundary_exact_allowlist(paths)?,
        Some(_) => {
            return Err(invalid_field_shape(
                "public_boundary.exact_allowlist",
                "mapping",
            ))
        }
    };

    Ok(Some(PublicBoundaryPolicy {
        values,
        path_allowlist,
        exact_allowlist,
    }))
}

fn parse_boundary_exact_allowlist(
    paths: &BTreeMap<String, YamlValue>,
) -> Result<ExactBoundaryAllowlist, PolicyError> {
    const CODES: &[&str] = &[
        "private_home_path",
        "credential_assignment",
        "private_network_literal",
        "internal_agent_path",
        "proprietary_scoring_language",
    ];
    if paths.len() > 256 {
        return Err(PolicyError::MalformedYaml {
            line: 0,
            message: "public_boundary.exact_allowlist exceeds 256 paths".into(),
        });
    }
    let mut result = BTreeMap::new();
    let mut entry_count = 0usize;
    for (path, value) in paths {
        validate_repo_path(path)?;
        let codes = match value {
            YamlValue::Map(codes) if !codes.is_empty() => codes,
            _ => {
                return Err(PolicyError::MalformedYaml {
                    line: 0,
                    message: "public_boundary exact path entry must be a nonempty mapping".into(),
                })
            }
        };
        let mut parsed_codes = BTreeMap::new();
        for (code, value) in codes {
            if !CODES.contains(&code.as_str()) {
                return Err(PolicyError::UnknownNestedField {
                    parent: "public_boundary.exact_allowlist".into(),
                    field: code.clone(),
                });
            }
            let lines = match value {
                YamlValue::Map(lines) if !lines.is_empty() => lines,
                _ => {
                    return Err(PolicyError::MalformedYaml {
                        line: 0,
                        message: "public_boundary exact code entry must be a nonempty mapping"
                            .into(),
                    })
                }
            };
            let mut parsed_lines = BTreeMap::new();
            for (line, value) in lines {
                entry_count =
                    entry_count
                        .checked_add(1)
                        .ok_or_else(|| PolicyError::MalformedYaml {
                            line: 0,
                            message: "public_boundary exact allowlist entry count overflow".into(),
                        })?;
                if entry_count > 4096 {
                    return Err(PolicyError::MalformedYaml {
                        line: 0,
                        message: "public_boundary.exact_allowlist exceeds 4096 lines".into(),
                    });
                }
                let line_number =
                    line.parse::<usize>()
                        .map_err(|_| PolicyError::MalformedYaml {
                            line: 0,
                            message: "public_boundary exact line must be a positive integer".into(),
                        })?;
                if line_number == 0 {
                    return Err(PolicyError::MalformedYaml {
                        line: 0,
                        message: "public_boundary exact line must be a positive integer".into(),
                    });
                }
                let digest = match value {
                    YamlValue::Scalar(value)
                        if value.len() == 64
                            && value.bytes().all(|byte| {
                                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
                            }) =>
                    {
                        value.clone()
                    }
                    _ => {
                        return Err(PolicyError::MalformedYaml {
                            line: 0,
                            message: "public_boundary exact digest must be lowercase SHA-256"
                                .into(),
                        })
                    }
                };
                parsed_lines.insert(line_number, digest);
            }
            parsed_codes.insert(code.clone(), parsed_lines);
        }
        result.insert(path.clone(), parsed_codes);
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Helper: extract an optional string sequence from a map node
// ---------------------------------------------------------------------------

fn extract_string_seq(
    node: &BTreeMap<String, YamlValue>,
    key: &str,
) -> Result<Option<Vec<String>>, PolicyError> {
    match node.get(key) {
        Some(YamlValue::Seq(s)) => Ok(Some(s.clone())),
        Some(_) => Err(invalid_field_shape(
            format!("document_coverage.{key}"),
            "sequence",
        )),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Internal parsed value representation
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq)]
enum YamlValue {
    Scalar(String),
    Map(BTreeMap<String, YamlValue>),
    Seq(Vec<String>),
}

struct ParsedPolicy {
    map: BTreeMap<String, YamlValue>,
}

// ---------------------------------------------------------------------------
// Minimal YAML subset parser (zero deps, ponytail full)
//
// Handles only the BRAN policy subset:
//   - top-level mapping
//   - nested mappings (indent-delimited)
//   - sequences ( `- value` )
//   - string scalars (unquoted, single-quoted, double-quoted)
//   - empty lines and `#` comments
//
// Not handled (YAGNI for policy files):
//   - flow style ({}, [])
//   - block scalars (|, >)
//   - anchors / aliases
//   - tags
//   - multi-document streams
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
#[allow(dead_code)]
enum Token {
    /// A mapping key with an inline scalar value: `key: value`
    MappingKey {
        indent: usize,
        line: usize,
        key: String,
        value: YamlValue,
    },
    /// A sequence entry: `- value`
    SequenceEntry {
        indent: usize,
        line: usize,
        value: String,
    },
    /// A mapping key whose value is a nested block starting on the next line.
    NestedKey {
        indent: usize,
        line: usize,
        key: String,
    },
}

fn parse_policy_yaml(raw: &str) -> Result<ParsedPolicy, PolicyError> {
    let tokens = tokenize(raw)?;
    build_parsed_policy(&tokens, raw)
}

/// Convert raw text into an ordered list of tokens.
fn tokenize(raw: &str) -> Result<Vec<Token>, PolicyError> {
    let mut tokens: Vec<Token> = Vec::new();

    for (line_no, line) in raw.lines().enumerate() {
        let display_line = line_no + 1;
        let trimmed = line.trim_end();

        // skip empty lines and comment-only lines
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let indent = line.len() - line.trim_start().len();

        // Sequence entry: "- value" or "- value # comment"
        if let Some(rest) = trimmed.trim_start().strip_prefix("- ") {
            let val = strip_inline_comment(rest);
            tokens.push(Token::SequenceEntry {
                indent,
                line: display_line,
                value: parse_scalar(val, display_line)?,
            });
            continue;
        }

        // Mapping key: "key: value"  or  "key:" (nested block)
        if let Some((key, rest)) = split_key_value(trimmed) {
            let key = key.trim().to_owned();
            if key.is_empty() {
                return Err(PolicyError::MalformedYaml {
                    line: display_line,
                    message: "empty mapping key".into(),
                });
            }

            let rest = rest.trim();
            if rest.is_empty() {
                // Nested block (mapping or sequence follows)
                tokens.push(Token::NestedKey {
                    indent,
                    line: display_line,
                    key,
                });
            } else {
                let val = strip_inline_comment(rest);
                tokens.push(Token::MappingKey {
                    indent,
                    line: display_line,
                    key,
                    value: parse_inline_value(val, display_line)?,
                });
            }
            continue;
        }

        // Non-echoing diagnostic: never include raw line content
        return Err(PolicyError::MalformedYaml {
            line: display_line,
            message: "unexpected content".into(),
        });
    }

    Ok(tokens)
}

/// Build a parsed policy tree from the flat token list.
///
/// Strategy: partition tokens into groups by top-level (indent 0) key,
/// then recursively build nested structure from each group.
fn build_parsed_policy(tokens: &[Token], _raw: &str) -> Result<ParsedPolicy, PolicyError> {
    let mut map: BTreeMap<String, YamlValue> = BTreeMap::new();
    let mut i = 0;

    while i < tokens.len() {
        match &tokens[i] {
            Token::MappingKey {
                indent: 0,
                line,
                key,
                value,
            } => {
                if map.contains_key(key) {
                    return Err(PolicyError::DuplicateKey {
                        line: *line,
                        key: key.clone(),
                    });
                }
                map.insert(key.clone(), value.clone());
                i += 1;
            }
            Token::NestedKey {
                indent: 0,
                line,
                key,
            } => {
                if map.contains_key(key) {
                    return Err(PolicyError::DuplicateKey {
                        line: *line,
                        key: key.clone(),
                    });
                }
                let key = key.clone();
                i += 1;

                // Determine the base indent of children: first child's indent.
                let child_indent = if i < tokens.len() {
                    token_indent(&tokens[i])
                } else {
                    // Key with no children — treat as empty map.
                    map.insert(key, YamlValue::Map(BTreeMap::new()));
                    continue;
                };

                // peek ahead to determine if first child is a MappingKey/NestedKey (→ Map)
                // or SequenceEntry (→ Seq)
                if i < tokens.len() {
                    match &tokens[i] {
                        Token::SequenceEntry { .. } if token_indent(&tokens[i]) == child_indent => {
                            let seq = eat_sequence(tokens, &mut i, child_indent);
                            map.insert(key, YamlValue::Seq(seq));
                        }
                        _ => {
                            let child_map = eat_mapping(tokens, &mut i, child_indent)?;
                            map.insert(key, YamlValue::Map(child_map));
                        }
                    }
                } else {
                    map.insert(key, YamlValue::Map(BTreeMap::new()));
                }
            }
            _ => {
                // Non-top-level token at top level — malformed.
                return Err(PolicyError::MalformedYaml {
                    line: 0,
                    message: "unexpected indented content at top level".into(),
                });
            }
        }
    }

    Ok(ParsedPolicy { map })
}

fn token_indent(t: &Token) -> usize {
    match t {
        Token::MappingKey { indent, .. } => *indent,
        Token::SequenceEntry { indent, .. } => *indent,
        Token::NestedKey { indent, .. } => *indent,
    }
}

/// Consume a sequence of `SequenceEntry` tokens at the given indent.
fn eat_sequence(tokens: &[Token], i: &mut usize, indent: usize) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    while *i < tokens.len() {
        match &tokens[*i] {
            Token::SequenceEntry {
                indent: si, value, ..
            } if *si == indent => {
                items.push(value.clone());
                *i += 1;
            }
            _ => break,
        }
    }
    items
}

/// Consume mapping children at the given indent into a `BTreeMap`.
fn eat_mapping(
    tokens: &[Token],
    i: &mut usize,
    indent: usize,
) -> Result<BTreeMap<String, YamlValue>, PolicyError> {
    let mut map: BTreeMap<String, YamlValue> = BTreeMap::new();

    while *i < tokens.len() {
        match &tokens[*i] {
            Token::MappingKey {
                indent: mi,
                line,
                key,
                value,
            } if *mi == indent => {
                if map.contains_key(key) {
                    return Err(PolicyError::DuplicateKey {
                        line: *line,
                        key: key.clone(),
                    });
                }
                map.insert(key.clone(), value.clone());
                *i += 1;
            }
            Token::NestedKey {
                indent: ni,
                line,
                key,
                ..
            } if *ni == indent => {
                if map.contains_key(key) {
                    return Err(PolicyError::DuplicateKey {
                        line: *line,
                        key: key.clone(),
                    });
                }
                let key = key.clone();
                *i += 1;

                if *i >= tokens.len() {
                    map.insert(key, YamlValue::Map(BTreeMap::new()));
                    break;
                }

                let child_indent = token_indent(&tokens[*i]);
                if child_indent <= indent {
                    // no children, just the key
                    map.insert(key, YamlValue::Map(BTreeMap::new()));
                    continue;
                }

                match &tokens[*i] {
                    Token::SequenceEntry { .. } => {
                        let seq = eat_sequence(tokens, i, child_indent);
                        map.insert(key, YamlValue::Seq(seq));
                    }
                    _ => {
                        let child_map = eat_mapping(tokens, i, child_indent)?;
                        map.insert(key, YamlValue::Map(child_map));
                    }
                }
            }
            _ => break,
        }
    }

    Ok(map)
}

// ---------------------------------------------------------------------------
// Scalar helpers
// ---------------------------------------------------------------------------

fn parse_inline_value(raw: &str, line: usize) -> Result<YamlValue, PolicyError> {
    let value = raw.trim();
    if value.starts_with('[') {
        return parse_flow_sequence(value, line).map(YamlValue::Seq);
    }
    Ok(YamlValue::Scalar(parse_scalar(value, line)?))
}

fn parse_flow_sequence(raw: &str, line: usize) -> Result<Vec<String>, PolicyError> {
    let inner = raw
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(|| PolicyError::MalformedYaml {
            line,
            message: "unmatched flow-sequence delimiter".into(),
        })?
        .trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }

    let mut items = Vec::new();
    let mut start = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for (index, character) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if in_double => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ',' if !in_single && !in_double => {
                let item = inner[start..index].trim();
                if item.is_empty() {
                    return Err(PolicyError::MalformedYaml {
                        line,
                        message: "empty flow-sequence item".into(),
                    });
                }
                items.push(parse_scalar(item, line)?);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    if in_single || in_double || escaped {
        return Err(PolicyError::MalformedYaml {
            line,
            message: "unmatched quote in flow sequence".into(),
        });
    }
    let item = inner[start..].trim();
    if item.is_empty() {
        return Err(PolicyError::MalformedYaml {
            line,
            message: "empty flow-sequence item".into(),
        });
    }
    items.push(parse_scalar(item, line)?);
    Ok(items)
}

fn parse_scalar(raw: &str, line: usize) -> Result<String, PolicyError> {
    let s = raw.trim();

    // double-quoted
    if s.starts_with('"') {
        return decode_double_quoted(s, line);
    }

    // single-quoted
    if s.starts_with('\'') {
        return decode_single_quoted(s, line);
    }

    // unquoted — reject common ambiguous forms (non-echoing)
    if s.starts_with('{') || s.starts_with('[') || s.starts_with('|') || s.starts_with('>') {
        return Err(PolicyError::MalformedYaml {
            line,
            message: "unsupported YAML construct".into(),
        });
    }

    Ok(s.to_owned())
}

/// Decode a double-quoted YAML scalar (DES-11).
/// Only `\\` → `\` and `\"` → `"` are valid escapes.
/// Unsupported escapes, literal control chars, and unmatched delimiters are
/// rejected via MalformedYaml (non-echoing).
fn decode_double_quoted(raw: &str, line: usize) -> Result<String, PolicyError> {
    let inner = raw
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .ok_or_else(|| PolicyError::MalformedYaml {
            line,
            message: "unmatched double-quote delimiter".into(),
        })?;

    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(_) => {
                    return Err(PolicyError::MalformedYaml {
                        line,
                        message: "unsupported escape in double-quoted scalar".into(),
                    });
                }
                None => {
                    return Err(PolicyError::MalformedYaml {
                        line,
                        message: "dangling backslash at end of double-quoted scalar".into(),
                    });
                }
            }
        } else {
            if c == '"' {
                return Err(PolicyError::MalformedYaml {
                    line,
                    message: "unescaped double quote in double-quoted scalar".into(),
                });
            }
            let b = c as u32;
            if b < 0x20 || b == 0x7F {
                return Err(PolicyError::MalformedYaml {
                    line,
                    message: "control character in double-quoted scalar".into(),
                });
            }
            out.push(c);
        }
    }

    Ok(out)
}

/// Decode a single-quoted YAML scalar (DES-11).
/// Decode `''` → `'`. Reject literal control chars and unmatched delimiters.
fn decode_single_quoted(raw: &str, line: usize) -> Result<String, PolicyError> {
    let inner = raw
        .strip_prefix('\'')
        .and_then(|t| t.strip_suffix('\''))
        .ok_or_else(|| PolicyError::MalformedYaml {
            line,
            message: "unmatched single-quote delimiter".into(),
        })?;

    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\'' {
            // '' → ' (only valid escape: doubled apostrophe)
            match chars.next() {
                Some('\'') => out.push('\''),
                _ => {
                    return Err(PolicyError::MalformedYaml {
                        line,
                        message: "isolated apostrophe in single-quoted scalar".into(),
                    });
                }
            }
        } else {
            let b = c as u32;
            if b < 0x20 || b == 0x7F {
                return Err(PolicyError::MalformedYaml {
                    line,
                    message: "control character in single-quoted scalar".into(),
                });
            }
            out.push(c);
        }
    }
    Ok(out)
}

fn split_key_value(line: &str) -> Option<(&str, &str)> {
    // Find the first colon that is not inside quotes
    let bytes = line.as_bytes();
    let mut in_single = false;
    let mut in_double = false;

    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' if !in_single => in_double = !in_double,
            b'\'' if !in_double => in_single = !in_single,
            b':' if !in_single && !in_double => {
                return Some((&line[..i], &line[i + 1..]));
            }
            _ => {}
        }
    }
    None
}

fn strip_inline_comment(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut in_single = false;
    let mut in_double = false;

    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' if !in_single => in_double = !in_double,
            b'\'' if !in_double => in_single = !in_single,
            b'#' if !in_single && !in_double => {
                let before = &s[..i];
                return before.trim_end();
            }
            _ => {}
        }
    }
    s
}

// ---------------------------------------------------------------------------
// Tests (CMD-POLICY: SEIT-2, SEIT-12, SEIT-16, SEIT-17)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_path(name: &str) -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest_dir.join("../../fixtures/policy").join(name)
    }

    // --- scratch directory for isolated I/O tests ---

    fn scratch_root(prefix: &str) -> (PathBuf, PathBuf) {
        let mut dir = std::env::temp_dir();
        dir.push(format!("bran-policy-test-{prefix}-{}", std::process::id()));
        let bran_dir = dir.join(".bran");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&bran_dir).expect("create scratch .bran dir");
        (dir, bran_dir)
    }

    #[test]
    fn repository_policy_parses_exact_public_boundary_allowlist() {
        let digest = "364b18721a1747a4f755cc2eaf12629ce5d4848eab48e2817d9b7b9cdda110d9";
        let raw = format!(
            "schema_version: \"1\"\ndocument_coverage:\n  bridge_targets:\n    - tools/public.mjs\npublic_boundary:\n  exact_allowlist:\n    tools/public.mjs:\n      private_home_path:\n        10: {digest}\n"
        );
        let policy = RepositoryPolicy::parse_from_string(&raw).expect("valid exact allowlist");
        assert_eq!(
            policy.public_boundary.as_ref().unwrap().exact_allowlist["tools/public.mjs"]
                ["private_home_path"][&10],
            digest
        );
    }

    #[test]
    fn repository_policy_rejects_exact_allowlist_outside_bridge_targets() {
        let raw = "schema_version: \"1\"\npublic_boundary:\n  exact_allowlist:\n    tools/public.mjs:\n      private_home_path:\n        1: 364b18721a1747a4f755cc2eaf12629ce5d4848eab48e2817d9b7b9cdda110d9\n";
        assert!(matches!(
            RepositoryPolicy::parse_from_string(raw),
            Err(PolicyError::MalformedYaml { .. })
        ));
    }

    // ===================================================================
    // SEIT-2 positive: valid native policy with all DES-1/DES-8/DES-9/DES-10 fields
    // ===================================================================

    #[test]
    fn repository_policy_valid_v1_full_roundtrips() {
        let raw =
            std::fs::read_to_string(fixture_path("valid-v1.yaml")).expect("read valid-v1 fixture");
        let parsed = parse_policy_yaml(&raw).expect("parse valid policy");

        // schema_version
        let v = match parsed.map.get("schema_version") {
            Some(YamlValue::Scalar(s)) => s.as_str(),
            _ => panic!("schema_version missing"),
        };
        assert_eq!(v, SUPPORTED_SCHEMA_VERSION);

        // frontmatter is a Map with typed sub-fields
        let fm = match parsed.map.get("frontmatter") {
            Some(YamlValue::Map(m)) => m,
            _ => panic!("frontmatter must be a map"),
        };
        assert!(matches!(fm.get("required"), Some(YamlValue::Seq(_))));
        assert!(matches!(fm.get("optional"), Some(YamlValue::Seq(_))));
        assert!(matches!(fm.get("allowed"), Some(YamlValue::Seq(_))));
        assert!(matches!(
            fm.get("preserved_canonical_keys"),
            Some(YamlValue::Seq(_))
        ));
        assert!(matches!(
            fm.get("canonical_docs_frontmatter_state"),
            Some(YamlValue::Scalar(_))
        ));

        // coverage is a Seq
        assert!(matches!(
            parsed.map.get("coverage"),
            Some(YamlValue::Seq(_))
        ));

        // document_coverage is a Map
        let dc = match parsed.map.get("document_coverage") {
            Some(YamlValue::Map(m)) => m,
            _ => panic!("document_coverage must be a map"),
        };
        assert!(matches!(dc.get("roots"), Some(YamlValue::Seq(_))));
        assert!(matches!(dc.get("native_bundle"), Some(YamlValue::Seq(_))));
        assert!(matches!(
            dc.get("canonical_documents"),
            Some(YamlValue::Seq(_))
        ));
        assert!(matches!(
            dc.get("legacy_documents"),
            Some(YamlValue::Seq(_))
        ));
        assert!(matches!(
            dc.get("excluded_documents"),
            Some(YamlValue::Map(_))
        ));
        assert!(matches!(dc.get("bridge_targets"), Some(YamlValue::Seq(_))));

        // source_links is a Map
        assert!(matches!(
            parsed.map.get("source_links"),
            Some(YamlValue::Map(_))
        ));

        // tags is a Map
        assert!(matches!(parsed.map.get("tags"), Some(YamlValue::Map(_))));

        // status is a Map
        assert!(matches!(parsed.map.get("status"), Some(YamlValue::Map(_))));

        // public_boundary is a Map with path_allowlist
        let pb = match parsed.map.get("public_boundary") {
            Some(YamlValue::Map(m)) => m,
            _ => panic!("public_boundary must be a map"),
        };
        assert!(matches!(pb.get("values"), Some(YamlValue::Seq(_))));
        assert!(matches!(pb.get("path_allowlist"), Some(YamlValue::Seq(_))));

        // Deterministic re-parse
        let parsed2 = parse_policy_yaml(&raw).expect("re-parse");
        assert_eq!(parsed.map, parsed2.map, "parser must be deterministic");
    }

    #[test]
    fn repository_policy_load_from_root_typed_fields() {
        let (root, bran_dir) = scratch_root("load");
        let fixture_raw =
            std::fs::read_to_string(fixture_path("valid-v1.yaml")).expect("read fixture");
        fs::write(bran_dir.join("policy.yaml"), &fixture_raw).expect("write policy");

        let policy = RepositoryPolicy::load(&root).expect("load valid policy");

        // schema_version
        assert_eq!(policy.schema_version, "1");

        // frontmatter typed
        let fm = policy.frontmatter.as_ref().expect("frontmatter present");
        assert!(!fm.required.is_empty());
        assert!(!fm.optional.is_empty());
        assert!(!fm.allowed.is_empty());
        assert!(!fm.preserved_canonical_keys.is_empty());
        assert_eq!(
            fm.canonical_docs_frontmatter_state.as_deref(),
            Some("legacy_baseline")
        );

        // coverage typed
        let cov = policy.coverage.as_ref().expect("coverage present");
        assert!(!cov.is_empty());

        // document_coverage typed
        let dc = policy
            .document_coverage
            .as_ref()
            .expect("document_coverage present");
        assert!(!dc.roots.is_empty());
        assert!(!dc.native_bundle.is_empty());
        assert!(!dc.canonical_documents.is_empty());
        assert!(!dc.legacy_documents.is_empty());
        assert!(!dc.excluded_documents.is_empty());
        assert!(!dc.bridge_targets.is_empty());

        // source_links typed
        let sl = policy.source_links.as_ref().expect("source_links present");
        assert!(!sl.require_prefix.is_empty());

        // tags typed
        let tags = policy.tags.as_ref().expect("tags present");
        assert!(!tags.allowed.is_empty());

        // status typed
        let status = policy.status.as_ref().expect("status present");
        assert!(!status.allowed.is_empty());

        // public_boundary typed with path_allowlist
        let pb = policy
            .public_boundary
            .as_ref()
            .expect("public_boundary present");
        assert!(!pb.values.is_empty());
        assert!(!pb.path_allowlist.is_empty());

        // normalize identity
        assert_eq!(policy.normalize().schema_version, "1");

        // load yields same policy from file vs parse_from_string
        let policy_from_str =
            RepositoryPolicy::parse_from_string(&fixture_raw).expect("parse_from_string");
        assert_eq!(policy, policy_from_str);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repository_policy_coverage_accepts_only_native_classes() {
        let allowed = "schema_version: \"1\"\ncoverage:\n  - canonical\n  - legacy\n  - excluded\n  - unclassified\n";
        assert!(RepositoryPolicy::parse_from_string(allowed).is_ok());
        assert!(matches!(
            RepositoryPolicy::parse_from_string("schema_version: \"1\"\ncoverage:\n  - canoncal\n"),
            Err(PolicyError::InvalidCoverageClass)
        ));
    }

    /// Minimal policy with only schema_version must load with all optional fields None.
    #[test]
    fn repository_policy_minimal_valid() {
        let (root, bran_dir) = scratch_root("minimal");
        fs::write(bran_dir.join("policy.yaml"), "schema_version: \"1\"\n").expect("write");
        let policy = RepositoryPolicy::load(&root).expect("load minimal");
        assert_eq!(policy.schema_version, "1");
        assert!(policy.frontmatter.is_none());
        assert!(policy.coverage.is_none());
        assert!(policy.document_coverage.is_none());
        assert!(policy.source_links.is_none());
        assert!(policy.tags.is_none());
        assert!(policy.status.is_none());
        assert!(policy.public_boundary.is_none());

        // parse_from_string parity
        let policy2 =
            RepositoryPolicy::parse_from_string("schema_version: \"1\"\n").expect("parse str");
        assert_eq!(policy, policy2);

        let _ = fs::remove_dir_all(&root);
    }

    /// unsafe-path.yaml fixture content is valid and loads from a safe path.
    #[test]
    fn repository_policy_unsafe_fixture_content_loads() {
        let (root, bran_dir) = scratch_root("unsafe-content");
        let raw =
            std::fs::read_to_string(fixture_path("unsafe-path.yaml")).expect("read unsafe fixture");
        fs::write(bran_dir.join("policy.yaml"), &raw).expect("write");
        let policy = RepositoryPolicy::load(&root).expect("unsafe fixture content should be valid");
        assert_eq!(policy.schema_version, "1");
        let _ = fs::remove_dir_all(&root);
    }

    // ===================================================================
    // SEIT-2 / SEIT-12 / SEIT-16 negative cases
    // ===================================================================

    #[test]
    fn repository_policy_unsupported_version() {
        let (root, bran_dir) = scratch_root("version");
        fs::write(bran_dir.join("policy.yaml"), "schema_version: \"99\"\n").expect("write");
        let err = RepositoryPolicy::load(&root).unwrap_err();
        assert!(
            matches!(err, PolicyError::UnsupportedVersion { ref found } if found == "99"),
            "expected UnsupportedVersion, got {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repository_policy_missing_schema_version() {
        let (root, bran_dir) = scratch_root("missing");
        fs::write(
            bran_dir.join("policy.yaml"),
            "frontmatter:\n  required:\n    - type\n",
        )
        .expect("write");
        let err = RepositoryPolicy::load(&root).unwrap_err();
        assert!(
            matches!(err, PolicyError::MissingSchemaVersion),
            "expected MissingSchemaVersion, got {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repository_policy_unknown_field() {
        let (root, bran_dir) = scratch_root("unknown");
        fs::write(
            bran_dir.join("policy.yaml"),
            "schema_version: \"1\"\nwombat: true\n",
        )
        .expect("write");
        let err = RepositoryPolicy::load(&root).unwrap_err();
        assert!(
            matches!(err, PolicyError::UnknownField { ref field } if field == "wombat"),
            "expected UnknownField, got {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repository_policy_unknown_nested_field() {
        let (root, bran_dir) = scratch_root("nested-unknown");
        fs::write(
            bran_dir.join("policy.yaml"),
            "schema_version: \"1\"\nfrontmatter:\n  wombat: true\n",
        )
        .expect("write");
        let err = RepositoryPolicy::load(&root).unwrap_err();
        assert!(
            matches!(err, PolicyError::UnknownNestedField { ref parent, ref field }
                     if parent == "frontmatter" && field == "wombat"),
            "expected UnknownNestedField, got {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repository_policy_unknown_nested_in_document_coverage() {
        let (root, bran_dir) = scratch_root("dc-unknown");
        fs::write(
            bran_dir.join("policy.yaml"),
            "schema_version: \"1\"\ndocument_coverage:\n  roots:\n    - \".\"\n  bogus: x\n",
        )
        .expect("write");
        let err = RepositoryPolicy::load(&root).unwrap_err();
        assert!(
            matches!(err, PolicyError::UnknownNestedField { ref parent, ref field }
                     if parent == "document_coverage" && field == "bogus"),
            "expected UnknownNestedField in document_coverage, got {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repository_policy_malformed_yaml() {
        let (root, bran_dir) = scratch_root("malformed");
        fs::write(bran_dir.join("policy.yaml"), "= garbage\n").expect("write");
        let err = RepositoryPolicy::load(&root).unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml, got {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repository_policy_accepts_valid_flow_sequences() {
        let policy = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\ndocument_coverage:\n  roots: [docs, crates]\n  native_bundle: []\n",
        )
        .expect("flow sequences are valid YAML");
        let coverage = policy.document_coverage.unwrap();
        assert_eq!(coverage.roots, ["docs", "crates"]);
        assert!(coverage.native_bundle.is_empty());
    }

    #[test]
    fn repository_policy_oversized() {
        let err =
            RepositoryPolicy::parse_from_string(&"x".repeat(MAX_POLICY_BYTES + 1)).unwrap_err();
        assert!(
            matches!(err, PolicyError::Oversized { size, max } if size > max),
            "expected Oversized, got {err}"
        );
    }

    #[test]
    fn repository_policy_exactly_max_size_ok() {
        let raw = format!(
            "schema_version: \"1\"\n{}",
            "#".repeat(MAX_POLICY_BYTES - 20)
        );
        // The first line takes ~20 bytes; pad with comments to exactly max.
        assert!(raw.len() <= MAX_POLICY_BYTES);
        let policy = RepositoryPolicy::parse_from_string(&raw).expect("at max size");
        assert_eq!(policy.schema_version, "1");
    }

    #[test]
    fn repository_policy_invalid_migration_state() {
        let (root, bran_dir) = scratch_root("mig-state");
        fs::write(
            bran_dir.join("policy.yaml"),
            "schema_version: \"1\"\nfrontmatter:\n  canonical_docs_frontmatter_state: \"future\"\n",
        )
        .expect("write");
        let err = RepositoryPolicy::load(&root).unwrap_err();
        assert!(
            matches!(err, PolicyError::InvalidMigrationState),
            "expected InvalidMigrationState, got {err}"
        );
        // Non-echo: Display must not include the raw scalar value.
        let msg = err.to_string();
        assert!(
            !msg.contains("future"),
            "diagnostic must not echo raw value: {msg}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repository_policy_valid_migration_states() {
        for state in &["legacy_baseline", "strict"] {
            let (root, bran_dir) = scratch_root(&format!("mig-{state}"));
            fs::write(
                bran_dir.join("policy.yaml"),
                format!("schema_version: \"1\"\nfrontmatter:\n  canonical_docs_frontmatter_state: \"{state}\"\n"),
            )
            .expect("write");
            let policy = RepositoryPolicy::load(&root).expect("valid migration state");
            let fm = policy.frontmatter.as_ref().unwrap();
            assert_eq!(fm.canonical_docs_frontmatter_state.as_deref(), Some(*state));
            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn repository_policy_unsafe_document_path_absolute() {
        let (root, bran_dir) = scratch_root("abs-path");
        fs::write(
            bran_dir.join("policy.yaml"),
            "schema_version: \"1\"\ndocument_coverage:\n  roots:\n    - /etc/passwd\n",
        )
        .expect("write");
        let err = RepositoryPolicy::load(&root).unwrap_err();
        assert!(
            matches!(err, PolicyError::UnsafeDocumentPath),
            "expected UnsafeDocumentPath, got {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repository_policy_unsafe_document_path_traversal() {
        let (root, bran_dir) = scratch_root("dotdot-path");
        fs::write(
            bran_dir.join("policy.yaml"),
            "schema_version: \"1\"\ndocument_coverage:\n  native_bundle:\n    - ../escape.md\n",
        )
        .expect("write");
        let err = RepositoryPolicy::load(&root).unwrap_err();
        assert!(
            matches!(err, PolicyError::UnsafeDocumentPath),
            "expected UnsafeDocumentPath, got {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repository_policy_unsafe_document_path_empty() {
        let err = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\ndocument_coverage:\n  canonical_documents:\n    - \"\"\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::UnsafeDocumentPath),
            "expected UnsafeDocumentPath, got {err}"
        );
    }

    #[test]
    fn repository_policy_duplicate_path_in_list() {
        let (root, bran_dir) = scratch_root("dup-path");
        fs::write(
            bran_dir.join("policy.yaml"),
            "schema_version: \"1\"\ndocument_coverage:\n  native_bundle:\n    - a.md\n    - b.md\n    - a.md\n",
        )
        .expect("write");
        let err = RepositoryPolicy::load(&root).unwrap_err();
        assert!(
            matches!(err, PolicyError::DuplicateDocumentPath { .. }),
            "expected DuplicateDocumentPath, got {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repository_policy_duplicate_path_in_path_allowlist() {
        let err = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\npublic_boundary:\n  path_allowlist:\n    - x\n    - x\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::DuplicateDocumentPath { .. }),
            "expected DuplicateDocumentPath, got {err}"
        );
    }

    #[test]
    fn repository_policy_overlapping_classification() {
        let (root, bran_dir) = scratch_root("overlap");
        fs::write(
            bran_dir.join("policy.yaml"),
            "schema_version: \"1\"\ndocument_coverage:\n  canonical_documents:\n    - shared.md\n  excluded_documents:\n    shared.md: \"reason\"\n",
        )
        .expect("write");
        let err = RepositoryPolicy::load(&root).unwrap_err();
        assert!(
            matches!(err, PolicyError::OverlappingClassification { .. }),
            "expected OverlappingClassification, got {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repository_policy_empty_exclusion_reason() {
        let err = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\ndocument_coverage:\n  excluded_documents:\n    a.md: \"\"\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::EmptyExclusionReason),
            "expected EmptyExclusionReason, got {err}"
        );
    }

    #[test]
    fn repository_policy_roots_may_overlap_with_classified() {
        // Roots are scan roots; a path in roots may also be in native_bundle.
        let raw = "schema_version: \"1\"\ndocument_coverage:\n  roots:\n    - docs/bran/index.md\n  native_bundle:\n    - docs/bran/index.md\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("roots overlap allowed");
        assert!(policy.document_coverage.is_some());
    }

    // --- unsafe path (component-based) ---

    #[test]
    fn repository_policy_unsafe_path_traversal() {
        let root = Path::new("/tmp/../escape");
        let err = RepositoryPolicy::load(root).unwrap_err();
        assert!(
            matches!(err, PolicyError::UnsafePath { .. }),
            "expected UnsafePath, got {err}"
        );
    }

    #[test]
    fn repository_policy_unsafe_path_dotdot_component() {
        let root = Path::new("/home/user/repos/../../etc");
        let err = RepositoryPolicy::load(root).unwrap_err();
        assert!(
            matches!(err, PolicyError::UnsafePath { .. }),
            "expected UnsafePath, got {err}"
        );
    }

    #[test]
    fn repository_policy_unsafe_path_absolute_root_is_valid() {
        let (root, bran_dir) = scratch_root("absroot");
        fs::write(bran_dir.join("policy.yaml"), "schema_version: \"1\"\n").expect("write");
        let policy = RepositoryPolicy::load(&root).expect("absolute root should load");
        assert_eq!(policy.schema_version, "1");
        let _ = fs::remove_dir_all(&root);
    }

    // --- Legacy config must not become native authority (SEIT-12) ---

    #[test]
    fn repository_policy_rejects_legacy_as_native() {
        let (root, bran_dir) = scratch_root("legacy");
        fs::write(
            bran_dir.join("policy.yaml"),
            "okf_version: \"0.1\"\nvalidate:\n  strict: true\n",
        )
        .expect("write");
        let err = RepositoryPolicy::load(&root).unwrap_err();
        assert!(
            matches!(err, PolicyError::MissingSchemaVersion),
            "legacy config must not pass as native: got {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    // --- Deterministic parser ---

    #[test]
    fn repository_policy_deterministic_parse() {
        let raw = "schema_version: \"1\"\nfrontmatter:\n  required:\n    - type\n    - okf_status\n  allowed:\n    - tags\n";
        let a = parse_policy_yaml(raw).expect("parse A");
        let b = parse_policy_yaml(raw).expect("parse B");
        assert_eq!(a.map, b.map);

        let raw2 = "schema_version: \"1\"\nfrontmatter:\n  allowed:\n    - tags\n  required:\n    - okf_status\n    - type\n";
        let c = parse_policy_yaml(raw2).expect("parse C");
        assert_eq!(
            a.map.keys().collect::<Vec<_>>(),
            c.map.keys().collect::<Vec<_>>()
        );
    }

    // ===================================================================
    // Defect 1: duplicate key detection (top-level, nested, excluded_documents)
    // ===================================================================

    #[test]
    fn repository_policy_duplicate_top_level_key() {
        let err =
            RepositoryPolicy::parse_from_string("schema_version: \"1\"\nschema_version: \"1\"\n")
                .unwrap_err();
        assert!(
            matches!(err, PolicyError::DuplicateKey { .. }),
            "expected DuplicateKey, got {err}"
        );
    }

    #[test]
    fn repository_policy_duplicate_nested_key() {
        let err = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\nfrontmatter:\n  required:\n    - type\n  required:\n    - okf_status\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::DuplicateKey { .. }),
            "expected DuplicateKey, got {err}"
        );
    }

    #[test]
    fn repository_policy_duplicate_excluded_document_key() {
        let err = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\ndocument_coverage:\n  excluded_documents:\n    a.md: first\n    a.md: second\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::DuplicateKey { .. }),
            "expected DuplicateKey, got {err}"
        );
    }

    // ===================================================================
    // Defect 2: non-echo diagnostics (InvalidMigrationState, UnsafeDocumentPath)
    // ===================================================================

    #[test]
    fn repository_policy_unsafe_path_non_echo() {
        let err = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\ndocument_coverage:\n  roots:\n    - /etc/passwd\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::UnsafeDocumentPath),
            "expected UnsafeDocumentPath, got {err}"
        );
        let msg = err.to_string();
        assert!(
            !msg.contains("/etc/passwd"),
            "diagnostic must not echo unsafe path: {msg}"
        );
    }

    #[test]
    fn repository_policy_invalid_migration_state_non_echo_str() {
        let err = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\nfrontmatter:\n  canonical_docs_frontmatter_state: \"attack\"\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::InvalidMigrationState),
            "expected InvalidMigrationState, got {err}"
        );
        let msg = err.to_string();
        assert!(
            !msg.contains("attack"),
            "diagnostic must not echo raw value: {msg}"
        );
    }

    // ===================================================================
    // Defect 3: Windows/UNC/NUL path rejection
    // ===================================================================

    #[test]
    fn repository_policy_unsafe_document_path_windows_drive() {
        let err = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\ndocument_coverage:\n  native_bundle:\n    - C:\\\\windows\\\\system32\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::UnsafeDocumentPath),
            "expected UnsafeDocumentPath, got {err}"
        );
    }

    #[test]
    fn repository_policy_unsafe_document_path_windows_drive_forward() {
        let err = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\ndocument_coverage:\n  native_bundle:\n    - D:/data/file.md\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::UnsafeDocumentPath),
            "expected UnsafeDocumentPath, got {err}"
        );
    }

    #[test]
    fn repository_policy_unsafe_document_path_unc() {
        let err = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\ndocument_coverage:\n  native_bundle:\n    - \\\\\\\\server\\\\share\\\\file.md\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::UnsafeDocumentPath),
            "expected UnsafeDocumentPath, got {err}"
        );
    }

    #[test]
    fn repository_policy_unsafe_document_path_nul() {
        // NUL is a control char and is now caught in the quoted-scalar decoder
        // before reaching path validation (DEC-11: control chars rejected early).
        let err = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\ndocument_coverage:\n  native_bundle:\n    - \"file\0name.md\"\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "NUL in double-quoted scalar must produce MalformedYaml, got {err}"
        );
    }

    // ===================================================================
    // Defect 4: whitespace-only exclusion reasons rejected
    // ===================================================================

    #[test]
    fn repository_policy_whitespace_only_exclusion_reason() {
        let err = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\ndocument_coverage:\n  excluded_documents:\n    a.md: \"   \"\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::EmptyExclusionReason),
            "expected EmptyExclusionReason, got {err}"
        );
    }

    #[test]
    fn repository_policy_exclusion_reason_preserves_text() {
        // The original reason text is preserved even when it has surrounding whitespace.
        let raw = "schema_version: \"1\"\ndocument_coverage:\n  excluded_documents:\n    a.md: \" valid reason \"\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid reason");
        let dc = policy.document_coverage.as_ref().unwrap();
        let reason = dc.excluded_documents.get("a.md").unwrap();
        assert_eq!(reason, " valid reason ");
    }

    // ===================================================================
    // Defect 5: oversized file rejected via metadata before read
    // ===================================================================

    #[test]
    fn repository_policy_oversized_file_metadata() {
        let (root, bran_dir) = scratch_root("oversized-file");
        let content = "x".repeat(MAX_POLICY_BYTES + 1);
        fs::write(bran_dir.join("policy.yaml"), &content).expect("write oversized");
        let err = RepositoryPolicy::load(&root).unwrap_err();
        assert!(
            matches!(err, PolicyError::Oversized { .. }),
            "expected Oversized, got {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repository_policy_exact_max_file_loads() {
        let (root, bran_dir) = scratch_root("exact-max-file");
        let content = format!(
            "schema_version: \"1\"\n{}",
            "#".repeat(MAX_POLICY_BYTES - 20)
        );
        assert!(content.len() <= MAX_POLICY_BYTES);
        fs::write(bran_dir.join("policy.yaml"), &content).expect("write exact max");
        let policy = RepositoryPolicy::load(&root).expect("exact max file loads");
        assert_eq!(policy.schema_version, "1");
        let _ = fs::remove_dir_all(&root);
    }

    // ===================================================================
    // Defect 6: unmatched quote delimiters rejected
    // ===================================================================

    #[test]
    fn repository_policy_unmatched_double_quote() {
        let err = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\ncoverage:\n   - \"canonical\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml for unmatched double quote, got {err}"
        );
    }

    #[test]
    fn repository_policy_unmatched_single_quote() {
        let err = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\ncoverage:\n   - 'canonical",
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml for unmatched single quote, got {err}"
        );
    }

    #[test]
    fn repository_policy_trailing_quote_unmatched() {
        // Value that ends with a quote but didn't start with one: should parse as literal.
        let policy = RepositoryPolicy::parse_from_string(
            "schema_version: \"1\"\ndocument_coverage:\n  excluded_documents:\n    a.md: reason\"\n",
        )
        .expect("trailing quote in unquoted value");
        let dc = policy.document_coverage.as_ref().unwrap();
        let reason = dc.excluded_documents.get("a.md").unwrap();
        assert_eq!(reason, "reason\"");
    }

    #[test]
    fn repository_policy_malformed_yaml_non_echo() {
        let err = RepositoryPolicy::parse_from_string("= garbage secret\n").unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml, got {err}"
        );
        let msg = err.to_string();
        assert!(
            !msg.contains("garbage"),
            "MalformedYaml must not echo raw content: {msg}"
        );
        assert!(
            !msg.contains("secret"),
            "MalformedYaml must not echo raw content: {msg}"
        );
    }

    // ===================================================================
    // Slice 3.1-A: native quoted-scalar parser (SEIT-18, DES-11, CONTRACT-11)
    // ===================================================================

    // --- single-quoted decoding ---

    #[test]
    fn quoted_single_doubled_apostrophe() {
        let raw = "schema_version: '1'\ntags:\n  allowed:\n    - 'it''s a test'\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid");
        let tags = policy.tags.as_ref().expect("tags");
        assert_eq!(tags.allowed[0], "it's a test");
    }

    #[test]
    fn quoted_single_multiple_doubled_apostrophes() {
        let raw = "schema_version: '1'\ntags:\n  allowed:\n    - '''hello'''''\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid");
        let tags = policy.tags.as_ref().expect("tags");
        assert_eq!(tags.allowed[0], "'hello''");
    }

    #[test]
    fn quoted_single_no_apostrophe() {
        let raw = "schema_version: '1'\ntags:\n  allowed:\n    - 'plain text'\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid");
        let tags = policy.tags.as_ref().expect("tags");
        assert_eq!(tags.allowed[0], "plain text");
    }

    #[test]
    fn quoted_single_preserves_hash() {
        let raw = "schema_version: '1'\ntags:\n  allowed:\n    - '# not a comment'\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid");
        let tags = policy.tags.as_ref().expect("tags");
        assert_eq!(tags.allowed[0], "# not a comment");
    }

    #[test]
    fn quoted_single_preserves_colon_space() {
        let raw = "schema_version: '1'\ntags:\n  allowed:\n    - 'key: value'\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid");
        let tags = policy.tags.as_ref().expect("tags");
        assert_eq!(tags.allowed[0], "key: value");
    }

    #[test]
    fn quoted_single_preserves_surrounding_spaces() {
        let raw = "schema_version: '1'\ntags:\n  allowed:\n    - '  padded  '\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid");
        let tags = policy.tags.as_ref().expect("tags");
        assert_eq!(tags.allowed[0], "  padded  ");
    }

    // --- double-quoted decoding ---

    #[test]
    fn quoted_double_escaped_quote() {
        let raw = "schema_version: \"1\"\ntags:\n  allowed:\n    - \"she said \\\"hello\\\"\"\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid");
        let tags = policy.tags.as_ref().expect("tags");
        assert_eq!(tags.allowed[0], "she said \"hello\"");
    }

    #[test]
    fn quoted_double_escaped_backslash() {
        let raw = "schema_version: \"1\"\ntags:\n  allowed:\n    - \"a\\\\b\"\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid");
        let tags = policy.tags.as_ref().expect("tags");
        assert_eq!(tags.allowed[0], "a\\b");
    }

    #[test]
    fn quoted_double_preserves_hash() {
        let raw = "schema_version: \"1\"\ntags:\n  allowed:\n    - \"# not a comment\"\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid");
        let tags = policy.tags.as_ref().expect("tags");
        assert_eq!(tags.allowed[0], "# not a comment");
    }

    #[test]
    fn quoted_double_preserves_colon_space() {
        let raw = "schema_version: \"1\"\ntags:\n  allowed:\n    - \"key: value\"\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid");
        let tags = policy.tags.as_ref().expect("tags");
        assert_eq!(tags.allowed[0], "key: value");
    }

    #[test]
    fn quoted_double_preserves_surrounding_spaces() {
        let raw = "schema_version: \"1\"\ntags:\n  allowed:\n    - \"  padded  \"\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid");
        let tags = policy.tags.as_ref().expect("tags");
        assert_eq!(tags.allowed[0], "  padded  ");
    }

    // --- file/string equivalence (CONTRACT-11) ---

    #[test]
    fn quoted_file_and_string_equivalence() {
        let raw =
            "schema_version: \"1\"\ntags:\n  allowed:\n    - \"quoted 'val'\"\n    - 'it''s ok'\n";
        let policy_from_str = RepositoryPolicy::parse_from_string(raw).expect("string");

        let (root, bran_dir) = scratch_root("quote-equiv");
        fs::write(bran_dir.join("policy.yaml"), raw).expect("write");
        let policy_from_file = RepositoryPolicy::load(&root).expect("file");

        assert_eq!(policy_from_str, policy_from_file);
        let _ = fs::remove_dir_all(&root);
    }

    // --- rejection: unsupported escape ---

    #[test]
    fn quoted_double_unsupported_escape_n() {
        let raw = "schema_version: \"1\"\ncoverage:\n  - \"line\\nbreak\"\n";
        let err = RepositoryPolicy::parse_from_string(raw).unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml for \\n escape, got {err}"
        );
    }

    #[test]
    fn quoted_double_unsupported_escape_t() {
        let raw = "schema_version: \"1\"\ncoverage:\n  - \"tab\\tsep\"\n";
        let err = RepositoryPolicy::parse_from_string(raw).unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml for \\t escape, got {err}"
        );
    }

    #[test]
    fn quoted_double_unsupported_escape_u() {
        let raw = "schema_version: \"1\"\ncoverage:\n  - \"unicode\\u0041\"\n";
        let err = RepositoryPolicy::parse_from_string(raw).unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml for \\u escape, got {err}"
        );
    }

    // --- rejection: control character ---

    #[test]
    fn quoted_double_control_rejected() {
        // NUL byte inside double quotes
        let raw = "schema_version: \"1\"\ncoverage:\n  - \"before\x00after\"\n";
        let err = RepositoryPolicy::parse_from_string(raw).unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml for NUL, got {err}"
        );
    }

    #[test]
    fn quoted_single_control_rejected() {
        let raw = "schema_version: '1'\ncoverage:\n  - 'before\x01after'\n";
        let err = RepositoryPolicy::parse_from_string(raw).unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml for SOH, got {err}"
        );
    }

    // --- rejection: unmatched delimiters ---

    #[test]
    fn quoted_double_unmatched() {
        let raw = "schema_version: \"1\"\ncoverage:\n  - \"unclosed\n";
        let err = RepositoryPolicy::parse_from_string(raw).unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml for unmatched double quote, got {err}"
        );
    }

    #[test]
    fn quoted_single_unmatched() {
        let raw = "schema_version: '1'\ncoverage:\n  - 'unclosed\n";
        let err = RepositoryPolicy::parse_from_string(raw).unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml for unmatched single quote, got {err}"
        );
    }

    // --- non-echo: secret sentinel not leaked in diagnostics ---

    #[test]
    fn quoted_double_unsupported_escape_non_echo() {
        let raw = "schema_version: \"1\"\ncoverage:\n  - \"secret: \\sneaky\"\n";
        let err = RepositoryPolicy::parse_from_string(raw).unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml, got {err}"
        );
        let msg = err.to_string();
        assert!(
            !msg.contains("sneaky"),
            "diagnostic must not echo raw scalar content: {msg}"
        );
        assert!(
            !msg.contains("secret"),
            "diagnostic must not echo raw scalar content: {msg}"
        );
    }

    #[test]
    fn quoted_double_control_non_echo() {
        let raw = "schema_version: \"1\"\ncoverage:\n  - \"secret-sentinel\x1F-value\"\n";
        let err = RepositoryPolicy::parse_from_string(raw).unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("secret"),
            "diagnostic must not echo raw scalar: {msg}"
        );
        assert!(
            !msg.contains("sentinel"),
            "diagnostic must not echo raw scalar: {msg}"
        );
    }

    #[test]
    fn quoted_double_dangling_backslash() {
        // YAML content: "trailing\" — single backslash at end, no escape pair
        let raw = "schema_version: \"1\"\ncoverage:\n  - \"trailing\\\"\n";
        let err = RepositoryPolicy::parse_from_string(raw).unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml for dangling backslash, got {err}"
        );
    }

    // --- existing behavior preserved: unquoted scalars unchanged ---

    #[test]
    fn quoted_unquoted_literal_backslash_preserved() {
        let raw = "schema_version: \"1\"\ntags:\n  allowed:\n    - backslash\\\\path\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid");
        let tags = policy.tags.as_ref().expect("tags");
        assert_eq!(tags.allowed[0], "backslash\\\\path");
    }

    #[test]
    fn quoted_unquoted_hash_stripped_by_comment() {
        // Unquoted: # starts an inline comment, so "value # comment" → "value"
        let raw = "schema_version: \"1\"\ntags:\n  allowed:\n    - value # not kept\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid");
        let tags = policy.tags.as_ref().expect("tags");
        assert_eq!(tags.allowed[0], "value");
    }

    // --- full round-trip: all quote forms in a realistic policy ---

    #[test]
    fn quoted_full_roundtrip_all_forms() {
        let raw = "schema_version: '1'\nfrontmatter:\n  canonical_docs_frontmatter_state: \"legacy_baseline\"\ntags:\n  allowed:\n    - 'canonical'\n    - \"native\"\n    - unquoted\n";
        let policy = RepositoryPolicy::parse_from_string(raw).expect("valid");
        assert_eq!(policy.schema_version, "1");
        let fm = policy.frontmatter.as_ref().expect("frontmatter");
        assert_eq!(
            fm.canonical_docs_frontmatter_state.as_deref(),
            Some("legacy_baseline")
        );
        let tags = policy.tags.as_ref().expect("tags");
        assert_eq!(&tags.allowed[..], &["canonical", "native", "unquoted"]);
    }

    // --- deterministic re-parse with quoted scalars ---

    #[test]
    fn quoted_deterministic_reparse() {
        let raw = "schema_version: \"1\"\ndocument_coverage:\n  excluded_documents:\n    'it''s.md': \"escaped: \\\" value \"\n";
        let p1 = RepositoryPolicy::parse_from_string(raw).expect("first");
        let p2 = RepositoryPolicy::parse_from_string(raw).expect("second");
        assert_eq!(p1, p2);
    }

    // --- AC-1: isolated apostrophe rejected (was silently accepted via replace) ---

    #[test]
    fn quoted_single_isolated_apostrophe_rejected() {
        let raw = "schema_version: '1'\ncoverage:\n  - 'it's bad'\n";
        let err = RepositoryPolicy::parse_from_string(raw).unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml for isolated apostrophe, got {err}"
        );
    }

    #[test]
    fn quoted_single_isolated_apostrophe_non_echo() {
        let raw = "schema_version: '1'\ncoverage:\n  - 'secret-sentinel's value'\n";
        let err = RepositoryPolicy::parse_from_string(raw).unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("secret"),
            "diagnostic must not echo raw scalar: {msg}"
        );
        assert!(
            !msg.contains("sentinel"),
            "diagnostic must not echo raw scalar: {msg}"
        );
    }

    // --- AC-7: unescaped inner double quote rejected (was silently accepted) ---

    #[test]
    fn quoted_double_unescaped_inner_quote_rejected() {
        let raw = "schema_version: \"1\"\ncoverage:\n  - \"a\"b\"\n";
        let err = RepositoryPolicy::parse_from_string(raw).unwrap_err();
        assert!(
            matches!(err, PolicyError::MalformedYaml { .. }),
            "expected MalformedYaml for unescaped inner quote, got {err}"
        );
    }

    #[test]
    fn quoted_double_unescaped_inner_quote_non_echo() {
        let raw = "schema_version: \"1\"\ncoverage:\n  - \"secret-sentinel\"leak\"\n";
        let err = RepositoryPolicy::parse_from_string(raw).unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("secret"),
            "diagnostic must not echo raw scalar: {msg}"
        );
        assert!(
            !msg.contains("sentinel"),
            "diagnostic must not echo raw scalar: {msg}"
        );
    }
}
