//! Deterministic immutable graph topology. Query algorithms live in Slice 2.2 A2.

pub mod model;
pub mod query;

pub use model::{
    Confidence, EdgeCertainty, EdgeId, EdgeInput, EdgeRelationship, GraphError, GraphInput,
    GraphLimits, NodeFacts, NodeId, NodeInput, NodeRole, Provenance, StructuralAuthority,
    StructuralEdge, StructuralEdgeCertainty, StructuralEdgeKind, StructuralGeneratedStatus,
    StructuralGraphSnapshot, StructuralLifecycle, StructuralNode, StructuralPathCasePolicy,
    StructuralSnapshotReceipt, StructuralSourceType, STRUCTURAL_GRAPH_ORDERING_VERSION,
    STRUCTURAL_GRAPH_SCHEMA_VERSION,
};

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Complete data-only input for one immutable structural graph candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralGraphCandidate {
    scan_input_digest: String,
    nodes: Vec<StructuralNode>,
    edges: Vec<StructuralEdge>,
}

impl StructuralGraphCandidate {
    pub fn new(
        scan_input_digest: impl Into<String>,
        nodes: Vec<StructuralNode>,
        edges: Vec<StructuralEdge>,
    ) -> Self {
        Self {
            scan_input_digest: scan_input_digest.into(),
            nodes,
            edges,
        }
    }
}

/// Stage at which a complete structural candidate was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphBuildFailureKind {
    Limit,
    Receipt,
    Validation,
}

/// Deterministic graph-build rejection with stable candidate-content identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphBuildFailure {
    kind: GraphBuildFailureKind,
    candidate_digest: String,
    diagnostic: String,
}

impl GraphBuildFailure {
    pub const fn kind(&self) -> GraphBuildFailureKind {
        self.kind
    }
    pub fn candidate_digest(&self) -> &str {
        &self.candidate_digest
    }
    pub fn diagnostic(&self) -> &str {
        &self.diagnostic
    }
}

/// Stateless complete-candidate builder and validator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphBuilder {
    max_nodes: usize,
    max_edges: usize,
    frozen_frequency_digest: String,
}

impl GraphBuilder {
    pub fn new(
        max_nodes: usize,
        max_edges: usize,
        frozen_frequency_digest: impl Into<String>,
    ) -> Self {
        Self {
            max_nodes,
            max_edges,
            frozen_frequency_digest: frozen_frequency_digest.into(),
        }
    }

    /// Builds and validates one complete immutable snapshot.
    pub fn build(
        &self,
        candidate: StructuralGraphCandidate,
        parent_digest: Option<String>,
        operation_id: impl Into<String>,
    ) -> Result<StructuralGraphSnapshot, GraphBuildFailure> {
        let candidate_digest = model::structural_content_digest(&candidate.nodes, &candidate.edges);
        if candidate.nodes.len() > self.max_nodes {
            return Err(build_failure(
                GraphBuildFailureKind::Limit,
                candidate_digest,
                format!(
                    "structural node limit exceeded: {} > {}",
                    candidate.nodes.len(),
                    self.max_nodes
                ),
            ));
        }
        if candidate.edges.len() > self.max_edges {
            return Err(build_failure(
                GraphBuildFailureKind::Limit,
                candidate_digest,
                format!(
                    "structural edge limit exceeded: {} > {}",
                    candidate.edges.len(),
                    self.max_edges
                ),
            ));
        }
        let receipt = StructuralSnapshotReceipt::new(
            operation_id,
            parent_digest.clone(),
            None,
            None,
            vec![
                "complete-candidate".to_owned(),
                "lineage-acyclic".to_owned(),
                "structural-graph-valid".to_owned(),
            ],
        )
        .map_err(|error| {
            build_failure(
                GraphBuildFailureKind::Receipt,
                candidate_digest.clone(),
                error.to_string(),
            )
        })?;
        let snapshot = StructuralGraphSnapshot::new(
            STRUCTURAL_GRAPH_SCHEMA_VERSION,
            parent_digest,
            candidate.scan_input_digest,
            candidate_digest.clone(),
            self.frozen_frequency_digest.clone(),
            candidate.nodes,
            candidate.edges,
            receipt,
        )
        .map_err(|error| {
            build_failure(
                GraphBuildFailureKind::Validation,
                candidate_digest.clone(),
                error.to_string(),
            )
        })?;
        validate_structural_candidate(snapshot.nodes(), snapshot.edges()).map_err(|error| {
            build_failure(
                GraphBuildFailureKind::Validation,
                candidate_digest,
                error.to_string(),
            )
        })?;
        Ok(snapshot)
    }
}

