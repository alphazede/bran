//! Immutable, scanner-neutral graph input and topology records.

use std::collections::BTreeMap;
use std::fmt;

/// A stable graph identity accepted at the scanner-to-graph boundary.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId(String);

impl NodeId {
    pub fn parse(value: impl Into<String>) -> Result<Self, GraphError> {
        let value = value.into();
        if valid_identity(&value) {
            Ok(Self(value))
        } else {
            Err(GraphError::InvalidNodeId(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A stable graph edge identity accepted at the scanner-to-graph boundary.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EdgeId(String);

impl EdgeId {
    pub fn parse(value: impl Into<String>) -> Result<Self, GraphError> {
        let value = value.into();
        if valid_identity(&value) {
            Ok(Self(value))
        } else {
            Err(GraphError::InvalidEdgeId(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The role a node plays in the scanned knowledge graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeRole {
    Document,
    Section,
    Symbol,
    External,
    Entrypoint,
    Test,
    Generated,
    Archived,
}

/// Bounded scanner-supplied semantic facts. Values remain free-form evidence,
/// not graph-owned classifications or an ontology.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NodeFacts {
    fields: BTreeMap<String, Vec<String>>,
    semantic_bytes: usize,
}

impl NodeFacts {
    /// Maximum number of distinct semantic keys carried by one node.
    pub const MAX_FIELDS: usize = 32;
    /// Maximum UTF-8 byte length of one semantic key.
    pub const MAX_FIELD_KEY_BYTES: usize = 64;
    /// Maximum Unicode scalar count of one semantic key.
    pub const MAX_FIELD_KEY_CHARS: usize = 64;
    /// Maximum distinct values retained for one semantic key.
    pub const MAX_VALUES_PER_FIELD: usize = 32;
    /// Maximum UTF-8 byte length of one semantic value.
    pub const MAX_VALUE_BYTES: usize = 256;
    /// Maximum UTF-8 payload bytes: every stored key plus every stored value.
    pub const MAX_SEMANTIC_BYTES: usize = 8 * 1024;

    /// Adds one value to a semantic key. Keys and values are retained in
    /// lexical order and duplicate values are ignored, so lookup is stable.
    pub fn with_field_value(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, GraphError> {
        let key = valid_fact_key(key.into())?;
        let value = valid_fact_value(&key, value.into())?;
        let new_field = !self.fields.contains_key(&key);
        if let Some(values) = self.fields.get(&key) {
            if values.contains(&value) {
                return Ok(self);
            }
            if values.len() == Self::MAX_VALUES_PER_FIELD {
                return Err(GraphError::FactValuesPerFieldExceeded {
                    key,
                    limit: Self::MAX_VALUES_PER_FIELD,
                });
            }
        } else if self.fields.len() == Self::MAX_FIELDS {
            return Err(GraphError::FactFieldLimitExceeded {
                limit: Self::MAX_FIELDS,
            });
        }

        let additional_bytes = value.len() + usize::from(new_field) * key.len();
        let actual = self.semantic_bytes.saturating_add(additional_bytes);
        if actual > Self::MAX_SEMANTIC_BYTES {
            return Err(GraphError::SemanticByteLimitExceeded {
                limit: Self::MAX_SEMANTIC_BYTES,
                actual,
            });
        }

        let values = self.fields.entry(key).or_default();
        values.push(value);
        values.sort();
        self.semantic_bytes = actual;
        Ok(self)
    }

    /// Returns the exact values for a semantic key, in stable lexical order.
    pub fn values(&self, key: &str) -> Option<&[String]> {
        self.fields.get(key).map(Vec::as_slice)
    }

    /// Returns whether the exact semantic key has the exact supplied value.
    pub fn contains_value(&self, key: &str, value: &str) -> bool {
        self.values(key).is_some_and(|values| {
            values
                .binary_search_by(|item| item.as_str().cmp(value))
                .is_ok()
        })
    }

    /// Returns the bounded semantic payload byte count.
    pub fn semantic_bytes(&self) -> usize {
        self.semantic_bytes
    }

    pub fn with_status(self, value: impl Into<String>) -> Result<Self, GraphError> {
        self.replace_single_value("status", value.into())
    }

    pub fn with_tag(self, value: impl Into<String>) -> Result<Self, GraphError> {
        self.with_field_value("tags", value)
    }

    pub fn with_freshness(self, value: impl Into<String>) -> Result<Self, GraphError> {
        self.replace_single_value("freshness", value.into())
    }

    pub fn with_subsystem(self, value: impl Into<String>) -> Result<Self, GraphError> {
        self.replace_single_value("subsystem", value.into())
    }

    pub fn with_purpose(self, value: impl Into<String>) -> Result<Self, GraphError> {
        self.replace_single_value("purpose", value.into())
    }

    pub fn with_task(self, value: impl Into<String>) -> Result<Self, GraphError> {
        self.replace_single_value("task", value.into())
    }

    pub fn with_audience(self, value: impl Into<String>) -> Result<Self, GraphError> {
        self.replace_single_value("audience", value.into())
    }

    pub fn status(&self) -> Option<&str> {
        self.values("status")
            .and_then(|values| values.first())
            .map(String::as_str)
    }
    pub fn tags(&self) -> &[String] {
        self.values("tags").unwrap_or(&[])
    }
    pub fn freshness(&self) -> Option<&str> {
        self.values("freshness")
            .and_then(|values| values.first())
            .map(String::as_str)
    }
    pub fn subsystem(&self) -> Option<&str> {
        self.values("subsystem")
            .and_then(|values| values.first())
            .map(String::as_str)
    }
    pub fn purpose(&self) -> Option<&str> {
        self.values("purpose")
            .and_then(|values| values.first())
            .map(String::as_str)
    }
    pub fn task(&self) -> Option<&str> {
        self.values("task")
            .and_then(|values| values.first())
            .map(String::as_str)
    }
    pub fn audience(&self) -> Option<&str> {
        self.values("audience")
            .and_then(|values| values.first())
            .map(String::as_str)
    }

    fn replace_single_value(mut self, key: &str, value: String) -> Result<Self, GraphError> {
        if let Some(values) = self.fields.remove(key) {
            self.semantic_bytes -= key.len() + values.iter().map(String::len).sum::<usize>();
        }
        self.with_field_value(key, value)
    }
}

/// Scanner evidence carried through graph construction without interpretation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Provenance {
    source: String,
    locator: String,
}

impl Provenance {
    /// Maximum UTF-8 bytes accepted for a scanner/provider name.
    pub const MAX_SOURCE_BYTES: usize = 128;
    /// Maximum UTF-8 bytes accepted for a source locator.
    pub const MAX_LOCATOR_BYTES: usize = 1_024;

    pub fn new(source: impl Into<String>, locator: impl Into<String>) -> Result<Self, GraphError> {
        let source = source.into();
        let locator = locator.into();
        if source.trim().is_empty()
            || locator.trim().is_empty()
            || source.len() > Self::MAX_SOURCE_BYTES
            || locator.len() > Self::MAX_LOCATOR_BYTES
        {
            return Err(GraphError::InvalidProvenance);
        }
        Ok(Self { source, locator })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn locator(&self) -> &str {
        &self.locator
    }
}

/// A bounded scanner confidence score, preserved without graph-side inference.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Confidence(u8);

impl Confidence {
    pub fn new(value: u8) -> Result<Self, GraphError> {
        if value <= 100 {
            Ok(Self(value))
        } else {
            Err(GraphError::InvalidConfidence(value))
        }
    }

    pub fn value(self) -> u8 {
        self.0
    }
}

/// The certainty of a candidate edge. No variant authorizes graph repair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EdgeCertainty {
    Known,
    Dynamic,
    Unknown,
    MissingTarget,
}

/// Directed scanner-supplied relationship meaning. `Unspecified` preserves
/// compatibility for callers that only have topology and certainty evidence.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EdgeRelationship {
    #[default]
    Unspecified,
    Dependency,
    Implementation,
    Replacement,
    Supersedes,
    Validation,
    Reachability,
    Contradiction,
    Conflict,
}

/// Immutable scanner-neutral node input retained by the graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeInput {
    id: NodeId,
    role: NodeRole,
    provenance: Provenance,
    confidence: Confidence,
    facts: NodeFacts,
}

impl NodeInput {
    pub fn new(id: NodeId, role: NodeRole, provenance: Provenance, confidence: Confidence) -> Self {
        Self {
            id,
            role,
            provenance,
            confidence,
            facts: NodeFacts::default(),
        }
    }

    pub fn with_facts(mut self, facts: NodeFacts) -> Self {
        self.facts = facts;
        self
    }

    pub fn id(&self) -> &NodeId {
        &self.id
    }
    pub fn role(&self) -> NodeRole {
        self.role
    }
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
    pub fn confidence(&self) -> Confidence {
        self.confidence
    }
    pub fn facts(&self) -> &NodeFacts {
        &self.facts
    }
}

/// Immutable scanner-neutral edge input retained by the graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EdgeInput {
    id: EdgeId,
    source: NodeId,
    target: NodeId,
    provenance: Provenance,
    confidence: Confidence,
    certainty: EdgeCertainty,
    relationship: EdgeRelationship,
}

impl EdgeInput {
    pub fn new(
        id: EdgeId,
        source: NodeId,
        target: NodeId,
        provenance: Provenance,
        confidence: Confidence,
        certainty: EdgeCertainty,
    ) -> Self {
        Self {
            id,
            source,
            target,
            provenance,
            confidence,
            certainty,
            relationship: EdgeRelationship::Unspecified,
        }
    }

    pub fn with_relationship(mut self, relationship: EdgeRelationship) -> Self {
        self.relationship = relationship;
        self
    }

    pub fn id(&self) -> &EdgeId {
        &self.id
    }
    pub fn source(&self) -> &NodeId {
        &self.source
    }
    pub fn target(&self) -> &NodeId {
        &self.target
    }
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
    pub fn confidence(&self) -> Confidence {
        self.confidence
    }
    pub fn certainty(&self) -> EdgeCertainty {
        self.certainty
    }
    pub fn relationship(&self) -> EdgeRelationship {
        self.relationship
    }
}

/// Narrow scanner adapter boundary: data only, without filesystem semantics.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GraphInput {
    nodes: Vec<NodeInput>,
    edges: Vec<EdgeInput>,
}

impl GraphInput {
    pub fn new(nodes: Vec<NodeInput>, edges: Vec<EdgeInput>) -> Self {
        Self { nodes, edges }
    }
    pub fn nodes(&self) -> &[NodeInput] {
        &self.nodes
    }
    pub fn edges(&self) -> &[EdgeInput] {
        &self.edges
    }
    pub(super) fn into_parts(self) -> (Vec<NodeInput>, Vec<EdgeInput>) {
        (self.nodes, self.edges)
    }
}

/// Explicit resource bounds for deterministic graph construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphLimits {
    max_nodes: usize,
    max_edges: usize,
}

impl GraphLimits {
    pub fn new(max_nodes: usize, max_edges: usize) -> Result<Self, GraphError> {
        if max_nodes == 0 || max_edges == 0 {
            return Err(GraphError::InvalidLimits);
        }
        Ok(Self {
            max_nodes,
            max_edges,
        })
    }

    pub fn max_nodes(self) -> usize {
        self.max_nodes
    }
    pub fn max_edges(self) -> usize {
        self.max_edges
    }
}

/// The only portable structural snapshot schema accepted by this crate.
pub const STRUCTURAL_GRAPH_SCHEMA_VERSION: u16 = 1;
/// The only ordering convention accepted by this snapshot schema.
pub const STRUCTURAL_GRAPH_ORDERING_VERSION: u16 = 1;

/// The stable canonical-path identity policy for structural snapshots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuralPathCasePolicy {
    CaseSensitive,
}

impl StructuralPathCasePolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CaseSensitive => "case-sensitive",
        }
    }
}

