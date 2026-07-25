//! Deterministic, dependency-free context packet assembly.
//!
//! Packet assembly consumes a compiled view and caller-supplied evidence bytes.
//! It deliberately has no SQZ, provider, filesystem, or persisted-token-policy
//! dependency; compression and provider telemetry are separate adapter concerns.

use crate::agent::result_store::ResultId;
use crate::graph::{
    query::{
        EligibilityKey, EvidenceClass, OmissionReason, RankingItemDecision, RankingItemReceipt,
        RankingReceipt, StructuralPathStep, StructuralTraversalReceipt,
        StructuralTraversalTerminal, Traversal,
    },
    EdgeCertainty, EdgeRelationship, KnowledgeGraph, NodeId, Provenance, StructuralAuthority,
    StructuralEdgeKind, StructuralGeneratedStatus, StructuralGraphSnapshot, StructuralLifecycle,
    StructuralSourceType,
};
use crate::view::CompiledView;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const PACKET_RECEIPT_SCHEMA_VERSION: &str = "1.0.0";

/// The selection class assigned by the caller to evidence for a view node.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EvidencePriority {
    Required,
    Recommended,
    Related,
}

const MAX_PRESERVATION_ANCHOR_ID_BYTES: usize = 64;
const MAX_PRESERVATION_ANCHOR_VALUE_BYTES: usize = 512;

/// A bounded semantic fact whose value must survive compression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreservationAnchor {
    id: String,
    value: String,
}

impl PreservationAnchor {
    pub fn new(
        id: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, PreservationAnchorError> {
        let id = id.into();
        let value = value.into();
        if id.is_empty() || id.len() > MAX_PRESERVATION_ANCHOR_ID_BYTES {
            return Err(PreservationAnchorError::InvalidIdLength);
        }
        if !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(PreservationAnchorError::InvalidIdCharacter);
        }
        if value.trim().is_empty() {
            return Err(PreservationAnchorError::BlankValue);
        }
        if value.len() > MAX_PRESERVATION_ANCHOR_VALUE_BYTES {
            return Err(PreservationAnchorError::ValueTooLong);
        }
        Ok(Self { id, value })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreservationAnchorError {
    InvalidIdLength,
    InvalidIdCharacter,
    BlankValue,
    ValueTooLong,
}

/// Caller-supplied content and ranking facts for one selected view node.
///
/// Larger authority and freshness values take precedence within an equal
/// [`EvidencePriority`]. Node identity is the final stable tie-break.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceContent {
    pub id: NodeId,
    pub content: String,
    pub priority: EvidencePriority,
    pub authority: u64,
    pub freshness: u64,
    pub preservation_anchors: Vec<PreservationAnchor>,
}

impl EvidenceContent {
    pub fn new(
        id: NodeId,
        content: impl Into<String>,
        priority: EvidencePriority,
        authority: u64,
        freshness: u64,
        preservation_anchors: Vec<PreservationAnchor>,
    ) -> Self {
        Self {
            id,
            content: content.into(),
            priority,
            authority,
            freshness,
            preservation_anchors,
        }
    }
}

/// Runtime assembly limits. Token ceilings are intentionally not a ViewSpec field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacketLimits {
    pub max_items: usize,
    pub max_bytes: usize,
    pub runtime_token_ceiling: Option<usize>,
}

impl PacketLimits {
    pub fn new(max_items: usize, max_bytes: usize, runtime_token_ceiling: Option<usize>) -> Self {
        Self {
            max_items,
            max_bytes,
            runtime_token_ceiling,
        }
    }
}

/// Input to one packet assembly attempt.
#[derive(Clone, Debug)]
pub struct PacketAssemblyRequest<'a> {
    pub view: &'a CompiledView,
    pub graph: &'a KnowledgeGraph,
    pub evidence: &'a [EvidenceContent],
    pub limits: PacketLimits,
    pub dependency_limits: DependencyClosureLimits,
}

/// Validated limits for dependency expansion beyond compiled View seeds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DependencyClosureLimits {
    max_depth: usize,
    max_nodes: usize,
}

impl DependencyClosureLimits {
    pub const HARD_MAX_DEPTH: usize = 64;
    pub const HARD_MAX_NODES: usize = 4_096;

    pub fn new(max_depth: usize, max_nodes: usize) -> Result<Self, PacketError> {
        if max_depth > Self::HARD_MAX_DEPTH || max_nodes == 0 || max_nodes > Self::HARD_MAX_NODES {
            return Err(PacketError::InvalidDependencyClosureLimits {
                max_depth,
                max_nodes,
            });
        }
        Ok(Self {
            max_depth,
            max_nodes,
        })
    }

    pub fn max_depth(self) -> usize {
        self.max_depth
    }

    pub fn max_nodes(self) -> usize {
        self.max_nodes
    }
}

/// The bound that rejected a load-bearing evidence item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketBound {
    Items,
    Bytes,
    RuntimeTokens,
}

/// Explicit receipt method for the non-provider token estimate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenEstimateMethod {
    BytesDividedByFourCeiling,
}

/// A selected evidence item with immutable graph provenance retained verbatim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextItem {
    pub id: NodeId,
    pub provenance: Provenance,
    pub content: String,
    pub priority: EvidencePriority,
    pub authority: u64,
    pub freshness: u64,
    pub preservation_anchors: Vec<PreservationAnchor>,
    pub raw_bytes: usize,
}

/// Deterministic accounting for one context packet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PacketReceipt {
    pub schema_version: &'static str,
    pub seed_ids: Vec<NodeId>,
    pub admitted_dependency_ids: Vec<NodeId>,
    pub selected_ids: Vec<NodeId>,
    pub omitted_ids: Vec<NodeId>,
    pub dependency_closure_truncated: bool,
    pub dependency_max_depth: usize,
    pub dependency_max_nodes: usize,
    pub raw_bytes: usize,
    pub estimated_tokens: usize,
    pub token_estimate_method: TokenEstimateMethod,
    pub effective_max_items: usize,
    pub effective_max_bytes: usize,
    pub runtime_token_ceiling: Option<usize>,
    pub truncated: bool,
}

/// A provider-neutral payload plus its lossless identity and provenance record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextPacket {
    pub items: Vec<ContextItem>,
    pub payload: String,
    pub receipt: PacketReceipt,
}

/// Deterministic packet assembly failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PacketError {
    InvalidDependencyClosureLimits {
        max_depth: usize,
        max_nodes: usize,
    },
    ViewSeedNotInGraph(NodeId),
    MissingSelectedEvidence(NodeId),
    DuplicateSelectedEvidence(NodeId),
    EvidenceNotInView(NodeId),
    UnrelatedEvidence(NodeId),
    RequiredEvidenceMissingPreservationAnchors(NodeId),
    PreservationAnchorNotInEvidence {
        evidence_id: NodeId,
        anchor_id: String,
    },
    RequiredEvidenceDoesNotFit {
        id: NodeId,
        bound: PacketBound,
    },
    ArithmeticOverflow {
        operation: &'static str,
    },
}

/// Stateless assembler for one compiled view and its external evidence content.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PacketAssembler;

/// A caller-materialized, bounded excerpt. The assembler never reads files or expands it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralExcerpt {
    pub node_id: String,
    pub locator: String,
    pub content: String,
    pub content_digest: String,
    pub complete_file: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StructuralPacketLimits {
    pub max_items: usize,
    pub max_packet_bytes: usize,
    pub max_excerpt_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StructuralPacketTier {
    PrimaryCanonical,
    RequiredDependency,
    ConflictSuperseded,
    FollowUpLocator,
}

impl StructuralPacketTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PrimaryCanonical => "primary-canonical",
            Self::RequiredDependency => "required-dependency",
            Self::ConflictSuperseded => "conflict-superseded",
            Self::FollowUpLocator => "follow-up-locator",
        }
    }
}