fn build_failure(
    kind: GraphBuildFailureKind,
    candidate_digest: String,
    diagnostic: String,
) -> GraphBuildFailure {
    GraphBuildFailure {
        kind,
        candidate_digest,
        diagnostic,
    }
}

fn validate_structural_candidate(
    nodes: &[StructuralNode],
    edges: &[StructuralEdge],
) -> Result<(), GraphError> {
    let nodes_by_id: BTreeMap<_, _> = nodes.iter().map(|node| (node.id(), node)).collect();
    let supersedes: BTreeSet<_> = edges
        .iter()
        .filter(|edge| edge.kind() == StructuralEdgeKind::Supersedes)
        .map(|edge| (edge.source(), edge.target()))
        .collect();
    let predecessors: BTreeSet<_> = edges
        .iter()
        .filter(|edge| edge.kind() == StructuralEdgeKind::Predecessor)
        .map(|edge| (edge.source(), edge.target()))
        .collect();
    for edge in edges {
        if edge.kind() == StructuralEdgeKind::GeneratedFrom {
            let valid = nodes_by_id
                .get(edge.target())
                .is_some_and(|node| node.generated_status() == StructuralGeneratedStatus::Source);
            if !valid {
                return Err(GraphError::InvalidGeneratedFromTarget {
                    edge: edge.id().to_owned(),
                    target: edge.target().to_owned(),
                });
            }
        }
        let tuple = (edge.source(), edge.target());
        let reciprocal = !nodes_by_id.contains_key(edge.target())
            || match edge.kind() {
                StructuralEdgeKind::Supersedes => predecessors.contains(&tuple),
                StructuralEdgeKind::Predecessor => supersedes.contains(&tuple),
                _ => true,
            };
        if !reciprocal {
            return Err(GraphError::MissingReciprocalLineage {
                edge: edge.id().to_owned(),
            });
        }
    }
    let mut lineage: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for edge in edges.iter().filter(|edge| {
        matches!(
            edge.kind(),
            StructuralEdgeKind::Lifecycle
                | StructuralEdgeKind::Supersedes
                | StructuralEdgeKind::Predecessor
        )
    }) {
        lineage
            .entry(edge.source().to_owned())
            .or_default()
            .insert(edge.target().to_owned());
    }
    if let Some(trail) = directed_cycle(&lineage) {
        return Err(GraphError::StructuralLineageCycle { trail });
    }
    Ok(())
}

fn directed_cycle(adjacency: &BTreeMap<String, BTreeSet<String>>) -> Option<Vec<String>> {
    let mut state = BTreeMap::new();
    let mut stack = Vec::new();
    for node in adjacency.keys() {
        if state.get(node).copied().unwrap_or(0) == 0 {
            if let Some(trail) = visit_lineage(node, adjacency, &mut state, &mut stack) {
                return Some(trail);
            }
        }
    }
    None
}

fn visit_lineage(
    node: &str,
    adjacency: &BTreeMap<String, BTreeSet<String>>,
    state: &mut BTreeMap<String, u8>,
    stack: &mut Vec<String>,
) -> Option<Vec<String>> {
    state.insert(node.to_owned(), 1);
    stack.push(node.to_owned());
    if let Some(targets) = adjacency.get(node) {
        for target in targets {
            match state.get(target).copied().unwrap_or(0) {
                0 => {
                    if let Some(trail) = visit_lineage(target, adjacency, state, stack) {
                        return Some(trail);
                    }
                }
                1 => {
                    let start = stack.iter().position(|item| item == target).unwrap_or(0);
                    let mut trail = stack[start..].to_vec();
                    trail.push(target.clone());
                    return Some(trail);
                }
                _ => {}
            }
        }
    }
    stack.pop();
    state.insert(node.to_owned(), 2);
    None
}

/// Immutable deterministic topology built solely from scanner-neutral input records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnowledgeGraph {
    nodes: Vec<NodeInput>,
    node_index: HashMap<NodeId, usize>,
    edges: Vec<EdgeInput>,
    edge_index: HashMap<EdgeId, usize>,
    forward: HashMap<NodeId, Vec<EdgeId>>,
    reverse: HashMap<NodeId, Vec<EdgeId>>,
}