/// Closed source vocabulary for structural nodes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StructuralSourceType {
    Text,
    Code,
    Markdown,
    Mermaid,
    Metadata,
    PreextractedText,
}

impl StructuralSourceType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Code => "code",
            Self::Markdown => "markdown",
            Self::Mermaid => "mermaid",
            Self::Metadata => "metadata",
            Self::PreextractedText => "preextracted-text",
        }
    }
}

/// Closed relationship vocabulary for structural edges.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StructuralEdgeKind {
    Ownership,
    Lifecycle,
    Supersedes,
    Predecessor,
    Dependency,
    Impact,
    Requirement,
    Design,
    Implementation,
    Validation,
    Domain,
    Architecture,
    BusinessFlow,
    GeneratedFrom,
    SourceLink,
}

impl StructuralEdgeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ownership => "ownership",
            Self::Lifecycle => "lifecycle",
            Self::Supersedes => "supersedes",
            Self::Predecessor => "predecessor",
            Self::Dependency => "dependency",
            Self::Impact => "impact",
            Self::Requirement => "requirement",
            Self::Design => "design",
            Self::Implementation => "implementation",
            Self::Validation => "validation",
            Self::Domain => "domain",
            Self::Architecture => "architecture",
            Self::BusinessFlow => "business-flow",
            Self::GeneratedFrom => "generated-from",
            Self::SourceLink => "source-link",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StructuralLifecycle {
    Current,
    Superseded,
    Draft,
    Archive,
    Rejected,
}

impl StructuralLifecycle {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Superseded => "superseded",
            Self::Draft => "draft",
            Self::Archive => "archive",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StructuralAuthority {
    Canonical,
    Approved,
    Supporting,
    Unknown,
}

impl StructuralAuthority {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Canonical => "canonical",
            Self::Approved => "approved",
            Self::Supporting => "supporting",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StructuralGeneratedStatus {
    Source,
    Generated,
    Preextracted,
}

impl StructuralGeneratedStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Generated => "generated",
            Self::Preextracted => "preextracted",
        }
    }
}

/// Whether a structural edge has a resolved target.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StructuralEdgeCertainty {
    Known,
    Dynamic,
    Unknown,
    MissingTarget,
}

impl StructuralEdgeCertainty {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Known => "known",
            Self::Dynamic => "dynamic",
            Self::Unknown => "unknown",
            Self::MissingTarget => "missing-target",
        }
    }
}

