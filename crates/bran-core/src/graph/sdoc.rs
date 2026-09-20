//! Validated SDoc receipt projection and SDoc-specific ranking.
//!
//! This adapter deliberately uses MID, rather than a path, for graph identity.
//! It accepts only bridge-validated receipts and never parses or rewrites SDoc.

use super::{
    Confidence, EdgeCertainty, EdgeId, EdgeInput, EdgeRelationship, GraphError, GraphInput,
    KnowledgeGraph, NodeFacts, NodeId, NodeInput, NodeRole, Provenance,
};
use crate::sdoc::{SdocDocument, SdocLineRange, SdocNode, SdocReceipt};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Failure while projecting already-validated StrictDoc data into graph facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SdocGraphError {
    Graph(GraphError),
    DuplicateMid {
        mid: String,
        first_locator: String,
        second_locator: String,
    },
    DuplicateUid {
        uid: String,
        first_mid: String,
        second_mid: String,
    },
    DuplicateRelationMid(String),
    UnresolvedRelation {
        relation_mid: String,
        target_mid: String,
    },
}

impl From<GraphError> for SdocGraphError {
    fn from(value: GraphError) -> Self {
        Self::Graph(value)
    }
}

impl fmt::Display for SdocGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Graph(error) => error.fmt(formatter),
            Self::DuplicateMid { mid, .. } => write!(formatter, "duplicate SDoc MID: {mid}"),
            Self::DuplicateUid { uid, .. } => write!(formatter, "duplicate SDoc UID: {uid}"),
            Self::DuplicateRelationMid(mid) => {
                write!(formatter, "duplicate SDoc relation MID: {mid}")
            }
            Self::UnresolvedRelation {
                relation_mid,
                target_mid,
            } => write!(
                formatter,
                "SDoc relation {relation_mid} has unresolved target MID: {target_mid}"
            ),
        }
    }
}

impl std::error::Error for SdocGraphError {}

/// Projects validated receipts to the scanner-neutral graph input contract.
///
/// Node identity is the native MID. Source paths are retained only as facts and
/// provenance, so a relocation does not create a new graph node.
pub fn graph_input(receipts: &[SdocReceipt]) -> Result<GraphInput, SdocGraphError> {
    let mut mids = BTreeMap::new();
    let mut uids = BTreeMap::new();
    let mut relation_mids = BTreeSet::new();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut relations = BTreeMap::new();
    let mut relation_facts = BTreeMap::<String, Vec<String>>::new();

    for receipt in receipts {
        for relation in &receipt.relations {
            if !relation_mids.insert(relation.mid.clone()) {
                return Err(SdocGraphError::DuplicateRelationMid(relation.mid.clone()));
            }
            relation_facts
                .entry(relation.source_mid.clone())
                .or_default()
                .push(format!(
                    "{}:{}:{}",
                    relation.mid, relation.relation_type, relation.target_mid
                ));
            relations.insert(relation.mid.clone(), (receipt, relation));
        }
    }
    for receipt in receipts {
        let inherited = inherited_facts(&receipt.document);
        insert_node(
            &mut nodes,
            &mut mids,
            &mut uids,
            &receipt.source_locator,
            &receipt.source_sha256,
            &receipt.document.mid,
            &receipt.document.uid,
            NodeRole::Document,
            "DOCUMENT",
            &receipt.document.title,
            &receipt.document.metadata,
            &receipt.document.line_range,
            &inherited,
            relation_facts
                .get(&receipt.document.mid)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )?;
        for node in &receipt.nodes {
            insert_sdoc_node(
                &mut nodes,
                &mut mids,
                &mut uids,
                receipt,
                node,
                &inherited,
                relation_facts
                    .get(&node.mid)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            )?;
        }
    }

    let known: BTreeSet<_> = mids.keys().cloned().collect();
    for (mid, (receipt, relation)) in relations {
        if !known.contains(&relation.source_mid) {
            return Err(SdocGraphError::UnresolvedRelation {
                relation_mid: mid,
                target_mid: relation.source_mid.clone(),
            });
        }
        if !known.contains(&relation.target_mid) {
            return Err(SdocGraphError::UnresolvedRelation {
                relation_mid: mid,
                target_mid: relation.target_mid.clone(),
            });
        }
        edges.push(
            EdgeInput::new(
                EdgeId::parse(mid.clone())?,
                NodeId::parse(relation.source_mid.clone())?,
                NodeId::parse(relation.target_mid.clone())?,
                Provenance::new(
                    "strictdoc-bridge",
                    line_locator(&receipt.source_locator, &relation.line_range, &mid),
                )?,
                Confidence::new(100)?,
                EdgeCertainty::Known,
            )
            .with_relationship(relationship(&relation.relation_type)),
        );
    }
    Ok(GraphInput::new(nodes, edges))
}

