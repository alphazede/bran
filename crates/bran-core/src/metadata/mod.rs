//! Read-only, bounded metadata normalization for repository scanning.

use std::collections::{BTreeMap, BTreeSet};

use crate::graph::model::{StructuralEdgeKind, StructuralSourceType};

pub const DEFAULT_MAX_INPUT_BYTES: usize = 256 * 1024;
type Fields = Vec<(String, String)>;
type ParsedHeader = Option<(Fields, FactProvenance)>;
const MAX_GIT_FACT_BYTES: usize = 256;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PackageDefaults(BTreeMap<String, String>);

impl PackageDefaults {
    pub fn new(facts: impl IntoIterator<Item = (String, String)>) -> Self {
        Self(facts.into_iter().collect())
    }

    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.0.insert(key.into(), value.into());
    }

    pub(crate) fn overlay(&mut self, other: &Self) {
        self.0.extend(other.0.clone());
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FactProvenance {
    MarkdownFrontmatter,
    CommentedYaml,
    Language,
    GitState,
    PackageDefault,
    Filename,
    Symbol,
    Import,
    Test,
    Dependency,
    BoundaryClassification,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FactConfidence {
    Low,
    Medium,
    High,
}

/// Facts are candidates or explicitly ambiguous; they are never verified here.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FactState {
    Candidate,
    Ambiguous,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MetadataFact {
    pub key: String,
    pub value: String,
    pub provenance: FactProvenance,
    pub confidence: FactConfidence,
    pub state: FactState,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MetadataReport {
    pub facts: Vec<MetadataFact>,
    pub warnings: Vec<String>,
    pub proposals: Vec<String>,
}

/// A repository-state fact supplied by the scanner's caller.
///
/// Metadata parsing never reads Git state itself. Keeping this seam explicit
/// lets an integration attach an already-verified state without granting this
/// read-only parser filesystem or process access.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitStateFact {
    pub key: String,
    pub value: String,
    pub confidence: FactConfidence,
}

/// One frozen parser selected by the structural boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParserDescriptor {
    source_type: StructuralSourceType,
    version: &'static str,
}

impl ParserDescriptor {
    fn new(source_type: StructuralSourceType) -> Self {
        Self {
            source_type,
            version: "v1",
        }
    }
    pub const fn source_type(&self) -> StructuralSourceType {
        self.source_type
    }
    pub const fn version(&self) -> &'static str {
        self.version
    }
}

/// Candidate receipt status; accepted inputs are never graph truth.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuralParseStatus {
    Accepted,
    Rejected,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StructuralFact {
    key: String,
    value: String,
    locator: String,
}

impl StructuralFact {
    fn new(key: impl Into<String>, value: impl Into<String>, locator: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            locator: locator.into(),
        }
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn value(&self) -> &str {
        &self.value
    }
    pub fn locator(&self) -> &str {
        &self.locator
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StructuralEdgeCandidate {
    kind: StructuralEdgeKind,
    target: String,
    locator: String,
}

impl StructuralEdgeCandidate {
    fn new(
        kind: StructuralEdgeKind,
        target: impl Into<String>,
        locator: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            target: target.into(),
            locator: locator.into(),
        }
    }
    pub const fn kind(&self) -> StructuralEdgeKind {
        self.kind
    }
    pub fn target(&self) -> &str {
        &self.target
    }
    pub fn locator(&self) -> &str {
        &self.locator
    }
}

/// Caller-supplied evidence binding extracted UTF-8 to its original artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreextractedTextProvenance {
    original_artifact_digest: String,
    extractor_identity: String,
    extractor_version: String,
    extraction_digest: String,
}

impl PreextractedTextProvenance {
    pub fn new(
        original_artifact_digest: impl Into<String>,
        extractor_identity: impl Into<String>,
        extractor_version: impl Into<String>,
        extraction_digest: impl Into<String>,
    ) -> Result<Self, String> {
        let value = Self {
            original_artifact_digest: original_artifact_digest.into(),
            extractor_identity: extractor_identity.into(),
            extractor_version: extractor_version.into(),
            extraction_digest: extraction_digest.into(),
        };
        if !digest_is_valid(&value.original_artifact_digest)
            || !digest_is_valid(&value.extraction_digest)
            || value.extractor_identity.trim().is_empty()
            || value.extractor_version.trim().is_empty()
        {
            return Err("invalid-preextracted-provenance".to_owned());
        }
        Ok(value)
    }
    pub fn original_artifact_digest(&self) -> &str {
        &self.original_artifact_digest
    }
    pub fn extractor_identity(&self) -> &str {
        &self.extractor_identity
    }
    pub fn extractor_version(&self) -> &str {
        &self.extractor_version
    }
    pub fn extraction_digest(&self) -> &str {
        &self.extraction_digest
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralParseResult {
    status: StructuralParseStatus,
    source_type: Option<StructuralSourceType>,
    facts: Vec<StructuralFact>,
    edges: Vec<StructuralEdgeCandidate>,
    diagnostics: Vec<String>,
    provenance: Option<PreextractedTextProvenance>,
}

impl StructuralParseResult {
    pub const fn status(&self) -> StructuralParseStatus {
        self.status
    }
    pub const fn source_type(&self) -> Option<StructuralSourceType> {
        self.source_type
    }
    pub fn facts(&self) -> &[StructuralFact] {
        &self.facts
    }
    pub fn edges(&self) -> &[StructuralEdgeCandidate] {
        &self.edges
    }
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
    pub fn provenance(&self) -> Option<&PreextractedTextProvenance> {
        self.provenance.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataParserRegistry {
    max_input_bytes: usize,
    structural_parsers: BTreeMap<StructuralSourceType, ParserDescriptor>,
}

impl Default for MetadataParserRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_INPUT_BYTES)
    }
}

impl MetadataParserRegistry {
    pub fn new(max_input_bytes: usize) -> Self {
        let structural_parsers = [
            StructuralSourceType::Text,
            StructuralSourceType::Code,
            StructuralSourceType::Markdown,
            StructuralSourceType::Mermaid,
            StructuralSourceType::Metadata,
            StructuralSourceType::PreextractedText,
        ]
        .into_iter()
        .map(|source_type| (source_type, ParserDescriptor::new(source_type)))
        .collect();
        Self {
            max_input_bytes,
            structural_parsers,
        }
    }