/// One immutable structural node. Its identity is derived from path and source type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralNode {
    id: String,
    path: String,
    content_digest: String,
    source_type: StructuralSourceType,
    domains: Vec<String>,
    owner: Option<String>,
    lifecycle: StructuralLifecycle,
    authority: StructuralAuthority,
    generated_status: StructuralGeneratedStatus,
    provenance: Vec<Provenance>,
}

impl StructuralNode {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        path: impl Into<String>,
        content_digest: impl Into<String>,
        source_type: StructuralSourceType,
        mut domains: Vec<String>,
        owner: Option<String>,
        lifecycle: StructuralLifecycle,
        authority: StructuralAuthority,
        generated_status: StructuralGeneratedStatus,
        mut provenance: Vec<Provenance>,
    ) -> Result<Self, GraphError> {
        let path = path.into();
        validate_structural_path(&path)?;
        let content_digest = content_digest.into();
        validate_digest(&content_digest)?;
        normalize_strings(&mut domains, "domain")?;
        if let Some(owner) = &owner {
            validate_structural_text(owner, "owner")?;
        }
        normalize_provenance(&mut provenance);
        Ok(Self {
            id: structural_node_identity(&path, source_type),
            path,
            content_digest,
            source_type,
            domains,
            owner,
            lifecycle,
            authority,
            generated_status,
            provenance,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn content_digest(&self) -> &str {
        &self.content_digest
    }
    pub const fn source_type(&self) -> StructuralSourceType {
        self.source_type
    }
    pub fn domains(&self) -> &[String] {
        &self.domains
    }
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }
    pub const fn lifecycle(&self) -> StructuralLifecycle {
        self.lifecycle
    }
    pub const fn authority(&self) -> StructuralAuthority {
        self.authority
    }
    pub const fn generated_status(&self) -> StructuralGeneratedStatus {
        self.generated_status
    }
    pub fn provenance(&self) -> &[Provenance] {
        &self.provenance
    }

    fn validate(&self) -> Result<(), GraphError> {
        validate_structural_path(&self.path)?;
        validate_digest(&self.content_digest)?;
        if self.id != structural_node_identity(&self.path, self.source_type) {
            return Err(GraphError::InvalidStructuralIdentity(self.id.clone()));
        }
        validate_sorted_unique_strings(&self.domains, "domain")?;
        if let Some(owner) = &self.owner {
            validate_structural_text(owner, "owner")?;
        }
        validate_sorted_unique_provenance(&self.provenance)
    }
}

/// One immutable structural edge. Its identity is derived from its complete locator tuple.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralEdge {
    id: String,
    kind: StructuralEdgeKind,
    source: String,
    target: String,
    source_locator: String,
    provenance: Vec<Provenance>,
    certainty: StructuralEdgeCertainty,
}