fn insert_sdoc_node(
    nodes: &mut Vec<NodeInput>,
    mids: &mut BTreeMap<String, String>,
    uids: &mut BTreeMap<String, String>,
    receipt: &SdocReceipt,
    node: &SdocNode,
    inherited: &BTreeMap<String, String>,
    relation_facts: &[String],
) -> Result<(), SdocGraphError> {
    // A [TEXT] node has no TITLE field; its heading is the first statement
    // line (the architecture-specification floor encoding), so that is its
    // title before the UID is.
    let heading = field_value(&node.fields, "statement")
        .and_then(|statement| statement.lines().next())
        .filter(|line| !line.trim().is_empty());
    let title = field_value(&node.fields, "title")
        .or(heading)
        .unwrap_or(&node.uid);
    insert_node(
        nodes,
        mids,
        uids,
        &receipt.source_locator,
        &receipt.source_sha256,
        &node.mid,
        &node.uid,
        NodeRole::Section,
        &node.node_type,
        title,
        &node.fields,
        &node.line_range,
        inherited,
        relation_facts,
    )
}

#[allow(clippy::too_many_arguments)]
fn insert_node(
    nodes: &mut Vec<NodeInput>,
    mids: &mut BTreeMap<String, String>,
    uids: &mut BTreeMap<String, String>,
    source_locator: &str,
    source_sha256: &str,
    mid: &str,
    uid: &str,
    role: NodeRole,
    node_type: &str,
    title: &str,
    fields: &BTreeMap<String, String>,
    line_range: &SdocLineRange,
    inherited: &BTreeMap<String, String>,
    relation_facts: &[String],
) -> Result<(), SdocGraphError> {
    if let Some(first_locator) = mids.insert(mid.to_owned(), source_locator.to_owned()) {
        return Err(SdocGraphError::DuplicateMid {
            mid: mid.to_owned(),
            first_locator,
            second_locator: source_locator.to_owned(),
        });
    }
    if let Some(first_mid) = uids.insert(uid.to_owned(), mid.to_owned()) {
        return Err(SdocGraphError::DuplicateUid {
            uid: uid.to_owned(),
            first_mid,
            second_mid: mid.to_owned(),
        });
    }
    let mut facts = NodeFacts::default()
        .with_field_value("sdoc.mid", mid)?
        .with_field_value("sdoc.uid", uid)?
        .with_field_value("sdoc.alias", uid)?
        .with_field_value("sdoc.node_type", node_type)?
        .with_field_value("title", title)?
        .with_field_value("sdoc.title", title)?
        .with_field_value("sdoc.source_locator", source_locator)?
        .with_field_value("sdoc.source_sha256", source_sha256)?
        .with_field_value(
            "sdoc.line_range",
            format!("{}-{}", line_range.start, line_range.end),
        )?
        .with_field_value("sdoc.authority", "canonical")?
        .with_field_value("canonical", "true")?;
    for (key, value) in inherited
        .iter()
        .filter(|(key, _)| !fields.keys().any(|field| field.eq_ignore_ascii_case(key)))
        .chain(fields)
    {
        let value = fact_excerpt(value);
        facts = facts.with_field_value(format!("sdoc.{}", key.to_ascii_lowercase()), value)?;
        if key.eq_ignore_ascii_case("tag") || key.eq_ignore_ascii_case("tags") {
            facts = facts.with_tag(value)?;
        }
        if key.eq_ignore_ascii_case("statement") {
            facts = facts.with_field_value("sdoc.statement", value)?;
        }
        if key.eq_ignore_ascii_case("alias") || key.eq_ignore_ascii_case("aliases") {
            facts = facts.with_field_value("sdoc.alias", value)?;
        }
    }
    for relation in relation_facts {
        facts = facts.with_field_value("sdoc.relation", relation)?;
    }
    nodes.push(
        NodeInput::new(
            NodeId::parse(mid)?,
            role,
            Provenance::new(
                "strictdoc-bridge",
                line_locator(source_locator, line_range, mid),
            )?,
            Confidence::new(100)?,
        )
        .with_facts(facts),
    );
    Ok(())
}

