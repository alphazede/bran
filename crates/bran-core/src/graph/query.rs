//! Read-only bounded graph queries. Only `Known` edges establish reachability.

use super::{
    Confidence, EdgeCertainty, EdgeId, EdgeInput, EdgeRelationship, KnowledgeGraph, NodeFacts,
    NodeId, NodeInput, NodeRole, Provenance, StructuralAuthority, StructuralEdgeCertainty,
    StructuralEdgeKind, StructuralGeneratedStatus, StructuralGraphSnapshot, StructuralLifecycle,
    StructuralNode,
};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
};

/// Caller-provided bounds. Zero is valid and returns an explicitly truncated result when needed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueryBounds {
    pub max_results: usize,
    pub max_depth: usize,
}

impl QueryBounds {
    pub fn new(max_results: usize, max_depth: usize) -> Self {
        Self {
            max_results,
            max_depth,
        }
    }
}

/// A deterministic, bounded result list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bounded<T> {
    pub items: Vec<T>,
    pub truncated: bool,
}

/// A graph node with scanner evidence retained verbatim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeEvidence {
    pub id: NodeId,
    pub role: NodeRole,
    pub provenance: Provenance,
    pub confidence: Confidence,
    pub facts: NodeFacts,
}

/// A graph edge with candidate certainty and scanner evidence retained verbatim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EdgeEvidence {
    pub id: EdgeId,
    pub source: NodeId,
    pub target: NodeId,
    pub provenance: Provenance,
    pub confidence: Confidence,
    pub certainty: EdgeCertainty,
    pub relationship: EdgeRelationship,
}

/// A node query whose requested node may not exist.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NodeQuery<T> {
    Found(Bounded<T>),
    Missing(NodeId),
}

pub type FindResult = Bounded<NodeEvidence>;

/// A deterministic semantic index over evidence already retained by the graph.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum QueryIndex {
    Path,
    Title,
    Tag,
    Canonical,
}

/// Exact evidence explaining why an indexed node was selected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelectionReason {
    Exact { index: QueryIndex, value: String },
    Partial { index: QueryIndex, value: String },
    Related(EdgeEvidence),
    Backlink(EdgeEvidence),
}

/// A selected node plus deterministic, reviewable selection evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryHit {
    pub node: NodeEvidence,
    pub why: SelectionReason,
}

/// A direct source consuming the requested target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Consumer {
    pub node: NodeEvidence,
    pub edge: EdgeEvidence,
}

/// A node reached by reverse impact through one established edge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Impact {
    pub node: NodeEvidence,
    pub via: EdgeEvidence,
    pub depth: usize,
}

/// The non-speculative explanation for a reachability classification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReachabilityReason {
    Entrypoint,
    KnownPath { depth: usize },
    TestRole,
    GeneratedRole,
    NoKnownPath,
    Uncertain(EdgeEvidence),
    DepthBound,
    PartialCoverage,
}

/// Reachability is established only by `Known` edges.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reachability {
    Active(ReachabilityReason),
    Supporting(ReachabilityReason),
    TestOnly(ReachabilityReason),
    Generated(ReachabilityReason),
    Unreachable(ReachabilityReason),
    Unknown(ReachabilityReason),
}

/// A node classification with evidence and explicit traversal completeness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Classification {
    pub node: NodeEvidence,
    pub outcome: Reachability,
    pub truncated: bool,
}

/// Evidence for a safe zombie candidate; uncertain candidates are excluded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Zombie {
    pub node: NodeEvidence,
    pub reason: ReachabilityReason,
}

/// A shortest known-only trail, including the start and target nodes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Trail {
    pub nodes: Vec<NodeEvidence>,
    pub edges: Vec<EdgeEvidence>,
    pub truncated: bool,
}

/// Guided-trail status, including explicit absence and bounded incompleteness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrailStatus {
    Found(Trail),
    MissingStart(NodeId),
    MissingTarget(NodeId),
    Unreachable { truncated: bool },
}

pub const MAX_RANKING_SEED_BYTES: usize = 256;
pub const MAX_RANKING_SELECTION: usize = 4_096;
pub const MAX_RANKING_CANDIDATES: usize = 1_000_000;
const MAX_REQUIRED_DOMAINS: usize = 64;
const MAX_REQUIRED_DOMAIN_BYTES: usize = 128;
const FREQUENCY_CAP_BASIS_POINTS: u16 = 500;

/// The closed enterprise SDLC protocol accepted by structural ranking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SdlcStage {
    LegacyReconstruction,
    RequirementsReconciliation,
    ReplacementSpecification,
    Design,
    RegisterDriverContract,
    Implementation,
    ValidationRelease,
}

impl SdlcStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LegacyReconstruction => "legacy-reconstruction",
            Self::RequirementsReconciliation => "requirements-reconciliation",
            Self::ReplacementSpecification => "replacement-specification",
            Self::Design => "design",
            Self::RegisterDriverContract => "register-driver-contract",
            Self::Implementation => "implementation",
            Self::ValidationRelease => "validation-release",
        }
    }

    pub fn parse(value: &str) -> Result<Self, RankingError> {
        match value {
            "legacy-reconstruction" => Ok(Self::LegacyReconstruction),
            "requirements-reconciliation" => Ok(Self::RequirementsReconciliation),
            "replacement-specification" => Ok(Self::ReplacementSpecification),
            "design" => Ok(Self::Design),
            "register-driver-contract" => Ok(Self::RegisterDriverContract),
            "implementation" => Ok(Self::Implementation),
            "validation-release" => Ok(Self::ValidationRelease),
            _ => Err(RankingError::UnknownStage(value.to_owned())),
        }
    }

    const fn evidence_kind(self) -> StructuralEdgeKind {
        match self {
            Self::LegacyReconstruction => StructuralEdgeKind::Architecture,
            Self::RequirementsReconciliation | Self::ReplacementSpecification => {
                StructuralEdgeKind::Requirement
            }
            Self::Design | Self::RegisterDriverContract => StructuralEdgeKind::Design,
            Self::Implementation => StructuralEdgeKind::Implementation,
            Self::ValidationRelease => StructuralEdgeKind::Validation,
        }
    }
}

/// Closed structural evidence classes. No body text is inspected to infer these values.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EvidenceClass {
    Requirement,
    Design,
    Implementation,
    Validation,
    Architecture,
    BusinessFlow,
}