    pub(crate) fn fingerprint(&self) -> String {
        format!(
            "source-metadata-parser-v2:max-input-bytes={}",
            self.max_input_bytes
        )
    }

    pub fn structural_parsers(&self) -> &BTreeMap<StructuralSourceType, ParserDescriptor> {
        &self.structural_parsers
    }

    pub fn structural_fingerprint(&self) -> String {
        let descriptors = self
            .structural_parsers
            .values()
            .map(|descriptor| {
                format!(
                    "{}@{}",
                    descriptor.source_type().as_str(),
                    descriptor.version()
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "structural-metadata-parser-v1:max-input-bytes={}:{}",
            self.max_input_bytes, descriptors
        )
    }

    /// Parses supplied bytes only; the result is deterministic candidate evidence.
    pub fn parse_structural(
        &self,
        path: &str,
        bytes: &[u8],
        extraction: Option<PreextractedTextProvenance>,
    ) -> StructuralParseResult {
        let mut diagnostics = BTreeSet::new();
        if path.is_empty()
            || path.starts_with('/')
            || path.contains('\\')
            || path
                .split('/')
                .any(|part| part.is_empty() || matches!(part, "." | ".."))
        {
            diagnostics.insert("invalid-path".to_owned());
            return structural_rejected(diagnostics);
        }
        if bytes.len() > self.max_input_bytes {
            diagnostics.insert(format!(
                "input-too-large: {} bytes exceeds {} byte limit",
                bytes.len(),
                self.max_input_bytes
            ));
            return structural_rejected(diagnostics);
        }
        let source = match std::str::from_utf8(bytes) {
            Ok(source) => source,
            Err(_) => {
                diagnostics.insert("invalid-utf8".to_owned());
                return structural_rejected(diagnostics);
            }
        };
        let source_type = if let Some(provenance) = extraction.as_ref() {
            if provenance.extraction_digest() != digest(bytes) {
                diagnostics.insert("extraction-digest-mismatch".to_owned());
                return structural_rejected(diagnostics);
            }
            let _ = provenance;
            StructuralSourceType::PreextractedText
        } else if let Some(source_type) = structural_source_type(path) {
            source_type
        } else {
            diagnostics.insert(format!(
                "unsupported-source: {}",
                structural_extension(path)
            ));
            return structural_rejected(diagnostics);
        };
        let defaults = PackageDefaults::default();
        let report = self.parse(path, source, &defaults);
        if source_type == StructuralSourceType::Metadata
            && report
                .warnings
                .iter()
                .any(|warning| warning.starts_with("malformed-metadata:"))
        {
            diagnostics.extend(report.warnings);
            return structural_rejected(diagnostics);
        }
        let bare = if source_type == StructuralSourceType::Metadata {
            match bare_metadata(source) {
                Ok(fields) => fields,
                Err(reason) => {
                    diagnostics.insert(format!("malformed-metadata: {reason}"));
                    return structural_rejected(diagnostics);
                }
            }
        } else {
            Vec::new()
        };
        diagnostics.extend(report.warnings);
        let mut facts = BTreeSet::new();
        let mut edges = BTreeSet::new();
        for fact in report.facts {
            let locator = fact_locator(path, source, &fact.key, &fact.value);
            if let Some(kind) = relationship_kind(&fact.key) {
                edges.insert(StructuralEdgeCandidate::new(
                    kind,
                    fact.value.clone(),
                    locator.clone(),
                ));
            }
            facts.insert(StructuralFact::new(fact.key, fact.value, locator));
        }
        for (key, value, line) in bare {
            let locator = format!("{path}:{line}");
            if let Some(kind) = relationship_kind(&key) {
                edges.insert(StructuralEdgeCandidate::new(
                    kind,
                    value.clone(),
                    locator.clone(),
                ));
            }
            facts.insert(StructuralFact::new(key, value, locator));
        }
        match source_type {
            StructuralSourceType::Markdown => {
                markdown_structural(path, source, &mut facts, &mut edges)
            }
            StructuralSourceType::Mermaid => {
                mermaid_structural(path, source, &mut facts, &mut edges)
            }
            StructuralSourceType::PreextractedText => {
                let provenance = extraction.as_ref().expect("checked above");
                facts.insert(StructuralFact::new(
                    "original_artifact_digest",
                    provenance.original_artifact_digest(),
                    format!("{path}:provenance:original-artifact"),
                ));
                facts.insert(StructuralFact::new(
                    "extractor",
                    format!(
                        "{}@{}",
                        provenance.extractor_identity(),
                        provenance.extractor_version()
                    ),
                    format!("{path}:provenance:extractor"),
                ));
                facts.insert(StructuralFact::new(
                    "extraction_digest",
                    provenance.extraction_digest(),
                    format!("{path}:provenance:extraction"),
                ));
            }
            _ => {}
        }
        StructuralParseResult {
            status: StructuralParseStatus::Accepted,
            source_type: Some(source_type),
            facts: facts.into_iter().collect(),
            edges: edges.into_iter().collect(),
            diagnostics: diagnostics.into_iter().collect(),
            provenance: extraction,
        }
    }

    /// Parses only supplied text; it never reads or writes repository files.
    pub fn parse(&self, path: &str, source: &str, defaults: &PackageDefaults) -> MetadataReport {
        self.parse_with_git_state(path, source, defaults, None)
    }

    /// Parses supplied text with an optional caller-supplied repository-state
    /// fact; it never reads Git state or repository files.
    pub fn parse_with_git_state(
        &self,
        path: &str,
        source: &str,
        defaults: &PackageDefaults,
        git_state: Option<&GitStateFact>,
    ) -> MetadataReport {
        if source.len() > self.max_input_bytes {
            return MetadataReport {
                warnings: vec![format!(
                    "input-too-large: {} bytes exceeds {} byte limit",
                    source.len(),
                    self.max_input_bytes
                )],
                ..MetadataReport::default()
            };
        }

        let mut warnings = BTreeSet::new();
        let declared = match headers(path, source) {
            Ok(value) => value,
            Err(reason) => {
                warnings.insert(format!("malformed-metadata: {reason}"));
                None
            }
        };
        let declared_keys: BTreeSet<String> = declared
            .as_ref()
            .map(|(fields, _)| fields.iter().map(|(key, _)| key.clone()).collect())
            .unwrap_or_default();
        let mut facts = BTreeSet::new();
        for (key, value) in &defaults.0 {
            if !declared_keys.contains(key) {
                facts.insert(new_fact(
                    key,
                    value,
                    FactProvenance::PackageDefault,
                    FactConfidence::Medium,
                ));
            }
        }
        if let Some((fields, provenance)) = declared {
            for (key, value) in fields {
                facts.insert(new_fact(
                    key,
                    value,
                    provenance.clone(),
                    FactConfidence::High,
                ));
            }
        } else {
            warnings
                .insert("metadata-on-touch: proposal only; source remains unchanged".to_owned());
        }
        if let Some(language) = language(path) {
            facts.insert(new_fact(
                "language",
                language,
                FactProvenance::Language,
                FactConfidence::High,
            ));
        }
        if let Some(git_state) = git_state {
            if valid_key(&git_state.key)
                && !git_state.value.is_empty()
                && git_state.key.len() + git_state.value.len() <= MAX_GIT_FACT_BYTES
            {
                facts.insert(new_fact(
                    &git_state.key,
                    &git_state.value,
                    FactProvenance::GitState,
                    git_state.confidence,
                ));
            } else {
                warnings.insert("invalid-git-state-fact: caller value ignored".to_owned());
            }
        }
        infer(path, source, &mut facts);
        boundary(path, source, &mut facts);
        resolve(&mut facts, &mut warnings);
        MetadataReport {
            facts: facts.into_iter().collect(),
            warnings: warnings.into_iter().collect(),
            proposals: declared_keys
                .is_empty()
                .then(|| proposal(path))
                .flatten()
                .into_iter()
                .collect(),
        }
    }
}

fn structural_rejected(diagnostics: BTreeSet<String>) -> StructuralParseResult {
    StructuralParseResult {
        status: StructuralParseStatus::Rejected,
        source_type: None,
        facts: Vec::new(),
        edges: Vec::new(),
        diagnostics: diagnostics.into_iter().collect(),
        provenance: None,
    }
}

fn digest(bytes: &[u8]) -> String {
    crate::agent::result_store::ResultId::sha256(bytes)
        .value()
        .to_owned()
}

fn digest_is_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn structural_extension(path: &str) -> String {
    path.rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn structural_source_type(path: &str) -> Option<StructuralSourceType> {
    match structural_extension(path).as_str() {
        "txt" | "text" | "rst" => Some(StructuralSourceType::Text),
        "rs" | "java" | "js" | "jsx" | "ts" | "tsx" | "go" | "cs" | "c" | "cc" | "cpp" | "cxx"
        | "h" | "hh" | "hpp" | "py" | "rb" | "sh" | "bash" | "zsh" => {
            Some(StructuralSourceType::Code)
        }
        "md" | "markdown" => Some(StructuralSourceType::Markdown),
        "mmd" | "mermaid" => Some(StructuralSourceType::Mermaid),
        "yaml" | "yml" | "toml" | "ini" | "cfg" => Some(StructuralSourceType::Metadata),
        _ => None,
    }
}

fn fact_locator(path: &str, source: &str, key: &str, value: &str) -> String {
    let line = source
        .lines()
        .position(|line| line.contains(key) && line.contains(value))
        .map(|line| line + 1);
    line.map(|line| format!("{path}:{line}"))
        .unwrap_or_else(|| format!("{path}:inferred:{key}"))
}

fn relationship_kind(key: &str) -> Option<StructuralEdgeKind> {
    Some(match key {
        "ownership" => StructuralEdgeKind::Ownership,
        "supersedes" => StructuralEdgeKind::Supersedes,
        "predecessor" => StructuralEdgeKind::Predecessor,
        "dependency" => StructuralEdgeKind::Dependency,
        "impact" => StructuralEdgeKind::Impact,
        "requirement" => StructuralEdgeKind::Requirement,
        "design" => StructuralEdgeKind::Design,
        "implementation" => StructuralEdgeKind::Implementation,
        "validation" => StructuralEdgeKind::Validation,
        "domain" => StructuralEdgeKind::Domain,
        "architecture" => StructuralEdgeKind::Architecture,
        "business_flow" => StructuralEdgeKind::BusinessFlow,
        "generated_from" => StructuralEdgeKind::GeneratedFrom,
        "source_link" => StructuralEdgeKind::SourceLink,
        _ => return None,
    })
}

fn markdown_structural(
    path: &str,
    source: &str,
    facts: &mut BTreeSet<StructuralFact>,
    edges: &mut BTreeSet<StructuralEdgeCandidate>,
) {
    for (index, raw) in source.lines().enumerate() {
        let line = raw.trim();
        let locator = format!("{path}:{}", index + 1);
        let marker_end = line.bytes().take_while(|byte| *byte == b'#').count();
        let heading = &line[marker_end..];
        if marker_end > 0
            && heading.starts_with(|character: char| character.is_ascii_whitespace())
            && !heading.trim_start().is_empty()
        {
            facts.insert(StructuralFact::new(
                "heading",
                heading.trim_start(),
                locator.clone(),
            ));
        }
        let mut rest = line;
        while let Some(start) = rest.find("](") {
            let Some(end) = rest[start + 2..].find(')') else {
                break;
            };
            let target = &rest[start + 2..start + 2 + end];
            if !target.is_empty()
                && !target.starts_with('#')
                && !target.contains("://")
                && !target.starts_with("mailto:")
            {
                edges.insert(StructuralEdgeCandidate::new(
                    StructuralEdgeKind::SourceLink,
                    target,
                    locator.clone(),
                ));
            }
            rest = &rest[start + 2 + end + 1..];
        }
    }
}

fn mermaid_structural(
    path: &str,
    source: &str,
    facts: &mut BTreeSet<StructuralFact>,
    edges: &mut BTreeSet<StructuralEdgeCandidate>,
) {
    for (index, raw) in source.lines().enumerate() {
        let line = raw.trim();
        let locator = format!("{path}:{}", index + 1);
        if let Some((left, right)) = line.split_once("-->") {
            let left = left.trim();
            let right = right.trim();
            if simple_mermaid_id(left) && simple_mermaid_id(right) {
                facts.insert(StructuralFact::new("declaration", left, locator.clone()));
                facts.insert(StructuralFact::new("declaration", right, locator.clone()));
                edges.insert(StructuralEdgeCandidate::new(
                    StructuralEdgeKind::Dependency,
                    right,
                    locator,
                ));
            }
        }
    }
}

fn simple_mermaid_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn bare_metadata(source: &str) -> Result<Vec<(String, String, usize)>, String> {
    let mut fields = Vec::new();
    for (index, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line == "---" || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=').or_else(|| line.split_once(':')) else {
            return Err(format!("not key/value metadata: {line}"));
        };
        let key = key.trim();
        let value = value.trim();
        if !valid_key(key) || value.is_empty() || value.starts_with(['[', '{', '|', '>']) {
            return Err(format!("unsupported metadata scalar: {line}"));
        }
        fields.push((key.to_owned(), scalar(value)?, index + 1));
    }
    Ok(fields)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeaderSyntax {
    Markdown,
    SlashComment,
    CLikeComment,
    HashComment,
}

fn headers(path: &str, source: &str) -> Result<ParsedHeader, String> {
    match syntax_for(path) {
        Some(HeaderSyntax::Markdown) => markdown_header(source),
        Some(HeaderSyntax::SlashComment) => line_comment_header(source, "//"),
        Some(HeaderSyntax::CLikeComment) => {
            if source.trim_start().starts_with("/*") {
                block_comment_header(source)
            } else {
                line_comment_header(source, "//")
            }
        }
        Some(HeaderSyntax::HashComment) => line_comment_header(source, "#"),
        None => Ok(None),
    }
}

fn markdown_header(source: &str) -> Result<ParsedHeader, String> {
    let mut lines = source.lines();
    let first = lines
        .next()
        .unwrap_or("")
        .trim_start_matches('\u{feff}')
        .trim();
    if first != "---" {
        return Ok(None);
    }
    let mut body = String::new();
    for line in lines {
        if line.trim() == "---" {
            return yaml(&body).map(|fields| Some((fields, FactProvenance::MarkdownFrontmatter)));
        }
        body.push_str(line);
        body.push('\n');
    }
    Err("frontmatter is missing its closing delimiter".to_owned())
}

fn line_comment_header(source: &str, prefix: &str) -> Result<ParsedHeader, String> {
    let mut body = String::new();
    let mut open = false;
    for line in source.lines() {
        let Some(line) = line.trim_start().strip_prefix(prefix) else {
            break;
        };
        let line = line.strip_prefix(['/', '!']).unwrap_or(line);
        let line = line.strip_prefix(' ').unwrap_or(line);
        if line.trim() == "---" {
            if open {
                return yaml(&body).map(|fields| Some((fields, FactProvenance::CommentedYaml)));
            }
            open = true;
        } else if open {
            body.push_str(line);
            body.push('\n');
        }
    }
    if open {
        Err("commented YAML header is missing its closing delimiter".to_owned())
    } else {
        Ok(None)
    }
}

fn block_comment_header(source: &str) -> Result<ParsedHeader, String> {
    let mut lines = source.lines();
    let Some(first) = lines.next() else {
        return Ok(None);
    };
    let Some(first) = first.trim_start().strip_prefix("/*") else {
        return Ok(None);
    };
    if first.trim() != "---" {
        return Ok(None);
    }
    let mut body = String::new();
    while let Some(raw) = lines.next() {
        let line = raw.trim_start();
        if line == "*/" {
            return Err("commented YAML header is missing its closing delimiter".to_owned());
        }
        let Some(line) = line.strip_prefix('*') else {
            return Err("block YAML header line must begin with '*'".to_owned());
        };
        let line = line.strip_prefix(' ').unwrap_or(line);
        if line == "---" {
            let Some(close) = lines.next() else {
                return Err("block YAML header is missing its closing comment".to_owned());
            };
            if close.trim() != "*/" {
                return Err("block YAML header must close before source".to_owned());
            }
            return yaml(&body).map(|fields| Some((fields, FactProvenance::CommentedYaml)));
        }
        body.push_str(line);
        body.push('\n');
    }
    Err("commented YAML header is missing its closing delimiter".to_owned())
}

fn syntax_for(path: &str) -> Option<HeaderSyntax> {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())?;
    match extension.as_str() {
        "md" | "markdown" => Some(HeaderSyntax::Markdown),
        "rs" | "java" | "js" | "jsx" | "ts" | "tsx" | "go" | "cs" => {
            Some(HeaderSyntax::SlashComment)
        }
        "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" => Some(HeaderSyntax::CLikeComment),
        "py" | "rb" | "sh" | "bash" | "zsh" | "yaml" | "yml" | "toml" | "ini" | "cfg" => {
            Some(HeaderSyntax::HashComment)
        }
        _ => None,
    }
}

fn language(path: &str) -> Option<&'static str> {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())?;
    match extension.as_str() {
        "md" | "markdown" => Some("markdown"),
        "rs" => Some("rust"),
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hh" | "hpp" => Some("cpp"),
        "java" => Some("java"),
        "js" | "jsx" => Some("javascript"),
        "ts" | "tsx" => Some("typescript"),
        "go" => Some("go"),
        "cs" => Some("csharp"),
        "py" => Some("python"),
        "rb" => Some("ruby"),
        "sh" | "bash" | "zsh" => Some("shell"),
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "ini" | "cfg" => Some("ini"),
        _ => None,
    }
}

fn yaml(body: &str) -> Result<Fields, String> {
    let mut fields = Vec::new();
    let mut block_list: Option<(String, usize, bool)> = None;
    for raw in body.lines() {
        let indent = raw.len() - raw.trim_start().len();
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(value) = line.strip_prefix("- ") {
            let Some((key, key_indent, seen)) = block_list.as_mut() else {
                return Err(format!("YAML list item has no key: {line}"));
            };
            if indent <= *key_indent {
                return Err(format!("YAML list item is not indented: {line}"));
            }
            fields.push((key.clone(), scalar(value)?));
            *seen = true;
            continue;
        }
        if block_list.take().is_some_and(|(_, _, seen)| !seen) {
            return Err(format!("unsupported empty YAML value: {line}"));
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("not key/value YAML: {line}"));
        };
        let key = key.trim();
        let value = value.trim();
        if !valid_key(key)
            || value.starts_with('{')
            || value.starts_with('|')
            || value.starts_with('>')
        {
            return Err(format!("unsupported YAML scalar: {line}"));
        }
        if value.is_empty() {
            block_list = Some((key.to_owned(), indent, false));
            continue;
        }
        if value.starts_with('[') != value.ends_with(']') {
            return Err(format!("malformed YAML list: {line}"));
        }
        if let Some(values) = value
            .strip_prefix('[')
            .and_then(|item| item.strip_suffix(']'))
        {
            for value in values.split(',') {
                fields.push((key.to_owned(), scalar(value)?));
            }
        } else {
            fields.push((key.to_owned(), scalar(value)?));
        }
    }
    if block_list.is_some_and(|(_, _, seen)| !seen) {
        return Err("unsupported empty YAML value at end of header".to_owned());
    }
    Ok(fields)
}

fn scalar(value: &str) -> Result<String, String> {
    let value = value.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|item| item.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|item| item.strip_suffix('\''))
        })
        .unwrap_or(value)
        .trim();
    if value.is_empty() {
        Err("empty YAML scalar".to_owned())
    } else {
        Ok(value.to_owned())
    }
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn infer(path: &str, source: &str, facts: &mut BTreeSet<MetadataFact>) {
    if let Some(name) = path
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
    {
        facts.insert(new_fact(
            "filename",
            name,
            FactProvenance::Filename,
            FactConfidence::High,
        ));
    }
    let mut rust_test = false;
    let mut dependencies = false;
    for line in source.lines() {
        let line = line.trim();
        if path.ends_with("Cargo.toml") && line.starts_with('[') && line.ends_with(']') {
            dependencies = matches!(
                line,
                "[dependencies]" | "[dev-dependencies]" | "[build-dependencies]"
            );
            continue;
        }
        if dependencies && path.ends_with("Cargo.toml") {
            if let Some((name, _)) = line.split_once('=') {
                let name = name.trim();
                if valid_key(name) {
                    facts.insert(new_fact(
                        "dependency",
                        name,
                        FactProvenance::Dependency,
                        FactConfidence::Medium,
                    ));
                }
            }
        }
        if line == "#[test]" {
            rust_test = true;
            continue;
        }
        if let Some(name) = symbol(line) {
            facts.insert(new_fact(
                "symbol",
                name,
                FactProvenance::Symbol,
                FactConfidence::Medium,
            ));
            if rust_test {
                facts.insert(new_fact(
                    "test",
                    name,
                    FactProvenance::Test,
                    FactConfidence::High,
                ));
            }
        }
        rust_test = false;
        if let Some(name) = imported(line) {
            facts.insert(new_fact(
                "import",
                name,
                FactProvenance::Import,
                FactConfidence::Low,
            ));
        }
        if let Some(name) = line
            .strip_prefix("def test_")
            .map(identifier)
            .filter(|name| !name.is_empty())
        {
            facts.insert(new_fact(
                "test",
                name,
                FactProvenance::Test,
                FactConfidence::High,
            ));
        }
    }
}

