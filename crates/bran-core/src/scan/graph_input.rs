use super::{ContentIdentity, ScanChange, ScanSnapshot};
use crate::graph::{
    Confidence, EdgeCertainty, EdgeId, EdgeInput, EdgeRelationship, GraphError, GraphInput,
    KnowledgeGraph, NodeFacts, NodeId, NodeInput, NodeRole, Provenance,
};
use crate::metadata::{FactConfidence, FactProvenance, FactState, MetadataFact};
use std::collections::{BTreeMap, BTreeSet};

const MAX_MECHANICAL_ITEMS_PER_FILE: usize = 32;
const MAX_SEMANTIC_FACTS_PER_FILE: usize = 96;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScanGraphError {
    Graph(GraphError),
    IdentityCollision {
        identity: String,
        first: String,
        second: String,
    },
}

impl From<GraphError> for ScanGraphError {
    fn from(value: GraphError) -> Self {
        Self::Graph(value)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AffectedGraphNodes(pub Vec<NodeId>);

impl ScanSnapshot {
    pub fn graph_input(&self) -> Result<GraphInput, ScanGraphError> {
        let known: BTreeSet<_> = self.entries.keys().cloned().collect();
        let mut identities = BTreeMap::new();
        let mut edge_identities = BTreeMap::new();
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for (path, entry) in &self.entries {
            let file_id = checked_node_id(path, &mut identities)?;
            let mut facts = NodeFacts::default();
            let mut confidence = 50;
            let mut omitted = 0usize;
            let mut semantic_seen = 0usize;
            let mut semantic_keys = BTreeSet::new();
            let mut mechanical_seen = 0usize;
            for fact in &entry.metadata.facts {
                if is_relationship(&fact.key) || matches!(fact.key.as_str(), "symbol" | "test") {
                    if mechanical_seen == MAX_MECHANICAL_ITEMS_PER_FILE {
                        omitted += 1;
                        continue;
                    }
                    mechanical_seen += 1;
                    if matches!(fact.key.as_str(), "symbol" | "test") {
                        let evidence =
                            format!("{path}:{}:{}:{mechanical_seen}", fact.key, fact.value);
                        let mechanical_id = checked_node_id(&evidence, &mut identities)?;
                        let role = if fact.key == "test" {
                            NodeRole::Test
                        } else {
                            NodeRole::Symbol
                        };
                        nodes.push(NodeInput::new(
                            mechanical_id.clone(),
                            role,
                            Provenance::new(
                                provenance_name(&fact.provenance),
                                mechanical_locator(&fact.key, mechanical_seen, &evidence),
                            )?,
                            Confidence::new(confidence_value(fact))?,
                        ));
                        edges.push(edge(
                            &evidence,
                            file_id.clone(),
                            mechanical_id,
                            fact,
                            EdgeCertainty::Known,
                            if fact.key == "test" {
                                EdgeRelationship::Validation
                            } else {
                                EdgeRelationship::Implementation
                            },
                            &mut edge_identities,
                        )?);
                    } else {
                        let target = checked_node_id(&fact.value, &mut identities)?;
                        let certainty = if known.contains(&fact.value) {
                            EdgeCertainty::Known
                        } else {
                            EdgeCertainty::MissingTarget
                        };
                        edges.push(edge(
                            &format!("{path}:{}:{}:{mechanical_seen}", fact.key, fact.value),
                            file_id.clone(),
                            target,
                            fact,
                            certainty,
                            relationship(&fact.key).expect("relationship key checked"),
                            &mut edge_identities,
                        )?);
                    }
                } else if semantic_seen == MAX_SEMANTIC_FACTS_PER_FILE {
                    omitted += 1;
                } else {
                    semantic_seen += 1;
                    if !semantic_keys.contains(&fact.key)
                        && semantic_keys.len() == NodeFacts::MAX_FIELDS - 2
                    {
                        omitted += 1;
                        continue;
                    }
                    match facts
                        .clone()
                        .with_field_value(fact.key.clone(), fact.value.clone())
                    {
                        Ok(updated) => {
                            semantic_keys.insert(fact.key.clone());
                            facts = updated;
                            confidence = confidence.min(confidence_value(fact));
                        }
                        Err(_) => omitted += 1,
                    }
                }
            }
            if omitted > 0 {
                facts = facts
                    .with_field_value("bran.omitted_facts", omitted.to_string())?
                    .with_field_value("coverage", "partial")?;
            }
            nodes.push(
                NodeInput::new(
                    file_id,
                    file_role(&facts),
                    Provenance::new("repository-scanner", file_locator(path))?,
                    Confidence::new(confidence)?,
                )
                .with_facts(facts),
            );
        }
        Ok(GraphInput::new(nodes, edges))
    }
}

impl ScanChange {
    pub fn affected_graph_nodes(
        &self,
        previous: &KnowledgeGraph,
        current: &KnowledgeGraph,
    ) -> Result<AffectedGraphNodes, ScanGraphError> {
        let mut ids = BTreeSet::new();
        for affected in &self.affected {
            let seed = stable_node_id(&affected.path)?;
            ids.insert(seed.clone());
            if affected.previous.is_some() {
                add_neighbors(previous, &seed, &mut ids);
            }
            if affected.current.is_some() {
                add_neighbors(current, &seed, &mut ids);
            }
        }
        Ok(AffectedGraphNodes(ids.into_iter().collect()))
    }
}

fn add_neighbors(graph: &KnowledgeGraph, seed: &NodeId, ids: &mut BTreeSet<NodeId>) {
    for edge_id in graph
        .forward_edges(seed)
        .iter()
        .chain(graph.reverse_edges(seed))
    {
        if let Some(edge) = graph.edge(edge_id) {
            ids.insert(edge.source().clone());
            ids.insert(edge.target().clone());
        }
    }
}

fn stable_node_id(evidence: &str) -> Result<NodeId, GraphError> {
    let digest = ContentIdentity::from_bytes(evidence.as_bytes());
    NodeId::parse(format!(
        "n:{}:{:016x}{:016x}{:016x}",
        digest.byte_len, digest.lanes[0], digest.lanes[1], digest.lanes[2]
    ))
}

fn checked_node_id(
    evidence: &str,
    seen: &mut BTreeMap<NodeId, String>,
) -> Result<NodeId, ScanGraphError> {
    let id = stable_node_id(evidence)?;
    if let Some(first) = seen.insert(id.clone(), evidence.to_owned()) {
        if first != evidence {
            return Err(ScanGraphError::IdentityCollision {
                identity: id.as_str().to_owned(),
                first,
                second: evidence.to_owned(),
            });
        }
    }
    Ok(id)
}

fn edge(
    evidence: &str,
    source: NodeId,
    target: NodeId,
    fact: &MetadataFact,
    certainty: EdgeCertainty,
    relationship: EdgeRelationship,
    seen: &mut BTreeMap<EdgeId, String>,
) -> Result<EdgeInput, ScanGraphError> {
    let digest = ContentIdentity::from_bytes(evidence.as_bytes());
    let id = EdgeId::parse(format!(
        "e:{}:{:016x}{:016x}{:016x}",
        digest.byte_len, digest.lanes[0], digest.lanes[1], digest.lanes[2]
    ))?;
    if let Some(first) = seen.insert(id.clone(), evidence.to_owned()) {
        if first != evidence {
            return Err(ScanGraphError::IdentityCollision {
                identity: id.as_str().to_owned(),
                first,
                second: evidence.to_owned(),
            });
        }
    }
    Ok(EdgeInput::new(
        id,
        source,
        target,
        Provenance::new(
            provenance_name(&fact.provenance),
            mechanical_locator(&fact.key, 0, evidence),
        )?,
        Confidence::new(confidence_value(fact))?,
        certainty,
    )
    .with_relationship(relationship))
}

fn file_locator(path: &str) -> String {
    if path.len() <= Provenance::MAX_LOCATOR_BYTES {
        path.to_owned()
    } else {
        digest_locator("file", path)
    }
}

fn mechanical_locator(key: &str, ordinal: usize, evidence: &str) -> String {
    digest_locator(&format!("{key}:{ordinal}"), evidence)
}

fn digest_locator(prefix: &str, evidence: &str) -> String {
    let digest = ContentIdentity::from_bytes(evidence.as_bytes());
    format!(
        "{prefix}:{:016x}{:016x}{:016x}",
        digest.lanes[0], digest.lanes[1], digest.lanes[2]
    )
}

fn confidence_value(fact: &MetadataFact) -> u8 {
    if fact.state == FactState::Ambiguous {
        return 10;
    }
    match fact.confidence {
        FactConfidence::Low => 25,
        FactConfidence::Medium => 40,
        FactConfidence::High => 50,
    }
}

fn file_role(facts: &NodeFacts) -> NodeRole {
    if facts.contains_value("status", "archived") {
        NodeRole::Archived
    } else if facts.contains_value("type", "generated") {
        NodeRole::Generated
    } else {
        NodeRole::Document
    }
}

fn is_relationship(key: &str) -> bool {
    relationship(key).is_some()
}
fn relationship(key: &str) -> Option<EdgeRelationship> {
    match key {
        "dependency" | "import" => Some(EdgeRelationship::Dependency),
        "implementation" => Some(EdgeRelationship::Implementation),
        "replacement" => Some(EdgeRelationship::Replacement),
        "supersedes" => Some(EdgeRelationship::Supersedes),
        "validation" => Some(EdgeRelationship::Validation),
        "reachability" => Some(EdgeRelationship::Reachability),
        "contradiction" => Some(EdgeRelationship::Contradiction),
        "conflict" => Some(EdgeRelationship::Conflict),
        _ => None,
    }
}
fn provenance_name(value: &FactProvenance) -> &'static str {
    match value {
        FactProvenance::MarkdownFrontmatter => "markdown-frontmatter",
        FactProvenance::CommentedYaml => "commented-yaml",
        FactProvenance::Language => "language",
        FactProvenance::GitState => "git-state",
        FactProvenance::PackageDefault => "package-default",
        FactProvenance::Filename => "filename",
        FactProvenance::Symbol => "symbol",
        FactProvenance::Import => "import",
        FactProvenance::Test => "test",
        FactProvenance::Dependency => "dependency",
        FactProvenance::BoundaryClassification => "boundary-classification",
    }
}