impl EvidenceClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requirement => "requirement",
            Self::Design => "design",
            Self::Implementation => "implementation",
            Self::Validation => "validation",
            Self::Architecture => "architecture",
            Self::BusinessFlow => "business-flow",
        }
    }

    pub fn parse(value: &str) -> Result<Self, RankingError> {
        match value {
            "requirement" => Ok(Self::Requirement),
            "design" => Ok(Self::Design),
            "implementation" => Ok(Self::Implementation),
            "validation" => Ok(Self::Validation),
            "architecture" => Ok(Self::Architecture),
            "business-flow" => Ok(Self::BusinessFlow),
            _ => Err(RankingError::UnknownEvidenceClass(value.to_owned())),
        }
    }

    const fn edge_kind(self) -> StructuralEdgeKind {
        match self {
            Self::Requirement => StructuralEdgeKind::Requirement,
            Self::Design => StructuralEdgeKind::Design,
            Self::Implementation => StructuralEdgeKind::Implementation,
            Self::Validation => StructuralEdgeKind::Validation,
            Self::Architecture => StructuralEdgeKind::Architecture,
            Self::BusinessFlow => StructuralEdgeKind::BusinessFlow,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RankingLimits {
    pub selection_limit: usize,
}

impl RankingLimits {
    pub fn new(selection_limit: usize) -> Result<Self, RankingError> {
        if selection_limit == 0 || selection_limit > MAX_RANKING_SELECTION {
            return Err(RankingError::InvalidSelectionLimit(selection_limit));
        }
        Ok(Self { selection_limit })
    }
}

/// Validated, immutable input for one authority-first ranking operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StageRankingRequest {
    pub seed: String,
    pub stage: SdlcStage,
    pub required_domains: Vec<String>,
    pub required_evidence_classes: Vec<EvidenceClass>,
    pub limits: RankingLimits,
}

impl StageRankingRequest {
    pub fn new(
        seed: impl Into<String>,
        stage: SdlcStage,
        required_domains: Vec<String>,
        required_evidence_classes: Vec<EvidenceClass>,
        limits: RankingLimits,
    ) -> Result<Self, RankingError> {
        let seed = seed.into();
        if seed.is_empty() {
            return Err(RankingError::EmptySeed);
        }
        if seed.len() > MAX_RANKING_SEED_BYTES {
            return Err(RankingError::SeedTooLong(seed.len()));
        }
        if required_domains.len() > MAX_REQUIRED_DOMAINS {
            return Err(RankingError::TooManyRequiredDomains(required_domains.len()));
        }
        let mut seen_domains = BTreeSet::new();
        for domain in &required_domains {
            if domain.is_empty() || domain.len() > MAX_REQUIRED_DOMAIN_BYTES {
                return Err(RankingError::InvalidRequiredDomain(domain.clone()));
            }
            if !seen_domains.insert(domain.clone()) {
                return Err(RankingError::DuplicateRequiredDomain(domain.clone()));
            }
        }
        let mut seen_classes = BTreeSet::new();
        for evidence_class in &required_evidence_classes {
            if !seen_classes.insert(*evidence_class) {
                return Err(RankingError::DuplicateEvidenceClass(*evidence_class));
            }
        }
        Ok(Self {
            seed,
            stage,
            required_domains,
            required_evidence_classes,
            limits,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FrozenFrequencyValue {
    raw_basis_points: u64,
    capped_basis_points: u16,
    cap_applied: bool,
}

/// Frozen, scrubbed evaluator-qualified history only.
///
/// Qualification excludes bot, personal, current-attempt, and cross-attempt activity. The
/// ranker deliberately has no API for mutable events or current-attempt updates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrozenHistoricalFrequency {
    aggregate_digest: String,
    values: BTreeMap<String, FrozenFrequencyValue>,
}

impl FrozenHistoricalFrequency {
    pub const QUALIFICATION: &'static str =
        "frozen scrubbed history; excludes bot, personal, current-attempt, and cross-attempt activity";

    pub fn new(
        aggregate_digest: impl Into<String>,
        raw_basis_points: BTreeMap<String, i64>,
    ) -> Result<Self, RankingError> {
        let aggregate_digest = aggregate_digest.into();
        if !is_lowercase_sha256(&aggregate_digest) {
            return Err(RankingError::MalformedFrequencyDigest(aggregate_digest));
        }
        let mut values = BTreeMap::new();
        for (node_id, raw) in raw_basis_points {
            let raw_basis_points =
                u64::try_from(raw).map_err(|_| RankingError::InvalidFrequencyValue {
                    node_id: node_id.clone(),
                    raw,
                })?;
            let capped_basis_points =
                raw_basis_points.min(u64::from(FREQUENCY_CAP_BASIS_POINTS)) as u16;
            values.insert(
                node_id,
                FrozenFrequencyValue {
                    raw_basis_points,
                    capped_basis_points,
                    cap_applied: raw_basis_points > u64::from(FREQUENCY_CAP_BASIS_POINTS),
                },
            );
        }
        Ok(Self {
            aggregate_digest,
            values,
        })
    }

    pub fn aggregate_digest(&self) -> &str {
        &self.aggregate_digest
    }

    fn value(&self, node_id: &str) -> FrozenFrequencyValue {
        self.values
            .get(node_id)
            .copied()
            .unwrap_or(FrozenFrequencyValue {
                raw_basis_points: 0,
                capped_basis_points: 0,
                cap_applied: false,
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EligibilityKey {
    Eligible,
    Ineligible,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AuthorityKey {
    CanonicalOwned,
    ApprovedOwned,
    CanonicalUnowned,
    ApprovedUnowned,
    SupportingOwned,
    SupportingUnowned,
    UnknownOwned,
    UnknownUnowned,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RevisionKey {
    Current,
    Predecessor,
    Superseded,
    Draft,
    Archive,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ArtifactKey {
    Source,
    GeneratedOrPreextracted,
    Archive,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RelationshipDistanceKey {
    Seed,
    Direct,
    Absent,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RelevanceKey {
    None,
    Partial,
    Exact,
}

/// Exact ten-part authority-first comparison key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StageRankKey {
    pub eligibility: EligibilityKey,
    pub authority: AuthorityKey,
    pub revision: RevisionKey,
    pub artifact: ArtifactKey,
    pub relationship_distance: RelationshipDistanceKey,
    pub relevance: RelevanceKey,
    pub required_contribution: u16,
    pub frequency_basis_points: u16,
    pub canonical_path: String,
    pub node_id: String,
}

impl Ord for StageRankKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.eligibility
            .cmp(&other.eligibility)
            .then_with(|| self.authority.cmp(&other.authority))
            .then_with(|| self.revision.cmp(&other.revision))
            .then_with(|| self.artifact.cmp(&other.artifact))
            .then_with(|| self.relationship_distance.cmp(&other.relationship_distance))
            .then_with(|| other.relevance.cmp(&self.relevance))
            .then_with(|| other.required_contribution.cmp(&self.required_contribution))
            .then_with(|| {
                other
                    .frequency_basis_points
                    .cmp(&self.frequency_basis_points)
            })
            .then_with(|| self.canonical_path.cmp(&other.canonical_path))
            .then_with(|| self.node_id.cmp(&other.node_id))
    }
}

impl PartialOrd for StageRankKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrequencyReceipt {
    pub raw_basis_points: u64,
    pub capped_basis_points: u16,
    pub cap_applied: bool,
    pub aggregate_digest: String,
    pub rank_key_position: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OmissionReason {
    SelectionLimit,
    LifecycleConflict,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RankingItemDecision {
    Selected,
    Omitted(OmissionReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RankingItemReceipt {
    pub node_id: String,
    pub canonical_path: String,
    pub rank_key: StageRankKey,
    pub frequency: FrequencyReceipt,
    pub decision: RankingItemDecision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotRun {
    NotRun,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraversalReceipt {
    pub status: NotRun,
    pub limits: NotRun,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConflictDecision {
    pub conflicts_visible: bool,
    pub node_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingRequiredEvidenceDecision {
    pub missing: bool,
    pub domains: Vec<String>,
    pub evidence_classes: Vec<EvidenceClass>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RankingReceipt {
    pub snapshot_digest: String,
    pub seed: String,
    pub stage: SdlcStage,
    pub required_domains: Vec<String>,
    pub required_evidence_classes: Vec<EvidenceClass>,
    pub candidate_count: usize,
    pub requested_selection_limit: usize,
    pub selected: Vec<RankingItemReceipt>,
    pub omitted_conflicts: Vec<RankingItemReceipt>,
    pub traversal: TraversalReceipt,
    pub conflicts: ConflictDecision,
    pub missing_required_evidence: MissingRequiredEvidenceDecision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RankingError {
    EmptySeed,
    SeedTooLong(usize),
    InvalidSelectionLimit(usize),
    TooManyRequiredDomains(usize),
    InvalidRequiredDomain(String),
    DuplicateRequiredDomain(String),
    DuplicateEvidenceClass(EvidenceClass),
    UnknownStage(String),
    UnknownEvidenceClass(String),
    MalformedFrequencyDigest(String),
    InvalidFrequencyValue { node_id: String, raw: i64 },
    FrequencyDigestMismatch { snapshot: String, aggregate: String },
    UnknownFrequencyNode(String),
    CandidateLimitExceeded(usize),
}

/// Closed, read-only relationship set accepted by structural traversal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralTraversalRequest {
    pub seed: String,
    pub relationships: Vec<StructuralEdgeKind>,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_edges: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuralTraversalTerminal {
    Complete,
    DepthLimit,
    NodeLimit,
    EdgeLimit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralPathStep {
    pub edge_id: String,
    pub kind: StructuralEdgeKind,
    pub source: String,
    pub target: String,
    pub source_locator: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralPathReceipt {
    pub node_id: String,
    pub steps: Vec<StructuralPathStep>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkippedStructuralEdge {
    pub edge_id: String,
    pub certainty: StructuralEdgeCertainty,
    pub reason: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralTraversalReceipt {
    pub snapshot_digest: String,
    pub seed: String,
    pub relationships: Vec<StructuralEdgeKind>,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_edges: usize,
    pub visited_ids: Vec<String>,
    pub paths: Vec<StructuralPathReceipt>,
    pub inspected_edge_count: usize,
    pub accepted_edge_count: usize,
    pub skipped_edges: Vec<SkippedStructuralEdge>,
    pub terminal: StructuralTraversalTerminal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StructuralTraversalError {
    MissingSeed(String),
    EmptyRelationships,
    DuplicateRelationship(StructuralEdgeKind),
    UnsupportedRelationship(StructuralEdgeKind),
    InvalidLimits {
        max_depth: usize,
        max_nodes: usize,
        max_edges: usize,
    },
    ArithmeticOverflow(&'static str),
}

/// Stateless deterministic BFS over immutable structural adjacency.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Traversal;

impl Traversal {
    pub const MAX_DEPTH: usize = 64;
    pub const MAX_NODES: usize = 4_096;
    pub const MAX_EDGES: usize = 65_536;

    pub fn run(
        snapshot: &StructuralGraphSnapshot,
        request: &StructuralTraversalRequest,
    ) -> Result<StructuralTraversalReceipt, StructuralTraversalError> {
        if snapshot.node(&request.seed).is_none() {
            return Err(StructuralTraversalError::MissingSeed(request.seed.clone()));
        }
        if request.relationships.is_empty() {
            return Err(StructuralTraversalError::EmptyRelationships);
        }
        if request.max_depth == 0
            || request.max_depth > Self::MAX_DEPTH
            || request.max_nodes == 0
            || request.max_nodes > Self::MAX_NODES
            || request.max_edges == 0
            || request.max_edges > Self::MAX_EDGES
        {
            return Err(StructuralTraversalError::InvalidLimits {
                max_depth: request.max_depth,
                max_nodes: request.max_nodes,
                max_edges: request.max_edges,
            });
        }
        let mut allowed = BTreeSet::new();
        for kind in &request.relationships {
            if !traversable_structural_kind(*kind) {
                return Err(StructuralTraversalError::UnsupportedRelationship(*kind));
            }
            if !allowed.insert(*kind) {
                return Err(StructuralTraversalError::DuplicateRelationship(*kind));
            }
        }
        let mut visited = BTreeSet::from([request.seed.clone()]);
        let mut queue = VecDeque::from([(request.seed.clone(), 0usize, Vec::new())]);
        let mut visited_ids = vec![request.seed.clone()];
        let mut paths = vec![StructuralPathReceipt {
            node_id: request.seed.clone(),
            steps: Vec::new(),
        }];
        let mut inspected = 0usize;
        let mut accepted = 0usize;
        let mut skipped = Vec::new();
        let mut terminal = StructuralTraversalTerminal::Complete;
        while let Some((node_id, depth, path)) = queue.pop_front() {
            if depth == request.max_depth {
                if has_traversable_neighbor(snapshot, &node_id, &allowed, &visited) {
                    terminal = StructuralTraversalTerminal::DepthLimit;
                    break;
                }
                continue;
            }
            let mut edges: Vec<_> = snapshot
                .forward_edges(&node_id)
                .iter()
                .chain(snapshot.reverse_edges(&node_id))
                .filter_map(|id| snapshot.edge(id))
                .collect();
            edges.sort_by(|a, b| {
                (a.kind(), a.source(), a.target(), a.source_locator(), a.id()).cmp(&(
                    b.kind(),
                    b.source(),
                    b.target(),
                    b.source_locator(),
                    b.id(),
                ))
            });
            for edge in edges {
                if !allowed.contains(&edge.kind()) {
                    continue;
                }
                if inspected == request.max_edges {
                    terminal = StructuralTraversalTerminal::EdgeLimit;
                    break;
                }
                inspected = inspected.checked_add(1).ok_or(
                    StructuralTraversalError::ArithmeticOverflow("inspected edges"),
                )?;
                if edge.certainty() != StructuralEdgeCertainty::Known {
                    skipped.push(SkippedStructuralEdge {
                        edge_id: edge.id().to_owned(),
                        certainty: edge.certainty(),
                        reason: "non-known",
                    });
                    continue;
                }
                let next = if edge.source() == node_id {
                    edge.target()
                } else {
                    edge.source()
                };
                if snapshot.node(next).is_none() {
                    skipped.push(SkippedStructuralEdge {
                        edge_id: edge.id().to_owned(),
                        certainty: edge.certainty(),
                        reason: "missing-target",
                    });
                    continue;
                }
                if visited.contains(next) {
                    continue;
                }
                if visited.len() == request.max_nodes {
                    terminal = StructuralTraversalTerminal::NodeLimit;
                    break;
                }
                accepted =
                    accepted
                        .checked_add(1)
                        .ok_or(StructuralTraversalError::ArithmeticOverflow(
                            "accepted edges",
                        ))?;
                visited.insert(next.to_owned());
                let mut next_path = path.clone();
                next_path.push(StructuralPathStep {
                    edge_id: edge.id().to_owned(),
                    kind: edge.kind(),
                    source: edge.source().to_owned(),
                    target: edge.target().to_owned(),
                    source_locator: edge.source_locator().to_owned(),
                });
                visited_ids.push(next.to_owned());
                paths.push(StructuralPathReceipt {
                    node_id: next.to_owned(),
                    steps: next_path.clone(),
                });
                queue.push_back((next.to_owned(), depth + 1, next_path));
            }
            if terminal != StructuralTraversalTerminal::Complete {
                break;
            }
        }
        Ok(StructuralTraversalReceipt {
            snapshot_digest: snapshot.snapshot_digest().to_owned(),
            seed: request.seed.clone(),
            relationships: request.relationships.clone(),
            max_depth: request.max_depth,
            max_nodes: request.max_nodes,
            max_edges: request.max_edges,
            visited_ids,
            paths,
            inspected_edge_count: inspected,
            accepted_edge_count: accepted,
            skipped_edges: skipped,
            terminal,
        })
    }
}

fn traversable_structural_kind(kind: StructuralEdgeKind) -> bool {
    matches!(
        kind,
        StructuralEdgeKind::Dependency
            | StructuralEdgeKind::Impact
            | StructuralEdgeKind::Requirement
            | StructuralEdgeKind::Design
            | StructuralEdgeKind::Implementation
            | StructuralEdgeKind::Validation
            | StructuralEdgeKind::Domain
            | StructuralEdgeKind::Architecture
            | StructuralEdgeKind::BusinessFlow
    )
}

fn has_traversable_neighbor(
    snapshot: &StructuralGraphSnapshot,
    node_id: &str,
    allowed: &BTreeSet<StructuralEdgeKind>,
    visited: &BTreeSet<String>,
) -> bool {
    snapshot
        .forward_edges(node_id)
        .iter()
        .chain(snapshot.reverse_edges(node_id))
        .filter_map(|id| snapshot.edge(id))
        .any(|edge| {
            allowed.contains(&edge.kind())
                && edge.certainty() == StructuralEdgeCertainty::Known
                && snapshot
                    .node(if edge.source() == node_id {
                        edge.target()
                    } else {
                        edge.source()
                    })
                    .is_some_and(|node| !visited.contains(node.id()))
        })
}

/// One immutable-snapshot ranking operation. It owns no traversal, packet, or event state.
pub struct StageRanker<'a> {
    snapshot: &'a StructuralGraphSnapshot,
    request: StageRankingRequest,
    frequency: &'a FrozenHistoricalFrequency,
}

impl<'a> StageRanker<'a> {
    pub fn new(
        snapshot: &'a StructuralGraphSnapshot,
        request: StageRankingRequest,
        frequency: &'a FrozenHistoricalFrequency,
    ) -> Result<Self, RankingError> {
        if snapshot.nodes().len() > MAX_RANKING_CANDIDATES {
            return Err(RankingError::CandidateLimitExceeded(snapshot.nodes().len()));
        }
        if snapshot.frozen_frequency_digest() != frequency.aggregate_digest() {
            return Err(RankingError::FrequencyDigestMismatch {
                snapshot: snapshot.frozen_frequency_digest().to_owned(),
                aggregate: frequency.aggregate_digest().to_owned(),
            });
        }
        if let Some(node_id) = frequency
            .values
            .keys()
            .find(|node_id| snapshot.node(node_id).is_none())
        {
            return Err(RankingError::UnknownFrequencyNode(node_id.clone()));
        }
        Ok(Self {
            snapshot,
            request,
            frequency,
        })
    }

    pub fn rank(self) -> RankingReceipt {
        let seed_ids = seed_node_ids(self.snapshot, &self.request.seed);
        let mut candidates: Vec<_> = self
            .snapshot
            .nodes()
            .iter()
            .map(|node| {
                candidate_receipt(
                    self.snapshot,
                    &self.request,
                    self.frequency,
                    &seed_ids,
                    node,
                )
            })
            .collect();
        candidates.sort_by(|left, right| left.rank_key.cmp(&right.rank_key));

        let mut selected = Vec::new();
        let mut omitted_conflicts = Vec::new();
        for mut item in candidates {
            if item.rank_key.eligibility == EligibilityKey::Eligible
                && selected.len() < self.request.limits.selection_limit
            {
                item.decision = RankingItemDecision::Selected;
                selected.push(item);
            } else {
                item.decision = RankingItemDecision::Omitted(
                    if item.rank_key.eligibility == EligibilityKey::Ineligible {
                        OmissionReason::LifecycleConflict
                    } else {
                        OmissionReason::SelectionLimit
                    },
                );
                omitted_conflicts.push(item);
            }
        }

        let conflict_node_ids: Vec<String> = omitted_conflicts
            .iter()
            .filter(|item| {
                item.decision == RankingItemDecision::Omitted(OmissionReason::LifecycleConflict)
            })
            .map(|item| item.node_id.clone())
            .collect();
        let missing_required_evidence =
            missing_evidence_decision(self.snapshot, &self.request, &seed_ids, &selected);
        RankingReceipt {
            snapshot_digest: self.snapshot.snapshot_digest().to_owned(),
            seed: self.request.seed,
            stage: self.request.stage,
            required_domains: self.request.required_domains,
            required_evidence_classes: self.request.required_evidence_classes,
            candidate_count: self.snapshot.nodes().len(),
            requested_selection_limit: self.request.limits.selection_limit,
            selected,
            omitted_conflicts,
            traversal: TraversalReceipt {
                status: NotRun::NotRun,
                limits: NotRun::NotRun,
            },
            conflicts: ConflictDecision {
                conflicts_visible: !conflict_node_ids.is_empty(),
                node_ids: conflict_node_ids,
            },
            missing_required_evidence,
        }
    }
}

fn candidate_receipt(
    snapshot: &StructuralGraphSnapshot,
    request: &StageRankingRequest,
    frequency: &FrozenHistoricalFrequency,
    seed_ids: &BTreeSet<String>,
    node: &StructuralNode,
) -> RankingItemReceipt {
    let direct_kinds = direct_edge_kinds(snapshot, seed_ids, node.id());
    let frequency_value = frequency.value(node.id());
    let rank_key = StageRankKey {
        eligibility: if node.lifecycle() == StructuralLifecycle::Current {
            EligibilityKey::Eligible
        } else {
            EligibilityKey::Ineligible
        },
        authority: authority_key(node),
        revision: revision_key(snapshot, node),
        artifact: artifact_key(node),
        relationship_distance: if seed_ids.contains(node.id()) {
            RelationshipDistanceKey::Seed
        } else if direct_kinds.is_empty() {
            RelationshipDistanceKey::Absent
        } else {
            RelationshipDistanceKey::Direct
        },
        relevance: relevance_key(node, &request.seed),
        required_contribution: required_contribution(request, node, &direct_kinds),
        frequency_basis_points: frequency_value.capped_basis_points,
        canonical_path: node.path().to_owned(),
        node_id: node.id().to_owned(),
    };
    RankingItemReceipt {
        node_id: node.id().to_owned(),
        canonical_path: node.path().to_owned(),
        rank_key,
        frequency: FrequencyReceipt {
            raw_basis_points: frequency_value.raw_basis_points,
            capped_basis_points: frequency_value.capped_basis_points,
            cap_applied: frequency_value.cap_applied,
            aggregate_digest: frequency.aggregate_digest().to_owned(),
            rank_key_position: 8,
        },
        decision: RankingItemDecision::Selected,
    }
}

fn seed_node_ids(snapshot: &StructuralGraphSnapshot, seed: &str) -> BTreeSet<String> {
    snapshot
        .nodes()
        .iter()
        .filter(|node| node.id() == seed || node.path() == seed)
        .map(|node| node.id().to_owned())
        .collect()
}

fn direct_edge_kinds(
    snapshot: &StructuralGraphSnapshot,
    seed_ids: &BTreeSet<String>,
    node_id: &str,
) -> BTreeSet<StructuralEdgeKind> {
    let mut kinds = BTreeSet::new();
    for edge_id in snapshot
        .forward_edges(node_id)
        .iter()
        .chain(snapshot.reverse_edges(node_id))
    {
        let edge = snapshot
            .edge(edge_id)
            .expect("snapshot owns adjacency edge");
        if edge.certainty() != StructuralEdgeCertainty::Known {
            continue;
        }
        let adjacent_to_seed = seed_ids.contains(node_id)
            || (edge.source() == node_id && seed_ids.contains(edge.target()))
            || (edge.target() == node_id && seed_ids.contains(edge.source()));
        if adjacent_to_seed {
            kinds.insert(edge.kind());
        }
    }
    kinds
}

fn authority_key(node: &StructuralNode) -> AuthorityKey {
    match (node.authority(), node.owner().is_some()) {
        (StructuralAuthority::Canonical, true) => AuthorityKey::CanonicalOwned,
        (StructuralAuthority::Approved, true) => AuthorityKey::ApprovedOwned,
        (StructuralAuthority::Canonical, false) => AuthorityKey::CanonicalUnowned,
        (StructuralAuthority::Approved, false) => AuthorityKey::ApprovedUnowned,
        (StructuralAuthority::Supporting, true) => AuthorityKey::SupportingOwned,
        (StructuralAuthority::Supporting, false) => AuthorityKey::SupportingUnowned,
        (StructuralAuthority::Unknown, true) => AuthorityKey::UnknownOwned,
        (StructuralAuthority::Unknown, false) => AuthorityKey::UnknownUnowned,
    }
}

fn revision_key(snapshot: &StructuralGraphSnapshot, node: &StructuralNode) -> RevisionKey {
    match node.lifecycle() {
        StructuralLifecycle::Current => RevisionKey::Current,
        StructuralLifecycle::Superseded if is_predecessor(snapshot, node.id()) => {
            RevisionKey::Predecessor
        }
        StructuralLifecycle::Superseded => RevisionKey::Superseded,
        StructuralLifecycle::Draft => RevisionKey::Draft,
        StructuralLifecycle::Archive => RevisionKey::Archive,
        StructuralLifecycle::Rejected => RevisionKey::Rejected,
    }
}

fn is_predecessor(snapshot: &StructuralGraphSnapshot, node_id: &str) -> bool {
    snapshot.edges().iter().any(|edge| {
        if edge.certainty() != StructuralEdgeCertainty::Known
            || !matches!(
                edge.kind(),
                StructuralEdgeKind::Predecessor | StructuralEdgeKind::Supersedes
            )
        {
            return false;
        }
        let other = if edge.source() == node_id {
            edge.target()
        } else if edge.target() == node_id {
            edge.source()
        } else {
            return false;
        };
        snapshot
            .node(other)
            .is_some_and(|node| node.lifecycle() == StructuralLifecycle::Current)
    })
}

fn artifact_key(node: &StructuralNode) -> ArtifactKey {
    match node.lifecycle() {
        StructuralLifecycle::Archive => ArtifactKey::Archive,
        StructuralLifecycle::Rejected => ArtifactKey::Rejected,
        _ => match node.generated_status() {
            StructuralGeneratedStatus::Source => ArtifactKey::Source,
            StructuralGeneratedStatus::Generated | StructuralGeneratedStatus::Preextracted => {
                ArtifactKey::GeneratedOrPreextracted
            }
        },
    }
}

fn relevance_key(node: &StructuralNode, seed: &str) -> RelevanceKey {
    let exact = node.id() == seed
        || node.path() == seed
        || node.owner() == Some(seed)
        || node.domains().iter().any(|domain| domain == seed);
    if exact {
        return RelevanceKey::Exact;
    }
    let partial = node.id().contains(seed)
        || node.path().contains(seed)
        || node.owner().is_some_and(|owner| owner.contains(seed))
        || node.domains().iter().any(|domain| domain.contains(seed));
    if partial {
        RelevanceKey::Partial
    } else {
        RelevanceKey::None
    }
}

fn required_contribution(
    request: &StageRankingRequest,
    node: &StructuralNode,
    direct_kinds: &BTreeSet<StructuralEdgeKind>,
) -> u16 {
    let domain_count = request
        .required_domains
        .iter()
        .filter(|required| node.domains().contains(required))
        .count();
    let evidence_count = request
        .required_evidence_classes
        .iter()
        .filter(|class| direct_kinds.contains(&class.edge_kind()))
        .count();
    let stage_count = usize::from(direct_kinds.contains(&request.stage.evidence_kind()));
    (domain_count + evidence_count + stage_count) as u16
}

fn missing_evidence_decision(
    snapshot: &StructuralGraphSnapshot,
    request: &StageRankingRequest,
    seed_ids: &BTreeSet<String>,
    selected: &[RankingItemReceipt],
) -> MissingRequiredEvidenceDecision {
    let mut covered_domains = BTreeSet::new();
    let mut covered_kinds = BTreeSet::new();
    for item in selected {
        let node = snapshot
            .node(&item.node_id)
            .expect("ranking receipt node belongs to snapshot");
        covered_domains.extend(node.domains().iter().cloned());
        covered_kinds.extend(direct_edge_kinds(snapshot, seed_ids, node.id()));
    }
    let domains: Vec<_> = request
        .required_domains
        .iter()
        .filter(|domain| !covered_domains.contains(*domain))
        .cloned()
        .collect();
    let evidence_classes: Vec<_> = request
        .required_evidence_classes
        .iter()
        .filter(|class| !covered_kinds.contains(&class.edge_kind()))
        .copied()
        .collect();
    MissingRequiredEvidenceDecision {
        missing: !domains.is_empty() || !evidence_classes.is_empty(),
        domains,
        evidence_classes,
    }
}

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

impl KnowledgeGraph {
    /// Deterministically finds nodes by stable identity or retained provenance text.
    pub fn find(&self, needle: &str, max_results: usize) -> FindResult {
        let mut matches: Vec<_> = self
            .nodes
            .iter()
            .filter_map(|node| best_match(node, needle).map(|matched| (node, matched)))
            .collect();
        rank_matches(&mut matches);
        bounded(
            matches.into_iter().map(|(node, _)| node_evidence(node)),
            max_results,
        )
    }

    /// Looks up one explicit evidence index without searching document bodies.
    pub fn lookup(&self, index: QueryIndex, needle: &str, max_results: usize) -> Bounded<QueryHit> {
        let mut matches: Vec<_> = self
            .nodes
            .iter()
            .filter_map(|node| index_match(node, index, needle).map(|matched| (node, matched)))
            .collect();
        rank_matches(&mut matches);
        bounded(
            matches.into_iter().map(|(node, matched)| QueryHit {
                node: node_evidence(node),
                why: matched.reason,
            }),
            max_results,
        )
    }

    /// Returns direct neighbors established by bounded `Known` edges only.
    pub fn related(&self, start: &NodeId, bounds: QueryBounds) -> NodeQuery<QueryHit> {
        self.edge_neighbors(start, bounds, false)
    }

    /// Returns bounded reverse-edge sources while retaining candidate certainty and provenance.
    pub fn backlinks(&self, target: &NodeId, bounds: QueryBounds) -> NodeQuery<QueryHit> {
        self.edge_neighbors(target, bounds, true)
    }

    /// Returns direct consumers, retaining every edge certainty as candidate evidence.
    pub fn consumers(&self, target: &NodeId, max_results: usize) -> NodeQuery<Consumer> {
        if self.node(target).is_none() {
            return NodeQuery::Missing(target.clone());
        }
        let mut items = Vec::new();
        for edge_id in self.reverse_edges(target) {
            if items.len() == max_results {
                return NodeQuery::Found(Bounded {
                    items,
                    truncated: true,
                });
            }
            let edge = self.edge(edge_id).expect("topology owns adjacency edges");
            let node = self.node(edge.source()).expect("validated edge source");
            items.push(Consumer {
                node: node_evidence(node),
                edge: edge_evidence(edge),
            });
        }
        NodeQuery::Found(Bounded {
            items,
            truncated: false,
        })
    }

    /// Breadth-first reverse impact over `Known` edges, with cycles visited once.
    pub fn impact(&self, start: &NodeId, bounds: QueryBounds) -> NodeQuery<Impact> {
        if self.node(start).is_none() {
            return NodeQuery::Missing(start.clone());
        }
        let mut items = Vec::new();
        let mut seen = HashSet::from([start.clone()]);
        let mut queue = VecDeque::from([(start.clone(), 0usize)]);
        while let Some((current, depth)) = queue.pop_front() {
            for edge_id in self.reverse_edges(&current) {
                let edge = self.edge(edge_id).expect("topology owns adjacency edges");
                if edge.certainty() != EdgeCertainty::Known || seen.contains(edge.source()) {
                    continue;
                }
                if depth == bounds.max_depth || items.len() == bounds.max_results {
                    return NodeQuery::Found(Bounded {
                        items,
                        truncated: true,
                    });
                }
                let source = edge.source().clone();
                seen.insert(source.clone());
                queue.push_back((source.clone(), depth + 1));
                items.push(Impact {
                    node: node_evidence(self.node(&source).expect("validated edge source")),
                    via: edge_evidence(edge),
                    depth: depth + 1,
                });
            }
        }
        NodeQuery::Found(Bounded {
            items,
            truncated: false,
        })
    }

    /// Classifies a node without treating uncertain candidates as verified absence.
    pub fn reachability(&self, target: &NodeId, max_depth: usize) -> NodeQuery<Classification> {
        let Some(node) = self.node(target) else {
            return NodeQuery::Missing(target.clone());
        };
        let evidence = node_evidence(node);
        if node.facts().contains_value("coverage", "partial") {
            return NodeQuery::Found(Bounded {
                items: vec![Classification {
                    node: evidence,
                    outcome: Reachability::Unknown(ReachabilityReason::PartialCoverage),
                    truncated: false,
                }],
                truncated: false,
            });
        }
        let immediate = match node.role() {
            NodeRole::Entrypoint => Some(Reachability::Active(ReachabilityReason::Entrypoint)),
            NodeRole::Test => Some(Reachability::TestOnly(ReachabilityReason::TestRole)),
            NodeRole::Generated => Some(Reachability::Generated(ReachabilityReason::GeneratedRole)),
            _ => None,
        };
        if let Some(outcome) = immediate {
            return NodeQuery::Found(Bounded {
                items: vec![Classification {
                    node: evidence,
                    outcome,
                    truncated: false,
                }],
                truncated: false,
            });
        }

        let (depth, known_bounded) = self.known_distance_to(target, max_depth);
        let (uncertain, uncertain_bounded) = if depth.is_none() && !known_bounded {
            self.uncertain_path_to(target, max_depth)
        } else {
            (None, false)
        };
        let bounded = known_bounded || uncertain_bounded;
        let outcome = if let Some(depth) = depth {
            Reachability::Supporting(ReachabilityReason::KnownPath { depth })
        } else if known_bounded {
            Reachability::Unknown(ReachabilityReason::DepthBound)
        } else if let Some(edge) = uncertain {
            Reachability::Unknown(ReachabilityReason::Uncertain(edge))
        } else if uncertain_bounded {
            Reachability::Unknown(ReachabilityReason::DepthBound)
        } else {
            Reachability::Unreachable(ReachabilityReason::NoKnownPath)
        };
        NodeQuery::Found(Bounded {
            items: vec![Classification {
                node: evidence,
                outcome,
                truncated: bounded,
            }],
            truncated: false,
        })
    }

    /// Returns only nodes proven unreachable within the supplied known-edge depth bound.
    pub fn zombies(&self, bounds: QueryBounds) -> Bounded<Zombie> {
        let mut items = Vec::new();
        for id in self.node_ids() {
            let NodeQuery::Found(result) = self.reachability(&id, bounds.max_depth) else {
                continue;
            };
            let classification = &result.items[0];
            let Reachability::Unreachable(reason) = &classification.outcome else {
                continue;
            };
            if classification.truncated {
                continue;
            }
            if items.len() == bounds.max_results {
                return Bounded {
                    items,
                    truncated: true,
                };
            }
            items.push(Zombie {
                node: classification.node.clone(),
                reason: reason.clone(),
            });
        }
        Bounded {
            items,
            truncated: false,
        }
    }

    /// Finds the deterministic shortest `Known` trail. Equal paths use node then edge identity.
    pub fn trail(&self, start: &NodeId, target: &NodeId, bounds: QueryBounds) -> TrailStatus {
        if self.node(start).is_none() {
            return TrailStatus::MissingStart(start.clone());
        }
        if self.node(target).is_none() {
            return TrailStatus::MissingTarget(target.clone());
        }
        if start == target {
            return TrailStatus::Found(Trail {
                nodes: vec![node_evidence(self.node(start).expect("validated start"))],
                edges: vec![],
                truncated: false,
            });
        }
        let mut seen = HashSet::from([start.clone()]);
        let mut prior: HashMap<NodeId, (NodeId, EdgeId)> = HashMap::new();
        let mut queue = VecDeque::from([(start.clone(), 0usize)]);
        let mut truncated = false;
        while let Some((current, depth)) = queue.pop_front() {
            let mut adjacent: Vec<_> = self
                .forward_edges(&current)
                .iter()
                .filter_map(|id| self.edge(id))
                .filter(|edge| edge.certainty() == EdgeCertainty::Known)
                .collect();
            adjacent.sort_by_key(|edge| (edge.target().clone(), edge.id().clone()));
            for edge in adjacent {
                if seen.contains(edge.target()) {
                    continue;
                }
                if depth == bounds.max_depth || prior.len() == bounds.max_results {
                    truncated = true;
                    continue;
                }
                let next = edge.target().clone();
                prior.insert(next.clone(), (current.clone(), edge.id().clone()));
                if &next == target {
                    return TrailStatus::Found(
                        self.rebuild_trail(start, target, &prior, truncated),
                    );
                }
                seen.insert(next.clone());
                queue.push_back((next, depth + 1));
            }
        }
        TrailStatus::Unreachable { truncated }
    }

    fn known_distance_to(&self, target: &NodeId, max_depth: usize) -> (Option<usize>, bool) {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        for node in &self.nodes {
            if node.role() == NodeRole::Entrypoint {
                if node.id() == target {
                    return (Some(0), false);
                }
                seen.insert(node.id().clone());
                queue.push_back((node.id().clone(), 0usize));
            }
        }
        let mut bounded = false;
        while let Some((current, depth)) = queue.pop_front() {
            for edge_id in self.forward_edges(&current) {
                let edge = self.edge(edge_id).expect("topology owns adjacency edges");
                if edge.certainty() != EdgeCertainty::Known || seen.contains(edge.target()) {
                    continue;
                }
                if depth == max_depth {
                    bounded = true;
                    continue;
                }
                if edge.target() == target {
                    return (Some(depth + 1), false);
                }
                seen.insert(edge.target().clone());
                queue.push_back((edge.target().clone(), depth + 1));
            }
        }
        (None, bounded)
    }

    /// Finds a bounded forward path tainted by any non-`Known` edge.
    ///
    /// Both endpoints of a present uncertain edge are tainted immediately so
    /// incomplete scanner evidence cannot produce a negative conclusion. The
    /// originating edge remains attached while later `Known` edges propagate
    /// that taint to descendants.
    fn uncertain_path_to(&self, target: &NodeId, max_depth: usize) -> (Option<EdgeEvidence>, bool) {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        for edge in &self.edges {
            if edge.certainty() == EdgeCertainty::Known {
                continue;
            }
            let evidence = edge_evidence(edge);
            let source = edge.source().clone();
            if seen.insert(source.clone()) {
                queue.push_back((source, evidence.clone(), 0usize));
            }
            let target = edge.target().clone();
            if self.node(&target).is_some() && seen.insert(target.clone()) {
                queue.push_back((target, evidence, 0usize));
            }
        }

        let mut bounded = false;
        while let Some((current, evidence, depth)) = queue.pop_front() {
            if &current == target {
                return (Some(evidence), bounded);
            }
            for edge_id in self.forward_edges(&current) {
                let edge = self.edge(edge_id).expect("topology owns adjacency edges");
                if edge.certainty() != EdgeCertainty::Known || seen.contains(edge.target()) {
                    continue;
                }
                if depth == max_depth {
                    bounded = true;
                    continue;
                }
                let next = edge.target().clone();
                seen.insert(next.clone());
                queue.push_back((next, evidence.clone(), depth + 1));
            }
        }
        (None, bounded)
    }

    fn rebuild_trail(
        &self,
        start: &NodeId,
        target: &NodeId,
        prior: &HashMap<NodeId, (NodeId, EdgeId)>,
        truncated: bool,
    ) -> Trail {
        let mut node_ids = vec![target.clone()];
        let mut edge_ids = Vec::new();
        let mut current = target;
        while current != start {
            let (parent, edge) = prior
                .get(current)
                .expect("reached nodes retain predecessors");
            node_ids.push(parent.clone());
            edge_ids.push(edge.clone());
            current = parent;
        }
        node_ids.reverse();
        edge_ids.reverse();
        Trail {
            nodes: node_ids
                .iter()
                .map(|id| node_evidence(self.node(id).expect("trail node is indexed")))
                .collect(),
            edges: edge_ids
                .iter()
                .map(|id| edge_evidence(self.edge(id).expect("trail edge is indexed")))
                .collect(),
            truncated,
        }
    }

    fn edge_neighbors(
        &self,
        start: &NodeId,
        bounds: QueryBounds,
        backlinks_only: bool,
    ) -> NodeQuery<QueryHit> {
        if self.node(start).is_none() {
            return NodeQuery::Missing(start.clone());
        }
        let mut candidates = Vec::new();
        for edge_id in self.reverse_edges(start) {
            let edge = self.edge(edge_id).expect("topology owns adjacency edges");
            if !backlinks_only && edge.certainty() != EdgeCertainty::Known {
                continue;
            }
            candidates.push((edge.source().clone(), edge));
        }
        if !backlinks_only {
            for edge_id in self.forward_edges(start) {
                let edge = self.edge(edge_id).expect("topology owns adjacency edges");
                if edge.certainty() == EdgeCertainty::Known && self.node(edge.target()).is_some() {
                    candidates.push((edge.target().clone(), edge));
                }
            }
        }
        candidates.sort_by_key(|(node, edge)| (node.clone(), edge.id().clone()));
        let unavailable_depth = bounds.max_depth == 0 && !candidates.is_empty();
        let items = if unavailable_depth {
            Vec::new()
        } else {
            candidates
                .iter()
                .take(bounds.max_results)
                .map(|(id, edge)| QueryHit {
                    node: node_evidence(self.node(id).expect("validated adjacent node")),
                    why: if backlinks_only {
                        SelectionReason::Backlink(edge_evidence(edge))
                    } else {
                        SelectionReason::Related(edge_evidence(edge))
                    },
                })
                .collect()
        };
        NodeQuery::Found(Bounded {
            truncated: unavailable_depth || candidates.len() > items.len(),
            items,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Match {
    exact: bool,
    active: bool,
    canonical: bool,
    confidence: u8,
    freshness: String,
    reason: SelectionReason,
}

fn best_match(node: &NodeInput, needle: &str) -> Option<Match> {
    [
        QueryIndex::Path,
        QueryIndex::Title,
        QueryIndex::Tag,
        QueryIndex::Canonical,
    ]
    .into_iter()
    .filter_map(|index| index_match(node, index, needle))
    .max_by(|left, right| match_rank(left).cmp(&match_rank(right)))
}

fn index_match(node: &NodeInput, index: QueryIndex, needle: &str) -> Option<Match> {
    let values: Vec<&str> = match index {
        QueryIndex::Path => vec![
            node.id().as_str(),
            node.provenance().source(),
            node.provenance().locator(),
        ],
        QueryIndex::Title => fact_values(node, &["title"]),
        QueryIndex::Tag => fact_values(node, &["tags", "tag"]),
        QueryIndex::Canonical => fact_values(
            node,
            &["canonical", "authority", "resource", "status", "okf_status"],
        ),
    };
    let (exact, value) = values
        .iter()
        .copied()
        .find(|value| *value == needle)
        .map(|value| (true, value))
        .or_else(|| {
            values
                .iter()
                .copied()
                .find(|value| value.contains(needle))
                .map(|value| (false, value))
        })?;
    let reason = if exact {
        SelectionReason::Exact {
            index,
            value: value.to_owned(),
        }
    } else {
        SelectionReason::Partial {
            index,
            value: value.to_owned(),
        }
    };
    Some(Match {
        exact,
        active: node.facts().contains_value("status", "active")
            || node.facts().contains_value("okf_status", "active"),
        canonical: node.facts().contains_value("canonical", "true")
            || node.facts().contains_value("canonical", "yes")
            || ((node.facts().values("authority").is_some()
                || node.facts().values("resource").is_some())
                && (node.facts().contains_value("status", "active")
                    || node.facts().contains_value("okf_status", "active"))),
        confidence: node.confidence().value(),
        freshness: node.facts().freshness().unwrap_or_default().to_owned(),
        reason,
    })
}

fn fact_values<'a>(node: &'a NodeInput, keys: &[&str]) -> Vec<&'a str> {
    keys.iter()
        .filter_map(|key| node.facts().values(key))
        .flatten()
        .map(String::as_str)
        .collect()
}

fn rank_matches(matches: &mut [(&NodeInput, Match)]) {
    matches.sort_by(|(left_node, left), (right_node, right)| {
        match_rank(right)
            .cmp(&match_rank(left))
            .then_with(|| left_node.id().cmp(right_node.id()))
    });
}

fn match_rank(matched: &Match) -> (bool, bool, bool, u8, &str) {
    (
        matched.exact,
        matched.active,
        matched.canonical,
        matched.confidence,
        &matched.freshness,
    )
}

fn bounded<T>(items: impl Iterator<Item = T>, max_results: usize) -> Bounded<T> {
    let mut items: Vec<_> = items.take(max_results.saturating_add(1)).collect();
    let truncated = items.len() > max_results;
    items.truncate(max_results);
    Bounded { items, truncated }
}

fn node_evidence(node: &NodeInput) -> NodeEvidence {
    NodeEvidence {
        id: node.id().clone(),
        role: node.role(),
        provenance: node.provenance().clone(),
        confidence: node.confidence(),
        facts: node.facts().clone(),
    }
}

fn edge_evidence(edge: &EdgeInput) -> EdgeEvidence {
    EdgeEvidence {
        id: edge.id().clone(),
        source: edge.source().clone(),
        target: edge.target().clone(),
        provenance: edge.provenance().clone(),
        confidence: edge.confidence(),
        certainty: edge.certainty(),
        relationship: edge.relationship(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{
        GraphBuilder, GraphInput, GraphLimits, StructuralGraphCandidate, StructuralSourceType,
    };

    struct RankingFixture {
        snapshot: StructuralGraphSnapshot,
        seed_path: String,
        seed: String,
        canonical: String,
        generated: String,
        requirement: String,
        design: String,
        implementation: String,
        validation: String,
        frequency_high: String,
        frequency_low: String,
        path_first: String,
        path_second: String,
        same_path_code: String,
        same_path_text: String,
        current: String,
        predecessor: String,
        superseded: String,
        draft: String,
        archive: String,
        rejected: String,
    }

    fn hex(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn ranking_node(
        path: &str,
        source_type: StructuralSourceType,
        domains: &[&str],
        lifecycle: StructuralLifecycle,
        authority: StructuralAuthority,
        generated_status: StructuralGeneratedStatus,
    ) -> StructuralNode {
        StructuralNode::new(
            path,
            hex('1'),
            source_type,
            domains.iter().map(|domain| (*domain).to_owned()).collect(),
            Some("enterprise-core".to_owned()),
            lifecycle,
            authority,
            generated_status,
            vec![],
        )
        .unwrap()
    }

    fn ranking_edge(
        kind: StructuralEdgeKind,
        source: &StructuralNode,
        target: &StructuralNode,
        locator: &str,
    ) -> crate::graph::StructuralEdge {
        crate::graph::StructuralEdge::new(
            kind,
            source.id().to_owned(),
            target.id().to_owned(),
            locator,
            vec![],
            StructuralEdgeCertainty::Known,
        )
        .unwrap()
    }

    fn ranking_fixture() -> RankingFixture {
        let seed = ranking_node(
            "work/dma-seed.md",
            StructuralSourceType::Markdown,
            &["control"],
            StructuralLifecycle::Current,
            StructuralAuthority::Canonical,
            StructuralGeneratedStatus::Source,
        );
        let canonical = ranking_node(
            "docs/canonical-requirement.md",
            StructuralSourceType::Markdown,
            &["requirements"],
            StructuralLifecycle::Current,
            StructuralAuthority::Canonical,
            StructuralGeneratedStatus::Source,
        );
        let generated = ranking_node(
            "generated/work/dma-seed.md-copy.txt",
            StructuralSourceType::PreextractedText,
            &["requirements"],
            StructuralLifecycle::Current,
            StructuralAuthority::Supporting,
            StructuralGeneratedStatus::Generated,
        );
        let requirement = ranking_node(
            "evidence/requirement.md",
            StructuralSourceType::Markdown,
            &["software"],
            StructuralLifecycle::Current,
            StructuralAuthority::Approved,
            StructuralGeneratedStatus::Source,
        );
        let design = ranking_node(
            "evidence/design.md",
            StructuralSourceType::Markdown,
            &["hardware"],
            StructuralLifecycle::Current,
            StructuralAuthority::Approved,
            StructuralGeneratedStatus::Source,
        );
        let implementation = ranking_node(
            "evidence/implementation.rs",
            StructuralSourceType::Code,
            &["software"],
            StructuralLifecycle::Current,
            StructuralAuthority::Approved,
            StructuralGeneratedStatus::Source,
        );
        let validation = ranking_node(
            "evidence/validation.md",
            StructuralSourceType::Markdown,
            &["verification"],
            StructuralLifecycle::Current,
            StructuralAuthority::Approved,
            StructuralGeneratedStatus::Source,
        );
        let frequency_high = ranking_node(
            "ties/z-frequency.md",
            StructuralSourceType::Markdown,
            &["tie"],
            StructuralLifecycle::Current,
            StructuralAuthority::Approved,
            StructuralGeneratedStatus::Source,
        );
        let frequency_low = ranking_node(
            "ties/a-frequency.md",
            StructuralSourceType::Markdown,
            &["tie"],
            StructuralLifecycle::Current,
            StructuralAuthority::Approved,
            StructuralGeneratedStatus::Source,
        );
        let path_first = ranking_node(
            "ties/a-path.md",
            StructuralSourceType::Markdown,
            &["tie"],
            StructuralLifecycle::Current,
            StructuralAuthority::Approved,
            StructuralGeneratedStatus::Source,
        );
        let path_second = ranking_node(
            "ties/b-path.md",
            StructuralSourceType::Markdown,
            &["tie"],
            StructuralLifecycle::Current,
            StructuralAuthority::Approved,
            StructuralGeneratedStatus::Source,
        );
        let same_path_code = ranking_node(
            "ties/same-path",
            StructuralSourceType::Code,
            &["tie"],
            StructuralLifecycle::Current,
            StructuralAuthority::Approved,
            StructuralGeneratedStatus::Source,
        );
        let same_path_text = ranking_node(
            "ties/same-path",
            StructuralSourceType::Text,
            &["tie"],
            StructuralLifecycle::Current,
            StructuralAuthority::Approved,
            StructuralGeneratedStatus::Source,
        );
        let current = ranking_node(
            "lifecycle/current.md",
            StructuralSourceType::Markdown,
            &["lifecycle"],
            StructuralLifecycle::Current,
            StructuralAuthority::Canonical,
            StructuralGeneratedStatus::Source,
        );
        let predecessor = ranking_node(
            "lifecycle/predecessor.md",
            StructuralSourceType::Markdown,
            &["lifecycle"],
            StructuralLifecycle::Superseded,
            StructuralAuthority::Canonical,
            StructuralGeneratedStatus::Source,
        );
        let superseded = ranking_node(
            "lifecycle/superseded.md",
            StructuralSourceType::Markdown,
            &["lifecycle"],
            StructuralLifecycle::Superseded,
            StructuralAuthority::Canonical,
            StructuralGeneratedStatus::Source,
        );
        let draft = ranking_node(
            "lifecycle/draft.md",
            StructuralSourceType::Markdown,
            &["lifecycle"],
            StructuralLifecycle::Draft,
            StructuralAuthority::Canonical,
            StructuralGeneratedStatus::Source,
        );
        let archive = ranking_node(
            "lifecycle/archive.md",
            StructuralSourceType::Markdown,
            &["lifecycle"],
            StructuralLifecycle::Archive,
            StructuralAuthority::Canonical,
            StructuralGeneratedStatus::Source,
        );
        let rejected = ranking_node(
            "lifecycle/rejected.md",
            StructuralSourceType::Markdown,
            &["lifecycle"],
            StructuralLifecycle::Rejected,
            StructuralAuthority::Canonical,
            StructuralGeneratedStatus::Source,
        );
        let nodes = vec![
            seed.clone(),
            canonical.clone(),
            generated.clone(),
            requirement.clone(),
            design.clone(),
            implementation.clone(),
            validation.clone(),
            frequency_high.clone(),
            frequency_low.clone(),
            path_first.clone(),
            path_second.clone(),
            same_path_code.clone(),
            same_path_text.clone(),
            current.clone(),
            predecessor.clone(),
            superseded.clone(),
            draft.clone(),
            archive.clone(),
            rejected.clone(),
        ];
        let mut edges = vec![
            ranking_edge(StructuralEdgeKind::Requirement, &seed, &canonical, "rank:1"),
            ranking_edge(StructuralEdgeKind::Requirement, &seed, &generated, "rank:2"),
            ranking_edge(
                StructuralEdgeKind::GeneratedFrom,
                &generated,
                &canonical,
                "rank:3",
            ),
            ranking_edge(
                StructuralEdgeKind::Requirement,
                &seed,
                &requirement,
                "rank:4",
            ),
            ranking_edge(StructuralEdgeKind::Design, &seed, &design, "rank:5"),
            ranking_edge(
                StructuralEdgeKind::Implementation,
                &seed,
                &implementation,
                "rank:6",
            ),
            ranking_edge(StructuralEdgeKind::Validation, &seed, &validation, "rank:7"),
        ];
        let direct_nodes = [
            &frequency_high,
            &frequency_low,
            &path_first,
            &path_second,
            &same_path_code,
            &same_path_text,
            &current,
            &predecessor,
            &superseded,
            &draft,
            &archive,
            &rejected,
        ];
        edges.extend(direct_nodes.iter().enumerate().map(|(index, node)| {
            ranking_edge(
                StructuralEdgeKind::Dependency,
                &seed,
                node,
                &format!("rank:{}", index + 8),
            )
        }));
        edges.push(ranking_edge(
            StructuralEdgeKind::Predecessor,
            &predecessor,
            &current,
            "rank:20",
        ));
        edges.push(ranking_edge(
            StructuralEdgeKind::Supersedes,
            &predecessor,
            &current,
            "rank:21",
        ));
        let snapshot = GraphBuilder::new(100, 100, hex('f'))
            .build(
                StructuralGraphCandidate::new(hex('e'), nodes, edges),
                None,
                "p8-ranking",
            )
            .unwrap();
        RankingFixture {
            snapshot,
            seed_path: seed.path().to_owned(),
            seed: seed.id().to_owned(),
            canonical: canonical.id().to_owned(),
            generated: generated.id().to_owned(),
            requirement: requirement.id().to_owned(),
            design: design.id().to_owned(),
            implementation: implementation.id().to_owned(),
            validation: validation.id().to_owned(),
            frequency_high: frequency_high.id().to_owned(),
            frequency_low: frequency_low.id().to_owned(),
            path_first: path_first.id().to_owned(),
            path_second: path_second.id().to_owned(),
            same_path_code: same_path_code.id().to_owned(),
            same_path_text: same_path_text.id().to_owned(),
            current: current.id().to_owned(),
            predecessor: predecessor.id().to_owned(),
            superseded: superseded.id().to_owned(),
            draft: draft.id().to_owned(),
            archive: archive.id().to_owned(),
            rejected: rejected.id().to_owned(),
        }
    }

    fn ranking_request(fixture: &RankingFixture) -> StageRankingRequest {
        StageRankingRequest::new(
            fixture.seed_path.clone(),
            SdlcStage::Design,
            vec!["hardware".to_owned(), "missing-domain".to_owned()],
            vec![EvidenceClass::Design, EvidenceClass::BusinessFlow],
            RankingLimits::new(20).unwrap(),
        )
        .unwrap()
    }

    fn frozen_frequency(fixture: &RankingFixture) -> FrozenHistoricalFrequency {
        FrozenHistoricalFrequency::new(
            hex('f'),
            BTreeMap::from([
                (fixture.generated.clone(), 900),
                (fixture.validation.clone(), 900),
                (fixture.frequency_high.clone(), 900),
                (fixture.frequency_low.clone(), 0),
                (fixture.path_first.clone(), 100),
                (fixture.path_second.clone(), 100),
                (fixture.same_path_code.clone(), 100),
                (fixture.same_path_text.clone(), 100),
            ]),
        )
        .unwrap()
    }

    fn ranked_item<'a>(receipt: &'a RankingReceipt, node_id: &str) -> &'a RankingItemReceipt {
        receipt
            .selected
            .iter()
            .chain(&receipt.omitted_conflicts)
            .find(|item| item.node_id == node_id)
            .expect("fixture node has ranking receipt")
    }

    fn ranked_position(receipt: &RankingReceipt, node_id: &str) -> usize {
        receipt
            .selected
            .iter()
            .chain(&receipt.omitted_conflicts)
            .position(|item| item.node_id == node_id)
            .expect("fixture node has ranking position")
    }

    fn selected_ids(receipt: &RankingReceipt) -> Vec<String> {
        receipt
            .selected
            .iter()
            .map(|item| item.node_id.clone())
            .collect()
    }

    fn first_nine_equal(left: &StageRankKey, right: &StageRankKey) -> bool {
        left.eligibility == right.eligibility
            && left.authority == right.authority
            && left.revision == right.revision
            && left.artifact == right.artifact
            && left.relationship_distance == right.relationship_distance
            && left.relevance == right.relevance
            && left.required_contribution == right.required_contribution
            && left.frequency_basis_points == right.frequency_basis_points
            && left.canonical_path == right.canonical_path
    }

    #[test]
    fn p8_ranking() {
        let fixture = ranking_fixture();
        let frequency = frozen_frequency(&fixture);
        let request = ranking_request(&fixture);
        let receipt = StageRanker::new(&fixture.snapshot, request.clone(), &frequency)
            .unwrap()
            .rank();
        let repeated = StageRanker::new(&fixture.snapshot, request, &frequency)
            .unwrap()
            .rank();

        assert_eq!(receipt, repeated);
        assert_eq!(receipt.snapshot_digest, fixture.snapshot.snapshot_digest());
        assert_eq!(receipt.seed, fixture.seed_path);
        assert_eq!(receipt.stage, SdlcStage::Design);
        assert_eq!(receipt.candidate_count, 19);
        assert_eq!(receipt.requested_selection_limit, 20);
        assert_eq!(receipt.selected.len(), 14);
        assert_eq!(receipt.omitted_conflicts.len(), 5);
        assert_eq!(receipt.traversal.status, NotRun::NotRun);
        assert_eq!(receipt.traversal.limits, NotRun::NotRun);
        assert_eq!(selected_ids(&receipt), selected_ids(&repeated));

        assert!(
            ranked_position(&receipt, &fixture.canonical)
                < ranked_position(&receipt, &fixture.generated)
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.canonical).rank_key.authority,
            AuthorityKey::CanonicalOwned
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.generated).rank_key.relevance,
            RelevanceKey::Partial
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.generated).rank_key.artifact,
            ArtifactKey::GeneratedOrPreextracted
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.generated)
                .frequency
                .raw_basis_points,
            900
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.generated)
                .frequency
                .capped_basis_points,
            500
        );

        assert_eq!(
            ranked_item(&receipt, &fixture.current).rank_key.eligibility,
            EligibilityKey::Eligible
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.predecessor)
                .rank_key
                .revision,
            RevisionKey::Predecessor
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.superseded).rank_key.revision,
            RevisionKey::Superseded
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.draft).rank_key.revision,
            RevisionKey::Draft
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.archive).rank_key.artifact,
            ArtifactKey::Archive
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.rejected).rank_key.artifact,
            ArtifactKey::Rejected
        );
        assert!(
            ranked_position(&receipt, &fixture.current)
                < ranked_position(&receipt, &fixture.predecessor)
        );
        assert_eq!(
            receipt.conflicts.node_ids,
            vec![
                fixture.predecessor.clone(),
                fixture.superseded.clone(),
                fixture.draft.clone(),
                fixture.archive.clone(),
                fixture.rejected.clone()
            ]
        );

        assert_eq!(
            ranked_item(&receipt, &fixture.requirement)
                .rank_key
                .relationship_distance,
            RelationshipDistanceKey::Direct
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.design)
                .rank_key
                .relationship_distance,
            RelationshipDistanceKey::Direct
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.implementation)
                .rank_key
                .relationship_distance,
            RelationshipDistanceKey::Direct
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.validation)
                .rank_key
                .relationship_distance,
            RelationshipDistanceKey::Direct
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.design)
                .rank_key
                .required_contribution,
            3
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.validation)
                .rank_key
                .required_contribution,
            0
        );
        assert!(
            ranked_position(&receipt, &fixture.design)
                < ranked_position(&receipt, &fixture.validation)
        );

        assert!(
            ranked_position(&receipt, &fixture.frequency_high)
                < ranked_position(&receipt, &fixture.frequency_low)
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.frequency_high)
                .frequency
                .raw_basis_points,
            900
        );
        assert!(
            ranked_item(&receipt, &fixture.frequency_high)
                .frequency
                .cap_applied
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.frequency_high)
                .frequency
                .aggregate_digest,
            hex('f')
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.frequency_high)
                .frequency
                .rank_key_position,
            8
        );
        assert!(
            ranked_position(&receipt, &fixture.path_first)
                < ranked_position(&receipt, &fixture.path_second)
        );
        assert!(first_nine_equal(
            &ranked_item(&receipt, &fixture.same_path_code).rank_key,
            &ranked_item(&receipt, &fixture.same_path_text).rank_key
        ));
        assert_eq!(
            ranked_position(&receipt, &fixture.same_path_code)
                < ranked_position(&receipt, &fixture.same_path_text),
            fixture.same_path_code < fixture.same_path_text
        );

        assert!(receipt.conflicts.conflicts_visible);
        assert!(receipt.missing_required_evidence.missing);
        assert_eq!(
            receipt.missing_required_evidence.domains,
            vec!["missing-domain"]
        );
        assert_eq!(
            receipt.missing_required_evidence.evidence_classes,
            vec![EvidenceClass::BusinessFlow]
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.seed).rank_key.relevance,
            RelevanceKey::Exact
        );
        assert_eq!(
            ranked_item(&receipt, &fixture.frequency_low)
                .frequency
                .raw_basis_points,
            0
        );

        assert_eq!(
            SdlcStage::parse("outside-protocol"),
            Err(RankingError::UnknownStage("outside-protocol".to_owned()))
        );
        assert_eq!(
            EvidenceClass::parse("full-text"),
            Err(RankingError::UnknownEvidenceClass("full-text".to_owned()))
        );
        assert_eq!(
            RankingLimits::new(0),
            Err(RankingError::InvalidSelectionLimit(0))
        );
        assert_eq!(
            RankingLimits::new(MAX_RANKING_SELECTION + 1),
            Err(RankingError::InvalidSelectionLimit(
                MAX_RANKING_SELECTION + 1
            ))
        );
        assert_eq!(
            StageRankingRequest::new(
                "",
                SdlcStage::Design,
                vec![],
                vec![],
                RankingLimits::new(1).unwrap()
            ),
            Err(RankingError::EmptySeed)
        );
        assert!(matches!(
            StageRankingRequest::new(
                "x".repeat(MAX_RANKING_SEED_BYTES + 1),
                SdlcStage::Design,
                vec![],
                vec![],
                RankingLimits::new(1).unwrap()
            ),
            Err(RankingError::SeedTooLong(_))
        ));
        assert!(matches!(
            FrozenHistoricalFrequency::new("ABC", BTreeMap::new()),
            Err(RankingError::MalformedFrequencyDigest(_))
        ));
        assert!(matches!(
            FrozenHistoricalFrequency::new(hex('f'), BTreeMap::from([(fixture.seed.clone(), -1)])),
            Err(RankingError::InvalidFrequencyValue { .. })
        ));
        let unknown_frequency =
            FrozenHistoricalFrequency::new(hex('f'), BTreeMap::from([(hex('9'), 1)])).unwrap();
        assert_eq!(
            StageRanker::new(
                &fixture.snapshot,
                ranking_request(&fixture),
                &unknown_frequency
            )
            .err(),
            Some(RankingError::UnknownFrequencyNode(hex('9')))
        );
        assert!(FrozenHistoricalFrequency::QUALIFICATION.contains("current-attempt"));
        assert!(FrozenHistoricalFrequency::QUALIFICATION.contains("bot"));
    }

    // ===================================================================
    // Slice 1.2 retrieval parity tests (SEIT-6, CONTRACT-7)
    // ===================================================================

    fn id(s: &str) -> NodeId {
        NodeId::parse(s).unwrap()
    }

    fn prov(source: &str, locator: &str) -> Provenance {
        Provenance::new(source, locator).unwrap()
    }

    /// Prove that permuted node insertion order does not change retrieval outcome.
    #[test]
    fn retrieval_determinism_under_permuted_input() {
        let a = NodeInput::new(
            id("a"),
            NodeRole::Document,
            prov("a", "a.md"),
            Confidence::new(80).unwrap(),
        )
        .with_facts(
            NodeFacts::default()
                .with_field_value("title", "Alpha Source")
                .unwrap()
                .with_status("active")
                .unwrap()
                .with_field_value("canonical", "true")
                .unwrap()
                .with_freshness("2026-07-21")
                .unwrap(),
        );
        let b = NodeInput::new(
            id("b"),
            NodeRole::Document,
            prov("b", "b.md"),
            Confidence::new(40).unwrap(),
        )
        .with_facts(
            NodeFacts::default()
                .with_field_value("title", "Beta Source")
                .unwrap()
                .with_status("deprecated")
                .unwrap()
                .with_freshness("2025-01-01")
                .unwrap(),
        );
        let nodes_ab = vec![a.clone(), b.clone()];
        let nodes_ba = vec![b, a];
        let limits = GraphLimits::new(10, 10).unwrap();
        let g_ab = KnowledgeGraph::build(GraphInput::new(nodes_ab, vec![]), limits).unwrap();
        let g_ba = KnowledgeGraph::build(GraphInput::new(nodes_ba, vec![]), limits).unwrap();

        // Both graphs must return identical results for the same query.
        let r_ab = g_ab.find("Alpha", 10);
        let r_ba = g_ba.find("Alpha", 10);
        assert_eq!(
            r_ab, r_ba,
            "retrieval must be deterministic under permuted input"
        );
        assert!(!r_ab.items.is_empty());
    }

    /// Prove canonical stronger source wins: canonical+active node ranks above
    /// a non-canonical match on the same query.
    #[test]
    fn canonical_stronger_source_wins() {
        let canonical = NodeInput::new(
            id("docs/canon"),
            NodeRole::Document,
            prov("docs/canon", "docs/canon.md"),
            Confidence::new(80).unwrap(),
        )
        .with_facts(
            NodeFacts::default()
                .with_field_value("title", "Shared Title")
                .unwrap()
                .with_status("active")
                .unwrap()
                .with_field_value("canonical", "true")
                .unwrap()
                .with_freshness("2026-07-21")
                .unwrap(),
        );
        let weaker = NodeInput::new(
            id("archive/canon"),
            NodeRole::Document,
            prov("archive/canon", "archive/canon.md"),
            Confidence::new(60).unwrap(),
        )
        .with_facts(
            NodeFacts::default()
                .with_field_value("title", "Shared Title")
                .unwrap()
                .with_status("deprecated")
                .unwrap()
                .with_freshness("2024-01-01")
                .unwrap(),
        );
        let graph = KnowledgeGraph::build(
            GraphInput::new(vec![weaker, canonical.clone()], vec![]),
            GraphLimits::new(10, 10).unwrap(),
        )
        .unwrap();
        let result = graph.find("Shared Title", 10);
        assert_eq!(result.items.len(), 2);
        // stronger canonical source must appear first
        assert_eq!(result.items[0].id.as_str(), canonical.id().as_str());
    }

    /// Prove retrieval outcome is invariant to node ordering when ties exist.
    #[test]
    fn tied_nodes_stable_order() {
        let a = NodeInput::new(
            id("ties/a"),
            NodeRole::Document,
            prov("ties/a", "ties/a.md"),
            Confidence::new(80).unwrap(),
        )
        .with_facts(
            NodeFacts::default()
                .with_field_value("title", "Tie Entry")
                .unwrap()
                .with_status("active")
                .unwrap()
                .with_freshness("2026-07-21")
                .unwrap(),
        );
        let b = NodeInput::new(
            id("ties/b"),
            NodeRole::Document,
            prov("ties/b", "ties/b.md"),
            Confidence::new(80).unwrap(),
        )
        .with_facts(
            NodeFacts::default()
                .with_field_value("title", "Tie Entry")
                .unwrap()
                .with_status("active")
                .unwrap()
                .with_freshness("2026-07-21")
                .unwrap(),
        );
        let limits = GraphLimits::new(10, 10).unwrap();
        let g_ab =
            KnowledgeGraph::build(GraphInput::new(vec![a.clone(), b.clone()], vec![]), limits)
                .unwrap();
        let g_ba = KnowledgeGraph::build(GraphInput::new(vec![b, a], vec![]), limits).unwrap();

        let r1 = g_ab.find("Tie Entry", 10);
        let r2 = g_ba.find("Tie Entry", 10);
        assert_eq!(r1.items.len(), r2.items.len());
        for (l, r) in r1.items.iter().zip(r2.items.iter()) {
            assert_eq!(l.id, r.id, "tie-break must be deterministic");
        }
    }
}