fn symbol(line: &str) -> Option<&str> {
    let line = line.strip_prefix("pub ").unwrap_or(line);
    [
        "fn ",
        "struct ",
        "enum ",
        "trait ",
        "class ",
        "def ",
        "function ",
    ]
    .iter()
    .find_map(|prefix| line.strip_prefix(prefix).map(identifier))
    .filter(|name| !name.is_empty())
}

fn imported(line: &str) -> Option<&str> {
    let value = line
        .strip_prefix("use ")
        .map(|item| item.trim_end_matches(';'))
        .or_else(|| line.strip_prefix("import "))
        .or_else(|| line.strip_prefix("from "))
        .or_else(|| {
            line.strip_prefix("require(").map(|item| {
                item.trim_start_matches(['\'', '"'])
                    .split(['\'', '"'])
                    .next()
                    .unwrap_or("")
            })
        })?;
    value
        .split_whitespace()
        .next()
        .filter(|value| !value.is_empty())
}

fn identifier(value: &str) -> &str {
    let end = value
        .bytes()
        .position(|byte| !(byte.is_ascii_alphanumeric() || byte == b'_'))
        .unwrap_or(value.len());
    &value[..end]
}

fn boundary(path: &str, source: &str, facts: &mut BTreeSet<MetadataFact>) {
    let path = path.to_ascii_lowercase();
    if [
        ".github/",
        "auth",
        "ci/",
        "credential",
        "deploy",
        "infra/",
        "policy",
        "secret",
        "security",
        "workflow",
        "cargo.toml",
    ]
    .iter()
    .any(|marker| path.contains(marker))
        || source.contains("BEGIN PRIVATE KEY")
    {
        facts.insert(new_fact(
            "important_boundary",
            "true",
            FactProvenance::BoundaryClassification,
            FactConfidence::Medium,
        ));
    }
}