impl StructuralEdge {
    pub fn new(
        kind: StructuralEdgeKind,
        source: impl Into<String>,
        target: impl Into<String>,
        source_locator: impl Into<String>,
        mut provenance: Vec<Provenance>,
        certainty: StructuralEdgeCertainty,
    ) -> Result<Self, GraphError> {
        let source = source.into();
        let target = target.into();
        let source_locator = source_locator.into();
        validate_digest(&source)?;
        validate_digest(&target)?;
        validate_structural_text(&source_locator, "source locator")?;
        normalize_provenance(&mut provenance);
        Ok(Self {
            id: structural_edge_identity(kind, &source, &target, &source_locator),
            kind,
            source,
            target,
            source_locator,
            provenance,
            certainty,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub const fn kind(&self) -> StructuralEdgeKind {
        self.kind
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn target(&self) -> &str {
        &self.target
    }
    pub fn source_locator(&self) -> &str {
        &self.source_locator
    }
    pub fn provenance(&self) -> &[Provenance] {
        &self.provenance
    }
    pub const fn certainty(&self) -> StructuralEdgeCertainty {
        self.certainty
    }

    fn validate(&self) -> Result<(), GraphError> {
        validate_digest(&self.source)?;
        validate_digest(&self.target)?;
        validate_structural_text(&self.source_locator, "source locator")?;
        if self.id
            != structural_edge_identity(self.kind, &self.source, &self.target, &self.source_locator)
        {
            return Err(GraphError::InvalidStructuralIdentity(self.id.clone()));
        }
        validate_sorted_unique_provenance(&self.provenance)
    }
}

/// Immutable, validation-only evidence for a snapshot operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralSnapshotReceipt {
    operation_id: String,
    before_digest: Option<String>,
    candidate_digest: Option<String>,
    published_digest: Option<String>,
    validation_facts: Vec<String>,
    ordering_version: u16,
}

impl StructuralSnapshotReceipt {
    pub fn new(
        operation_id: impl Into<String>,
        before_digest: Option<String>,
        candidate_digest: Option<String>,
        published_digest: Option<String>,
        mut validation_facts: Vec<String>,
    ) -> Result<Self, GraphError> {
        let operation_id = operation_id.into();
        validate_structural_text(&operation_id, "operation id")?;
        validate_optional_digest(before_digest.as_deref())?;
        validate_optional_digest(candidate_digest.as_deref())?;
        validate_optional_digest(published_digest.as_deref())?;
        normalize_strings(&mut validation_facts, "validation fact")?;
        Ok(Self {
            operation_id,
            before_digest,
            candidate_digest,
            published_digest,
            validation_facts,
            ordering_version: STRUCTURAL_GRAPH_ORDERING_VERSION,
        })
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn before_digest(&self) -> Option<&str> {
        self.before_digest.as_deref()
    }
    pub fn candidate_digest(&self) -> Option<&str> {
        self.candidate_digest.as_deref()
    }
    pub fn published_digest(&self) -> Option<&str> {
        self.published_digest.as_deref()
    }
    pub fn validation_facts(&self) -> &[String] {
        &self.validation_facts
    }
    pub const fn ordering_version(&self) -> u16 {
        self.ordering_version
    }

    fn validate(&self) -> Result<(), GraphError> {
        validate_structural_text(&self.operation_id, "operation id")?;
        validate_optional_digest(self.before_digest.as_deref())?;
        validate_optional_digest(self.candidate_digest.as_deref())?;
        validate_optional_digest(self.published_digest.as_deref())?;
        validate_sorted_unique_strings(&self.validation_facts, "validation fact")?;
        if self.ordering_version != STRUCTURAL_GRAPH_ORDERING_VERSION {
            return Err(GraphError::InvalidStructuralOrderingVersion(
                self.ordering_version,
            ));
        }
        Ok(())
    }
}

/// A portable, immutable v1 structural graph with derived indexes and adjacency.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralGraphSnapshot {
    schema_version: u16,
    snapshot_digest: String,
    case_policy: StructuralPathCasePolicy,
    parent_digest: Option<String>,
    scan_input_digest: String,
    graph_digest: String,
    frozen_frequency_digest: String,
    nodes: Vec<StructuralNode>,
    edges: Vec<StructuralEdge>,
    node_index: BTreeMap<String, usize>,
    edge_index: BTreeMap<String, usize>,
    forward: BTreeMap<String, Vec<String>>,
    reverse: BTreeMap<String, Vec<String>>,
    receipt: StructuralSnapshotReceipt,
}

impl StructuralGraphSnapshot {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        schema_version: u16,
        parent_digest: Option<String>,
        scan_input_digest: impl Into<String>,
        graph_digest: impl Into<String>,
        frozen_frequency_digest: impl Into<String>,
        mut nodes: Vec<StructuralNode>,
        mut edges: Vec<StructuralEdge>,
        receipt: StructuralSnapshotReceipt,
    ) -> Result<Self, GraphError> {
        if schema_version != STRUCTURAL_GRAPH_SCHEMA_VERSION {
            return Err(GraphError::UnknownStructuralSchemaVersion(schema_version));
        }
        let scan_input_digest = scan_input_digest.into();
        let graph_digest = graph_digest.into();
        let frozen_frequency_digest = frozen_frequency_digest.into();
        validate_optional_digest(parent_digest.as_deref())?;
        validate_digest(&scan_input_digest)?;
        validate_digest(&graph_digest)?;
        validate_digest(&frozen_frequency_digest)?;
        nodes.sort_by(|left, right| left.id.cmp(&right.id));
        edges.sort_by(|left, right| left.id.cmp(&right.id));
        let (node_index, edge_index, forward, reverse) = structural_indexes(&nodes, &edges)?;
        let mut snapshot = Self {
            schema_version,
            snapshot_digest: String::new(),
            case_policy: StructuralPathCasePolicy::CaseSensitive,
            parent_digest,
            scan_input_digest,
            graph_digest,
            frozen_frequency_digest,
            nodes,
            edges,
            node_index,
            edge_index,
            forward,
            reverse,
            receipt,
        };
        snapshot.snapshot_digest = digest(snapshot.canonical_json_without_digest().as_bytes());
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }
    pub fn snapshot_digest(&self) -> &str {
        &self.snapshot_digest
    }
    pub const fn case_policy(&self) -> StructuralPathCasePolicy {
        self.case_policy
    }
    pub fn parent_digest(&self) -> Option<&str> {
        self.parent_digest.as_deref()
    }
    pub fn scan_input_digest(&self) -> &str {
        &self.scan_input_digest
    }
    pub fn graph_digest(&self) -> &str {
        &self.graph_digest
    }
    pub fn frozen_frequency_digest(&self) -> &str {
        &self.frozen_frequency_digest
    }
    pub fn nodes(&self) -> &[StructuralNode] {
        &self.nodes
    }
    pub fn edges(&self) -> &[StructuralEdge] {
        &self.edges
    }
    pub fn receipt(&self) -> &StructuralSnapshotReceipt {
        &self.receipt
    }
    pub fn node(&self, id: &str) -> Option<&StructuralNode> {
        self.node_index.get(id).map(|index| &self.nodes[*index])
    }
    pub fn edge(&self, id: &str) -> Option<&StructuralEdge> {
        self.edge_index.get(id).map(|index| &self.edges[*index])
    }
    pub fn forward_edges(&self, id: &str) -> &[String] {
        self.forward.get(id).map(Vec::as_slice).unwrap_or(&[])
    }
    pub fn reverse_edges(&self, id: &str) -> &[String] {
        self.reverse.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Returns canonical UTF-8 JSON with lexicographic object keys.
    pub fn canonical_json(&self) -> String {
        let mut out = self.canonical_json_without_digest();
        out.pop();
        out.push_str(",\"snapshot_digest\":");
        write_json_string(&mut out, &self.snapshot_digest);
        out.push('}');
        out
    }

    /// Rechecks identities, ordering, derived indexes, endpoints, and digest.
    pub fn validate(&self) -> Result<(), GraphError> {
        if self.schema_version != STRUCTURAL_GRAPH_SCHEMA_VERSION {
            return Err(GraphError::UnknownStructuralSchemaVersion(
                self.schema_version,
            ));
        }
        match self.case_policy {
            StructuralPathCasePolicy::CaseSensitive => {}
        }
        validate_digest(&self.snapshot_digest)?;
        validate_optional_digest(self.parent_digest.as_deref())?;
        validate_digest(&self.scan_input_digest)?;
        validate_digest(&self.graph_digest)?;
        validate_digest(&self.frozen_frequency_digest)?;
        self.receipt.validate()?;
        validate_sorted_nodes(&self.nodes)?;
        validate_sorted_edges(&self.edges)?;
        let (node_index, edge_index, forward, reverse) =
            structural_indexes(&self.nodes, &self.edges)?;
        if self.node_index != node_index
            || self.edge_index != edge_index
            || self.forward != forward
            || self.reverse != reverse
        {
            return Err(GraphError::InvalidStructuralIndexes);
        }
        let actual = digest(self.canonical_json_without_digest().as_bytes());
        if self.snapshot_digest != actual {
            return Err(GraphError::StructuralDigestMismatch {
                expected: actual,
                actual: self.snapshot_digest.clone(),
            });
        }
        Ok(())
    }

    fn canonical_json_without_digest(&self) -> String {
        let mut out = String::new();
        out.push('{');
        out.push_str("\"adjacency\":[");
        write_adjacency(&mut out, self);
        out.push_str("],\"case_policy\":");
        write_json_string(&mut out, self.case_policy.as_str());
        out.push_str(",\"edges\":[");
        write_json_array(&mut out, &self.edges, write_structural_edge);
        out.push_str("],\"frozen_frequency_digest\":");
        write_json_string(&mut out, &self.frozen_frequency_digest);
        out.push_str(",\"graph_digest\":");
        write_json_string(&mut out, &self.graph_digest);
        out.push_str(",\"indexes\":");
        write_indexes(&mut out, self);
        out.push_str(",\"nodes\":[");
        write_json_array(&mut out, &self.nodes, write_structural_node);
        out.push_str("],\"parent_digest\":");
        write_json_option(&mut out, self.parent_digest.as_deref());
        out.push_str(",\"receipt\":");
        write_receipt(&mut out, &self.receipt);
        out.push_str(",\"scan_input_digest\":");
        write_json_string(&mut out, &self.scan_input_digest);
        out.push_str(",\"schema_version\":");
        out.push_str(&self.schema_version.to_string());
        out.push('}');
        out
    }
}

fn structural_node_identity(path: &str, source_type: StructuralSourceType) -> String {
    digest(format!("{path}\n{}", source_type.as_str()).as_bytes())
}

fn structural_edge_identity(
    kind: StructuralEdgeKind,
    source: &str,
    target: &str,
    locator: &str,
) -> String {
    digest(format!("{}\n{source}\n{target}\n{locator}", kind.as_str()).as_bytes())
}

pub(super) fn structural_content_digest(
    nodes: &[StructuralNode],
    edges: &[StructuralEdge],
) -> String {
    let mut nodes = nodes.to_vec();
    let mut edges = edges.to_vec();
    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    edges.sort_by(|left, right| left.id.cmp(&right.id));
    let mut out = String::from("{\"edges\":[");
    write_json_array(&mut out, &edges, write_structural_edge);
    out.push_str("],\"nodes\":[");
    write_json_array(&mut out, &nodes, write_structural_node);
    out.push_str("]}");
    digest(out.as_bytes())
}

fn digest(bytes: &[u8]) -> String {
    crate::agent::result_store::ResultId::sha256(bytes)
        .value()
        .to_owned()
}

fn validate_digest(value: &str) -> Result<(), GraphError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(GraphError::InvalidStructuralDigest(value.to_owned()))
    }
}

fn validate_optional_digest(value: Option<&str>) -> Result<(), GraphError> {
    if let Some(value) = value {
        validate_digest(value)
    } else {
        Ok(())
    }
}

fn validate_structural_path(path: &str) -> Result<(), GraphError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        Err(GraphError::InvalidStructuralPath(path.to_owned()))
    } else {
        Ok(())
    }
}