impl KnowledgeGraph {
    pub fn build(input: GraphInput, limits: GraphLimits) -> Result<Self, GraphError> {
        let (mut nodes, mut edges) = input.into_parts();
        if nodes.len() > limits.max_nodes() {
            return Err(GraphError::NodeLimitExceeded {
                limit: limits.max_nodes(),
                actual: nodes.len(),
            });
        }
        if edges.len() > limits.max_edges() {
            return Err(GraphError::EdgeLimitExceeded {
                limit: limits.max_edges(),
                actual: edges.len(),
            });
        }

        nodes.sort_by(|left, right| left.id().cmp(right.id()));
        let mut node_index = HashMap::with_capacity(nodes.len());
        for (position, node) in nodes.iter().enumerate() {
            if node_index.insert(node.id().clone(), position).is_some() {
                return Err(GraphError::DuplicateNodeId(node.id().clone()));
            }
        }

        edges.sort_by(|left, right| left.id().cmp(right.id()));
        let mut edge_index = HashMap::with_capacity(edges.len());
        let mut forward = HashMap::new();
        let mut reverse = HashMap::new();
        for (position, edge) in edges.iter().enumerate() {
            if !node_index.contains_key(edge.source()) {
                return Err(GraphError::MissingSource(edge.source().clone()));
            }
            let target_present = node_index.contains_key(edge.target());
            if !target_present && edge.certainty() != EdgeCertainty::MissingTarget {
                return Err(GraphError::MissingTargetMustBeExplicit {
                    edge: edge.id().clone(),
                    target: edge.target().clone(),
                });
            }
            if target_present && edge.certainty() == EdgeCertainty::MissingTarget {
                return Err(GraphError::MissingTargetMustBeAbsent {
                    edge: edge.id().clone(),
                    target: edge.target().clone(),
                });
            }
            if edge_index.insert(edge.id().clone(), position).is_some() {
                return Err(GraphError::DuplicateEdgeId(edge.id().clone()));
            }
            forward
                .entry(edge.source().clone())
                .or_insert_with(Vec::new)
                .push(edge.id().clone());
            reverse
                .entry(edge.target().clone())
                .or_insert_with(Vec::new)
                .push(edge.id().clone());
        }
        Ok(Self {
            nodes,
            node_index,
            edges,
            edge_index,
            forward,
            reverse,
        })
    }