fn inherited_facts(document: &SdocDocument) -> BTreeMap<String, String> {
    document
        .metadata
        .iter()
        .filter(|(key, _)| {
            key.eq_ignore_ascii_case("status")
                || key.eq_ignore_ascii_case("freshness")
                || key.eq_ignore_ascii_case("public_boundary")
                || key.eq_ignore_ascii_case("boundary")
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Facts are search evidence, never canonical content.  A field longer than
/// the per-value fact limit (a prose section carried as a [TEXT] statement)
/// is carried as its leading bytes, cut at a character boundary, rather than
/// refusing the whole document.  The canonical text stays in the SDoc.
fn fact_excerpt(value: &str) -> &str {
    if value.len() <= NodeFacts::MAX_VALUE_BYTES {
        return value;
    }
    let mut end = NodeFacts::MAX_VALUE_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn field_value<'a>(fields: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value.as_str()))
}

fn line_locator(source: &str, range: &SdocLineRange, identity: &str) -> String {
    format!("{source}:{}-{}:{identity}", range.start, range.end)
}

fn relationship(value: &str) -> EdgeRelationship {
    let value = value.to_ascii_lowercase();
    if value.contains("verif") || value.contains("valid") || value.contains("test") {
        EdgeRelationship::Validation
    } else if value.contains("implement") || value.contains("satisf") || value.contains("alloc") {
        EdgeRelationship::Implementation
    } else {
        EdgeRelationship::Dependency
    }
}

/// SDoc query ranking inputs, in the approved order after identity and content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdocRankKey {
    pub exact_mid: bool,
    pub exact_uid_or_alias: bool,
    pub content_match: bool,
    pub active_or_published: bool,
    pub canonical: bool,
    pub boundary: SdocBoundaryKey,
    pub confidence: u8,
    pub freshness: String,
    pub mid: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SdocBoundaryKey {
    Unknown,
    Public,
    Internal,
}

/// One deterministically ordered SDoc graph result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdocRankedNode {
    pub node: NodeInput,
    pub rank_key: SdocRankKey,
}

/// Ranks only nodes projected by [`graph_input`]. Exact MID and UID/alias win
/// before content. Locator text is intentionally not a query input or tie-breaker.
pub fn rank(graph: &KnowledgeGraph, needle: &str, max_results: usize) -> Vec<SdocRankedNode> {
    let mut ranked: Vec<_> = graph
        .node_ids()
        .into_iter()
        .filter_map(|id| graph.node(&id))
        .filter(|node| node.facts().contains_value("sdoc.mid", node.id().as_str()))
        .filter_map(|node| rank_node(node, needle))
        .collect();
    ranked.sort_by(|left, right| compare_rank(&left.rank_key, &right.rank_key));
    ranked.truncate(max_results);
    ranked
}

fn rank_node(node: &NodeInput, needle: &str) -> Option<SdocRankedNode> {
    let exact_mid = node.id().as_str() == needle;
    let exact_uid_or_alias = values(node, &["sdoc.uid", "sdoc.alias"])
        .into_iter()
        .any(|value| value == needle);
    let content_match = values(node, &["title", "sdoc.title", "sdoc.statement", "tags"])
        .into_iter()
        .any(|value| contains_case_insensitive(value, needle));
    (exact_mid || exact_uid_or_alias || content_match).then(|| SdocRankedNode {
        node: node.clone(),
        rank_key: SdocRankKey {
            exact_mid,
            exact_uid_or_alias,
            content_match,
            active_or_published: values(node, &["sdoc.status"])
                .into_iter()
                .any(|value| matches!(value.to_ascii_lowercase().as_str(), "active" | "published")),
            canonical: node.facts().contains_value("canonical", "true"),
            boundary: boundary_key(
                values(node, &["sdoc.public_boundary", "sdoc.boundary"]).into_iter(),
            ),
            confidence: node.confidence().value(),
            freshness: values(node, &["sdoc.freshness"])
                .into_iter()
                .max()
                .unwrap_or_default()
                .to_owned(),
            mid: node.id().as_str().to_owned(),
        },
    })
}

fn values<'a>(node: &'a NodeInput, keys: &[&str]) -> Vec<&'a str> {
    keys.iter()
        .filter_map(|key| node.facts().values(key))
        .flatten()
        .map(String::as_str)
        .collect()
}