fn validate_structural_text(value: &str, field: &str) -> Result<(), GraphError> {
    if value.trim().is_empty() {
        Err(GraphError::InvalidStructuralField {
            field: field.to_owned(),
            value: value.to_owned(),
        })
    } else {
        Ok(())
    }
}

fn normalize_strings(values: &mut Vec<String>, field: &str) -> Result<(), GraphError> {
    for value in values.iter() {
        validate_structural_text(value, field)?;
    }
    values.sort();
    values.dedup();
    Ok(())
}

fn validate_sorted_unique_strings(values: &[String], field: &str) -> Result<(), GraphError> {
    for value in values {
        validate_structural_text(value, field)?;
    }
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        Err(GraphError::NonCanonicalStructuralOrder(field.to_owned()))
    } else {
        Ok(())
    }
}

fn normalize_provenance(values: &mut Vec<Provenance>) {
    values.sort_by(|left, right| {
        (left.source(), left.locator()).cmp(&(right.source(), right.locator()))
    });
    values.dedup_by(|left, right| {
        left.source() == right.source() && left.locator() == right.locator()
    });
}

fn validate_sorted_unique_provenance(values: &[Provenance]) -> Result<(), GraphError> {
    if values
        .windows(2)
        .any(|pair| (pair[0].source(), pair[0].locator()) >= (pair[1].source(), pair[1].locator()))
    {
        Err(GraphError::NonCanonicalStructuralOrder(
            "provenance".to_owned(),
        ))
    } else {
        Ok(())
    }
}

type StructuralIndexes = (
    BTreeMap<String, usize>,
    BTreeMap<String, usize>,
    BTreeMap<String, Vec<String>>,
    BTreeMap<String, Vec<String>>,
);

fn structural_indexes(
    nodes: &[StructuralNode],
    edges: &[StructuralEdge],
) -> Result<StructuralIndexes, GraphError> {
    let mut node_index = BTreeMap::new();
    for (position, node) in nodes.iter().enumerate() {
        node.validate()?;
        if node_index.insert(node.id.clone(), position).is_some() {
            return Err(GraphError::DuplicateStructuralNodeId(node.id.clone()));
        }
    }
    let mut edge_index = BTreeMap::new();
    let mut forward: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut reverse: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (position, edge) in edges.iter().enumerate() {
        edge.validate()?;
        if !node_index.contains_key(&edge.source) {
            return Err(GraphError::MissingStructuralSource(edge.source.clone()));
        }
        let target_present = node_index.contains_key(&edge.target);
        if !target_present && edge.certainty != StructuralEdgeCertainty::MissingTarget {
            return Err(GraphError::MissingStructuralTarget(edge.target.clone()));
        }
        if target_present && edge.certainty == StructuralEdgeCertainty::MissingTarget {
            return Err(GraphError::PresentStructuralTargetMarkedMissing(
                edge.target.clone(),
            ));
        }
        if edge_index.insert(edge.id.clone(), position).is_some() {
            return Err(GraphError::DuplicateStructuralEdgeId(edge.id.clone()));
        }
        forward
            .entry(edge.source.clone())
            .or_default()
            .push(edge.id.clone());
        reverse
            .entry(edge.target.clone())
            .or_default()
            .push(edge.id.clone());
    }
    Ok((node_index, edge_index, forward, reverse))
}

fn validate_sorted_nodes(nodes: &[StructuralNode]) -> Result<(), GraphError> {
    if nodes.windows(2).any(|pair| pair[0].id >= pair[1].id) {
        Err(GraphError::NonCanonicalStructuralOrder("nodes".to_owned()))
    } else {
        Ok(())
    }
}
fn validate_sorted_edges(edges: &[StructuralEdge]) -> Result<(), GraphError> {
    if edges.windows(2).any(|pair| pair[0].id >= pair[1].id) {
        Err(GraphError::NonCanonicalStructuralOrder("edges".to_owned()))
    } else {
        Ok(())
    }
}

fn write_json_string(out: &mut String, value: &str) {
    crate::schema::write_escaped_string_for_bundle(out, value);
}
fn write_json_option(out: &mut String, value: Option<&str>) {
    if let Some(value) = value {
        write_json_string(out, value)
    } else {
        out.push_str("null")
    }
}
fn write_json_array<T>(out: &mut String, values: &[T], write: fn(&mut String, &T)) {
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write(out, value);
    }
}
fn write_json_strings(out: &mut String, values: &[String]) {
    write_json_array(out, values, |out, value| write_json_string(out, value));
}
fn write_provenance(out: &mut String, value: &Provenance) {
    out.push_str("{\"locator\":");
    write_json_string(out, value.locator());
    out.push_str(",\"source\":");
    write_json_string(out, value.source());
    out.push('}');
}
fn write_structural_node(out: &mut String, value: &StructuralNode) {
    out.push_str("{\"authority\":");
    write_json_string(out, value.authority.as_str());
    out.push_str(",\"content_digest\":");
    write_json_string(out, &value.content_digest);
    out.push_str(",\"domains\":[");
    write_json_strings(out, &value.domains);
    out.push_str("],\"generated_status\":");
    write_json_string(out, value.generated_status.as_str());
    out.push_str(",\"id\":");
    write_json_string(out, &value.id);
    out.push_str(",\"lifecycle\":");
    write_json_string(out, value.lifecycle.as_str());
    out.push_str(",\"owner\":");
    write_json_option(out, value.owner.as_deref());
    out.push_str(",\"path\":");
    write_json_string(out, &value.path);
    out.push_str(",\"provenance\":[");
    write_json_array(out, &value.provenance, write_provenance);
    out.push_str("],\"source_type\":");
    write_json_string(out, value.source_type.as_str());
    out.push('}');
}
fn write_structural_edge(out: &mut String, value: &StructuralEdge) {
    out.push_str("{\"certainty\":");
    write_json_string(out, value.certainty.as_str());
    out.push_str(",\"id\":");
    write_json_string(out, &value.id);
    out.push_str(",\"kind\":");
    write_json_string(out, value.kind.as_str());
    out.push_str(",\"provenance\":[");
    write_json_array(out, &value.provenance, write_provenance);
    out.push_str("],\"source\":");
    write_json_string(out, &value.source);
    out.push_str(",\"source_locator\":");
    write_json_string(out, &value.source_locator);
    out.push_str(",\"target\":");
    write_json_string(out, &value.target);
    out.push('}');
}
fn write_adjacency(out: &mut String, snapshot: &StructuralGraphSnapshot) {
    for (position, node) in snapshot.nodes.iter().enumerate() {
        if position > 0 {
            out.push(',');
        }
        out.push_str("{\"forward\":[");
        write_json_strings(out, snapshot.forward_edges(&node.id));
        out.push_str("],\"node_id\":");
        write_json_string(out, &node.id);
        out.push_str(",\"reverse\":[");
        write_json_strings(out, snapshot.reverse_edges(&node.id));
        out.push_str("]}");
    }
}
fn write_index_records(out: &mut String, index: &BTreeMap<String, usize>) {
    for (record, (id, position)) in index.iter().enumerate() {
        if record > 0 {
            out.push(',');
        }
        out.push_str("{\"id\":");
        write_json_string(out, id);
        out.push_str(",\"position\":");
        out.push_str(&position.to_string());
        out.push('}');
    }
}
fn write_indexes(out: &mut String, snapshot: &StructuralGraphSnapshot) {
    out.push_str("{\"edges\":[");
    write_index_records(out, &snapshot.edge_index);
    out.push_str("],\"nodes\":[");
    write_index_records(out, &snapshot.node_index);
    out.push_str("]}");
}
fn write_receipt(out: &mut String, value: &StructuralSnapshotReceipt) {
    out.push_str("{\"before_digest\":");
    write_json_option(out, value.before_digest.as_deref());
    out.push_str(",\"candidate_digest\":");
    write_json_option(out, value.candidate_digest.as_deref());
    out.push_str(",\"operation_id\":");
    write_json_string(out, &value.operation_id);
    out.push_str(",\"ordering_version\":");
    out.push_str(&value.ordering_version.to_string());
    out.push_str(",\"published_digest\":");
    write_json_option(out, value.published_digest.as_deref());
    out.push_str(",\"validation_facts\":[");
    write_json_strings(out, &value.validation_facts);
    out.push_str("]}");
}