#[derive(Clone, Debug)]
pub struct StructuralPacketRequest<'a> {
    pub snapshot: &'a StructuralGraphSnapshot,
    pub ranking: &'a RankingReceipt,
    pub traversal: Option<&'a StructuralTraversalReceipt>,
    pub excerpts: &'a [StructuralExcerpt],
    pub required_domains: Vec<String>,
    pub required_evidence_classes: Vec<EvidenceClass>,
    pub limits: StructuralPacketLimits,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralPacketItem {
    pub node_id: String,
    pub canonical_path: String,
    pub tier: StructuralPacketTier,
    pub role: String,
    pub source_type: StructuralSourceType,
    pub lifecycle: StructuralLifecycle,
    pub authority: StructuralAuthority,
    pub generated_status: StructuralGeneratedStatus,
    pub why_selected: String,
    pub edge_trail: Vec<StructuralPathStep>,
    pub locator: String,
    pub content_digest: String,
    pub excerpt: String,
    pub excerpt_bytes: usize,
    pub locator_only: bool,
    pub non_authoritative: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralSufficiencyReceipt {
    pub snapshot_digest: String,
    pub ranking_seed: String,
    pub traversal_seed: Option<String>,
    pub ranking_binding_digest: String,
    pub traversal_binding_digest: Option<String>,
    pub required_domains: Vec<String>,
    pub covered_domains: Vec<String>,
    pub missing_domains: Vec<String>,
    pub required_evidence_classes: Vec<EvidenceClass>,
    pub covered_evidence_classes: Vec<EvidenceClass>,
    pub missing_evidence_classes: Vec<EvidenceClass>,
    pub tier_counts: BTreeMap<StructuralPacketTier, usize>,
    pub tier_bytes: BTreeMap<StructuralPacketTier, usize>,
    pub max_items: usize,
    pub max_packet_bytes: usize,
    pub max_excerpt_bytes: usize,
    pub selected_ids: Vec<String>,
    pub omitted_ids: Vec<String>,
    pub omitted_reasons: Vec<String>,
    pub conflict_ids: Vec<String>,
    pub bounded_omissions: Vec<StructuralPacketOmission>,
    pub ambiguity_visible: bool,
    pub total_locator_bytes: usize,
    pub total_excerpt_bytes: usize,
    pub total_packet_bytes: usize,
    pub sufficient: bool,
    pub digest: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuralPacketOmissionReason {
    ItemLimit,
    PacketByteLimit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralPacketOmission {
    pub node_id: String,
    pub tier: StructuralPacketTier,
    pub reason: StructuralPacketOmissionReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralPacket {
    pub items: Vec<StructuralPacketItem>,
    pub receipt: StructuralSufficiencyReceipt,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StructuralPacketError {
    SnapshotDigestMismatch,
    TraversalSeedMismatch,
    UnknownReceiptNode(String),
    InvalidRankingReceipt(String),
    InvalidTraversalReceipt(String),
    DuplicateExcerpt(String),
    UnrelatedExcerpt(String),
    ExcerptDigestMismatch(String),
    InvalidExcerptLocator(String),
    ExcerptTooLarge(String),
    CompleteFileRejected(String),
    InvalidLimits,
    RequiredItemDoesNotFit(String),
    ArithmeticOverflow(&'static str),
}

impl PacketAssembler {
    pub fn new() -> Self {
        Self
    }

    /// Returns the deterministic seed-and-dependency evidence IDs required for
    /// an assembly request before callers materialize evidence content.
    pub fn evidence_ids(
        view: &CompiledView,
        graph: &KnowledgeGraph,
        dependency_limits: DependencyClosureLimits,
    ) -> Result<Vec<NodeId>, PacketError> {
        let closure = dependency_closure(&PacketAssemblyRequest {
            view,
            graph,
            evidence: &[],
            limits: PacketLimits::new(0, 0, None),
            dependency_limits,
        })?;
        let mut ids = view
            .items()
            .iter()
            .map(|item| item.id.clone())
            .collect::<BTreeSet<_>>();
        ids.extend(closure.admitted_ids);
        Ok(ids.into_iter().collect())
    }

    /// Selects evidence by requiredness, authority, freshness, then node ID.
    ///
    /// Effective item and byte ceilings cannot relax the persisted view limits.
    /// A runtime token ceiling only narrows this individual assembly attempt.
    pub fn assemble(
        &self,
        request: &PacketAssemblyRequest<'_>,
    ) -> Result<ContextPacket, PacketError> {
        let closure = dependency_closure(request)?;
        let evidence = index_evidence(request, &closure.admitted_ids)?;
        let limits = effective_limits(request, closure.admitted_ids.len())?;
        let mut candidates = Vec::with_capacity(
            request
                .view
                .items()
                .len()
                .checked_add(closure.admitted_ids.len())
                .ok_or(PacketError::ArithmeticOverflow {
                    operation: "packet candidate capacity",
                })?,
        );
        for item in request.view.items() {
            candidates.push(Candidate {
                provenance: item.provenance.clone(),
                evidence: evidence
                    .get(&item.id)
                    .expect("seed evidence was validated before sorting"),
            });
        }
        for id in &closure.admitted_ids {
            let node = request
                .graph
                .node(id)
                .expect("admitted dependency is a present graph node");
            candidates.push(Candidate {
                provenance: node.provenance().clone(),
                evidence: evidence
                    .get(id)
                    .expect("dependency evidence was validated before sorting"),
            });
        }
        candidates.sort_by(candidate_order);

        let mut items = Vec::new();
        let mut payload = String::new();
        let mut selected_ids = Vec::new();
        let mut omitted_ids = Vec::new();
        let mut raw_bytes = 0usize;

        for candidate in candidates {
            let encoded = encode_item(
                &candidate.evidence.id,
                &candidate.provenance,
                candidate.evidence,
            );
            let next_bytes =
                raw_bytes
                    .checked_add(encoded.len())
                    .ok_or(PacketError::ArithmeticOverflow {
                        operation: "packet raw byte count",
                    })?;
            let next_tokens = estimate_tokens(next_bytes);
            let rejection = if items.len() == limits.max_items {
                Some(PacketBound::Items)
            } else if next_bytes > limits.max_bytes {
                Some(PacketBound::Bytes)
            } else if limits
                .runtime_token_ceiling
                .is_some_and(|ceiling| next_tokens > ceiling)
            {
                Some(PacketBound::RuntimeTokens)
            } else {
                None
            };

            if let Some(bound) = rejection {
                if candidate.evidence.priority == EvidencePriority::Required {
                    return Err(PacketError::RequiredEvidenceDoesNotFit {
                        id: candidate.evidence.id.clone(),
                        bound,
                    });
                }
                omitted_ids.push(candidate.evidence.id.clone());
                continue;
            }

            let item = ContextItem {
                id: candidate.evidence.id.clone(),
                provenance: candidate.provenance.clone(),
                content: candidate.evidence.content.clone(),
                priority: candidate.evidence.priority,
                authority: candidate.evidence.authority,
                freshness: candidate.evidence.freshness,
                preservation_anchors: candidate.evidence.preservation_anchors.clone(),
                raw_bytes: encoded.len(),
            };
            raw_bytes = next_bytes;
            payload.push_str(&encoded);
            selected_ids.push(item.id.clone());
            items.push(item);
        }

        let estimated_tokens = estimate_tokens(raw_bytes);
        let omitted_any = !omitted_ids.is_empty();
        Ok(ContextPacket {
            items,
            payload,
            receipt: PacketReceipt {
                schema_version: PACKET_RECEIPT_SCHEMA_VERSION,
                seed_ids: request
                    .view
                    .items()
                    .iter()
                    .map(|item| item.id.clone())
                    .collect(),
                admitted_dependency_ids: closure.admitted_ids,
                selected_ids,
                omitted_ids,
                dependency_closure_truncated: closure.truncated,
                dependency_max_depth: request.dependency_limits.max_depth(),
                dependency_max_nodes: request.dependency_limits.max_nodes(),
                raw_bytes,
                estimated_tokens,
                token_estimate_method: TokenEstimateMethod::BytesDividedByFourCeiling,
                effective_max_items: limits.max_items,
                effective_max_bytes: limits.max_bytes,
                runtime_token_ceiling: limits.runtime_token_ceiling,
                truncated: request.view.truncated() || closure.truncated || omitted_any,
            },
        })
    }
}

impl PacketAssembler {
    /// Assembles only immutable structural facts and caller-bounded excerpts.
    pub fn assemble_structural(
        &self,
        request: &StructuralPacketRequest<'_>,
    ) -> Result<StructuralPacket, StructuralPacketError> {
        if request.ranking.snapshot_digest != request.snapshot.snapshot_digest() {
            return Err(StructuralPacketError::SnapshotDigestMismatch);
        }
        if request.limits.max_items == 0
            || request.limits.max_items > 4_096
            || request.limits.max_packet_bytes == 0
            || request.limits.max_packet_bytes > 1_048_576
            || request.limits.max_excerpt_bytes == 0
            || request.limits.max_excerpt_bytes > 65_536
        {
            return Err(StructuralPacketError::InvalidLimits);
        }
        let ranked_ids = validate_ranking_receipt(request.snapshot, request.ranking)?;
        let traversal_paths = match request.traversal {
            Some(traversal) => {
                validate_traversal_receipt(request.snapshot, request.ranking, traversal)?;
                traversal
                    .paths
                    .iter()
                    .map(|path| (path.node_id.as_str(), path))
                    .collect()
            }
            None => BTreeMap::new(),
        };
        let traversal_ids: BTreeSet<_> = request
            .traversal
            .into_iter()
            .flat_map(|value| value.visited_ids.iter().cloned())
            .collect();
        let mut excerpts = BTreeMap::new();
        for excerpt in request.excerpts {
            if !ranked_ids.contains(&excerpt.node_id) && !traversal_ids.contains(&excerpt.node_id) {
                return Err(StructuralPacketError::UnrelatedExcerpt(
                    excerpt.node_id.clone(),
                ));
            }
            if excerpts.insert(excerpt.node_id.clone(), excerpt).is_some() {
                return Err(StructuralPacketError::DuplicateExcerpt(
                    excerpt.node_id.clone(),
                ));
            }
            let node = request.snapshot.node(&excerpt.node_id).ok_or_else(|| {
                StructuralPacketError::UnknownReceiptNode(excerpt.node_id.clone())
            })?;
            if node.content_digest() != excerpt.content_digest {
                return Err(StructuralPacketError::ExcerptDigestMismatch(
                    excerpt.node_id.clone(),
                ));
            }
            if excerpt.locator.is_empty()
                || excerpt.locator.starts_with('/')
                || excerpt.locator.split('/').any(|part| part == "..")
                || excerpt.locator.len() > request.limits.max_packet_bytes
            {
                return Err(StructuralPacketError::InvalidExcerptLocator(
                    excerpt.node_id.clone(),
                ));
            }
            if excerpt.content.len() > request.limits.max_excerpt_bytes {
                return Err(StructuralPacketError::ExcerptTooLarge(
                    excerpt.node_id.clone(),
                ));
            }
            if excerpt.complete_file {
                return Err(StructuralPacketError::CompleteFileRejected(
                    excerpt.node_id.clone(),
                ));
            }
        }
        let mut items = Vec::new();
        let mut decided = BTreeSet::new();
        let mut packet_bytes = 0usize;
        let mut bounded_omissions = Vec::new();
        let mut covered_domains = BTreeSet::new();
        let mut covered_classes = BTreeSet::new();
        for ranked in &request.ranking.selected {
            let node = request
                .snapshot
                .node(&ranked.node_id)
                .ok_or_else(|| StructuralPacketError::UnknownReceiptNode(ranked.node_id.clone()))?;
            if !is_primary_canonical(ranked, node) {
                continue;
            }
            decided.insert(ranked.node_id.clone());
            let edge_trail = traversal_paths
                .get(ranked.node_id.as_str())
                .map(|path| path.steps.clone())
                .unwrap_or_default();
            add_structural_item(
                &mut items,
                &mut packet_bytes,
                request,
                ranked.node_id.as_str(),
                StructuralPacketTier::PrimaryCanonical,
                "ranked-selected",
                edge_trail,
                false,
                &mut covered_domains,
                &mut covered_classes,
            )?;
        }
        let selected_ids: BTreeSet<_> = request
            .ranking
            .selected
            .iter()
            .map(|item| item.node_id.as_str())
            .collect();
        if let Some(traversal) = request.traversal {
            for path in &traversal.paths {
                if decided.contains(&path.node_id) || selected_ids.contains(path.node_id.as_str()) {
                    continue;
                }
                let node = request.snapshot.node(&path.node_id).ok_or_else(|| {
                    StructuralPacketError::UnknownReceiptNode(path.node_id.clone())
                })?;
                if node.lifecycle() != StructuralLifecycle::Current
                    || node.generated_status() != StructuralGeneratedStatus::Source
                {
                    continue;
                }
                let helps_domain = node.domains().iter().any(|domain| {
                    request.required_domains.contains(domain) && !covered_domains.contains(domain)
                });
                let helps_class = path
                    .steps
                    .last()
                    .and_then(|step| evidence_class(step.kind))
                    .is_some_and(|class| {
                        request.required_evidence_classes.contains(&class)
                            && !covered_classes.contains(&class)
                    });
                if helps_domain || helps_class {
                    decided.insert(path.node_id.clone());
                    add_structural_item(
                        &mut items,
                        &mut packet_bytes,
                        request,
                        &path.node_id,
                        StructuralPacketTier::RequiredDependency,
                        "traversed-required",
                        path.steps.clone(),
                        false,
                        &mut covered_domains,
                        &mut covered_classes,
                    )?;
                }
            }
        }
        let mut conflict_ids = Vec::new();
        for ranked in &request.ranking.omitted_conflicts {
            if ranked.decision == RankingItemDecision::Omitted(OmissionReason::LifecycleConflict) {
                conflict_ids.push(ranked.node_id.clone());
                if !decided.insert(ranked.node_id.clone()) {
                    continue;
                }
                if let StructuralAdmission::Omitted(reason) = add_structural_item(
                    &mut items,
                    &mut packet_bytes,
                    request,
                    &ranked.node_id,
                    StructuralPacketTier::ConflictSuperseded,
                    "ranking-conflict",
                    Vec::new(),
                    true,
                    &mut covered_domains,
                    &mut covered_classes,
                )? {
                    bounded_omissions.push(StructuralPacketOmission {
                        node_id: ranked.node_id.clone(),
                        tier: StructuralPacketTier::ConflictSuperseded,
                        reason,
                    });
                }
            }
        }
        for ranked in &request.ranking.selected {
            if !decided.insert(ranked.node_id.clone()) {
                continue;
            }
            let edge_trail = traversal_paths
                .get(ranked.node_id.as_str())
                .map(|path| path.steps.clone())
                .unwrap_or_default();
            if let StructuralAdmission::Omitted(reason) = add_structural_item(
                &mut items,
                &mut packet_bytes,
                request,
                &ranked.node_id,
                StructuralPacketTier::FollowUpLocator,
                "ranking-follow-up",
                edge_trail,
                true,
                &mut covered_domains,
                &mut covered_classes,
            )? {
                bounded_omissions.push(StructuralPacketOmission {
                    node_id: ranked.node_id.clone(),
                    tier: StructuralPacketTier::FollowUpLocator,
                    reason,
                });
            }
        }
        if let Some(traversal) = request.traversal {
            for path in &traversal.paths {
                if !decided.insert(path.node_id.clone()) {
                    continue;
                }
                let node = request.snapshot.node(&path.node_id).ok_or_else(|| {
                    StructuralPacketError::UnknownReceiptNode(path.node_id.clone())
                })?;
                let non_authoritative = !is_authoritative_current_source(node);
                if let StructuralAdmission::Omitted(reason) = add_structural_item(
                    &mut items,
                    &mut packet_bytes,
                    request,
                    &path.node_id,
                    StructuralPacketTier::FollowUpLocator,
                    "traversal-follow-up",
                    path.steps.clone(),
                    non_authoritative,
                    &mut covered_domains,
                    &mut covered_classes,
                )? {
                    bounded_omissions.push(StructuralPacketOmission {
                        node_id: path.node_id.clone(),
                        tier: StructuralPacketTier::FollowUpLocator,
                        reason,
                    });
                }
            }
        }
        let required_domains: BTreeSet<_> = request.required_domains.iter().cloned().collect();
        let required_classes: BTreeSet<_> =
            request.required_evidence_classes.iter().copied().collect();
        let missing_domains: Vec<_> = required_domains
            .difference(&covered_domains)
            .cloned()
            .collect();
        let missing_classes: Vec<_> = required_classes
            .difference(&covered_classes)
            .copied()
            .collect();
        let mut tier_counts = BTreeMap::new();
        let mut tier_bytes = BTreeMap::new();
        let mut locator_bytes = 0usize;
        let mut excerpt_bytes = 0usize;
        for item in &items {
            *tier_counts.entry(item.tier).or_insert(0) += 1;
            let bytes = item
                .locator
                .len()
                .checked_add(item.excerpt_bytes)
                .ok_or(StructuralPacketError::ArithmeticOverflow("item bytes"))?;
            *tier_bytes.entry(item.tier).or_insert(0) += bytes;
            locator_bytes = locator_bytes
                .checked_add(item.locator.len())
                .ok_or(StructuralPacketError::ArithmeticOverflow("locator bytes"))?;
            excerpt_bytes = excerpt_bytes
                .checked_add(item.excerpt_bytes)
                .ok_or(StructuralPacketError::ArithmeticOverflow("excerpt bytes"))?;
        }
        let total = locator_bytes
            .checked_add(excerpt_bytes)
            .ok_or(StructuralPacketError::ArithmeticOverflow("packet bytes"))?;
        let selected_ids: Vec<_> = request
            .ranking
            .selected
            .iter()
            .map(|item| item.node_id.clone())
            .collect();
        let omitted_ids: Vec<_> = request
            .ranking
            .omitted_conflicts
            .iter()
            .map(|item| item.node_id.clone())
            .collect();
        let omitted_reasons: Vec<_> = request
            .ranking
            .omitted_conflicts
            .iter()
            .map(|item| format!("{:?}", item.decision))
            .collect();
        let ranking_binding_digest = ResultId::sha256(
            format!("bran-ranking-receipt-binding:v1:{:?}", request.ranking).as_bytes(),
        )
        .value()
        .to_owned();
        let traversal_binding_digest = request.traversal.map(|value| {
            ResultId::sha256(format!("bran-traversal-receipt-binding:v1:{value:?}").as_bytes())
                .value()
                .to_owned()
        });
        let mut receipt = StructuralSufficiencyReceipt {
            snapshot_digest: request.snapshot.snapshot_digest().to_owned(),
            ranking_seed: request.ranking.seed.clone(),
            traversal_seed: request.traversal.map(|value| value.seed.clone()),
            ranking_binding_digest,
            traversal_binding_digest,
            required_domains: request.required_domains.clone(),
            covered_domains: covered_domains.into_iter().collect(),
            missing_domains,
            required_evidence_classes: request.required_evidence_classes.clone(),
            covered_evidence_classes: covered_classes.into_iter().collect(),
            missing_evidence_classes: missing_classes,
            tier_counts,
            tier_bytes,
            max_items: request.limits.max_items,
            max_packet_bytes: request.limits.max_packet_bytes,
            max_excerpt_bytes: request.limits.max_excerpt_bytes,
            selected_ids,
            omitted_ids,
            omitted_reasons,
            conflict_ids,
            bounded_omissions,
            ambiguity_visible: !request.ranking.omitted_conflicts.is_empty(),
            total_locator_bytes: locator_bytes,
            total_excerpt_bytes: excerpt_bytes,
            total_packet_bytes: total,
            sufficient: false,
            digest: String::new(),
        };
        receipt.sufficient =
            receipt.missing_domains.is_empty() && receipt.missing_evidence_classes.is_empty();
        receipt.digest = ResultId::sha256(format!("{:?}", receipt).as_bytes())
            .value()
            .to_owned();
        Ok(StructuralPacket { items, receipt })
    }
}

fn validate_ranking_receipt(
    snapshot: &StructuralGraphSnapshot,
    ranking: &RankingReceipt,
) -> Result<BTreeSet<String>, StructuralPacketError> {
    if ranking.candidate_count != snapshot.nodes().len()
        || ranking.selected.len() + ranking.omitted_conflicts.len() != ranking.candidate_count
        || ranking.selected.len() > ranking.requested_selection_limit
    {
        return Err(StructuralPacketError::InvalidRankingReceipt(
            "candidate-count".to_owned(),
        ));
    }
    let mut ids = BTreeSet::new();
    let mut lifecycle_conflicts = Vec::new();
    for (item, selected) in ranking
        .selected
        .iter()
        .map(|item| (item, true))
        .chain(ranking.omitted_conflicts.iter().map(|item| (item, false)))
    {
        if !ids.insert(item.node_id.clone()) {
            return Err(StructuralPacketError::InvalidRankingReceipt(format!(
                "duplicate-node:{}",
                item.node_id
            )));
        }
        let node = snapshot
            .node(&item.node_id)
            .ok_or_else(|| StructuralPacketError::UnknownReceiptNode(item.node_id.clone()))?;
        if item.canonical_path != node.path()
            || item.rank_key.node_id != item.node_id
            || item.rank_key.canonical_path != item.canonical_path
        {
            return Err(StructuralPacketError::InvalidRankingReceipt(format!(
                "rank-identity:{}",
                item.node_id
            )));
        }
        let eligible = node.lifecycle() == StructuralLifecycle::Current;
        if (item.rank_key.eligibility == EligibilityKey::Eligible) != eligible {
            return Err(StructuralPacketError::InvalidRankingReceipt(format!(
                "eligibility:{}",
                item.node_id
            )));
        }
        match (selected, item.decision, eligible) {
            (true, RankingItemDecision::Selected, true) => {}
            (false, RankingItemDecision::Omitted(OmissionReason::LifecycleConflict), false) => {
                lifecycle_conflicts.push(item.node_id.clone())
            }
            (false, RankingItemDecision::Omitted(OmissionReason::SelectionLimit), true) => {}
            _ => {
                return Err(StructuralPacketError::InvalidRankingReceipt(format!(
                    "decision:{}",
                    item.node_id
                )))
            }
        }
    }
    if ranking.conflicts.conflicts_visible == lifecycle_conflicts.is_empty()
        || ranking.conflicts.node_ids != lifecycle_conflicts
    {
        return Err(StructuralPacketError::InvalidRankingReceipt(
            "conflict-decision".to_owned(),
        ));
    }
    Ok(ids)
}

fn validate_traversal_receipt(
    snapshot: &StructuralGraphSnapshot,
    ranking: &RankingReceipt,
    traversal: &StructuralTraversalReceipt,
) -> Result<(), StructuralPacketError> {
    if traversal.snapshot_digest != snapshot.snapshot_digest() {
        return Err(StructuralPacketError::SnapshotDigestMismatch);
    }
    if traversal.seed != ranking.seed {
        return Err(StructuralPacketError::TraversalSeedMismatch);
    }
    if traversal.max_depth == 0
        || traversal.max_depth > Traversal::MAX_DEPTH
        || traversal.max_nodes == 0
        || traversal.max_nodes > Traversal::MAX_NODES
        || traversal.max_edges == 0
        || traversal.max_edges > Traversal::MAX_EDGES
        || traversal.visited_ids.is_empty()
        || traversal.visited_ids.len() > traversal.max_nodes
        || traversal.paths.len() != traversal.visited_ids.len()
        || traversal.inspected_edge_count > traversal.max_edges
    {
        return Err(StructuralPacketError::InvalidTraversalReceipt(
            "limits-or-counts".to_owned(),
        ));
    }
    let mut relationships = BTreeSet::new();
    for kind in &traversal.relationships {
        if !is_traversable_relationship(*kind) || !relationships.insert(*kind) {
            return Err(StructuralPacketError::InvalidTraversalReceipt(
                "relationships".to_owned(),
            ));
        }
    }
    if relationships.is_empty() {
        return Err(StructuralPacketError::InvalidTraversalReceipt(
            "relationships".to_owned(),
        ));
    }
    let mut visited = BTreeSet::new();
    let mut path_ids = BTreeSet::new();
    for (visited_id, path) in traversal.visited_ids.iter().zip(&traversal.paths) {
        if visited_id != &path.node_id
            || !visited.insert(visited_id.clone())
            || !path_ids.insert(path.node_id.clone())
            || snapshot.node(visited_id).is_none()
            || path.steps.len() > traversal.max_depth
        {
            return Err(StructuralPacketError::InvalidTraversalReceipt(format!(
                "path-node:{visited_id}"
            )));
        }
        let mut current = traversal.seed.as_str();
        let mut path_nodes = BTreeSet::from([traversal.seed.as_str()]);
        for step in &path.steps {
            let edge = snapshot.edge(&step.edge_id).ok_or_else(|| {
                StructuralPacketError::InvalidTraversalReceipt(format!(
                    "unknown-edge:{}",
                    step.edge_id
                ))
            })?;
            if edge.kind() != step.kind
                || edge.source() != step.source
                || edge.target() != step.target
                || edge.source_locator() != step.source_locator
                || edge.certainty() != crate::graph::StructuralEdgeCertainty::Known
                || !relationships.contains(&step.kind)
            {
                return Err(StructuralPacketError::InvalidTraversalReceipt(format!(
                    "edge-binding:{}",
                    step.edge_id
                )));
            }
            let next = if edge.source() == current {
                edge.target()
            } else if edge.target() == current {
                edge.source()
            } else {
                return Err(StructuralPacketError::InvalidTraversalReceipt(format!(
                    "disconnected-edge:{}",
                    step.edge_id
                )));
            };
            if snapshot.node(next).is_none() || !path_nodes.insert(next) {
                return Err(StructuralPacketError::InvalidTraversalReceipt(format!(
                    "invalid-path:{}",
                    path.node_id
                )));
            }
            current = next;
        }
        if current != path.node_id {
            return Err(StructuralPacketError::InvalidTraversalReceipt(format!(
                "path-endpoint:{}",
                path.node_id
            )));
        }
    }
    if traversal.visited_ids[0] != traversal.seed
        || !traversal.paths[0].steps.is_empty()
        || traversal.accepted_edge_count != traversal.paths.len() - 1
        || traversal.inspected_edge_count < traversal.accepted_edge_count
    {
        return Err(StructuralPacketError::InvalidTraversalReceipt(
            "visit-accounting".to_owned(),
        ));
    }
    for skipped in &traversal.skipped_edges {
        let edge = snapshot.edge(&skipped.edge_id).ok_or_else(|| {
            StructuralPacketError::InvalidTraversalReceipt(format!(
                "unknown-skipped-edge:{}",
                skipped.edge_id
            ))
        })?;
        if edge.certainty() != skipped.certainty
            || !relationships.contains(&edge.kind())
            || match skipped.reason {
                "non-known" => edge.certainty() == crate::graph::StructuralEdgeCertainty::Known,
                "missing-target" => {
                    edge.certainty() != crate::graph::StructuralEdgeCertainty::Known
                        || (snapshot.node(edge.source()).is_some()
                            && snapshot.node(edge.target()).is_some())
                }
                _ => true,
            }
        {
            return Err(StructuralPacketError::InvalidTraversalReceipt(format!(
                "skipped-edge:{}",
                skipped.edge_id
            )));
        }
    }
    let terminal_bound_matches = match traversal.terminal {
        StructuralTraversalTerminal::Complete => true,
        StructuralTraversalTerminal::DepthLimit => traversal
            .paths
            .iter()
            .any(|path| path.steps.len() == traversal.max_depth),
        StructuralTraversalTerminal::NodeLimit => {
            traversal.visited_ids.len() == traversal.max_nodes
        }
        StructuralTraversalTerminal::EdgeLimit => {
            traversal.inspected_edge_count == traversal.max_edges
        }
    };
    if !terminal_bound_matches {
        return Err(StructuralPacketError::InvalidTraversalReceipt(
            "terminal-bound".to_owned(),
        ));
    }
    Ok(())
}

fn is_traversable_relationship(kind: StructuralEdgeKind) -> bool {
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

fn evidence_class(kind: StructuralEdgeKind) -> Option<EvidenceClass> {
    match kind {
        StructuralEdgeKind::Requirement => Some(EvidenceClass::Requirement),
        StructuralEdgeKind::Design => Some(EvidenceClass::Design),
        StructuralEdgeKind::Implementation => Some(EvidenceClass::Implementation),
        StructuralEdgeKind::Validation => Some(EvidenceClass::Validation),
        StructuralEdgeKind::Architecture => Some(EvidenceClass::Architecture),
        StructuralEdgeKind::BusinessFlow => Some(EvidenceClass::BusinessFlow),
        _ => None,
    }
}

fn is_authoritative_current_source(node: &crate::graph::StructuralNode) -> bool {
    node.lifecycle() == StructuralLifecycle::Current
        && node.generated_status() == StructuralGeneratedStatus::Source
        && matches!(
            node.authority(),
            StructuralAuthority::Canonical | StructuralAuthority::Approved
        )
}

fn is_primary_canonical(ranked: &RankingItemReceipt, node: &crate::graph::StructuralNode) -> bool {
    ranked.rank_key.eligibility == EligibilityKey::Eligible && is_authoritative_current_source(node)
}

enum StructuralAdmission {
    Added,
    Omitted(StructuralPacketOmissionReason),
}

#[allow(clippy::too_many_arguments)]
fn add_structural_item(
    items: &mut Vec<StructuralPacketItem>,
    packet_bytes: &mut usize,
    request: &StructuralPacketRequest<'_>,
    id: &str,
    tier: StructuralPacketTier,
    role: &str,
    edge_trail: Vec<StructuralPathStep>,
    non_authoritative: bool,
    covered_domains: &mut BTreeSet<String>,
    covered_classes: &mut BTreeSet<EvidenceClass>,
) -> Result<StructuralAdmission, StructuralPacketError> {
    let node = request
        .snapshot
        .node(id)
        .ok_or_else(|| StructuralPacketError::UnknownReceiptNode(id.to_owned()))?;
    let excerpt = request.excerpts.iter().find(|value| value.node_id == id);
    let locator = excerpt
        .map(|value| value.locator.clone())
        .unwrap_or_else(|| node.path().to_owned());
    let content = if tier == StructuralPacketTier::FollowUpLocator {
        String::new()
    } else {
        excerpt
            .map(|value| value.content.clone())
            .unwrap_or_default()
    };
    let bytes = locator
        .len()
        .checked_add(content.len())
        .ok_or(StructuralPacketError::ArithmeticOverflow("item bytes"))?;
    let bound = if items.len() == request.limits.max_items {
        Some(StructuralPacketOmissionReason::ItemLimit)
    } else if packet_bytes
        .checked_add(bytes)
        .ok_or(StructuralPacketError::ArithmeticOverflow("packet bytes"))?
        > request.limits.max_packet_bytes
    {
        Some(StructuralPacketOmissionReason::PacketByteLimit)
    } else {
        None
    };
    if let Some(reason) = bound {
        if matches!(
            tier,
            StructuralPacketTier::PrimaryCanonical | StructuralPacketTier::RequiredDependency
        ) {
            return Err(StructuralPacketError::RequiredItemDoesNotFit(id.to_owned()));
        }
        return Ok(StructuralAdmission::Omitted(reason));
    }
    *packet_bytes = packet_bytes
        .checked_add(bytes)
        .ok_or(StructuralPacketError::ArithmeticOverflow("packet bytes"))?;
    if !non_authoritative {
        covered_domains.extend(node.domains().iter().cloned());
        if let Some(step) = edge_trail.last() {
            if let Some(class) = evidence_class(step.kind) {
                covered_classes.insert(class);
            }
        }
    }
    items.push(StructuralPacketItem {
        node_id: id.to_owned(),
        canonical_path: node.path().to_owned(),
        tier,
        role: role.to_owned(),
        source_type: node.source_type(),
        lifecycle: node.lifecycle(),
        authority: node.authority(),
        generated_status: node.generated_status(),
        why_selected: role.to_owned(),
        edge_trail,
        locator,
        content_digest: node.content_digest().to_owned(),
        excerpt_bytes: content.len(),
        excerpt: content,
        locator_only: tier == StructuralPacketTier::FollowUpLocator || excerpt.is_none(),
        non_authoritative,
    });
    Ok(StructuralAdmission::Added)
}

struct Candidate<'a> {
    provenance: Provenance,
    evidence: &'a EvidenceContent,
}

struct DependencyClosure {
    admitted_ids: Vec<NodeId>,
    truncated: bool,
}

fn dependency_closure(
    request: &PacketAssemblyRequest<'_>,
) -> Result<DependencyClosure, PacketError> {
    let seeds = request
        .view
        .items()
        .iter()
        .map(|item| item.id.clone())
        .collect::<BTreeSet<_>>();
    for seed in &seeds {
        if request.graph.node(seed).is_none() {
            return Err(PacketError::ViewSeedNotInGraph(seed.clone()));
        }
    }

    let mut visited = seeds.clone();
    let mut frontier = seeds.into_iter().map(|id| (id, 0usize)).collect::<Vec<_>>();
    let mut cursor = 0usize;
    let mut admitted = BTreeSet::new();
    let mut truncated = false;
    while cursor < frontier.len() {
        let (source, depth) = frontier[cursor].clone();
        cursor += 1;
        for edge_id in request.graph.forward_edges(&source) {
            let edge = request
                .graph
                .edge(edge_id)
                .expect("graph adjacency references a present edge");
            if edge.certainty() != EdgeCertainty::Known
                || !positive_context_relationship(edge.relationship())
                || request.graph.node(edge.target()).is_none()
                || visited.contains(edge.target())
            {
                continue;
            }
            if depth == request.dependency_limits.max_depth()
                || admitted.len() == request.dependency_limits.max_nodes()
            {
                truncated = true;
                continue;
            }
            let target = edge.target().clone();
            visited.insert(target.clone());
            admitted.insert(target.clone());
            frontier.push((target, depth + 1));
        }
    }
    Ok(DependencyClosure {
        admitted_ids: admitted.into_iter().collect(),
        truncated,
    })
}

fn positive_context_relationship(relationship: EdgeRelationship) -> bool {
    matches!(
        relationship,
        EdgeRelationship::Dependency
            | EdgeRelationship::Implementation
            | EdgeRelationship::Replacement
            | EdgeRelationship::Supersedes
            | EdgeRelationship::Validation
            | EdgeRelationship::Reachability
    )
}

fn index_evidence<'a>(
    request: &'a PacketAssemblyRequest<'a>,
    dependency_ids: &[NodeId],
) -> Result<BTreeMap<NodeId, &'a EvidenceContent>, PacketError> {
    let mut selected_ids = request
        .view
        .items()
        .iter()
        .map(|item| item.id.clone())
        .collect::<BTreeSet<_>>();
    selected_ids.extend(dependency_ids.iter().cloned());
    let mut evidence = BTreeMap::new();
    let mut duplicates = BTreeSet::new();
    for item in request.evidence {
        if evidence.insert(item.id.clone(), item).is_some() {
            duplicates.insert(item.id.clone());
        }
    }
    if let Some(id) = duplicates.into_iter().next() {
        return Err(PacketError::DuplicateSelectedEvidence(id));
    }
    for id in &selected_ids {
        if !evidence.contains_key(id) {
            return Err(PacketError::MissingSelectedEvidence(id.clone()));
        }
    }
    if let Some(id) = evidence.keys().find(|id| !selected_ids.contains(*id)) {
        return Err(PacketError::UnrelatedEvidence(id.clone()));
    }
    if let Some(item) = evidence.values().find(|item| {
        item.priority == EvidencePriority::Required && item.preservation_anchors.is_empty()
    }) {
        return Err(PacketError::RequiredEvidenceMissingPreservationAnchors(
            item.id.clone(),
        ));
    }
    if let Some((item, anchor)) = evidence.values().find_map(|item| {
        item.preservation_anchors
            .iter()
            .find(|anchor| !item.content.contains(anchor.value()))
            .map(|anchor| (*item, anchor))
    }) {
        return Err(PacketError::PreservationAnchorNotInEvidence {
            evidence_id: item.id.clone(),
            anchor_id: anchor.id().to_owned(),
        });
    }
    Ok(evidence)
}

fn effective_limits(
    request: &PacketAssemblyRequest<'_>,
    admitted_dependencies: usize,
) -> Result<PacketLimits, PacketError> {
    let expanded_view_ceiling = request
        .view
        .spec()
        .max_items
        .checked_add(admitted_dependencies)
        .ok_or(PacketError::ArithmeticOverflow {
            operation: "expanded packet item ceiling",
        })?;
    Ok(PacketLimits {
        max_items: request.limits.max_items.min(expanded_view_ceiling),
        max_bytes: request.limits.max_bytes.min(request.view.spec().max_bytes),
        runtime_token_ceiling: request.limits.runtime_token_ceiling,
    })
}

fn candidate_order(left: &Candidate<'_>, right: &Candidate<'_>) -> std::cmp::Ordering {
    left.evidence
        .priority
        .cmp(&right.evidence.priority)
        .then_with(|| right.evidence.authority.cmp(&left.evidence.authority))
        .then_with(|| right.evidence.freshness.cmp(&left.evidence.freshness))
        .then_with(|| left.evidence.id.cmp(&right.evidence.id))
}

fn encode_item(id: &NodeId, provenance: &Provenance, evidence: &EvidenceContent) -> String {
    format!(
        "node={}\nsource={}\nlocator={}\ncontent={}\n",
        id.as_str(),
        provenance.source(),
        provenance.locator(),
        evidence.content
    )
}

fn estimate_tokens(raw_bytes: usize) -> usize {
    raw_bytes / 4 + usize::from(!raw_bytes.is_multiple_of(4))
}

// ── Packet lifecycle & superseded-reference validator (Slice 1.2-C) ──────────

/// Stateless validator for packet lifecycle and superseded-reference semantics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PacketValidator;

/// A single deterministic finding from packet validation.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PacketFinding {
    pub path: String,
    pub code: String,
    pub message: String,
}

/// Complete read-only packet validation report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PacketReport {
    pub findings: Vec<PacketFinding>,
}

impl PacketValidator {
    pub fn new() -> Self {
        Self
    }

    /// Accepts `&Bundle` and returns a deterministic typed report without filesystem writes.
    pub fn validate(bundle: &crate::bundle::Bundle) -> PacketReport {
        let mut findings: BTreeSet<PacketFinding> = BTreeSet::new();
        let mut statuses: BTreeMap<&str, &str> = BTreeMap::new();
        let mut helpers: BTreeSet<&str> = BTreeSet::new();

        for (path, doc) in bundle.docs() {
            if let Some(map) = doc.frontmatter().parsed() {
                if let Some(s) = yaml_str(map, "status") {
                    statuses.insert(path.as_str(), s);
                }
                if is_helper_candidate(map) {
                    helpers.insert(path.as_str());
                }
            }
        }

        // Helper candidates must have status active or superseded.
        for &helper_path in &helpers {
            match statuses.get(helper_path) {
                Some(&"active") | Some(&"superseded") => {}
                _ => {
                    findings.insert(PacketFinding {
                        path: sanitize_path(helper_path).to_owned(),
                        code: "packet_missing_authority_field".to_owned(),
                        message: "Helper packet requires status: active or status: superseded"
                            .to_owned(),
                    });
                }
            }
        }

        // For active documents, check references against superseded.
        for (path, doc) in bundle.docs() {
            if statuses.get(path.as_str()).copied() != Some("active") {
                continue;
            }
            if let Some(map) = doc.frontmatter().parsed() {
                let refs = collect_references(map, doc.body());
                for r in &refs {
                    if let Some(superseded_path) = resolve_superseded(r, path, &statuses) {
                        findings.insert(PacketFinding {
                            path: sanitize_path(path).to_owned(),
                            code: "superseded_prompt_reference".to_owned(),
                            message: format!(
                                "executable packet references superseded prompt {}",
                                sanitize_path(&superseded_path)
                            ),
                        });
                    }
                }
            }
        }

        PacketReport {
            findings: findings.into_iter().collect(),
        }
    }
}

fn yaml_str<'a>(map: &'a BTreeMap<String, crate::schema::YamlValue>, key: &str) -> Option<&'a str> {
    match map.get(key) {
        Some(crate::schema::YamlValue::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

fn is_helper_candidate(map: &BTreeMap<String, crate::schema::YamlValue>) -> bool {
    use crate::schema::YamlValue;

    let type_or_role_exact = |key: &str| -> bool {
        matches!(
            map.get(key)
                .and_then(|v| match v {
                    YamlValue::String(s) => Some(s.to_ascii_lowercase()),
                    _ => None,
                })
                .as_deref(),
            Some(
                "research-assistant"
                    | "ingestion-librarian"
                    | "synthesis-librarian"
                    | "integrity-librarian"
            )
        )
    };

    if type_or_role_exact("type") || type_or_role_exact("role") {
        return true;
    }

    if map.contains_key("work_class") {
        return true;
    }

    ["type", "role"].iter().any(|key| {
        map.get(*key)
            .and_then(|v| match v {
                YamlValue::String(s) => Some(s.to_ascii_lowercase()),
                _ => None,
            })
            .is_some_and(|lower| {
                lower.contains("librarian") || lower.contains("research-assistant")
            })
    })
}

fn collect_references(
    map: &BTreeMap<String, crate::schema::YamlValue>,
    body: &str,
) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
    collect_yaml_strings(map, &mut refs);
    for link in extract_all_markdown_links(body) {
        refs.insert(link);
    }
    refs
}

fn collect_yaml_strings(
    map: &BTreeMap<String, crate::schema::YamlValue>,
    out: &mut BTreeSet<String>,
) {
    for v in map.values() {
        push_yaml_strings(v, out);
    }
}

fn push_yaml_strings(v: &crate::schema::YamlValue, out: &mut BTreeSet<String>) {
    match v {
        crate::schema::YamlValue::String(s) => {
            out.insert(s.clone());
        }
        crate::schema::YamlValue::Sequence(seq) => {
            for item in seq {
                push_yaml_strings(item, out);
            }
        }
        crate::schema::YamlValue::Mapping(m) => collect_yaml_strings(m, out),
        _ => {}
    }
}

fn extract_all_markdown_links(body: &str) -> BTreeSet<String> {
    let mut links = BTreeSet::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b']' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            let start = i + 2;
            let mut end = start;
            while end < bytes.len()
                && bytes[end] != b')'
                && bytes[end] != b' '
                && bytes[end] != b'\t'
                && bytes[end] != b'\r'
                && bytes[end] != b'\n'
            {
                end += 1;
            }
            if end > start {
                let raw = std::str::from_utf8(&bytes[start..end]).unwrap_or("");
                let stripped = raw.split('#').next().unwrap_or(raw);
                if !stripped.is_empty() {
                    links.insert(stripped.to_owned());
                }
            }
            i = end;
        }
        i += 1;
    }
    links
}