    pub fn node(&self, id: &NodeId) -> Option<&NodeInput> {
        self.node_index
            .get(id)
            .map(|position| &self.nodes[*position])
    }
    pub fn edge(&self, id: &EdgeId) -> Option<&EdgeInput> {
        self.edge_index
            .get(id)
            .map(|position| &self.edges[*position])
    }
    pub fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.iter().map(|node| node.id().clone()).collect()
    }
    pub fn edge_ids(&self) -> Vec<EdgeId> {
        self.edges.iter().map(|edge| edge.id().clone()).collect()
    }
    pub fn forward_edges(&self, id: &NodeId) -> &[EdgeId] {
        self.forward.get(id).map(Vec::as_slice).unwrap_or(&[])
    }
    pub fn reverse_edges(&self, id: &NodeId) -> &[EdgeId] {
        self.reverse.get(id).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::query::{NodeQuery, QueryBounds, Reachability, ReachabilityReason, TrailStatus};
    use super::*;
    use crate::metadata::{
        FactConfidence, FactProvenance, FactState, MetadataFact, MetadataReport,
    };
    use crate::scan::{AffectedNode, ContentIdentity, ScanChange, ScanEntry, ScanSnapshot};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn id(value: &str) -> NodeId {
        NodeId::parse(value).unwrap()
    }
    fn edge_id(value: &str) -> EdgeId {
        EdgeId::parse(value).unwrap()
    }
    fn source(value: &str) -> Provenance {
        Provenance::new("scanner", value).unwrap()
    }
    fn graph_input() -> GraphInput {
        let a = id("node.a");
        let b = id("node.b");
        let c = id("node.c");
        let missing = id("node.missing");
        GraphInput::new(
            vec![
                NodeInput::new(
                    c.clone(),
                    NodeRole::Symbol,
                    source("c"),
                    Confidence::new(70).unwrap(),
                ),
                NodeInput::new(
                    a.clone(),
                    NodeRole::Document,
                    source("a"),
                    Confidence::new(100).unwrap(),
                )
                .with_facts(
                    NodeFacts::default()
                        .with_status("active")
                        .unwrap()
                        .with_tag("reference")
                        .unwrap()
                        .with_tag("graph")
                        .unwrap()
                        .with_freshness("2026-07-18")
                        .unwrap()
                        .with_subsystem("graph")
                        .unwrap()
                        .with_purpose("topology")
                        .unwrap()
                        .with_task("analysis")
                        .unwrap()
                        .with_audience("engineers")
                        .unwrap(),
                ),
                NodeInput::new(
                    b.clone(),
                    NodeRole::Section,
                    source("b"),
                    Confidence::new(80).unwrap(),
                ),
            ],
            vec![
                EdgeInput::new(
                    edge_id("edge.unknown"),
                    c,
                    a.clone(),
                    source("unknown"),
                    Confidence::new(40).unwrap(),
                    EdgeCertainty::Unknown,
                )
                .with_relationship(EdgeRelationship::Conflict),
                EdgeInput::new(
                    edge_id("edge.missing"),
                    a.clone(),
                    missing,
                    source("candidate"),
                    Confidence::new(15).unwrap(),
                    EdgeCertainty::MissingTarget,
                )
                .with_relationship(EdgeRelationship::Replacement),
                EdgeInput::new(
                    edge_id("edge.known"),
                    a,
                    b.clone(),
                    source("known"),
                    Confidence::new(100).unwrap(),
                    EdgeCertainty::Known,
                )
                .with_relationship(EdgeRelationship::Dependency),
                EdgeInput::new(
                    edge_id("edge.dynamic"),
                    b,
                    id("node.c"),
                    source("dynamic"),
                    Confidence::new(55).unwrap(),
                    EdgeCertainty::Dynamic,
                )
                .with_relationship(EdgeRelationship::Reachability),
            ],
        )
    }

    fn q_node(name: &str, role: NodeRole) -> NodeInput {
        NodeInput::new(id(name), role, source(name), Confidence::new(90).unwrap())
    }

    fn ranked_node(
        name: &str,
        status: &str,
        canonical: bool,
        confidence: u8,
        freshness: &str,
    ) -> NodeInput {
        let mut facts = NodeFacts::default()
            .with_field_value("title", "Ranked Source")
            .unwrap()
            .with_status(status)
            .unwrap()
            .with_freshness(freshness)
            .unwrap();
        if canonical {
            facts = facts.with_field_value("canonical", "true").unwrap();
        }
        NodeInput::new(
            id(name),
            NodeRole::Document,
            source(name),
            Confidence::new(confidence).unwrap(),
        )
        .with_facts(facts)
    }

    fn q_edge(
        name: &str,
        source_id: &str,
        target_id: &str,
        certainty: EdgeCertainty,
        confidence: u8,
    ) -> EdgeInput {
        EdgeInput::new(
            edge_id(name),
            id(source_id),
            id(target_id),
            source(name),
            Confidence::new(confidence).unwrap(),
            certainty,
        )
    }

    fn query_graph() -> KnowledgeGraph {
        KnowledgeGraph::build(
            GraphInput::new(
                vec![
                    q_node("node.entry", NodeRole::Entrypoint).with_facts(
                        NodeFacts::default()
                            .with_subsystem("query")
                            .unwrap()
                            .with_field_value("title", "Retry Queue")
                            .unwrap()
                            .with_status("active")
                            .unwrap()
                            .with_field_value("resource", "docs/task.md")
                            .unwrap()
                            .with_freshness("2026-07-20")
                            .unwrap(),
                    ),
                    q_node("node.support", NodeRole::Symbol).with_facts(
                        NodeFacts::default()
                            .with_field_value("title", "Retry Queue Worker")
                            .unwrap()
                            .with_tag("queue")
                            .unwrap(),
                    ),
                    q_node("node.test", NodeRole::Test),
                    q_node("node.generated", NodeRole::Generated),
                    q_node("node.orphan", NodeRole::Archived),
                    q_node("node.partial", NodeRole::Symbol).with_facts(
                        NodeFacts::default()
                            .with_field_value("coverage", "partial")
                            .unwrap(),
                    ),
                    q_node("node.dynamic", NodeRole::Symbol),
                    q_node("node.unknown", NodeRole::Symbol),
                    q_node("node.cycle.a", NodeRole::Symbol),
                    q_node("node.cycle.b", NodeRole::Symbol),
                    q_node("node.cycle.c", NodeRole::Symbol),
                    q_node("node.left", NodeRole::Symbol),
                    q_node("node.right", NodeRole::Symbol),
                    q_node("node.target", NodeRole::Symbol),
                    q_node("node.dynamic.descendant", NodeRole::Symbol),
                    q_node("node.unknown.descendant", NodeRole::Symbol),
                    q_node("node.missing.source", NodeRole::Symbol),
                    q_node("node.missing.descendant", NodeRole::Symbol),
                ],
                vec![
                    q_edge(
                        "edge.entry-support",
                        "node.entry",
                        "node.support",
                        EdgeCertainty::Known,
                        100,
                    ),
                    q_edge(
                        "edge.entry-cycle",
                        "node.entry",
                        "node.cycle.a",
                        EdgeCertainty::Known,
                        100,
                    ),
                    q_edge(
                        "edge.z-left",
                        "node.entry",
                        "node.left",
                        EdgeCertainty::Known,
                        100,
                    ),
                    q_edge(
                        "edge.a-right",
                        "node.entry",
                        "node.right",
                        EdgeCertainty::Known,
                        100,
                    ),
                    q_edge(
                        "edge.left-target",
                        "node.left",
                        "node.target",
                        EdgeCertainty::Known,
                        100,
                    ),
                    q_edge(
                        "edge.right-target",
                        "node.right",
                        "node.target",
                        EdgeCertainty::Known,
                        100,
                    ),
                    q_edge(
                        "edge.cycle-a-b",
                        "node.cycle.a",
                        "node.cycle.b",
                        EdgeCertainty::Known,
                        100,
                    ),
                    q_edge(
                        "edge.cycle-b-c",
                        "node.cycle.b",
                        "node.cycle.c",
                        EdgeCertainty::Known,
                        100,
                    ),
                    q_edge(
                        "edge.cycle-c-a",
                        "node.cycle.c",
                        "node.cycle.a",
                        EdgeCertainty::Known,
                        100,
                    ),
                    q_edge(
                        "edge.dynamic",
                        "node.dynamic",
                        "node.support",
                        EdgeCertainty::Dynamic,
                        11,
                    )
                    .with_relationship(EdgeRelationship::Reachability),
                    q_edge(
                        "edge.unknown",
                        "node.unknown",
                        "node.support",
                        EdgeCertainty::Unknown,
                        12,
                    ),
                    q_edge(
                        "edge.dynamic-descendant",
                        "node.dynamic",
                        "node.dynamic.descendant",
                        EdgeCertainty::Known,
                        100,
                    ),
                    q_edge(
                        "edge.unknown-descendant",
                        "node.unknown",
                        "node.unknown.descendant",
                        EdgeCertainty::Known,
                        100,
                    ),
                    q_edge(
                        "edge.missing-target",
                        "node.missing.source",
                        "node.missing.target",
                        EdgeCertainty::MissingTarget,
                        10,
                    ),
                    q_edge(
                        "edge.missing-descendant",
                        "node.missing.source",
                        "node.missing.descendant",
                        EdgeCertainty::Known,
                        100,
                    ),
                ],
            ),
            GraphLimits::new(18, 15).unwrap(),
        )
        .unwrap()
    }

    fn classification(graph: &KnowledgeGraph, node: &str) -> Reachability {
        let NodeQuery::Found(result) = graph.reachability(&id(node), 8) else {
            panic!("known query node missing");
        };
        result.items[0].outcome.clone()
    }

    #[test]
    fn p2_graph() {
        let public_fixture = include_str!("../../../../fixtures/graph/knowledge-graph-v1.json");
        assert!(public_fixture.contains("\"schema_version\": \"1.0.0\""));
        assert!(public_fixture.contains("\"id\": \"file:src/lib.rs\""));
        assert!(public_fixture.contains("\"locator\": \"src/support.rs\""));
        assert!(public_fixture.contains("\"certainty\": \"unknown\""));
        let limits = GraphLimits::new(3, 4).unwrap();
        let graph = KnowledgeGraph::build(graph_input(), limits).unwrap();
        assert_eq!(
            graph.node_ids(),
            vec![id("node.a"), id("node.b"), id("node.c")]
        );
        assert_eq!(
            graph.edge_ids(),
            vec![
                edge_id("edge.dynamic"),
                edge_id("edge.known"),
                edge_id("edge.missing"),
                edge_id("edge.unknown")
            ]
        );
        assert_eq!(
            graph.forward_edges(&id("node.a")),
            &[edge_id("edge.known"), edge_id("edge.missing")]
        );
        assert_eq!(
            graph.reverse_edges(&id("node.a")),
            &[edge_id("edge.unknown")]
        );
        assert_eq!(
            graph.forward_edges(&id("node.c")),
            &[edge_id("edge.unknown")]
        );
        assert_eq!(
            graph.reverse_edges(&id("node.c")),
            &[edge_id("edge.dynamic")]
        );
        assert_eq!(KnowledgeGraph::build(graph_input(), limits).unwrap(), graph);
        let node = graph.node(&id("node.a")).unwrap();
        assert_eq!(node.role(), NodeRole::Document);
        assert_eq!(node.provenance().locator(), "a");
        assert_eq!(node.confidence().value(), 100);
        assert_eq!(node.facts().status(), Some("active"));
        assert_eq!(
            node.facts().tags(),
            &["graph".to_owned(), "reference".to_owned()]
        );
        assert_eq!(node.facts().freshness(), Some("2026-07-18"));
        assert_eq!(node.facts().subsystem(), Some("graph"));
        assert_eq!(node.facts().purpose(), Some("topology"));
        assert_eq!(node.facts().task(), Some("analysis"));
        assert_eq!(node.facts().audience(), Some("engineers"));
        let missing = graph.edge(&edge_id("edge.missing")).unwrap();
        assert_eq!(missing.certainty(), EdgeCertainty::MissingTarget);
        assert_eq!(missing.provenance().locator(), "candidate");
        assert_eq!(missing.confidence().value(), 15);
        assert_eq!(missing.relationship(), EdgeRelationship::Replacement);
        assert_eq!(
            graph.edge(&edge_id("edge.dynamic")).unwrap().certainty(),
            EdgeCertainty::Dynamic
        );
        assert_eq!(
            graph.edge(&edge_id("edge.dynamic")).unwrap().relationship(),
            EdgeRelationship::Reachability
        );
        assert_eq!(
            graph.edge(&edge_id("edge.unknown")).unwrap().certainty(),
            EdgeCertainty::Unknown
        );
        assert_eq!(
            graph.edge(&edge_id("edge.unknown")).unwrap().relationship(),
            EdgeRelationship::Conflict
        );
        let candidate = MetadataFact {
            key: "dependency".to_owned(),
            value: "src/b.rs".to_owned(),
            provenance: FactProvenance::CommentedYaml,
            confidence: FactConfidence::Medium,
            state: FactState::Candidate,
        };
        let entry = |facts| {
            Arc::new(ScanEntry {
                identity: ContentIdentity::from_bytes(b"source"),
                source: Arc::from(b"source".as_slice()),
                metadata: MetadataReport {
                    facts,
                    warnings: Vec::new(),
                    proposals: Vec::new(),
                },
            })
        };
        let snapshot = ScanSnapshot {
            entries: BTreeMap::from([
                ("src/a.rs".to_owned(), entry(vec![candidate])),
                ("src/b.rs".to_owned(), entry(Vec::new())),
            ]),
            ..ScanSnapshot::default()
        };
        let scanned = KnowledgeGraph::build(
            snapshot.graph_input().unwrap(),
            GraphLimits::new(2, 1).unwrap(),
        )
        .unwrap();
        let scanned_edge = scanned.edge(&scanned.edge_ids()[0]).unwrap();
        let scanned_a = scanned.node(scanned_edge.source()).unwrap();
        assert_eq!(scanned_a.provenance().source(), "repository-scanner");
        assert_eq!(scanned_a.confidence().value(), 50);
        assert!(!scanned_a.facts().contains_value("dependency", "src/b.rs"));
        assert_eq!(scanned_edge.relationship(), EdgeRelationship::Dependency);
        assert_eq!(scanned_edge.certainty(), EdgeCertainty::Known);
        assert_eq!(scanned_edge.provenance().source(), "commented-yaml");
        let change = ScanChange {
            affected: vec![AffectedNode {
                path: "src/a.rs".to_owned(),
                previous: Some(ContentIdentity::from_bytes(b"old")),
                current: Some(ContentIdentity::from_bytes(b"new")),
            }],
            ..ScanChange::default()
        };
        assert_eq!(
            change
                .affected_graph_nodes(&scanned, &scanned)
                .unwrap()
                .0
                .len(),
            2
        );
        assert_eq!(
            graph.edge(&edge_id("edge.known")).unwrap().relationship(),
            EdgeRelationship::Dependency
        );
        assert_eq!(
            KnowledgeGraph::build(
                GraphInput::new(
                    vec![NodeInput::new(
                        id("node.a"),
                        NodeRole::Document,
                        source("a"),
                        Confidence::new(1).unwrap()
                    )],
                    vec![EdgeInput::new(
                        edge_id("edge.absent"),
                        id("node.a"),
                        id("node.missing"),
                        source("absent"),
                        Confidence::new(1).unwrap(),
                        EdgeCertainty::Known
                    )]
                ),
                GraphLimits::new(1, 1).unwrap()
            ),
            Err(GraphError::MissingTargetMustBeExplicit {
                edge: edge_id("edge.absent"),
                target: id("node.missing")
            })
        );
        assert_eq!(
            KnowledgeGraph::build(
                GraphInput::new(
                    vec![
                        NodeInput::new(
                            id("node.a"),
                            NodeRole::Document,
                            source("one"),
                            Confidence::new(1).unwrap()
                        ),
                        NodeInput::new(
                            id("node.a"),
                            NodeRole::Document,
                            source("two"),
                            Confidence::new(1).unwrap()
                        )
                    ],
                    vec![]
                ),
                GraphLimits::new(3, 1).unwrap()
            ),
            Err(GraphError::DuplicateNodeId(id("node.a")))
        );
        assert_eq!(
            KnowledgeGraph::build(
                GraphInput::new(
                    vec![NodeInput::new(
                        id("node.a"),
                        NodeRole::Document,
                        source("a"),
                        Confidence::new(1).unwrap()
                    )],
                    vec![
                        EdgeInput::new(
                            edge_id("edge.a"),
                            id("node.a"),
                            id("node.a"),
                            source("a"),
                            Confidence::new(1).unwrap(),
                            EdgeCertainty::Known
                        ),
                        EdgeInput::new(
                            edge_id("edge.a"),
                            id("node.a"),
                            id("node.a"),
                            source("b"),
                            Confidence::new(1).unwrap(),
                            EdgeCertainty::Known
                        )
                    ]
                ),
                GraphLimits::new(1, 2).unwrap()
            ),
            Err(GraphError::DuplicateEdgeId(edge_id("edge.a")))
        );
        assert_eq!(
            KnowledgeGraph::build(graph_input(), GraphLimits::new(2, 4).unwrap()),
            Err(GraphError::NodeLimitExceeded {
                limit: 2,
                actual: 3
            })
        );
        assert_eq!(
            KnowledgeGraph::build(graph_input(), GraphLimits::new(3, 3).unwrap()),
            Err(GraphError::EdgeLimitExceeded {
                limit: 3,
                actual: 4
            })
        );
        assert!(matches!(
            Provenance::new(
                "s".repeat(Provenance::MAX_SOURCE_BYTES + 1),
                "bounded-locator"
            ),
            Err(GraphError::InvalidProvenance)
        ));
        assert!(matches!(
            Provenance::new(
                "bounded-source",
                "l".repeat(Provenance::MAX_LOCATOR_BYTES + 1)
            ),
            Err(GraphError::InvalidProvenance)
        ));
    }

    #[test]
    fn p2_queries() {
        use super::query::{QueryIndex, SelectionReason};

        let public_fixture = include_str!("../../../../fixtures/queries/query-result-v1.json");
        assert!(public_fixture.contains("\"schema_version\": \"1.0.0\""));
        assert!(public_fixture.contains("\"id\": \"file:src/lib.rs\""));
        assert!(public_fixture.contains("\"source\": \"rust-import\""));
        assert!(public_fixture.contains("\"status\": \"partial\""));
        assert!(public_fixture.contains("\"reason\": \"partial-coverage\""));
        assert!(public_fixture.contains("\"truncated\": true"));
        let graph = query_graph();
        assert_eq!(graph.find("node.", 2), graph.find("node.", 2));
        assert_eq!(graph.find("node.", 2).items.len(), 2);
        assert!(graph.find("node.", 0).truncated);
        let entry = graph.find("node.entry", 1);
        assert_eq!(entry.items[0].facts.subsystem(), Some("query"));
        assert_eq!(graph.find("Retry Queue", 2).items[0].id, id("node.entry"));
        let title = graph.lookup(QueryIndex::Title, "Retry Queue", 8);
        assert_eq!(title.items[0].node.id, id("node.entry"));
        assert_eq!(
            title.items[0].why,
            SelectionReason::Exact {
                index: QueryIndex::Title,
                value: "Retry Queue".to_owned(),
            }
        );
        assert_eq!(title.items[1].node.id, id("node.support"));
        let ranked = KnowledgeGraph::build(
            GraphInput::new(
                vec![
                    ranked_node("rank.active", "active", false, 100, "2026-07-20"),
                    ranked_node("rank.canonical", "active", true, 10, "2026-07-19"),
                    ranked_node("rank.confidence", "active", true, 9, "2026-07-21"),
                    ranked_node("rank.freshness", "active", true, 10, "2026-07-20"),
                    ranked_node("rank.inactive", "draft", true, 100, "2026-07-21"),
                ],
                vec![],
            ),
            GraphLimits::new(5, 1).unwrap(),
        )
        .unwrap()
        .lookup(QueryIndex::Title, "Ranked Source", 8);
        assert_eq!(ranked.items.len(), 5);
        assert_eq!(ranked.items[0].node.id.as_str(), "rank.freshness");
        assert_eq!(ranked.items[1].node.id.as_str(), "rank.canonical");
        assert_eq!(ranked.items[2].node.id.as_str(), "rank.confidence");
        assert_eq!(ranked.items[3].node.id.as_str(), "rank.active");
        assert_eq!(ranked.items[4].node.id.as_str(), "rank.inactive");
        assert_eq!(
            graph.lookup(QueryIndex::Tag, "queue", 8).items[0].node.id,
            id("node.support")
        );
        assert_eq!(
            graph.lookup(QueryIndex::Canonical, "docs/task.md", 8).items[0]
                .node
                .id,
            id("node.entry")
        );
        let NodeQuery::Found(related) = graph.related(&id("node.support"), QueryBounds::new(8, 1))
        else {
            panic!("known related node missing");
        };
        assert_eq!(related.items.len(), 1);
        assert_eq!(related.items[0].node.id, id("node.entry"));
        assert!(matches!(
            related.items[0].why,
            SelectionReason::Related(ref edge) if edge.certainty == EdgeCertainty::Known
        ));
        let NodeQuery::Found(backlinks) =
            graph.backlinks(&id("node.support"), QueryBounds::new(8, 1))
        else {
            panic!("known backlink target missing");
        };
        assert_eq!(backlinks.items.len(), 3);
        assert!(matches!(
            backlinks.items[0].why,
            SelectionReason::Backlink(ref edge)
                if edge.provenance.locator() == "edge.dynamic"
                    && edge.certainty == EdgeCertainty::Dynamic
        ));
        let NodeQuery::Found(depth_bound) =
            graph.related(&id("node.support"), QueryBounds::new(8, 0))
        else {
            panic!("known related node missing");
        };
        assert!(depth_bound.items.is_empty());
        assert!(depth_bound.truncated);
        let NodeQuery::Found(consumers) = graph.consumers(&id("node.support"), 8) else {
            panic!("known consumer target missing");
        };
        assert_eq!(consumers.items.len(), 3);
        assert_eq!(consumers.items[0].node.id, id("node.dynamic"));
        assert_eq!(consumers.items[0].edge.certainty, EdgeCertainty::Dynamic);
        assert_eq!(
            consumers.items[0].edge.relationship,
            EdgeRelationship::Reachability
        );
        assert_eq!(consumers.items[0].edge.provenance.locator(), "edge.dynamic");
        assert_eq!(consumers.items[0].edge.confidence.value(), 11);
        let NodeQuery::Found(impact) = graph.impact(&id("node.cycle.a"), QueryBounds::new(8, 8))
        else {
            panic!("known impact start missing");
        };
        assert_eq!(impact.items.len(), 3);
        assert_eq!(impact.items[0].node.id, id("node.cycle.c"));
        assert_eq!(impact.items[1].node.id, id("node.entry"));
        assert_eq!(impact.items[2].node.id, id("node.cycle.b"));
        assert!(matches!(
            classification(&graph, "node.entry"),
            Reachability::Active(_)
        ));
        assert!(matches!(
            classification(&graph, "node.support"),
            Reachability::Supporting(_)
        ));
        assert!(matches!(
            classification(&graph, "node.test"),
            Reachability::TestOnly(_)
        ));
        assert!(matches!(
            classification(&graph, "node.generated"),
            Reachability::Generated(_)
        ));
        assert!(matches!(
            classification(&graph, "node.orphan"),
            Reachability::Unreachable(_)
        ));
        assert_eq!(
            classification(&graph, "node.partial"),
            Reachability::Unknown(ReachabilityReason::PartialCoverage)
        );
        assert!(matches!(
            classification(&graph, "node.dynamic"),
            Reachability::Unknown(_)
        ));
        assert!(matches!(
            classification(&graph, "node.unknown"),
            Reachability::Unknown(_)
        ));
        assert!(matches!(
            classification(&graph, "node.dynamic.descendant"),
            Reachability::Unknown(_)
        ));
        assert!(matches!(
            classification(&graph, "node.unknown.descendant"),
            Reachability::Unknown(_)
        ));
        assert!(matches!(
            classification(&graph, "node.missing.descendant"),
            Reachability::Unknown(_)
        ));
        let zombies = graph.zombies(QueryBounds::new(8, 8));
        assert_eq!(zombies.items.len(), 1);
        assert_eq!(zombies.items[0].node.id, id("node.orphan"));
        let trail = graph.trail(
            &id("node.entry"),
            &id("node.target"),
            QueryBounds::new(8, 8),
        );
        assert_eq!(
            trail,
            graph.trail(
                &id("node.entry"),
                &id("node.target"),
                QueryBounds::new(8, 8)
            )
        );
        let TrailStatus::Found(trail) = trail else {
            panic!("known trail absent");
        };
        assert_eq!(trail.nodes[1].id, id("node.left"));
        assert_eq!(trail.edges[0].id, edge_id("edge.z-left"));
        assert_eq!(
            graph.trail(&id("node.none"), &id("node.target"), QueryBounds::new(8, 8)),
            TrailStatus::MissingStart(id("node.none"))
        );
        assert_eq!(
            graph.trail(&id("node.entry"), &id("node.none"), QueryBounds::new(8, 8)),
            TrailStatus::MissingTarget(id("node.none"))
        );
        let NodeQuery::Found(depth_bound) =
            graph.impact(&id("node.cycle.a"), QueryBounds::new(8, 1))
        else {
            panic!("known impact start missing");
        };
        assert!(depth_bound.truncated);
        let NodeQuery::Found(result_bound) =
            graph.impact(&id("node.cycle.a"), QueryBounds::new(1, 8))
        else {
            panic!("known impact start missing");
        };
        assert_eq!(result_bound.items.len(), 1);
        assert!(result_bound.truncated);
    }
}