/// Construction failures; all ambiguity remains represented, never repaired.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphError {
    InvalidNodeId(String),
    InvalidEdgeId(String),
    InvalidProvenance,
    InvalidConfidence(u8),
    InvalidFactKey(String),
    InvalidFactValue { key: String, value: String },
    FactFieldLimitExceeded { limit: usize },
    FactValuesPerFieldExceeded { key: String, limit: usize },
    SemanticByteLimitExceeded { limit: usize, actual: usize },
    InvalidLimits,
    NodeLimitExceeded { limit: usize, actual: usize },
    EdgeLimitExceeded { limit: usize, actual: usize },
    DuplicateNodeId(NodeId),
    DuplicateEdgeId(EdgeId),
    MissingSource(NodeId),
    MissingTargetMustBeExplicit { edge: EdgeId, target: NodeId },
    MissingTargetMustBeAbsent { edge: EdgeId, target: NodeId },
    UnknownStructuralSchemaVersion(u16),
    InvalidStructuralOrderingVersion(u16),
    InvalidStructuralPath(String),
    InvalidStructuralDigest(String),
    InvalidStructuralIdentity(String),
    InvalidStructuralField { field: String, value: String },
    NonCanonicalStructuralOrder(String),
    DuplicateStructuralNodeId(String),
    DuplicateStructuralEdgeId(String),
    MissingStructuralSource(String),
    MissingStructuralTarget(String),
    PresentStructuralTargetMarkedMissing(String),
    InvalidStructuralIndexes,
    StructuralDigestMismatch { expected: String, actual: String },
    InvalidGeneratedFromTarget { edge: String, target: String },
    MissingReciprocalLineage { edge: String },
    StructuralLineageCycle { trail: Vec<String> },
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNodeId(id) => write!(f, "invalid node identity: {id}"),
            Self::InvalidEdgeId(id) => write!(f, "invalid edge identity: {id}"),
            Self::InvalidProvenance => write!(
                f,
                "provenance source and locator must be nonblank and at most {} and {} bytes",
                Provenance::MAX_SOURCE_BYTES,
                Provenance::MAX_LOCATOR_BYTES
            ),
            Self::InvalidConfidence(value) => write!(f, "confidence must be at most 100: {value}"),
            Self::InvalidFactKey(key) => write!(
                f,
                "semantic key must be nonblank and at most {} bytes and {} characters: {key}",
                NodeFacts::MAX_FIELD_KEY_BYTES,
                NodeFacts::MAX_FIELD_KEY_CHARS,
            ),
            Self::InvalidFactValue { key, value } => write!(
                f,
                "{key} semantic value must be nonblank and at most {} bytes: {value}",
                NodeFacts::MAX_VALUE_BYTES
            ),
            Self::FactFieldLimitExceeded { limit } => {
                write!(f, "semantic field limit exceeded: {limit}")
            }
            Self::FactValuesPerFieldExceeded { key, limit } => {
                write!(f, "semantic value limit exceeded for {key}: {limit}")
            }
            Self::SemanticByteLimitExceeded { limit, actual } => {
                write!(f, "semantic byte limit {limit} exceeded: {actual}")
            }
            Self::InvalidLimits => write!(f, "node and edge limits must be nonzero"),
            Self::NodeLimitExceeded { limit, actual } => {
                write!(f, "node limit {limit} exceeded by {actual}")
            }
            Self::EdgeLimitExceeded { limit, actual } => {
                write!(f, "edge limit {limit} exceeded by {actual}")
            }
            Self::DuplicateNodeId(id) => write!(f, "duplicate node identity: {id:?}"),
            Self::DuplicateEdgeId(id) => write!(f, "duplicate edge identity: {id:?}"),
            Self::MissingSource(id) => write!(f, "edge source is not a graph node: {id:?}"),
            Self::MissingTargetMustBeExplicit { edge, target } => write!(
                f,
                "edge {edge:?} references absent target {target:?} without MissingTarget certainty"
            ),
            Self::MissingTargetMustBeAbsent { edge, target } => write!(
                f,
                "edge {edge:?} marks present target {target:?} MissingTarget"
            ),
            Self::UnknownStructuralSchemaVersion(version) => {
                write!(f, "unknown structural graph schema version: {version}")
            }
            Self::InvalidStructuralOrderingVersion(version) => {
                write!(f, "unknown structural graph ordering version: {version}")
            }
            Self::InvalidStructuralPath(path) => write!(f, "invalid structural path: {path}"),
            Self::InvalidStructuralDigest(digest) => {
                write!(f, "invalid structural digest: {digest}")
            }
            Self::InvalidStructuralIdentity(identity) => {
                write!(f, "invalid structural identity: {identity}")
            }
            Self::InvalidStructuralField { field, value } => {
                write!(f, "invalid structural {field}: {value}")
            }
            Self::NonCanonicalStructuralOrder(field) => {
                write!(f, "noncanonical structural ordering for {field}")
            }
            Self::DuplicateStructuralNodeId(id) => write!(f, "duplicate structural node: {id}"),
            Self::DuplicateStructuralEdgeId(id) => write!(f, "duplicate structural edge: {id}"),
            Self::MissingStructuralSource(id) => write!(f, "missing structural edge source: {id}"),
            Self::MissingStructuralTarget(id) => write!(f, "missing structural edge target: {id}"),
            Self::PresentStructuralTargetMarkedMissing(id) => {
                write!(f, "present structural target marked missing: {id}")
            }
            Self::InvalidStructuralIndexes => write!(f, "invalid structural indexes"),
            Self::StructuralDigestMismatch { expected, actual } => {
                write!(
                    f,
                    "structural digest mismatch: expected {expected}, got {actual}"
                )
            }
            Self::InvalidGeneratedFromTarget { edge, target } => {
                write!(
                    f,
                    "generated_from edge {edge} targets non-source node {target}"
                )
            }
            Self::MissingReciprocalLineage { edge } => {
                write!(f, "lineage edge {edge} lacks reciprocal evidence")
            }
            Self::StructuralLineageCycle { trail } => {
                write!(f, "structural lineage cycle: {}", trail.join(" -> "))
            }
        }
    }
}