/// Redacts unsafe paths so they never appear in findings or messages.
fn sanitize_path(path: &str) -> &str {
    if is_reference_safe(path)
        && !Path::new(path)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        && !path.contains('\\')
    {
        path
    } else {
        "[redacted]"
    }
}

fn is_reference_safe(path: &str) -> bool {
    if path.is_empty() || path.contains('\0') {
        return false;
    }
    // Windows drive-letter absolute.
    if path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':' {
        return false;
    }
    // UNC.
    if path.starts_with('\\') {
        return false;
    }
    // Unix absolute.
    if path.starts_with('/') {
        return false;
    }
    // URLs, mailto.
    if path.contains("://") || path.starts_with("mailto:") {
        return false;
    }
    true
}

fn resolve_safe(source_dir: &Path, target: &str) -> Option<String> {
    let resolved = source_dir.join(target);
    let mut stack: Vec<&str> = Vec::new();
    for comp in resolved.components() {
        match comp {
            std::path::Component::ParentDir => {
                stack.pop()?;
            }
            std::path::Component::Normal(s) => {
                stack.push(s.to_str()?);
            }
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    if stack.is_empty() {
        None
    } else {
        Some(stack.join("/"))
    }
}

fn resolve_superseded(
    reference: &str,
    source_path: &str,
    statuses: &BTreeMap<&str, &str>,
) -> Option<String> {
    let ref_path = reference.split('#').next().unwrap_or(reference);
    if !is_reference_safe(ref_path) {
        return None;
    }

    // Exact match against all known paths.
    if let Some(&status) = statuses.get(ref_path) {
        return if status == "superseded" {
            Some(ref_path.to_owned())
        } else {
            None
        };
    }

    // Relative resolution against all known paths.
    let source_dir = Path::new(source_path).parent().unwrap_or(Path::new("."));
    if let Some(candidate) = resolve_safe(source_dir, ref_path) {
        if let Some(&status) = statuses.get(candidate.as_str()) {
            return if status == "superseded" {
                Some(candidate)
            } else {
                None
            };
        }
    }

    // Basename-only fallback: only when no concrete target in any known path;
    // uniqueness checked across all paths, not merely superseded.
    if !ref_path.contains('/') {
        let hits: Vec<&&str> = statuses
            .keys()
            .filter(|k| k.rsplit('/').next() == Some(ref_path))
            .collect();
        if hits.len() == 1 && statuses.get(*hits[0]) == Some(&"superseded") {
            return Some((*hits[0]).to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::sqz::{
        DlpStatus, FidelityStatus, SqzAdapter, SqzAdapterConfig, SqzFailureReason, SqzIdentity,
        SqzPolicy, SqzPort, SqzPortError, SqzPortErrorCode, SqzPortOutput, SqzStatus,
    };
    use crate::agent::result_store::ResultId;
    use crate::graph::query::{
        EvidenceClass, FrozenHistoricalFrequency, RankingLimits, SdlcStage, StageRanker,
        StageRankingRequest, StructuralTraversalError, StructuralTraversalRequest,
        StructuralTraversalTerminal, Traversal,
    };
    use crate::graph::{
        Confidence, EdgeCertainty, EdgeId, EdgeInput, EdgeRelationship, GraphInput, GraphLimits,
        KnowledgeGraph, NodeInput, NodeRole, StructuralAuthority, StructuralEdge,
        StructuralEdgeCertainty, StructuralEdgeKind, StructuralGeneratedStatus,
        StructuralGraphSnapshot, StructuralLifecycle, StructuralNode, StructuralSnapshotReceipt,
        StructuralSourceType,
    };
    use crate::view::{
        Presentation, ViewCompiler, ViewField, ViewFilter, ViewGrouping, ViewSort, ViewSource,
        ViewSpec,
    };
    use std::cell::Cell;
    use std::rc::Rc;

    fn node_id(value: &str) -> NodeId {
        NodeId::parse(value).unwrap()
    }

    fn source(value: &str) -> Provenance {
        Provenance::new("scanner", value).unwrap()
    }

    fn anchor(id: &str, value: &str) -> PreservationAnchor {
        PreservationAnchor::new(id, value).unwrap()
    }

    fn structural_digest(ch: char) -> String {
        std::iter::repeat_n(ch, 64).collect()
    }

    fn structural_node(
        path: &str,
        domain: &str,
        lifecycle: StructuralLifecycle,
        generated: StructuralGeneratedStatus,
    ) -> StructuralNode {
        StructuralNode::new(
            path,
            structural_digest('a'),
            StructuralSourceType::Markdown,
            vec![domain.to_owned()],
            Some("owner".to_owned()),
            lifecycle,
            StructuralAuthority::Canonical,
            generated,
            vec![source(path)],
        )
        .unwrap()
    }

    fn compiled_packet_view() -> (KnowledgeGraph, CompiledView) {
        let graph = KnowledgeGraph::build(
            GraphInput::new(
                vec![
                    NodeInput::new(
                        node_id("node.alpha"),
                        NodeRole::Document,
                        source("alpha.md"),
                        Confidence::new(90).unwrap(),
                    ),
                    NodeInput::new(
                        node_id("node.beta"),
                        NodeRole::Section,
                        source("beta.md#one"),
                        Confidence::new(91).unwrap(),
                    ),
                    NodeInput::new(
                        node_id("node.delta"),
                        NodeRole::Symbol,
                        source("delta.rs:1"),
                        Confidence::new(92).unwrap(),
                    ),
                    NodeInput::new(
                        node_id("node.eta"),
                        NodeRole::Test,
                        source("eta.rs:1"),
                        Confidence::new(93).unwrap(),
                    ),
                    NodeInput::new(
                        node_id("node.gamma"),
                        NodeRole::Test,
                        source("gamma.rs:1"),
                        Confidence::new(94).unwrap(),
                    ),
                    NodeInput::new(
                        node_id("node.theta"),
                        NodeRole::External,
                        source("theta.txt"),
                        Confidence::new(95).unwrap(),
                    ),
                    NodeInput::new(
                        node_id("node.zeta"),
                        NodeRole::External,
                        source("zeta.txt"),
                        Confidence::new(96).unwrap(),
                    ),
                ],
                vec![],
            ),
            GraphLimits::new(7, 1).unwrap(),
        )
        .unwrap();
        let view = ViewCompiler::new()
            .compile(
                &ViewSpec {
                    source: ViewSource::All,
                    filter: ViewFilter::All,
                    sort: ViewSort::NodeId,
                    grouping: ViewGrouping::None,
                    fields: vec![ViewField::NodeId],
                    presentation: Presentation::Json,
                    max_items: 7,
                    max_bytes: 2_000,
                },
                &graph,
            )
            .unwrap();
        (graph, view)
    }

    fn dependency_limits() -> DependencyClosureLimits {
        DependencyClosureLimits::new(4, 16).unwrap()
    }

    fn edge_id(value: &str) -> EdgeId {
        EdgeId::parse(value).unwrap()
    }

    fn dependency_packet_fixture() -> (KnowledgeGraph, CompiledView, Vec<EvidenceContent>) {
        let seed = node_id("node.closure.seed");
        let dependency = node_id("node.closure.dependency");
        let unrelated = node_id("node.closure.unrelated");
        let graph = KnowledgeGraph::build(
            GraphInput::new(
                vec![
                    NodeInput::new(
                        seed.clone(),
                        NodeRole::Document,
                        source("seed.md"),
                        Confidence::new(100).unwrap(),
                    ),
                    NodeInput::new(
                        dependency.clone(),
                        NodeRole::Symbol,
                        source("dependency.rs:1"),
                        Confidence::new(100).unwrap(),
                    ),
                    NodeInput::new(
                        unrelated.clone(),
                        NodeRole::External,
                        source("unrelated.md"),
                        Confidence::new(20).unwrap(),
                    ),
                ],
                vec![
                    EdgeInput::new(
                        edge_id("edge.closure.dependency"),
                        seed.clone(),
                        dependency.clone(),
                        source("seed-to-dependency"),
                        Confidence::new(100).unwrap(),
                        EdgeCertainty::Known,
                    )
                    .with_relationship(EdgeRelationship::Dependency),
                    EdgeInput::new(
                        edge_id("edge.closure.cycle"),
                        dependency.clone(),
                        seed.clone(),
                        source("dependency-to-seed"),
                        Confidence::new(100).unwrap(),
                        EdgeCertainty::Known,
                    )
                    .with_relationship(EdgeRelationship::Implementation),
                    EdgeInput::new(
                        edge_id("edge.closure.uncertain"),
                        seed.clone(),
                        unrelated.clone(),
                        source("uncertain"),
                        Confidence::new(40).unwrap(),
                        EdgeCertainty::Unknown,
                    )
                    .with_relationship(EdgeRelationship::Reachability),
                    EdgeInput::new(
                        edge_id("edge.closure.conflict"),
                        seed.clone(),
                        unrelated.clone(),
                        source("conflict"),
                        Confidence::new(90).unwrap(),
                        EdgeCertainty::Known,
                    )
                    .with_relationship(EdgeRelationship::Conflict),
                ],
            ),
            GraphLimits::new(3, 4).unwrap(),
        )
        .unwrap();
        let view = ViewCompiler::new()
            .compile(
                &ViewSpec {
                    source: ViewSource::NodeIds(vec![seed.clone()]),
                    filter: ViewFilter::All,
                    sort: ViewSort::NodeId,
                    grouping: ViewGrouping::None,
                    fields: vec![ViewField::NodeId],
                    presentation: Presentation::Json,
                    max_items: 1,
                    max_bytes: 1_000,
                },
                &graph,
            )
            .unwrap();
        let evidence = vec![
            EvidenceContent::new(
                seed,
                "seed contract",
                EvidencePriority::Required,
                20,
                20,
                vec![anchor("required.seed", "seed contract")],
            ),
            EvidenceContent::new(
                dependency,
                "dependency contract",
                EvidencePriority::Required,
                10,
                10,
                vec![anchor("required.dependency", "dependency contract")],
            ),
            EvidenceContent::new(
                unrelated,
                "unrelated",
                EvidencePriority::Related,
                1,
                1,
                vec![],
            ),
        ];
        (graph, view, evidence)
    }

    fn packet_evidence() -> Vec<EvidenceContent> {
        vec![
            EvidenceContent::new(
                node_id("node.zeta"),
                "z".repeat(220),
                EvidencePriority::Related,
                1,
                1,
                vec![],
            ),
            EvidenceContent::new(
                node_id("node.eta"),
                "eta",
                EvidencePriority::Recommended,
                3,
                4,
                vec![],
            ),
            EvidenceContent::new(
                node_id("node.alpha"),
                "alpha",
                EvidencePriority::Required,
                9,
                3,
                vec![anchor("required.alpha", "alpha")],
            ),
            EvidenceContent::new(
                node_id("node.beta"),
                "beta",
                EvidencePriority::Required,
                9,
                7,
                vec![anchor("required.beta", "beta")],
            ),
            EvidenceContent::new(
                node_id("node.delta"),
                "delta",
                EvidencePriority::Required,
                5,
                99,
                vec![anchor("required.delta", "delta")],
            ),
            EvidenceContent::new(
                node_id("node.gamma"),
                "gamma",
                EvidencePriority::Required,
                9,
                7,
                vec![anchor("required.gamma", "gamma")],
            ),
            EvidenceContent::new(
                node_id("node.theta"),
                "theta",
                EvidencePriority::Related,
                4,
                8,
                vec![],
            ),
        ]
    }

    struct InMemorySqzPort {
        response: Result<SqzPortOutput, SqzPortError>,
        calls: Rc<Cell<usize>>,
    }

    impl InMemorySqzPort {
        fn output(payload: impl Into<String>) -> Self {
            let mut response = SqzPortOutput::new(payload, SqzIdentity::approved());
            response.actual_input_tokens = Some(111);
            response.actual_output_tokens = Some(17);
            Self {
                response: Ok(response),
                calls: Rc::new(Cell::new(0)),
            }
        }

        fn unavailable() -> Self {
            Self {
                response: Err(SqzPortError::new(SqzPortErrorCode::Unavailable)),
                calls: Rc::new(Cell::new(0)),
            }
        }
    }

    impl SqzPort for InMemorySqzPort {
        fn compress(&self, _input: &str) -> Result<SqzPortOutput, SqzPortError> {
            self.calls.set(self.calls.get() + 1);
            self.response.clone()
        }
    }

    fn sqz_config(policy: SqzPolicy, max_output_bytes: usize) -> SqzAdapterConfig {
        SqzAdapterConfig::new(
            policy,
            SqzIdentity::approved(),
            max_output_bytes,
            vec![
                anchor("global.beta-node", "node=node.beta"),
                anchor("global.gamma-node", "node=node.gamma"),
            ],
        )
    }

    #[test]
    fn p2_packet_sqz() {
        let public_fixture =
            include_str!("../../../../fixtures/packets/context-packet-sqz-v1.json");
        assert!(public_fixture.contains("\"schema_version\": \"1.0.0\""));
        assert!(public_fixture.contains("\"admitted_dependency_ids\""));
        assert!(public_fixture.contains("\"file:src/support.rs\""));
        assert!(
            public_fixture.contains("\"token_estimate_method\": \"bytes-divided-by-four-ceiling\"")
        );
        assert!(public_fixture.contains("\"actual_input_tokens\": null"));
        assert!(public_fixture.contains("\"fidelity_status\": \"passed\""));
        assert!(public_fixture.contains("\"dlp_status\": \"passed\""));
        assert!(public_fixture.contains("\"algorithm\": \"sha256\""));
        assert!(public_fixture.contains(
            "\"value\": \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\""
        ));
        assert_eq!(
            PreservationAnchor::new("blank.anchor", " \t"),
            Err(PreservationAnchorError::BlankValue)
        );
        let (graph, view) = compiled_packet_view();
        let evidence = packet_evidence();
        let assembler = PacketAssembler::new();
        let bounded = assembler
            .assemble(&PacketAssemblyRequest {
                view: &view,
                graph: &graph,
                evidence: &evidence,
                limits: PacketLimits::new(6, 800, Some(200)),
                dependency_limits: dependency_limits(),
            })
            .unwrap();
        assert_eq!(
            bounded.receipt.schema_version,
            PACKET_RECEIPT_SCHEMA_VERSION
        );
        assert_eq!(bounded.items[0].id, node_id("node.beta"));
        assert_eq!(bounded.items[1].id, node_id("node.gamma"));
        assert_eq!(bounded.items[2].id, node_id("node.alpha"));
        assert_eq!(bounded.items[3].id, node_id("node.delta"));
        assert_eq!(bounded.items[4].id, node_id("node.eta"));
        assert_eq!(bounded.items[5].id, node_id("node.theta"));
        assert_eq!(bounded.items[0].provenance, source("beta.md#one"));
        assert_eq!(
            bounded.receipt.selected_ids,
            vec![
                node_id("node.beta"),
                node_id("node.gamma"),
                node_id("node.alpha"),
                node_id("node.delta"),
                node_id("node.eta"),
                node_id("node.theta"),
            ]
        );
        assert_eq!(bounded.receipt.omitted_ids, vec![node_id("node.zeta")]);
        assert_eq!(bounded.receipt.raw_bytes, bounded.payload.len());
        assert_eq!(
            bounded.receipt.estimated_tokens,
            bounded.receipt.raw_bytes.div_ceil(4)
        );
        assert!(bounded.receipt.raw_bytes <= 800);
        assert!(bounded.receipt.estimated_tokens <= 200);
        assert_eq!(
            bounded.receipt.token_estimate_method,
            TokenEstimateMethod::BytesDividedByFourCeiling
        );
        assert!(bounded.receipt.truncated);
        let item_limited = assembler
            .assemble(&PacketAssemblyRequest {
                view: &view,
                graph: &graph,
                evidence: &evidence,
                limits: PacketLimits::new(5, 2_000, None),
                dependency_limits: dependency_limits(),
            })
            .unwrap();
        assert_eq!(
            item_limited.receipt.omitted_ids,
            vec![node_id("node.theta"), node_id("node.zeta")]
        );
        assert_eq!(item_limited.receipt.effective_max_items, 5);
        assert_eq!(
            assembler.assemble(&PacketAssemblyRequest {
                view: &view,
                graph: &graph,
                evidence: &evidence,
                limits: PacketLimits::new(7, 2_000, Some(1)),
                dependency_limits: dependency_limits(),
            }),
            Err(PacketError::RequiredEvidenceDoesNotFit {
                id: node_id("node.beta"),
                bound: PacketBound::RuntimeTokens,
            })
        );
        let replay = assembler
            .assemble(&PacketAssemblyRequest {
                view: &view,
                graph: &graph,
                evidence: &evidence,
                limits: PacketLimits::new(6, 800, Some(200)),
                dependency_limits: dependency_limits(),
            })
            .unwrap();
        assert_eq!(bounded, replay);
        let byte_limited = assembler
            .assemble(&PacketAssemblyRequest {
                view: &view,
                graph: &graph,
                evidence: &evidence,
                limits: PacketLimits::new(7, 500, None),
                dependency_limits: dependency_limits(),
            })
            .unwrap();
        assert_eq!(byte_limited.receipt.omitted_ids, vec![node_id("node.zeta")]);
        assert!(byte_limited.receipt.raw_bytes <= 500);
        let token_limited = assembler
            .assemble(&PacketAssemblyRequest {
                view: &view,
                graph: &graph,
                evidence: &evidence,
                limits: PacketLimits::new(7, 2_000, Some(120)),
                dependency_limits: dependency_limits(),
            })
            .unwrap();
        assert_eq!(
            token_limited.receipt.omitted_ids,
            vec![node_id("node.zeta")]
        );
        assert!(token_limited.receipt.estimated_tokens <= 120);
        let mut oversized_required = packet_evidence();
        oversized_required[3].content = format!("beta{}", "b".repeat(300));
        assert_eq!(
            assembler.assemble(&PacketAssemblyRequest {
                view: &view,
                graph: &graph,
                evidence: &oversized_required,
                limits: PacketLimits::new(7, 200, None),
                dependency_limits: dependency_limits(),
            }),
            Err(PacketError::RequiredEvidenceDoesNotFit {
                id: node_id("node.beta"),
                bound: PacketBound::Bytes,
            })
        );
        assert_eq!(
            assembler.assemble(&PacketAssemblyRequest {
                view: &view,
                graph: &graph,
                evidence: &evidence[1..],
                limits: PacketLimits::new(7, 2_000, None),
                dependency_limits: dependency_limits(),
            }),
            Err(PacketError::MissingSelectedEvidence(node_id("node.zeta")))
        );
        let mut duplicate = packet_evidence();
        duplicate.push(evidence[0].clone());
        assert_eq!(
            assembler.assemble(&PacketAssemblyRequest {
                view: &view,
                graph: &graph,
                evidence: &duplicate,
                limits: PacketLimits::new(7, 2_000, None),
                dependency_limits: dependency_limits(),
            }),
            Err(PacketError::DuplicateSelectedEvidence(node_id("node.zeta")))
        );
        let mut unanchored_required = packet_evidence();
        unanchored_required[2].preservation_anchors.clear();
        assert_eq!(
            assembler.assemble(&PacketAssemblyRequest {
                view: &view,
                graph: &graph,
                evidence: &unanchored_required,
                limits: PacketLimits::new(7, 2_000, None),
                dependency_limits: dependency_limits(),
            }),
            Err(PacketError::RequiredEvidenceMissingPreservationAnchors(
                node_id("node.alpha")
            ))
        );
        let mut absent_anchor_value = packet_evidence();
        absent_anchor_value[2].preservation_anchors =
            vec![anchor("required.alpha", "not-in-alpha-evidence")];
        assert_eq!(
            assembler.assemble(&PacketAssemblyRequest {
                view: &view,
                graph: &graph,
                evidence: &absent_anchor_value,
                limits: PacketLimits::new(7, 2_000, None),
                dependency_limits: dependency_limits(),
            }),
            Err(PacketError::PreservationAnchorNotInEvidence {
                evidence_id: node_id("node.alpha"),
                anchor_id: "required.alpha".to_owned(),
            })
        );

        let (closure_graph, closure_view, closure_evidence) = dependency_packet_fixture();
        let dependency_packet = assembler
            .assemble(&PacketAssemblyRequest {
                view: &closure_view,
                graph: &closure_graph,
                evidence: &closure_evidence[..2],
                limits: PacketLimits::new(2, 1_000, None),
                dependency_limits: DependencyClosureLimits::new(4, 2).unwrap(),
            })
            .unwrap();
        assert_eq!(
            dependency_packet.receipt.seed_ids,
            vec![node_id("node.closure.seed")]
        );
        assert_eq!(
            dependency_packet.receipt.admitted_dependency_ids,
            vec![node_id("node.closure.dependency")]
        );
        assert_eq!(
            dependency_packet.receipt.selected_ids,
            vec![
                node_id("node.closure.seed"),
                node_id("node.closure.dependency")
            ]
        );
        assert_eq!(
            dependency_packet.items[1].provenance,
            source("dependency.rs:1")
        );
        assert!(!dependency_packet.receipt.dependency_closure_truncated);
        assert_eq!(
            assembler.assemble(&PacketAssemblyRequest {
                view: &closure_view,
                graph: &closure_graph,
                evidence: &closure_evidence,
                limits: PacketLimits::new(3, 1_000, None),
                dependency_limits: DependencyClosureLimits::new(4, 2).unwrap(),
            }),
            Err(PacketError::UnrelatedEvidence(node_id(
                "node.closure.unrelated"
            )))
        );
        assert_eq!(
            assembler.assemble(&PacketAssemblyRequest {
                view: &closure_view,
                graph: &closure_graph,
                evidence: &closure_evidence[..2],
                limits: PacketLimits::new(1, 1_000, None),
                dependency_limits: DependencyClosureLimits::new(4, 2).unwrap(),
            }),
            Err(PacketError::RequiredEvidenceDoesNotFit {
                id: node_id("node.closure.dependency"),
                bound: PacketBound::Items,
            })
        );
        let depth_truncated = assembler
            .assemble(&PacketAssemblyRequest {
                view: &closure_view,
                graph: &closure_graph,
                evidence: &closure_evidence[..1],
                limits: PacketLimits::new(1, 1_000, None),
                dependency_limits: DependencyClosureLimits::new(0, 1).unwrap(),
            })
            .unwrap();
        assert!(depth_truncated.receipt.dependency_closure_truncated);
        assert!(depth_truncated.receipt.admitted_dependency_ids.is_empty());
        assert_eq!(depth_truncated.receipt.dependency_max_depth, 0);
        assert_eq!(depth_truncated.receipt.dependency_max_nodes, 1);

        let preserved = "node=node.beta\nnode=node.gamma\nalpha\nbeta\ndelta\ngamma\n";
        let applied_port = InMemorySqzPort::output(preserved);
        let applied = SqzAdapter::new(applied_port, sqz_config(SqzPolicy::PublicOn, 200))
            .evaluate(bounded.clone(), 150)
            .unwrap();
        assert_eq!(
            applied.receipt.schema_version,
            crate::adapters::sqz::SQZ_RECEIPT_SCHEMA_VERSION
        );
        assert_eq!(applied.receipt.status, SqzStatus::Applied);
        assert_eq!(applied.receipt.configured_identity, SqzIdentity::approved());
        assert_eq!(
            applied.receipt.returned_identity,
            Some(SqzIdentity::approved())
        );
        assert_eq!(applied.receipt.raw_bytes, bounded.payload.len());
        assert_eq!(
            applied.receipt.candidate_compressed_bytes,
            Some(applied.packet.payload.len())
        );
        assert_eq!(applied.receipt.returned_bytes, applied.packet.payload.len());
        assert_eq!(applied.receipt.actual_input_tokens, Some(111));
        assert_eq!(applied.receipt.actual_output_tokens, Some(17));
        assert_eq!(applied.receipt.fidelity_status, FidelityStatus::Passed);
        assert_eq!(
            applied.receipt.required_fidelity_anchor_ids,
            vec![
                "global.beta-node".to_owned(),
                "global.gamma-node".to_owned(),
                "required.alpha".to_owned(),
                "required.beta".to_owned(),
                "required.delta".to_owned(),
                "required.gamma".to_owned(),
            ]
        );
        assert!(applied.receipt.missing_fidelity_anchor_ids.is_empty());
        assert_eq!(applied.receipt.dlp_status, DlpStatus::Passed);
        assert!(applied.receipt.dlp_findings.is_empty());
        assert_eq!(applied.receipt.requested_max_output_bytes, 150);
        assert_eq!(applied.receipt.effective_max_output_bytes, 150);
        assert!(applied.receipt.monotonic_call_latency >= std::time::Duration::ZERO);
        let applied_id = ResultId::sha256(applied.packet.payload.as_bytes());
        assert_eq!(
            applied.receipt.sqz_id.as_ref().unwrap().algorithm,
            applied_id.algorithm()
        );
        assert_eq!(
            applied.receipt.sqz_id.as_ref().unwrap().value,
            applied_id.value()
        );

        let returned_identity = SqzIdentity::new(
            "unapproved-cargo-install:sqz-cli=1.1.1",
            "sqz 1.1.1",
            "f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5",
        );
        let mut identity_mismatch_port = InMemorySqzPort::output(preserved);
        identity_mismatch_port.response.as_mut().unwrap().identity = returned_identity.clone();
        let identity_mismatch =
            SqzAdapter::new(identity_mismatch_port, sqz_config(SqzPolicy::PublicOn, 200))
                .evaluate(bounded.clone(), 150)
                .unwrap();
        assert_eq!(identity_mismatch.receipt.status, SqzStatus::Failed);
        assert_eq!(
            identity_mismatch.receipt.failure_reason,
            Some(SqzFailureReason::ReturnedIdentityMismatch)
        );
        assert_eq!(
            identity_mismatch.receipt.returned_identity,
            Some(returned_identity)
        );
        assert_eq!(identity_mismatch.receipt.sqz_id, None);
        assert_eq!(identity_mismatch.packet, bounded);

        let off_port = InMemorySqzPort::output(preserved);
        let off_calls = off_port.calls.clone();
        let off = SqzAdapter::new(off_port, sqz_config(SqzPolicy::PublicOff, 200))
            .evaluate(bounded.clone(), 150)
            .unwrap();
        assert_eq!(off.receipt.status, SqzStatus::Off);
        assert_eq!(off.packet, bounded);
        assert_eq!(off.receipt.candidate_compressed_bytes, None);
        assert_eq!(off.receipt.actual_input_tokens, None);
        assert_eq!(off.receipt.fidelity_status, FidelityStatus::Passed);
        assert_eq!(off.receipt.dlp_status, DlpStatus::Passed);
        assert_eq!(off.receipt.sqz_id, None);
        assert_eq!(off_calls.get(), 0);

        let expanded_payload = format!("{}node=node.beta\nnode=node.gamma\n", bounded.payload);
        let rejected_candidate_id = ResultId::sha256(expanded_payload.as_bytes());
        let not_beneficial = SqzAdapter::new(
            InMemorySqzPort::output(expanded_payload),
            sqz_config(SqzPolicy::PublicOn, 2_000),
        )
        .evaluate(bounded.clone(), 1_500)
        .unwrap();
        assert_eq!(not_beneficial.receipt.status, SqzStatus::NotBeneficial);
        assert_eq!(not_beneficial.packet, bounded);
        assert!(
            not_beneficial.receipt.candidate_compressed_bytes.unwrap()
                >= not_beneficial.receipt.raw_bytes
        );
        let not_beneficial_id = ResultId::sha256(not_beneficial.packet.payload.as_bytes());
        assert_eq!(
            not_beneficial.receipt.sqz_id.as_ref().unwrap().algorithm,
            not_beneficial_id.algorithm()
        );
        assert_eq!(
            not_beneficial.receipt.sqz_id.as_ref().unwrap().value,
            not_beneficial_id.value()
        );
        assert_ne!(
            not_beneficial.receipt.sqz_id.as_ref().unwrap().value,
            rejected_candidate_id.value()
        );

        let missing_required = SqzAdapter::new(
            InMemorySqzPort::output("node=node.beta\nnode=node.gamma\nalpha\nbeta\ngamma\n"),
            sqz_config(SqzPolicy::PublicOn, 200),
        )
        .evaluate(bounded.clone(), 150)
        .unwrap();
        assert_eq!(missing_required.receipt.status, SqzStatus::Failed);
        assert_eq!(
            missing_required.receipt.failure_reason,
            Some(SqzFailureReason::MissingFidelityAnchors)
        );
        assert_eq!(
            missing_required.receipt.missing_fidelity_anchor_ids,
            vec!["required.delta"]
        );
        assert_eq!(missing_required.receipt.sqz_id, None);
        assert_eq!(missing_required.packet, bounded);

        let dlp_failure = SqzAdapter::new(
            InMemorySqzPort::output(format!("{preserved}api_key=abcdef\n")),
            sqz_config(SqzPolicy::PublicOn, 200),
        )
        .evaluate(bounded.clone(), 150)
        .unwrap();
        assert_eq!(dlp_failure.receipt.status, SqzStatus::Failed);
        assert_eq!(
            dlp_failure.receipt.failure_reason,
            Some(SqzFailureReason::DlpFindings)
        );
        assert_eq!(
            dlp_failure.receipt.dlp_findings,
            vec!["credential_assignment"]
        );
        assert_eq!(dlp_failure.receipt.dlp_status, DlpStatus::Findings);
        assert_eq!(dlp_failure.receipt.sqz_id, None);
        assert_eq!(dlp_failure.packet, bounded);

        let anchor_dlp_port = InMemorySqzPort::output(preserved);
        let anchor_dlp_calls = anchor_dlp_port.calls.clone();
        let anchor_dlp = SqzAdapter::new(
            anchor_dlp_port,
            SqzAdapterConfig::new(
                SqzPolicy::PublicOn,
                SqzIdentity::approved(),
                200,
                vec![anchor("global.secret", "api_key=abcdef")],
            ),
        )
        .evaluate(bounded.clone(), 150)
        .unwrap();
        assert_eq!(anchor_dlp.receipt.status, SqzStatus::Failed);
        assert_eq!(anchor_dlp.receipt.dlp_status, DlpStatus::Findings);
        assert_eq!(
            anchor_dlp.receipt.dlp_findings,
            vec!["credential_assignment"]
        );
        assert_eq!(anchor_dlp.receipt.sqz_id, None);
        assert_eq!(anchor_dlp_calls.get(), 0);

        let over_bound = SqzAdapter::new(
            InMemorySqzPort::output(preserved),
            sqz_config(SqzPolicy::PublicOn, 200),
        )
        .evaluate(bounded.clone(), 4)
        .unwrap();
        assert_eq!(over_bound.receipt.status, SqzStatus::Failed);
        assert_eq!(
            over_bound.receipt.failure_reason,
            Some(SqzFailureReason::OutputExceedsBound)
        );
        assert_eq!(over_bound.receipt.sqz_id, None);
        assert_eq!(over_bound.packet, bounded);

        let locked_port = InMemorySqzPort::unavailable();
        let locked_calls = locked_port.calls.clone();
        let locked = SqzAdapter::new(locked_port, sqz_config(SqzPolicy::InternalLocked, 200))
            .evaluate(bounded, 150);
        assert!(locked.is_err());
        let locked = locked.unwrap_err();
        assert_eq!(locked.receipt.status, SqzStatus::Failed);
        assert_eq!(
            locked.receipt.failure_reason,
            Some(SqzFailureReason::PortUnavailable(
                SqzPortErrorCode::Unavailable
            ))
        );
        assert_eq!(locked.receipt.policy, SqzPolicy::InternalLocked);
        assert_eq!(locked.receipt.sqz_id, None);
        assert_eq!(locked_calls.get(), 1);
    }
    fn p8_traversal_packet_journey() {
        let seed = structural_node(
            "docs/seed.md",
            "seed",
            StructuralLifecycle::Current,
            StructuralGeneratedStatus::Source,
        );
        let kinds = [
            StructuralEdgeKind::Dependency,
            StructuralEdgeKind::Impact,
            StructuralEdgeKind::Requirement,
            StructuralEdgeKind::Design,
            StructuralEdgeKind::Implementation,
            StructuralEdgeKind::Validation,
            StructuralEdgeKind::Domain,
            StructuralEdgeKind::Architecture,
            StructuralEdgeKind::BusinessFlow,
        ];
        let mut nodes = vec![seed.clone()];
        for (i, kind) in kinds.into_iter().enumerate() {
            nodes.push(structural_node(
                &format!("docs/{i}.md"),
                kind.as_str(),
                StructuralLifecycle::Current,
                StructuralGeneratedStatus::Source,
            ));
        }
        let deep = structural_node(
            "docs/deep.md",
            "deep",
            StructuralLifecycle::Current,
            StructuralGeneratedStatus::Source,
        );
        let deep_id = deep.id().to_owned();
        nodes.push(deep);
        let generated = structural_node(
            "docs/generated.md",
            "generated",
            StructuralLifecycle::Current,
            StructuralGeneratedStatus::Generated,
        );
        let generated_id = generated.id().to_owned();
        nodes.push(generated);
        nodes.push(structural_node(
            "docs/conflict.md",
            "conflict",
            StructuralLifecycle::Superseded,
            StructuralGeneratedStatus::Generated,
        ));
        let mut edges = Vec::new();
        for (i, kind) in kinds.into_iter().enumerate() {
            edges.push(
                StructuralEdge::new(
                    kind,
                    seed.id(),
                    nodes[i + 1].id(),
                    format!("edge:{i}"),
                    vec![],
                    StructuralEdgeCertainty::Known,
                )
                .unwrap(),
            );
        }
        edges.push(
            StructuralEdge::new(
                StructuralEdgeKind::Dependency,
                seed.id(),
                nodes[1].id(),
                "edge:alternate-locator",
                vec![],
                StructuralEdgeCertainty::Known,
            )
            .unwrap(),
        );
        edges.push(
            StructuralEdge::new(
                StructuralEdgeKind::Dependency,
                nodes[1].id(),
                &deep_id,
                "edge:deep",
                vec![],
                StructuralEdgeCertainty::Known,
            )
            .unwrap(),
        );
        edges.push(
            StructuralEdge::new(
                StructuralEdgeKind::Dependency,
                nodes[1].id(),
                seed.id(),
                "cycle",
                vec![],
                StructuralEdgeCertainty::Known,
            )
            .unwrap(),
        );
        edges.push(
            StructuralEdge::new(
                StructuralEdgeKind::Impact,
                seed.id(),
                nodes[2].id(),
                "dynamic",
                vec![],
                StructuralEdgeCertainty::Dynamic,
            )
            .unwrap(),
        );
        let dependency_id = nodes[1].id().to_owned();
        let snapshot = StructuralGraphSnapshot::new(
            1,
            None,
            structural_digest('b'),
            structural_digest('c'),
            structural_digest('d'),
            nodes,
            edges,
            StructuralSnapshotReceipt::new("p8", None, None, None, vec![]).unwrap(),
        )
        .unwrap();
        let request = StructuralTraversalRequest {
            seed: seed.id().to_owned(),
            relationships: kinds.to_vec(),
            max_depth: 2,
            max_nodes: 32,
            max_edges: 64,
        };
        let traversal = Traversal::run(&snapshot, &request).unwrap();
        assert_eq!(traversal.visited_ids.len(), 11);
        assert_eq!(
            traversal
                .paths
                .iter()
                .skip(1)
                .take(kinds.len())
                .map(|path| path.steps[0].kind)
                .collect::<Vec<_>>(),
            kinds
        );
        assert_eq!(traversal.terminal, StructuralTraversalTerminal::Complete);
        assert!(traversal
            .skipped_edges
            .iter()
            .any(|edge| edge.certainty == StructuralEdgeCertainty::Dynamic));
        assert!(matches!(
            Traversal::run(
                &snapshot,
                &StructuralTraversalRequest {
                    max_depth: 0,
                    ..request.clone()
                }
            ),
            Err(StructuralTraversalError::InvalidLimits { max_depth: 0, .. })
        ));
        assert_eq!(
            Traversal::run(
                &snapshot,
                &StructuralTraversalRequest {
                    max_depth: 1,
                    ..request.clone()
                }
            )
            .unwrap()
            .terminal,
            StructuralTraversalTerminal::DepthLimit
        );
        assert_eq!(
            Traversal::run(
                &snapshot,
                &StructuralTraversalRequest {
                    max_nodes: 1,
                    ..request.clone()
                }
            )
            .unwrap()
            .terminal,
            StructuralTraversalTerminal::NodeLimit
        );
        assert_eq!(
            Traversal::run(
                &snapshot,
                &StructuralTraversalRequest {
                    max_edges: 1,
                    ..request.clone()
                }
            )
            .unwrap()
            .terminal,
            StructuralTraversalTerminal::EdgeLimit
        );
        assert!(matches!(
            Traversal::run(
                &snapshot,
                &StructuralTraversalRequest {
                    seed: structural_digest('f'),
                    ..request.clone()
                }
            ),
            Err(StructuralTraversalError::MissingSeed(_))
        ));
        let frequency =
            FrozenHistoricalFrequency::new(snapshot.frozen_frequency_digest(), BTreeMap::new())
                .unwrap();
        let ranking = StageRanker::new(
            &snapshot,
            StageRankingRequest::new(
                seed.id(),
                SdlcStage::Design,
                vec![],
                vec![],
                RankingLimits::new(2).unwrap(),
            )
            .unwrap(),
            &frequency,
        )
        .unwrap()
        .rank();
        let requirement_id = traversal
            .paths
            .iter()
            .find(|path| {
                path.steps
                    .last()
                    .is_some_and(|step| step.kind == StructuralEdgeKind::Requirement)
            })
            .unwrap()
            .node_id
            .clone();
        let excerpts = vec![
            StructuralExcerpt {
                node_id: seed.id().to_owned(),
                locator: "docs/seed.md#s".to_owned(),
                content: "seed".to_owned(),
                content_digest: seed.content_digest().to_owned(),
                complete_file: false,
            },
            StructuralExcerpt {
                node_id: requirement_id.clone(),
                locator: "docs/2.md#r".to_owned(),
                content: "requirement".to_owned(),
                content_digest: snapshot
                    .node(&requirement_id)
                    .unwrap()
                    .content_digest()
                    .to_owned(),
                complete_file: false,
            },
        ];
        let packet_request = StructuralPacketRequest {
            snapshot: &snapshot,
            ranking: &ranking,
            traversal: Some(&traversal),
            excerpts: &excerpts,
            required_domains: vec!["requirement".to_owned()],
            required_evidence_classes: vec![EvidenceClass::Requirement],
            limits: StructuralPacketLimits {
                max_items: 32,
                max_packet_bytes: 4096,
                max_excerpt_bytes: 128,
            },
        };
        let packet = PacketAssembler::new()
            .assemble_structural(&packet_request)
            .unwrap();
        assert!(packet
            .items
            .windows(2)
            .all(|pair| pair[0].tier <= pair[1].tier));
        assert!(packet
            .items
            .iter()
            .any(|item| item.tier == StructuralPacketTier::RequiredDependency
                && item.node_id == requirement_id));
        assert!(packet
            .items
            .iter()
            .any(|item| item.tier == StructuralPacketTier::ConflictSuperseded
                && item.non_authoritative));
        assert_eq!(
            packet.receipt.total_packet_bytes,
            packet.receipt.total_locator_bytes + packet.receipt.total_excerpt_bytes
        );
        assert!(packet.receipt.sufficient && packet.receipt.missing_domains.is_empty());
        assert!(packet
            .receipt
            .covered_evidence_classes
            .contains(&EvidenceClass::Requirement));
        assert_eq!(
            packet,
            PacketAssembler::new()
                .assemble_structural(&packet_request)
                .unwrap()
        );
        assert!(
            !PacketAssembler::new()
                .assemble_structural(&StructuralPacketRequest {
                    required_domains: vec!["missing".to_owned()],
                    ..packet_request.clone()
                })
                .unwrap()
                .receipt
                .sufficient
        );
        let ranking_all = StageRanker::new(
            &snapshot,
            StageRankingRequest::new(
                seed.id(),
                SdlcStage::Design,
                vec![],
                vec![],
                RankingLimits::new(snapshot.nodes().len()).unwrap(),
            )
            .unwrap(),
            &frequency,
        )
        .unwrap()
        .rank();
        let mut all_excerpts = excerpts.clone();
        all_excerpts.push(StructuralExcerpt {
            node_id: generated_id.clone(),
            locator: "docs/generated.md#follow-up".to_owned(),
            content: "generated support".to_owned(),
            content_digest: snapshot
                .node(&generated_id)
                .unwrap()
                .content_digest()
                .to_owned(),
            complete_file: false,
        });
        let all_request = StructuralPacketRequest {
            ranking: &ranking_all,
            excerpts: &all_excerpts,
            ..packet_request.clone()
        };
        let all_packet = PacketAssembler::new()
            .assemble_structural(&all_request)
            .unwrap();
        assert!(all_packet
            .items
            .iter()
            .filter(|item| item.tier == StructuralPacketTier::PrimaryCanonical)
            .all(|item| {
                item.lifecycle == StructuralLifecycle::Current
                    && item.generated_status == StructuralGeneratedStatus::Source
                    && matches!(
                        item.authority,
                        StructuralAuthority::Canonical | StructuralAuthority::Approved
                    )
            }));
        let generated_follow_up = all_packet
            .items
            .iter()
            .find(|item| item.node_id == generated_id)
            .unwrap();
        assert_eq!(
            generated_follow_up.tier,
            StructuralPacketTier::FollowUpLocator
        );
        assert!(generated_follow_up.non_authoritative);
        assert_eq!(generated_follow_up.locator, "docs/generated.md#follow-up");
        assert!(generated_follow_up.excerpt.is_empty());
        assert_eq!(generated_follow_up.excerpt_bytes, 0);
        assert!(generated_follow_up.locator_only);
        let selected_requirement = all_packet
            .items
            .iter()
            .find(|item| item.node_id == requirement_id)
            .unwrap();
        assert_eq!(
            selected_requirement.tier,
            StructuralPacketTier::PrimaryCanonical
        );
        assert_eq!(
            selected_requirement.edge_trail.last().unwrap().kind,
            StructuralEdgeKind::Requirement
        );
        assert!(all_packet
            .receipt
            .covered_evidence_classes
            .contains(&EvidenceClass::Requirement));
        let mut alternate_ranking = ranking_all.clone();
        alternate_ranking.selected[0].rank_key.required_contribution ^= 1;
        let alternate_ranking_packet = PacketAssembler::new()
            .assemble_structural(&StructuralPacketRequest {
                ranking: &alternate_ranking,
                ..all_request.clone()
            })
            .unwrap();
        assert_ne!(
            all_packet.receipt.ranking_binding_digest,
            alternate_ranking_packet.receipt.ranking_binding_digest
        );
        let dependency_path = traversal
            .paths
            .iter()
            .position(|path| path.node_id == dependency_id)
            .unwrap();
        let selected_edge_id = &traversal.paths[dependency_path].steps[0].edge_id;
        let alternate_edge = snapshot
            .edges()
            .iter()
            .find(|edge| {
                edge.kind() == StructuralEdgeKind::Dependency
                    && edge.source() == seed.id()
                    && edge.target() == dependency_id
                    && edge.id() != selected_edge_id
            })
            .unwrap();
        let mut alternate_traversal = traversal.clone();
        alternate_traversal.paths[dependency_path].steps[0].edge_id =
            alternate_edge.id().to_owned();
        alternate_traversal.paths[dependency_path].steps[0].source_locator =
            alternate_edge.source_locator().to_owned();
        let alternate_traversal_packet = PacketAssembler::new()
            .assemble_structural(&StructuralPacketRequest {
                traversal: Some(&alternate_traversal),
                ..all_request.clone()
            })
            .unwrap();
        assert_ne!(
            all_packet.receipt.traversal_binding_digest,
            alternate_traversal_packet.receipt.traversal_binding_digest
        );
        let primary_count = all_packet
            .items
            .iter()
            .filter(|item| item.tier == StructuralPacketTier::PrimaryCanonical)
            .count();
        let bounded_packet = PacketAssembler::new()
            .assemble_structural(&StructuralPacketRequest {
                limits: StructuralPacketLimits {
                    max_items: primary_count,
                    ..all_request.limits
                },
                ..all_request.clone()
            })
            .unwrap();
        assert!(bounded_packet.receipt.bounded_omissions.iter().any(|item| {
            item.tier == StructuralPacketTier::ConflictSuperseded
                && item.reason == StructuralPacketOmissionReason::ItemLimit
        }));
        assert!(bounded_packet.receipt.bounded_omissions.iter().any(|item| {
            item.node_id == generated_id
                && item.tier == StructuralPacketTier::FollowUpLocator
                && item.reason == StructuralPacketOmissionReason::ItemLimit
        }));
        assert!(matches!(
            PacketAssembler::new().assemble_structural(&StructuralPacketRequest {
                excerpts: &[StructuralExcerpt {
                    content: "x".repeat(129),
                    ..excerpts[0].clone()
                }],
                ..packet_request.clone()
            }),
            Err(StructuralPacketError::ExcerptTooLarge(_))
        ));
        assert!(matches!(
            PacketAssembler::new().assemble_structural(&StructuralPacketRequest {
                excerpts: &[StructuralExcerpt {
                    locator: "x".repeat(4097),
                    ..excerpts[0].clone()
                }],
                ..packet_request.clone()
            }),
            Err(StructuralPacketError::InvalidExcerptLocator(_))
        ));
        assert!(matches!(
            PacketAssembler::new().assemble_structural(&StructuralPacketRequest {
                excerpts: &[StructuralExcerpt {
                    complete_file: true,
                    ..excerpts[0].clone()
                }],
                ..packet_request.clone()
            }),
            Err(StructuralPacketError::CompleteFileRejected(_))
        ));
        let mut forged_ranking = ranking.clone();
        forged_ranking.selected[0].canonical_path = "docs/forged.md".to_owned();
        assert!(matches!(
            PacketAssembler::new().assemble_structural(&StructuralPacketRequest {
                ranking: &forged_ranking,
                ..packet_request.clone()
            }),
            Err(StructuralPacketError::InvalidRankingReceipt(_))
        ));
        let mut forged_traversal = traversal.clone();
        forged_traversal.paths[1].steps[0].source_locator = "forged".to_owned();
        assert!(matches!(
            PacketAssembler::new().assemble_structural(&StructuralPacketRequest {
                traversal: Some(&forged_traversal),
                ..packet_request.clone()
            }),
            Err(StructuralPacketError::InvalidTraversalReceipt(_))
        ));
        let mut duplicate_traversal = traversal.clone();
        duplicate_traversal
            .visited_ids
            .push(traversal.visited_ids[0].clone());
        duplicate_traversal.paths.push(traversal.paths[0].clone());
        assert!(matches!(
            PacketAssembler::new().assemble_structural(&StructuralPacketRequest {
                traversal: Some(&duplicate_traversal),
                ..packet_request.clone()
            }),
            Err(StructuralPacketError::InvalidTraversalReceipt(_))
        ));
        let mut mismatch = ranking.clone();
        mismatch.snapshot_digest = structural_digest('e');
        assert_eq!(
            PacketAssembler::new().assemble_structural(&StructuralPacketRequest {
                ranking: &mismatch,
                ..packet_request.clone()
            }),
            Err(StructuralPacketError::SnapshotDigestMismatch)
        );
    }

    #[test]
    fn p8_traversal_packet() {
        p8_traversal_packet_journey();
    }

    // ── Packet lifecycle & superseded-reference tests (Slice 1.2-C) ──────────

    use crate::bundle::{Bundle, Doc, Frontmatter};
    use crate::schema::YamlValue;

    fn packet_doc(path: &str, status_val: &str, type_val: &str, body: &str) -> Doc {
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String(type_val.to_owned()));
        fields.insert(
            "status".to_owned(),
            YamlValue::String(status_val.to_owned()),
        );
        let raw = format!("---\ntype: {type_val}\nstatus: {status_val}\n---\n");
        Doc::new(
            path,
            format!("{raw}{body}"),
            body.to_owned(),
            Frontmatter::from_parsed(raw, fields),
        )
    }

    fn packet_doc_with_work_class(path: &str, status_val: &str, work_class: &str) -> Doc {
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "status".to_owned(),
            YamlValue::String(status_val.to_owned()),
        );
        fields.insert(
            "work_class".to_owned(),
            YamlValue::String(work_class.to_owned()),
        );
        let raw =
            format!("---\ntype: Concept\nstatus: {status_val}\nwork_class: {work_class}\n---\n");
        Doc::new(
            path,
            format!("{raw}body"),
            "body",
            Frontmatter::from_parsed(raw, fields),
        )
    }

    #[test]
    fn packet_active_helper_referencing_active_prompt_passes() {
        let active = packet_doc(
            "docs/plans/active/prompts/agent.md",
            "active",
            "research-assistant",
            "See [helper](helper.md) for context.",
        );
        let helper = packet_doc(
            "docs/plans/active/prompts/helper.md",
            "active",
            "Concept",
            "",
        );
        let bundle = Bundle::from_documents([active, helper]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn packet_superseded_helper_retained_not_executable() {
        // A superseded helper referencing an active prompt should not produce findings
        // because superseded is not executable (only active docs are checked for references).
        let superseded_helper = packet_doc(
            "docs/plans/old/prompts/old-helper.md",
            "superseded",
            "research-assistant",
            "Refers to [active-prompt](active-prompt.md).",
        );
        let active_prompt = packet_doc(
            "docs/plans/old/prompts/active-prompt.md",
            "active",
            "Concept",
            "",
        );
        let bundle = Bundle::from_documents([superseded_helper, active_prompt]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn packet_helper_missing_lifecycle_emits_error() {
        let helper = packet_doc(
            "docs/plans/test/prompts/agent.md",
            "draft",
            "research-assistant",
            "",
        );
        let bundle = Bundle::from_documents([helper]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "packet_missing_authority_field");
        assert_eq!(report.findings[0].path, "docs/plans/test/prompts/agent.md");
    }

    #[test]
    fn packet_helper_no_status_emits_error() {
        let mut fields = BTreeMap::new();
        fields.insert(
            "type".to_owned(),
            YamlValue::String("research-assistant".to_owned()),
        );
        let raw = "---\ntype: research-assistant\n---\n";
        let helper = Doc::new(
            "docs/plans/test/prompts/agent.md",
            format!("{raw}body"),
            "body",
            Frontmatter::from_parsed(raw, fields),
        );
        let bundle = Bundle::from_documents([helper]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "packet_missing_authority_field");
    }

    #[test]
    fn packet_helper_okf_status_alone_does_not_grant_lifecycle_authority() {
        // AC-1: okf_status must not grant packet lifecycle authority;
        // only the status field controls authority.
        let mut fields = BTreeMap::new();
        fields.insert(
            "type".to_owned(),
            YamlValue::String("research-assistant".to_owned()),
        );
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        let raw = "---\ntype: research-assistant\nokf_status: active\n---\n";
        let helper = Doc::new(
            "docs/plans/test/prompts/agent.md",
            format!("{raw}body"),
            "body",
            Frontmatter::from_parsed(raw, fields),
        );
        let bundle = Bundle::from_documents([helper]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "packet_missing_authority_field");
    }

    #[test]
    fn packet_work_class_helper_missing_status_emits_error() {
        let helper =
            packet_doc_with_work_class("docs/plans/test/prompts/worker.md", "draft", "ingestion");
        let bundle = Bundle::from_documents([helper]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "packet_missing_authority_field");
    }

    #[test]
    fn packet_active_helper_references_superseded_via_frontmatter() {
        let mut active_fields = BTreeMap::new();
        active_fields.insert(
            "type".to_owned(),
            YamlValue::String("research-assistant".to_owned()),
        );
        active_fields.insert("status".to_owned(), YamlValue::String("active".to_owned()));
        active_fields.insert(
            "source".to_owned(),
            YamlValue::String("docs/plans/old/prompts/old-packet.md".to_owned()),
        );
        let active_raw = "---\ntype: research-assistant\nstatus: active\nsource: docs/plans/old/prompts/old-packet.md\n---\n";
        let active = Doc::new(
            "docs/plans/current/prompts/agent.md",
            format!("{active_raw}body"),
            "body",
            Frontmatter::from_parsed(active_raw, active_fields),
        );
        let superseded = packet_doc(
            "docs/plans/old/prompts/old-packet.md",
            "superseded",
            "Concept",
            "",
        );
        let bundle = Bundle::from_documents([active, superseded]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "superseded_prompt_reference");
        assert!(report.findings[0]
            .message
            .contains("docs/plans/old/prompts/old-packet.md"));
    }

    #[test]
    fn packet_active_packet_references_superseded_via_markdown_link() {
        let active = packet_doc(
            "docs/plans/current/prompts/main.md",
            "active",
            "Concept",
            "See [old approach](../../old/prompts/old-packet.md) for history.",
        );
        let superseded = packet_doc(
            "docs/plans/old/prompts/old-packet.md",
            "superseded",
            "Concept",
            "",
        );
        let bundle = Bundle::from_documents([active, superseded]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "superseded_prompt_reference");
    }

    #[test]
    fn packet_active_helper_references_superseded_via_exact_path() {
        let mut active_fields = BTreeMap::new();
        active_fields.insert(
            "type".to_owned(),
            YamlValue::String("ingestion-librarian".to_owned()),
        );
        active_fields.insert("status".to_owned(), YamlValue::String("active".to_owned()));
        active_fields.insert(
            "reference".to_owned(),
            YamlValue::String("docs/old/prompts/old.md".to_owned()),
        );
        let active_raw = "---\ntype: ingestion-librarian\nstatus: active\nreference: docs/old/prompts/old.md\n---\n";
        let active = Doc::new(
            "docs/current/prompts/agent.md",
            format!("{active_raw}body"),
            "body",
            Frontmatter::from_parsed(active_raw, active_fields),
        );
        let superseded = packet_doc("docs/old/prompts/old.md", "superseded", "Concept", "");
        let bundle = Bundle::from_documents([active, superseded]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "superseded_prompt_reference");
    }

    #[test]
    fn packet_active_references_superseded_via_unique_basename() {
        let active = packet_doc(
            "docs/current/prompts/agent.md",
            "active",
            "Concept",
            "Refer to [old-packet](old-packet.md).",
        );
        let superseded = packet_doc(
            "docs/old/prompts/old-packet.md",
            "superseded",
            "Concept",
            "",
        );
        let bundle = Bundle::from_documents([active, superseded]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "superseded_prompt_reference");
        assert!(report.findings[0]
            .message
            .contains("docs/old/prompts/old-packet.md"));
    }

    #[test]
    fn packet_same_basename_different_dirs_no_false_positive() {
        let active = packet_doc(
            "docs/current/prompts/agent.md",
            "active",
            "research-assistant",
            "See [shared](shared.md).",
        );
        let superseded_a = packet_doc("docs/old-a/prompts/shared.md", "superseded", "Concept", "");
        let superseded_b = packet_doc("docs/old-b/prompts/shared.md", "superseded", "Concept", "");
        let bundle = Bundle::from_documents([active, superseded_a, superseded_b]).unwrap();
        let report = PacketValidator::validate(&bundle);
        // Two superseded entries with same basename → ambiguous → no false positive.
        assert!(report.findings.is_empty());
    }

    #[test]
    fn packet_same_basename_one_superseded_one_active_no_false_positive_for_active() {
        // Active same-directory target resolves concretely → no basename fallback → no finding.
        let active = packet_doc(
            "docs/current/prompts/agent.md",
            "active",
            "research-assistant",
            "See [shared](shared.md).",
        );
        let superseded_shared =
            packet_doc("docs/old/prompts/shared.md", "superseded", "Concept", "");
        let active_shared = packet_doc("docs/current/prompts/shared.md", "active", "Concept", "");
        let bundle = Bundle::from_documents([active, superseded_shared, active_shared]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn packet_permuted_bundle_construction_yields_identical_report() {
        let a = packet_doc(
            "docs/plans/active/prompts/agent.md",
            "active",
            "research-assistant",
            "See [old](../../old/prompts/old.md).",
        );
        let b = packet_doc("docs/plans/old/prompts/old.md", "superseded", "Concept", "");
        let bundle1 = Bundle::from_documents([a.clone(), b.clone()]).unwrap();
        let bundle2 = Bundle::from_documents([b, a]).unwrap();
        let report1 = PacketValidator::validate(&bundle1);
        let report2 = PacketValidator::validate(&bundle2);
        assert_eq!(report1, report2);
        assert_eq!(report1.findings.len(), 1);
        assert_eq!(report1.findings[0].code, "superseded_prompt_reference");
    }

    #[test]
    fn packet_unsafe_references_ignored() {
        let mut active_fields = BTreeMap::new();
        active_fields.insert(
            "type".to_owned(),
            YamlValue::String("research-assistant".to_owned()),
        );
        active_fields.insert("status".to_owned(), YamlValue::String("active".to_owned()));
        active_fields.insert(
            "source".to_owned(),
            YamlValue::String("/etc/passwd".to_owned()),
        );
        let active_raw =
            "---\ntype: research-assistant\nstatus: active\nsource: /etc/passwd\n---\n";
        let active = Doc::new(
            "docs/current/prompts/agent.md",
            format!("{active_raw}See [url](https://evil.com/payload) and [mail](mailto:x@y.com) and [abs](/absolute/path) and [nul](bad\0name.md)."),
            "See [url](https://evil.com/payload) and [mail](mailto:x@y.com) and [abs](/absolute/path) and [nul](bad\0name.md).",
            Frontmatter::from_parsed(active_raw, active_fields),
        );
        let bundle = Bundle::from_documents([active]).unwrap();
        let report = PacketValidator::validate(&bundle);
        // No unsafe references should echo; no superseded targets exist anyway.
        assert!(report.findings.is_empty());
    }

    #[test]
    fn packet_unsafe_doc_paths_redacted_in_findings() {
        // Docs with traversal/absolute/NUL paths: findings must not echo raw unsafe paths.
        let mut fields = BTreeMap::new();
        fields.insert(
            "type".to_owned(),
            YamlValue::String("research-assistant".to_owned()),
        );
        let raw = "---\ntype: research-assistant\n---\n";

        // Absolute path.
        let abs_doc = Doc::new(
            "/etc/shadow",
            format!("{raw}body"),
            "body",
            Frontmatter::from_parsed(raw, fields.clone()),
        );
        let bundle = Bundle::from_documents([abs_doc]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].path, "[redacted]");

        // Traversal path.
        let trav_doc = Doc::new(
            "../../../etc/shadow",
            format!("{raw}body"),
            "body",
            Frontmatter::from_parsed(raw, fields.clone()),
        );
        let bundle = Bundle::from_documents([trav_doc]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].path, "[redacted]");

        // NUL byte in path.
        let nul_doc = Doc::new(
            "bad\0name.md",
            format!("{raw}body"),
            "body",
            Frontmatter::from_parsed(raw, fields),
        );
        let bundle = Bundle::from_documents([nul_doc]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].path, "[redacted]");
    }

    #[test]
    fn packet_unsafe_superseded_path_redacted_in_message() {
        // An active doc with an unsafe reference path must redact it in findings.
        let mut active_fields = BTreeMap::new();
        active_fields.insert(
            "type".to_owned(),
            YamlValue::String("research-assistant".to_owned()),
        );
        active_fields.insert("status".to_owned(), YamlValue::String("active".to_owned()));
        active_fields.insert(
            "source".to_owned(),
            YamlValue::String("/etc/passwd".to_owned()),
        );
        let active_raw =
            "---\ntype: research-assistant\nstatus: active\nsource: /etc/passwd\n---\n";
        let active = Doc::new(
            "docs/plans/current/prompts/agent.md",
            format!("{active_raw}body"),
            "body",
            Frontmatter::from_parsed(active_raw, active_fields),
        );
        // The /etc/passwd reference is unsafe, so it's not checked as superseded.
        let bundle = Bundle::from_documents([active]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn packet_superseded_work_class_helper_with_valid_status_passes() {
        let helper = packet_doc_with_work_class(
            "docs/plans/test/prompts/worker.md",
            "superseded",
            "ingestion",
        );
        let bundle = Bundle::from_documents([helper]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn packet_active_doc_references_superseded_via_relative_link() {
        let active = packet_doc(
            "docs/plans/current/plan.md",
            "active",
            "Concept",
            "See [old plan](../old/plan/prompts/old-plan.md).",
        );
        let superseded = packet_doc(
            "docs/plans/old/plan/prompts/old-plan.md",
            "superseded",
            "Concept",
            "",
        );
        let bundle = Bundle::from_documents([active, superseded]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "superseded_prompt_reference");
    }

    #[test]
    fn packet_relative_reference_resolves_correctly() {
        let active = packet_doc(
            "docs/a/b/c/prompts/agent.md",
            "active",
            "Concept",
            "See [old](../../../../old/prompts/old.md).",
        );
        let superseded = packet_doc("docs/old/prompts/old.md", "superseded", "Concept", "");
        let bundle = Bundle::from_documents([active, superseded]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "superseded_prompt_reference");
        assert!(report.findings[0]
            .message
            .contains("docs/old/prompts/old.md"));
    }

    #[test]
    fn packet_index_and_log_helper_require_status() {
        // index.md and log.md helper candidates must have status (no skip exception).
        let mut index_fields = BTreeMap::new();
        index_fields.insert(
            "type".to_owned(),
            YamlValue::String("research-assistant".to_owned()),
        );
        let index_raw = "---\ntype: research-assistant\n---\n";
        let index_doc = Doc::new(
            "index.md",
            format!("{index_raw}body"),
            "body",
            Frontmatter::from_parsed(index_raw, index_fields),
        );
        let bundle = Bundle::from_documents([index_doc]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "packet_missing_authority_field");
        assert_eq!(report.findings[0].path, "index.md");

        // log.md helper candidate also needs status.
        let mut log_fields = BTreeMap::new();
        log_fields.insert(
            "type".to_owned(),
            YamlValue::String("research-assistant".to_owned()),
        );
        let log_raw = "---\ntype: research-assistant\n---\n";
        let log_doc = Doc::new(
            "log.md",
            format!("{log_raw}body"),
            "body",
            Frontmatter::from_parsed(log_raw, log_fields),
        );
        let bundle2 = Bundle::from_documents([log_doc]).unwrap();
        let report2 = PacketValidator::validate(&bundle2);
        assert_eq!(report2.findings.len(), 1);
        assert_eq!(report2.findings[0].code, "packet_missing_authority_field");
        assert_eq!(report2.findings[0].path, "log.md");
    }

    #[test]
    fn packet_synthesis_librarian_detected_as_helper() {
        let helper = packet_doc(
            "docs/plans/test/prompts/synth.md",
            "active",
            "synthesis-librarian",
            "See [old](../../old/prompts/old.md).",
        );
        let superseded = packet_doc("docs/plans/old/prompts/old.md", "superseded", "Concept", "");
        let bundle = Bundle::from_documents([helper, superseded]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "superseded_prompt_reference");
    }

    #[test]
    fn packet_integrity_librarian_detected_as_helper() {
        let helper = packet_doc(
            "docs/plans/test/prompts/integ.md",
            "active",
            "integrity-librarian",
            "",
        );
        let bundle = Bundle::from_documents([helper]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn packet_role_field_detected_as_helper() {
        let mut fields = BTreeMap::new();
        fields.insert(
            "role".to_owned(),
            YamlValue::String("research-assistant".to_owned()),
        );
        fields.insert("status".to_owned(), YamlValue::String("active".to_owned()));
        let raw = "---\nrole: research-assistant\nstatus: active\n---\n";
        let doc = Doc::new(
            "docs/plans/test/prompts/agent.md",
            format!("{raw}body"),
            "body",
            Frontmatter::from_parsed(raw, fields),
        );
        let bundle = Bundle::from_documents([doc]).unwrap();
        let report = PacketValidator::validate(&bundle);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn packet_type_contains_librarian_detected_as_helper() {
        let mut fields = BTreeMap::new();
        fields.insert(
            "type".to_owned(),
            YamlValue::String("custom-librarian".to_owned()),
        );
        fields.insert("status".to_owned(), YamlValue::String("draft".to_owned()));
        let raw = "---\ntype: custom-librarian\nstatus: draft\n---\n";
        let doc = Doc::new(
            "docs/plans/test/prompts/custom.md",
            format!("{raw}body"),
            "body",
            Frontmatter::from_parsed(raw, fields),
        );
        let bundle = Bundle::from_documents([doc]).unwrap();
        let report = PacketValidator::validate(&bundle);
        // Contains "librarian" → is helper candidate, but status is draft → error.
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "packet_missing_authority_field");
    }

    #[test]
    fn packet_dedup_identical_findings() {
        let mut active_fields = BTreeMap::new();
        active_fields.insert(
            "type".to_owned(),
            YamlValue::String("research-assistant".to_owned()),
        );
        active_fields.insert("status".to_owned(), YamlValue::String("active".to_owned()));
        // Same superseded path referenced twice in different frontmatter keys.
        active_fields.insert(
            "source".to_owned(),
            YamlValue::String("docs/old/prompts/old.md".to_owned()),
        );
        active_fields.insert(
            "reference".to_owned(),
            YamlValue::String("docs/old/prompts/old.md".to_owned()),
        );
        let active_raw = "---\ntype: research-assistant\nstatus: active\nsource: docs/old/prompts/old.md\nreference: docs/old/prompts/old.md\n---\n";
        let active = Doc::new(
            "docs/current/prompts/agent.md",
            format!("{active_raw}body"),
            "body",
            Frontmatter::from_parsed(active_raw, active_fields),
        );
        let superseded = packet_doc("docs/old/prompts/old.md", "superseded", "Concept", "");
        let bundle = Bundle::from_documents([active, superseded]).unwrap();
        let report = PacketValidator::validate(&bundle);
        // Same path+code+message → deduplicated to one finding.
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "superseded_prompt_reference");
    }

    // ── sanitize_path component-aware tests (Slice 1.2-C) ──────────

    #[test]
    fn sanitize_path_preserves_safe_dotted_filenames() {
        // AC-2: Safe dotted names must not be redacted.
        assert_eq!(
            sanitize_path("docs/prompts/v1..draft.md"),
            "docs/prompts/v1..draft.md"
        );
        assert_eq!(sanitize_path("v2.0..1.md"), "v2.0..1.md");
        assert_eq!(sanitize_path("...leading.md"), "...leading.md");
        assert_eq!(sanitize_path("trailing..."), "trailing...");
        assert_eq!(
            sanitize_path("..dots..in..middle.md"),
            "..dots..in..middle.md"
        );
    }

    #[test]
    fn sanitize_path_redacts_actual_traversal_components() {
        // AC-2: Actual parent-dir traversal components remain redacted.
        assert_eq!(sanitize_path("../escape.md"), "[redacted]");
        assert_eq!(sanitize_path("a/b/../../etc/shadow"), "[redacted]");
        assert_eq!(sanitize_path("docs/.."), "[redacted]");
        assert_eq!(sanitize_path(".."), "[redacted]");
        // Still redacts existing unsafe cases (absolute, drive, NUL).
        assert_eq!(sanitize_path("/etc/passwd"), "[redacted]");
        assert_eq!(sanitize_path("C:\\Windows"), "[redacted]");
        assert_eq!(sanitize_path("bad\0name.md"), "[redacted]");
    }
}