fn resolve(facts: &mut BTreeSet<MetadataFact>, warnings: &mut BTreeSet<String>) {
    let mut values = BTreeMap::<String, BTreeSet<String>>::new();
    let mut declared = BTreeSet::new();
    for fact in facts.iter() {
        if !multiple(&fact.key) {
            values
                .entry(fact.key.clone())
                .or_default()
                .insert(fact.value.clone());
            if matches!(
                fact.provenance,
                FactProvenance::MarkdownFrontmatter | FactProvenance::CommentedYaml
            ) {
                declared.insert(fact.key.clone());
            }
        }
    }
    let conflicts: BTreeSet<String> = values
        .into_iter()
        .filter_map(|(key, values)| (values.len() > 1 && declared.contains(&key)).then_some(key))
        .collect();
    if conflicts.is_empty() {
        return;
    }
    let mut normalized = BTreeSet::new();
    for mut fact in std::mem::take(facts) {
        if conflicts.contains(&fact.key) {
            fact.state = FactState::Ambiguous;
        }
        normalized.insert(fact);
    }
    for key in conflicts {
        warnings.insert(format!("ambiguous-fact: conflicting declared or inferred singleton {key}; no value is verified"));
    }
    *facts = normalized;
}

fn multiple(key: &str) -> bool {
    matches!(
        key,
        "tags"
            | "symbol"
            | "import"
            | "test"
            | "dependency"
            | "implementation"
            | "replacement"
            | "supersedes"
            | "validation"
            | "reachability"
            | "contradiction"
            | "conflict"
    )
}