impl std::error::Error for GraphError {}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/' | b':' | b'_' | b'-')
        })
}

fn valid_fact_key(key: String) -> Result<String, GraphError> {
    if key.trim().is_empty()
        || key.len() > NodeFacts::MAX_FIELD_KEY_BYTES
        || key.chars().count() > NodeFacts::MAX_FIELD_KEY_CHARS
    {
        Err(GraphError::InvalidFactKey(key))
    } else {
        Ok(key)
    }
}

fn valid_fact_value(key: &str, value: String) -> Result<String, GraphError> {
    if value.trim().is_empty() || value.len() > NodeFacts::MAX_VALUE_BYTES {
        Err(GraphError::InvalidFactValue {
            key: key.to_owned(),
            value,
        })
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod structural_tests {
    use super::*;

    fn hex(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn receipt() -> StructuralSnapshotReceipt {
        StructuralSnapshotReceipt::new(
            "refresh-1",
            Some(hex('a')),
            Some(hex('b')),
            Some(hex('c')),
            vec!["paths-contained".to_owned(), "schema-v1".to_owned()],
        )
        .unwrap()
    }

    fn node(path: &str, digest: char, source_type: StructuralSourceType) -> StructuralNode {
        StructuralNode::new(
            path,
            hex(digest),
            source_type,
            vec![
                "architecture".to_owned(),
                "api".to_owned(),
                "api".to_owned(),
            ],
            Some("core".to_owned()),
            StructuralLifecycle::Current,
            StructuralAuthority::Canonical,
            StructuralGeneratedStatus::Source,
            vec![Provenance::new("scanner", format!("{path}:1")).unwrap()],
        )
        .unwrap()
    }

    #[test]
    fn p8_schema_parsers() {
        let alpha = node("docs/alpha.md", '1', StructuralSourceType::Markdown);
        let beta = node("src/beta.rs", '2', StructuralSourceType::Code);
        let edge = StructuralEdge::new(
            StructuralEdgeKind::Dependency,
            alpha.id().to_owned(),
            beta.id().to_owned(),
            "docs/alpha.md:4",
            vec![Provenance::new("metadata", "docs/alpha.md:4").unwrap()],
            StructuralEdgeCertainty::Known,
        )
        .unwrap();
        let first = StructuralGraphSnapshot::new(
            STRUCTURAL_GRAPH_SCHEMA_VERSION,
            Some(hex('d')),
            hex('e'),
            hex('f'),
            hex('0'),
            vec![beta.clone(), alpha.clone()],
            vec![edge.clone()],
            receipt(),
        )
        .unwrap();
        let second = StructuralGraphSnapshot::new(
            STRUCTURAL_GRAPH_SCHEMA_VERSION,
            Some(hex('d')),
            hex('e'),
            hex('f'),
            hex('0'),
            vec![alpha.clone(), beta.clone()],
            vec![edge.clone()],
            receipt(),
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.canonical_json(), second.canonical_json());
        assert_eq!(
            first.snapshot_digest(),
            "942435066c93943869b8524d37217faaf2282b918c3c665b27ec8d8ec273e33d"
        );
        assert_eq!(
            first.canonical_json(),
            include_str!("../../../../fixtures/graph/structural-graph-snapshot-v1.json").trim_end()
        );
        assert!(first.canonical_json().contains(concat!(
            "\"ca",
            "se_policy\":\"ca",
            "se-sensitive\""
        )));
        assert!(first.canonical_json().contains(
            r#"{"forward":[],"node_id":"4cc59666a045db50e8432ececd58ed30ae426a506defbf555566c0a558cdbc13","reverse":["715962216736b87b41a61602ad4d5e9482e2b6fb895741033fdd904b36189c4d"]}"#
        ));
        assert!(first.canonical_json().contains(
            r#"{"forward":["715962216736b87b41a61602ad4d5e9482e2b6fb895741033fdd904b36189c4d"],"node_id":"5e231222c6adb5fe154872b472d9133f0cb230ad4497e87d4ebc8ac8cd234130","reverse":[]}"#
        ));
        assert!(first.canonical_json().contains(
            r#""indexes":{"edges":[{"id":"715962216736b87b41a61602ad4d5e9482e2b6fb895741033fdd904b36189c4d","position":0}],"nodes":[{"id":"4cc59666a045db50e8432ececd58ed30ae426a506defbf555566c0a558cdbc13","position":0},{"id":"5e231222c6adb5fe154872b472d9133f0cb230ad4497e87d4ebc8ac8cd234130","position":1}]}"#
        ));
        assert!(first
            .canonical_json()
            .contains("\"source_type\":\"markdown\""));
        assert!(first.canonical_json().contains("\"kind\":\"dependency\""));
        assert!(first.canonical_json().contains("\"schema_version\":1"));
        assert_eq!(first.nodes()[0].domains(), ["api", "architecture"]);
        assert_eq!(first.forward_edges(alpha.id()), [edge.id().to_owned()]);
        assert_eq!(first.reverse_edges(beta.id()), [edge.id().to_owned()]);
        assert!(matches!(
            StructuralGraphSnapshot::new(
                STRUCTURAL_GRAPH_SCHEMA_VERSION,
                None,
                hex('e'),
                hex('f'),
                hex('0'),
                vec![alpha.clone(), alpha.clone()],
                vec![],
                receipt(),
            ),
            Err(GraphError::DuplicateStructuralNodeId(_))
        ));
        assert!(matches!(
            StructuralGraphSnapshot::new(
                STRUCTURAL_GRAPH_SCHEMA_VERSION,
                None,
                hex('e'),
                hex('f'),
                hex('0'),
                vec![alpha.clone(), beta.clone()],
                vec![edge.clone(), edge.clone()],
                receipt(),
            ),
            Err(GraphError::DuplicateStructuralEdgeId(_))
        ));
        assert!(matches!(
            StructuralNode::new(
                "../escape.md",
                hex('1'),
                StructuralSourceType::Markdown,
                vec![],
                None,
                StructuralLifecycle::Current,
                StructuralAuthority::Unknown,
                StructuralGeneratedStatus::Source,
                vec![]
            ),
            Err(GraphError::InvalidStructuralPath(_))
        ));
        assert!(matches!(
            StructuralNode::new(
                "docs/bad.md",
                "ABC",
                StructuralSourceType::Markdown,
                vec![],
                None,
                StructuralLifecycle::Current,
                StructuralAuthority::Unknown,
                StructuralGeneratedStatus::Source,
                vec![]
            ),
            Err(GraphError::InvalidStructuralDigest(_))
        ));
        assert!(matches!(
            StructuralGraphSnapshot::new(
                2,
                None,
                hex('e'),
                hex('f'),
                hex('0'),
                vec![alpha.clone()],
                vec![],
                receipt()
            ),
            Err(GraphError::UnknownStructuralSchemaVersion(2))
        ));
        let absent = StructuralEdge::new(
            StructuralEdgeKind::Dependency,
            alpha.id().to_owned(),
            hex('9'),
            "docs/alpha.md:5",
            vec![],
            StructuralEdgeCertainty::Known,
        )
        .unwrap();
        assert!(matches!(
            StructuralGraphSnapshot::new(
                STRUCTURAL_GRAPH_SCHEMA_VERSION,
                None,
                hex('e'),
                hex('f'),
                hex('0'),
                vec![alpha.clone()],
                vec![absent],
                receipt()
            ),
            Err(GraphError::MissingStructuralTarget(_))
        ));
        let mut tampered = first.clone();
        tampered.snapshot_digest = hex('9');
        assert!(matches!(
            tampered.validate(),
            Err(GraphError::StructuralDigestMismatch { .. })
        ));
        let mut unordered = first;
        unordered.nodes.swap(0, 1);
        assert!(matches!(
            unordered.validate(),
            Err(GraphError::NonCanonicalStructuralOrder(_))
        ));
        let registry = crate::metadata::MetadataParserRegistry::new(512);
        assert_eq!(
            registry
                .structural_parsers()
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![
                StructuralSourceType::Text,
                StructuralSourceType::Code,
                StructuralSourceType::Markdown,
                StructuralSourceType::Mermaid,
                StructuralSourceType::Metadata,
                StructuralSourceType::PreextractedText,
            ]
        );
        assert_eq!(
            registry.structural_fingerprint(),
            "structural-metadata-parser-v1:max-input-bytes=512:text@v1,code@v1,markdown@v1,mermaid@v1,metadata@v1,preextracted-text@v1"
        );
        let code_source = b"// ---\n// owner: core\n// ---\npub fn entry() {}\n";
        let code = registry.parse_structural("src/lib.rs", code_source, None);
        assert_eq!(
            code,
            registry.parse_structural("src/lib.rs", code_source, None)
        );
        assert!(code
            .facts()
            .binary_search_by(|fact| fact
                .key()
                .cmp("owner")
                .then_with(|| fact.value().cmp("core")))
            .is_ok());
        assert!(registry
            .parse(
                "src/lib.rs",
                std::str::from_utf8(code_source).unwrap(),
                &crate::metadata::PackageDefaults::default()
            )
            .facts
            .contains(&crate::metadata::MetadataFact {
                key: "owner".to_owned(),
                value: "core".to_owned(),
                provenance: crate::metadata::FactProvenance::CommentedYaml,
                confidence: crate::metadata::FactConfidence::High,
                state: crate::metadata::FactState::Candidate,
            }));
        assert!(code
            .facts()
            .binary_search_by(|fact| fact
                .key()
                .cmp("symbol")
                .then_with(|| fact.value().cmp("entry")))
            .is_ok());
        let markdown =
            registry.parse_structural("docs/a.md", b"### Alpha\n[beta](docs/b.md)\n", None);
        assert!(markdown
            .facts()
            .binary_search_by(|fact| fact
                .key()
                .cmp("heading")
                .then_with(|| fact.value().cmp("Alpha")))
            .is_ok());
        assert!(markdown
            .edges()
            .binary_search_by(|edge| {
                edge.kind()
                    .cmp(&StructuralEdgeKind::SourceLink)
                    .then_with(|| edge.target().cmp("docs/b.md"))
            })
            .is_ok());
        let mermaid = registry.parse_structural("docs/flow.mmd", b"A --> B\n", None);
        assert_eq!(mermaid.edges()[0].kind(), StructuralEdgeKind::Dependency);
        assert_eq!(mermaid.edges()[0].target(), "B");
        let metadata = registry.parse_structural("config/app.toml", b"owner = \"core\"\n", None);
        assert_eq!(metadata.facts()[2].key(), "owner");
        assert_eq!(metadata.facts()[2].value(), "core");
        let absolute_path = registry.parse_structural("/docs/a.rs", &[0xff], None);
        assert_eq!(
            absolute_path.status(),
            crate::metadata::StructuralParseStatus::Rejected
        );
        assert_eq!(absolute_path.diagnostics(), ["invalid-path"]);
        let parent_path = registry.parse_structural("docs/../a.pdf", b"pdf", None);
        assert_eq!(
            parent_path.status(),
            crate::metadata::StructuralParseStatus::Rejected
        );
        assert_eq!(parent_path.diagnostics(), ["invalid-path"]);
        assert_eq!(
            registry
                .parse_structural("docs/a.pdf", b"pdf", None)
                .status(),
            crate::metadata::StructuralParseStatus::Rejected
        );
        assert_eq!(
            registry
                .parse_structural("src/a.rs", &[0xff], None)
                .status(),
            crate::metadata::StructuralParseStatus::Rejected
        );
        let bad = crate::metadata::PreextractedTextProvenance::new(
            "a".repeat(64),
            "extractor",
            "1",
            "b".repeat(64),
        )
        .unwrap();
        assert_eq!(
            registry
                .parse_structural("docs/a.pdf", b"extracted", Some(bad))
                .status(),
            crate::metadata::StructuralParseStatus::Rejected
        );
        assert!(crate::metadata::PreextractedTextProvenance::new(
            "A".repeat(64),
            "",
            "1",
            "b".repeat(64)
        )
        .is_err());
        let extraction = crate::metadata::PreextractedTextProvenance::new(
            "a".repeat(64),
            "extractor",
            "1",
            "7c375be828a16923f65095998609312a73318a1c01d89d3699e69e53a0883fac",
        )
        .unwrap();
        let extracted =
            registry.parse_structural("docs/a.pdf", b"extracted", Some(extraction.clone()));
        assert_eq!(
            extracted.status(),
            crate::metadata::StructuralParseStatus::Accepted
        );
        assert!(extracted
            .facts()
            .binary_search_by(
                |fact| fact.key().cmp("original_artifact_digest").then_with(|| {
                    fact.value()
                        .cmp("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                })
            )
            .is_ok());
        assert!(extracted
            .facts()
            .binary_search_by(|fact| fact.key().cmp("extraction_digest").then_with(|| {
                fact.value()
                    .cmp("7c375be828a16923f65095998609312a73318a1c01d89d3699e69e53a0883fac")
            }))
            .is_ok());
        assert!(extracted
            .facts()
            .binary_search_by(|fact| fact
                .key()
                .cmp("extractor")
                .then_with(|| fact.value().cmp("extractor@1")))
            .is_ok());
        assert_eq!(extracted.provenance(), Some(&extraction));
    }
}