fn boundary_key(values: impl Iterator<Item = impl AsRef<str>>) -> SdocBoundaryKey {
    values.fold(SdocBoundaryKey::Unknown, |best, value| {
        let candidate = match value.as_ref().to_ascii_lowercase().as_str() {
            "internal" | "private" => SdocBoundaryKey::Internal,
            "public" => SdocBoundaryKey::Public,
            _ => SdocBoundaryKey::Unknown,
        };
        best.max(candidate)
    })
}

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    !needle.is_empty()
        && haystack
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
}

fn compare_rank(left: &SdocRankKey, right: &SdocRankKey) -> std::cmp::Ordering {
    right
        .exact_mid
        .cmp(&left.exact_mid)
        .then_with(|| right.exact_uid_or_alias.cmp(&left.exact_uid_or_alias))
        .then_with(|| right.content_match.cmp(&left.content_match))
        .then_with(|| right.active_or_published.cmp(&left.active_or_published))
        .then_with(|| right.canonical.cmp(&left.canonical))
        .then_with(|| right.boundary.cmp(&left.boundary))
        .then_with(|| right.confidence.cmp(&left.confidence))
        .then_with(|| right.freshness.cmp(&left.freshness))
        .then_with(|| left.mid.cmp(&right.mid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphLimits;
    use crate::sdoc::{SdocEngine, SdocRelation, SdocValidation};

    fn receipt(locator: &str, document_mid: &str, requirement_mid: &str, uid: &str) -> SdocReceipt {
        SdocReceipt {
            source_locator: locator.to_owned(),
            source_sha256: "a".repeat(64),
            validation: SdocValidation::Valid,
            engine: SdocEngine {
                api: "strictdoc.api".to_owned(),
                version: "0.29.0".to_owned(),
                artifact_sha256: "b".repeat(64),
                requirements_sha256: "c".repeat(64),
            },
            document: SdocDocument {
                mid: document_mid.to_owned(),
                uid: format!("UID-{document_mid}"),
                title: "Architecture".to_owned(),
                metadata: BTreeMap::from([
                    ("STATUS".to_owned(), "Published".to_owned()),
                    ("PUBLIC_BOUNDARY".to_owned(), "internal".to_owned()),
                    ("FRESHNESS".to_owned(), "2026-09-02".to_owned()),
                ]),
                line_range: range(1),
            },
            nodes: vec![SdocNode {
                mid: requirement_mid.to_owned(),
                uid: uid.to_owned(),
                node_type: "Requirement".to_owned(),
                fields: BTreeMap::from([
                    (
                        "STATEMENT".to_owned(),
                        "The system shall retain traceability.".to_owned(),
                    ),
                    ("TAGS".to_owned(), "traceability".to_owned()),
                ]),
                line_range: range(2),
            }],
            relations: vec![SdocRelation {
                mid: format!("REL-{requirement_mid}"),
                relation_type: "VERIFIES".to_owned(),
                reference_type: "Reference".to_owned(),
                source_mid: requirement_mid.to_owned(),
                target_mid: document_mid.to_owned(),
                line_range: range(3),
                owner: None,
                revision: None,
            }],
        }
    }

    fn range(line: usize) -> SdocLineRange {
        SdocLineRange {
            start: line,
            end: line,
        }
    }

    fn graph(receipts: &[SdocReceipt]) -> KnowledgeGraph {
        KnowledgeGraph::build(
            graph_input(receipts).unwrap(),
            GraphLimits::new(64, 64).unwrap(),
        )
        .unwrap()
    }

    fn full_core_requirement_fields() -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "STATEMENT".to_owned(),
                "The system shall retain traceability.".to_owned(),
            ),
            ("TAGS".to_owned(), "traceability".to_owned()),
            ("ALIASES".to_owned(), "REQ-1-LEGACY".to_owned()),
            ("RATIONALE".to_owned(), "Required for review.".to_owned()),
            (
                "VERIFICATION_CRITERIA".to_owned(),
                "Trace exists.".to_owned(),
            ),
            ("VERIFICATION_METHOD".to_owned(), "Inspection".to_owned()),
            ("SOURCE".to_owned(), "AlphaZede".to_owned()),
            ("OWNER".to_owned(), "systems".to_owned()),
            ("PRIORITY".to_owned(), "high".to_owned()),
            ("RISK".to_owned(), "medium".to_owned()),
            ("ASSUMPTION".to_owned(), "baseline".to_owned()),
            ("CONSTRAINT".to_owned(), "stable-identity".to_owned()),
            ("DERIVATION".to_owned(), "portfolio".to_owned()),
            ("SATISFACTION".to_owned(), "model".to_owned()),
            ("ALLOCATION".to_owned(), "system".to_owned()),
            ("VALIDATION".to_owned(), "planned".to_owned()),
            ("TEST_EVIDENCE".to_owned(), "pending".to_owned()),
        ])
    }

    #[test]
    fn relocation_keeps_mid_identity_and_locator_is_only_a_fact() {
        let old = graph(&[receipt("docs/old.sdoc", "MID-DOC", "MID-REQ", "REQ-1")]);
        let new = graph(&[receipt(
            "architecture/new.sdoc",
            "MID-DOC",
            "MID-REQ",
            "REQ-1",
        )]);
        let old_node = old.node(&NodeId::parse("MID-REQ").unwrap()).unwrap();
        let new_node = new.node(&NodeId::parse("MID-REQ").unwrap()).unwrap();
        assert_eq!(old_node.id(), new_node.id());
        assert_ne!(
            old_node.provenance().locator(),
            new_node.provenance().locator()
        );
        assert!(old_node
            .facts()
            .contains_value("sdoc.relation", "REL-MID-REQ:VERIFIES:MID-DOC"));
    }

    /// A prose section carried as a [TEXT] statement (azedge#74) is longer
    /// than one fact value may be.  The graph carries an excerpt as evidence
    /// and titles the node by its heading; it never refuses the document.
    #[test]
    fn long_text_statement_projects_as_an_excerpt_titled_by_its_heading() {
        let mut receipt = receipt("architecture.sdoc", "MID-DOC", "MID-SEC", "SEC-1");
        let body = "Baselines\n\n".to_owned() + &"| Boundary | Current | Target |\n".repeat(80);
        assert!(body.len() > NodeFacts::MAX_VALUE_BYTES);
        receipt.nodes[0].node_type = "TEXT".to_owned();
        receipt.nodes[0].fields = BTreeMap::from([("STATEMENT".to_owned(), body.clone())]);
        receipt.relations.clear();
        let input = graph_input(&[receipt]).unwrap();
        let section = input
            .nodes()
            .iter()
            .find(|node| node.id().as_str() == "MID-SEC")
            .unwrap();
        assert!(section.facts().contains_value("title", "Baselines"));
        let excerpt = &section.facts().values("sdoc.statement").unwrap()[0];
        assert_eq!(excerpt.len(), NodeFacts::MAX_VALUE_BYTES);
        assert!(body.starts_with(excerpt.as_str()));
    }

    #[test]
    fn full_core_requirement_plus_project_extension_projects_without_truncation() {
        let mut receipt = receipt("architecture.sdoc", "MID-DOC", "MID-REQ", "REQ-1");
        receipt.nodes[0].fields = full_core_requirement_fields();
        receipt.nodes[0]
            .fields
            .insert("PROJECT_EXTENSION".to_owned(), "portfolio-a".to_owned());
        let input = graph_input(&[receipt]).unwrap();
        let requirement = input
            .nodes()
            .iter()
            .find(|node| node.id().as_str() == "MID-REQ")
            .unwrap();
        assert!(requirement
            .facts()
            .contains_value("sdoc.project_extension", "portfolio-a"));
        assert!(requirement
            .facts()
            .contains_value("sdoc.relation", "REL-MID-REQ:VERIFIES:MID-DOC"));
    }

    #[test]
    fn exact_identity_beats_content_and_path_noise_with_stable_ties() {
        let exact = receipt("noise/path.sdoc", "MID-DOC", "MID-REQ", "REQ-1");
        let mut content = receipt(
            "MID-REQ/looks-like-a-path.sdoc",
            "MID-DOC-2",
            "MID-REQ-2",
            "REQ-2",
        );
        content.nodes[0]
            .fields
            .insert("STATEMENT".to_owned(), "MID-REQ".to_owned());
        let forward = graph(&[content.clone(), exact.clone()]);
        let reverse = graph(&[exact, content]);
        let forward_ids: Vec<_> = rank(&forward, "MID-REQ", 8)
            .into_iter()
            .map(|item| item.rank_key.mid)
            .collect();
        let reverse_ids: Vec<_> = rank(&reverse, "MID-REQ", 8)
            .into_iter()
            .map(|item| item.rank_key.mid)
            .collect();
        assert_eq!(forward_ids, vec!["MID-REQ", "MID-REQ-2"]);
        assert_eq!(forward_ids, reverse_ids);

        let mut uid_content = receipt("another/path.sdoc", "MID-DOC-3", "MID-REQ-3", "REQ-3");
        uid_content.nodes[0]
            .fields
            .insert("STATEMENT".to_owned(), "REQ-1".to_owned());
        let uid_graph = graph(&[
            receipt("noise/path.sdoc", "MID-DOC", "MID-REQ", "REQ-1"),
            uid_content,
        ]);
        let uid_ids: Vec<_> = rank(&uid_graph, "REQ-1", 8)
            .into_iter()
            .map(|item| item.rank_key.mid)
            .collect();
        assert_eq!(uid_ids, vec!["MID-REQ", "MID-REQ-3"]);
    }

    #[test]
    fn state_then_authority_boundary_confidence_and_freshness_order_content_matches() {
        let mut draft = receipt("b.sdoc", "MID-DOC", "MID-REQ-1", "REQ-1");
        draft.nodes[0]
            .fields
            .insert("STATEMENT".to_owned(), "needle".to_owned());
        draft
            .document
            .metadata
            .insert("STATUS".to_owned(), "Draft".to_owned());
        draft
            .document
            .metadata
            .insert("PUBLIC_BOUNDARY".to_owned(), "public".to_owned());
        let mut published = receipt("a.sdoc", "MID-DOC-2", "MID-REQ-2", "REQ-2");
        published.nodes[0]
            .fields
            .insert("STATEMENT".to_owned(), "needle".to_owned());
        let graph = graph(&[draft, published]);
        let ids: Vec<_> = rank(&graph, "needle", 8)
            .into_iter()
            .map(|item| item.rank_key.mid)
            .collect();
        assert_eq!(ids, vec!["MID-REQ-2", "MID-REQ-1"]);
    }

    #[test]
    fn duplicate_identity_and_unresolved_relation_fail_closed() {
        let mut duplicate_node = receipt("b.sdoc", "MID-DOC-2", "MID-REQ", "REQ-2");
        duplicate_node.relations[0].mid = "REL-OTHER".to_owned();
        let duplicate = vec![
            receipt("a.sdoc", "MID-DOC", "MID-REQ", "REQ-1"),
            duplicate_node,
        ];
        assert!(matches!(
            graph_input(&duplicate),
            Err(SdocGraphError::DuplicateMid { .. })
        ));
        let mut duplicate_uid_node = receipt("b.sdoc", "MID-DOC-2", "MID-REQ-2", "REQ-1");
        duplicate_uid_node.relations[0].mid = "REL-OTHER".to_owned();
        let duplicate_uid = vec![
            receipt("a.sdoc", "MID-DOC", "MID-REQ", "REQ-1"),
            duplicate_uid_node,
        ];
        assert!(matches!(
            graph_input(&duplicate_uid),
            Err(SdocGraphError::DuplicateUid { .. })
        ));
        let mut unresolved = receipt("a.sdoc", "MID-DOC", "MID-REQ", "REQ-1");
        unresolved.relations[0].target_mid = "MID-MISSING".to_owned();
        assert!(matches!(
            graph_input(&[unresolved]),
            Err(SdocGraphError::UnresolvedRelation { target_mid, .. }) if target_mid == "MID-MISSING"
        ));
    }
}