fn new_fact(
    key: impl Into<String>,
    value: impl Into<String>,
    provenance: FactProvenance,
    confidence: FactConfidence,
) -> MetadataFact {
    MetadataFact {
        key: key.into(),
        value: value.into(),
        provenance,
        confidence,
        state: FactState::Candidate,
    }
}

fn proposal(path: &str) -> Option<String> {
    match syntax_for(path) {
        Some(HeaderSyntax::Markdown) => {
            Some("---\ntype: TODO\npublic_boundary: TODO\n---".to_owned())
        }
        Some(HeaderSyntax::SlashComment) => {
            Some("// ---\n// type: TODO\n// public_boundary: TODO\n// ---".to_owned())
        }
        Some(HeaderSyntax::CLikeComment) => {
            Some("/* ---\n * type: TODO\n * public_boundary: TODO\n * ---\n */".to_owned())
        }
        Some(HeaderSyntax::HashComment) => {
            Some("# ---\n# type: TODO\n# public_boundary: TODO\n# ---".to_owned())
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn fact<'a>(
        report: &'a MetadataReport,
        key: &str,
        value: &str,
        provenance: FactProvenance,
    ) -> &'a MetadataFact {
        report
            .facts
            .iter()
            .find(|item| item.key == key && item.value == value && item.provenance == provenance)
            .expect("expected fact")
    }

    fn warning(report: &MetadataReport, prefix: &str) -> bool {
        report.warnings.iter().any(|item| item.starts_with(prefix))
    }

    fn contains(
        report: &MetadataReport,
        key: &str,
        value: &str,
        provenance: FactProvenance,
    ) -> bool {
        report
            .facts
            .iter()
            .any(|item| item.key == key && item.value == value && item.provenance == provenance)
    }

    fn scan_entry(source: &[u8], metadata: MetadataReport) -> Arc<crate::scan::ScanEntry> {
        Arc::new(crate::scan::ScanEntry {
            identity: crate::scan::ContentIdentity::from_bytes(source),
            source: Arc::from(source),
            metadata,
        })
    }

    fn has_relationship(
        input: &crate::graph::GraphInput,
        relationship: crate::graph::EdgeRelationship,
    ) -> bool {
        input
            .edges()
            .iter()
            .any(|edge| edge.relationship() == relationship)
    }

    fn assert_relationship_facts(report: &MetadataReport) {
        for (key, value) in [
            ("implementation", "src/security.rs"),
            ("validation", "tests/security.rs"),
            ("producer_extension", "retained"),
        ] {
            assert_eq!(
                fact(report, key, value, FactProvenance::MarkdownFrontmatter).state,
                FactState::Candidate
            );
        }
    }

    #[test]
    fn p2_metadata() {
        let public_fixture =
            include_str!("../../../../fixtures/source-metadata/metadata-report-v1.json");
        assert!(public_fixture.contains("\"schema_version\": \"1.0.0\""));
        assert!(public_fixture.contains("\"source_path\": \"src/lib.rs\""));
        assert!(public_fixture.contains("\"state\": \"ambiguous\""));
        assert!(public_fixture.contains("// type: TODO"));
        let defaults = PackageDefaults::new([
            ("owner".to_owned(), "platform".to_owned()),
            ("type".to_owned(), "default".to_owned()),
        ]);
        let registry = MetadataParserRegistry::new(512);
        let markdown = registry.parse(
            "docs/security.md",
            "---\ntype: guide\ntags:\n  - security\n  - public\nimplementation:\n  - src/security.rs\nvalidation:\n  - tests/security.rs\nproducer_extension:\n  - retained\nfilename: declared.md\n---\n# Security\n",
            &defaults,
        );
        assert_eq!(
            fact(
                &markdown,
                "type",
                "guide",
                FactProvenance::MarkdownFrontmatter
            )
            .state,
            FactState::Candidate
        );
        assert_relationship_facts(&markdown);
        let snapshot = crate::scan::ScanSnapshot {
            entries: BTreeMap::from([
                (
                    "docs/security.md".to_owned(),
                    scan_entry(b"security", markdown.clone()),
                ),
                (
                    "src/security.rs".to_owned(),
                    scan_entry(b"source", MetadataReport::default()),
                ),
                (
                    "tests/security.rs".to_owned(),
                    scan_entry(b"test", MetadataReport::default()),
                ),
            ]),
            ..crate::scan::ScanSnapshot::default()
        };
        let graph_input = snapshot.graph_input().expect("OKF relationship graph");
        assert!(has_relationship(
            &graph_input,
            crate::graph::EdgeRelationship::Implementation
        ));
        assert!(has_relationship(
            &graph_input,
            crate::graph::EdgeRelationship::Validation
        ));
        assert_eq!(
            fact(
                &markdown,
                "owner",
                "platform",
                FactProvenance::PackageDefault
            )
            .state,
            FactState::Candidate
        );
        assert!(!contains(
            &markdown,
            "type",
            "default",
            FactProvenance::PackageDefault
        ));
        assert_eq!(
            fact(
                &markdown,
                "tags",
                "security",
                FactProvenance::MarkdownFrontmatter
            )
            .state,
            FactState::Candidate
        );
        assert_eq!(
            fact(
                &markdown,
                "tags",
                "public",
                FactProvenance::MarkdownFrontmatter
            )
            .state,
            FactState::Candidate
        );
        assert_eq!(
            fact(
                &markdown,
                "filename",
                "declared.md",
                FactProvenance::MarkdownFrontmatter
            )
            .state,
            FactState::Ambiguous
        );
        assert_eq!(
            fact(
                &markdown,
                "important_boundary",
                "true",
                FactProvenance::BoundaryClassification
            )
            .confidence,
            FactConfidence::Medium
        );
        assert!(warning(&markdown, "ambiguous-fact:"));
        assert_eq!(
            fact(&markdown, "language", "markdown", FactProvenance::Language).state,
            FactState::Candidate
        );
        let commented = registry.parse(
            "src/auth/service.rs",
            "// ---\n// type: service\n// tags:\n//   - security\n//   - public\n// ---\nuse crate::token;\n#[test]\nfn checks_token() {}\n",
            &defaults,
        );
        assert_eq!(
            fact(&commented, "type", "service", FactProvenance::CommentedYaml).state,
            FactState::Candidate
        );
        assert_eq!(
            fact(&commented, "symbol", "checks_token", FactProvenance::Symbol).state,
            FactState::Candidate
        );
        assert_eq!(
            fact(&commented, "import", "crate::token", FactProvenance::Import).state,
            FactState::Candidate
        );
        assert_eq!(
            fact(&commented, "test", "checks_token", FactProvenance::Test).state,
            FactState::Candidate
        );
        assert_eq!(
            fact(
                &commented,
                "tags",
                "security",
                FactProvenance::CommentedYaml
            )
            .state,
            FactState::Candidate
        );
        assert_eq!(
            fact(
                &commented,
                "important_boundary",
                "true",
                FactProvenance::BoundaryClassification
            )
            .state,
            FactState::Candidate
        );
        let dependencies = registry.parse(
            "Cargo.toml",
            "[dependencies]\nserde = \"1\"\n[dev-dependencies]\ninsta = \"1\"\n",
            &defaults,
        );
        assert_eq!(
            fact(
                &dependencies,
                "dependency",
                "serde",
                FactProvenance::Dependency
            )
            .state,
            FactState::Candidate
        );
        assert_eq!(
            fact(
                &dependencies,
                "dependency",
                "insta",
                FactProvenance::Dependency
            )
            .state,
            FactState::Candidate
        );
        assert_eq!(dependencies.proposals.len(), 1);
        assert!(dependencies.proposals[0].starts_with("# ---"));
        let block = registry.parse(
            "src/lock.c",
            "/* ---\n * type: service\n * ---\n */\nint lock(void);\n",
            &defaults,
        );
        assert_eq!(
            fact(&block, "type", "service", FactProvenance::CommentedYaml).state,
            FactState::Candidate
        );
        let hash = registry.parse(
            "tools/check.py",
            "# ---\n# type: check\n# ---\ndef test_ok(): pass\n",
            &defaults,
        );
        assert_eq!(
            fact(&hash, "type", "check", FactProvenance::CommentedYaml).state,
            FactState::Candidate
        );
        let git = registry.parse_with_git_state(
            "src/lib.rs",
            "pub fn current() {}\n",
            &defaults,
            Some(&GitStateFact {
                key: "git_ref".to_owned(),
                value: "abc123".to_owned(),
                confidence: FactConfidence::Medium,
            }),
        );
        assert_eq!(
            fact(&git, "git_ref", "abc123", FactProvenance::GitState).state,
            FactState::Candidate
        );
        assert!(warning(&dependencies, "metadata-on-touch:"));
        let malformed = registry.parse("docs/lib.md", "---\ntype: [\n---\n", &defaults);
        assert!(warning(&malformed, "malformed-metadata:"));
        let oversized = registry.parse("src/large.rs", &"x".repeat(513), &defaults);
        assert!(oversized.facts.is_empty());
        assert!(oversized.warnings[0].starts_with("input-too-large:"));
    }
}
