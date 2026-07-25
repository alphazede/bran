//! Independent Obsidian-ready portable export (Slice 3.3-A).
//!
//! Deterministic portable bundle of Markdown files using YAML frontmatter and
//! Obsidian wikilinks. Stable root-relative slash paths only.
//! Strict root-bound + edge validation; public_boundary: public accepted,
//! non-public values/important_boundary/canaries rejected (DLP distinct).
//! Unknown scalar frontmatter emitted deterministically as YAML strings.
//! Repeat export yields identical bytes/digest independent of input order.
//! Reparse recovers exact node/edge ids and declared link targets from records.

use std::collections::BTreeMap;
use std::fmt;

use crate::graph::{EdgeId, NodeId};
use crate::scan::ContentIdentity;

const PUBLIC_BOUNDARY_CANARY_FIXTURE: &str =
    include_str!("../../../../fixtures/public-boundary/rejected/synthetic-canaries.txt");

/// Export failures. All are deterministic and never repair input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExportError {
    RootBoundViolation { path: String, reason: String },
    PublicBoundaryViolation(String),
    DlpViolation(String),
    DuplicatePath(String),
    InvalidIdentity(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootBoundViolation { path, reason } => {
                write!(f, "root-bound path violation for {path}: {reason}")
            }
            Self::PublicBoundaryViolation(detail) => {
                write!(f, "public-boundary violation: {detail}")
            }
            Self::DlpViolation(detail) => write!(f, "DLP violation: {detail}"),
            Self::DuplicatePath(p) => write!(f, "duplicate export path: {p}"),
            Self::InvalidIdentity(s) => write!(f, "invalid identity: {s}"),
        }
    }
}

impl std::error::Error for ExportError {}

/// Logical node supplied to export. Path must be caller-chosen stable
/// root-relative slash path ending in .md. Frontmatter values are emitted as
/// YAML strings; the body is emitted before declared links.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportNode {
    pub id: NodeId,
    pub path: String,
    pub frontmatter: BTreeMap<String, String>,
    pub body: String,
}

/// Logical edge supplied to export. link_target is the exact declared target
/// used for the Obsidian wikilink (e.g. "concepts/beta" without .md).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportEdge {
    pub id: EdgeId,
    pub source: NodeId,
    pub target: NodeId,
    pub link_target: String,
}

/// Deterministic portable Obsidian bundle.
/// Docs keyed by stable root-relative slash path. Content includes YAML
/// frontmatter + body + wikilinks + embedded graph representation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObsidianBundle {
    docs: BTreeMap<String, String>,
}

impl ObsidianBundle {
    /// Read-only view of the bundle. Keys and values are stable.
    pub fn docs(&self) -> &BTreeMap<String, String> {
        &self.docs
    }

    /// Deterministic bytes for the entire bundle.
    /// Order is by sorted path; each entry path\0content\0 .
    pub fn as_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (path, content) in &self.docs {
            out.extend_from_slice(path.as_bytes());
            out.push(0);
            out.extend_from_slice(content.as_bytes());
            out.push(0);
        }
        out
    }

    /// Digest over as_bytes using existing ContentIdentity lanes (non-crypto).
    pub fn digest(&self) -> String {
        let id = ContentIdentity::from_bytes(&self.as_bytes());
        format!(
            "{:016x}:{:016x}:{:016x}:{}",
            id.lanes[0], id.lanes[1], id.lanes[2], id.byte_len
        )
    }
}

/// Produce an Obsidian-ready portable Markdown bundle.
/// - Paths and declared link targets validated (root-rel, no scheme/abs/\0/\./.. /|).
/// - Every edge source/target must exist exactly once; reject dup node ids/edge ids/paths.
/// - public_boundary: "public" accepted; "private"/"internal"/important_boundary/canaries rejected
///   (DLP for canaries, distinct from PublicBoundaryViolation).
/// - No silent preserve of conflicting bran_node_id (reject); add if absent.
/// - Unknown scalar values emitted as YAML strings with correct escapes for \n\r\t\/".
/// - Body + declared wikilinks after; graph rep always included.
/// - Output deterministic (sorted by identity/path) regardless of input order.
pub fn export_to_obsidian(
    nodes: &[ExportNode],
    edges: &[ExportEdge],
) -> Result<ObsidianBundle, ExportError> {
    // Fail-closed validation. Detect dups via BTree for determinism.
    let mut path_set: BTreeMap<String, ()> = BTreeMap::new();
    let mut node_paths: BTreeMap<NodeId, String> = BTreeMap::new();
    for node in nodes {
        validate_emitted_string(node.id.as_str())?;
        if node_paths.contains_key(&node.id) {
            return Err(ExportError::InvalidIdentity(format!(
                "duplicate node id: {}",
                node.id.as_str()
            )));
        }
        node_paths.insert(node.id.clone(), node.path.clone());

        if node.path == "bran-graph.md" || path_set.contains_key(&node.path) {
            return Err(ExportError::DuplicatePath(node.path.clone()));
        }
        path_set.insert(node.path.clone(), ());

        validate_root_bound_path(&node.path)?;

        // Reject conflicting reserved bran_node_id; allow matching or absent.
        if let Some(v) = node.frontmatter.get("bran_node_id") {
            if v != node.id.as_str() {
                return Err(ExportError::InvalidIdentity(format!(
                    "bran_node_id conflict: {} != {}",
                    v,
                    node.id.as_str()
                )));
            }
        }

        for k in node.frontmatter.keys() {
            if matches!(
                k.as_str(),
                "bran_type" | "bran_node_count" | "bran_edge_count"
            ) || k.starts_with("bran_edge_")
            {
                return Err(ExportError::InvalidIdentity(format!(
                    "reserved frontmatter key: {k}"
                )));
            }
            if !is_safe_yaml_key(k) {
                return Err(ExportError::InvalidIdentity(format!(
                    "unsafe YAML key: {}",
                    k
                )));
            }
        }

        check_public_boundary_and_dlp(&node.path, &node.frontmatter, &node.body)?;
    }

    for edge in edges {
        validate_emitted_string(edge.id.as_str())?;
        validate_emitted_string(edge.source.as_str())?;
        validate_emitted_string(edge.target.as_str())?;
        validate_emitted_string(edge.link_target.as_str())?;
        if !node_paths.contains_key(&edge.source) {
            return Err(ExportError::InvalidIdentity(format!(
                "edge source missing in nodes: {}",
                edge.source.as_str()
            )));
        }
        if !node_paths.contains_key(&edge.target) {
            return Err(ExportError::InvalidIdentity(format!(
                "edge target missing in nodes: {}",
                edge.target.as_str()
            )));
        }
        validate_obsidian_target(&edge.link_target)?;
        let target_path = node_paths
            .get(&edge.target)
            .expect("target existence checked above");
        let expected_link_target = target_path
            .strip_suffix(".md")
            .expect("node paths validated above");
        if edge.link_target != expected_link_target {
            return Err(ExportError::InvalidIdentity(format!(
                "edge link target conflict: {} != {}",
                edge.link_target, expected_link_target
            )));
        }

        // dup edge ids checked after source/target to keep simple
    }
    let mut edge_id_set: BTreeMap<EdgeId, ()> = BTreeMap::new();
    for edge in edges {
        if edge_id_set.contains_key(&edge.id) {
            return Err(ExportError::InvalidIdentity(format!(
                "duplicate edge id: {}",
                edge.id.as_str()
            )));
        }
        edge_id_set.insert(edge.id.clone(), ());
    }

    // Sort for deterministic output independent of input order (by id/path).
    let mut sorted_nodes: Vec<_> = nodes.to_vec();
    sorted_nodes.sort_by(|a, b| a.path.cmp(&b.path));
    let mut sorted_edges: Vec<_> = edges.to_vec();
    sorted_edges.sort_by(|a, b| a.id.cmp(&b.id));

    let mut docs: BTreeMap<String, String> = BTreeMap::new();
    for node in &sorted_nodes {
        let md = build_node_markdown(node, &sorted_edges);
        docs.insert(node.path.clone(), md);
    }

    let graph_md = build_graph_markdown(&sorted_nodes, &sorted_edges);
    docs.insert("bran-graph.md".to_string(), graph_md);

    Ok(ObsidianBundle { docs })
}

fn validate_root_bound_path(path: &str) -> Result<(), ExportError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path.len() > 4096
        || !path.ends_with(".md")
        || path
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return Err(ExportError::RootBoundViolation {
            path: path.to_owned(),
            reason: "must be non-empty root-relative slash path ending .md; no leading /, \\, NUL, ., .. segments, or length > 4096".to_owned(),
        });
    }
    for seg in path.split('/') {
        if !is_safe_fs_segment(seg) {
            return Err(ExportError::RootBoundViolation {
                path: path.to_owned(),
                reason:
                    "segment contains unsafe characters (only [A-Za-z0-9._-] allowed per segment)"
                        .to_owned(),
            });
        }
    }
    Ok(())
}

fn is_safe_fs_segment(seg: &str) -> bool {
    !seg.is_empty()
        && seg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn is_safe_yaml_key(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with(' ')
        && !key.starts_with('-')
        && !key.contains(':')
        && !key.contains('\n')
        && !key.contains('\r')
        && !key.contains('\t')
        && !key.contains('"')
        && !key.contains('\'')
        && !key.contains('|')
        && !key.contains('\\')
        && !key.contains('#')
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn validate_obsidian_target(target: &str) -> Result<(), ExportError> {
    if target.is_empty()
        || target.starts_with('/')
        || target.contains('\\')
        || target.contains('\0')
        || target.contains('|')
        || target.contains(':')
        || target.len() > 4096
        || target
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return Err(ExportError::RootBoundViolation {
            path: target.to_owned(),
            reason:
                "must be safe root-relative Obsidian target; no scheme, absolute, backslash, NUL, dot segments, pipe"
                    .to_owned(),
        });
    }
    for seg in target.split('/') {
        if !is_safe_fs_segment(seg) {
            return Err(ExportError::RootBoundViolation {
                path: target.to_owned(),
                reason:
                    "segment contains unsafe characters (only [A-Za-z0-9._-] allowed per segment)"
                        .to_owned(),
            });
        }
    }
    Ok(())
}

fn check_public_boundary_and_dlp(
    path: &str,
    fm: &BTreeMap<String, String>,
    body: &str,
) -> Result<(), ExportError> {
    // Shared emitted-string validator covers canary + important_boundary for path/body/fm.
    validate_emitted_string(path)?;
    validate_emitted_string(body)?;
    for (k, v) in fm {
        validate_emitted_string(k)?;
        validate_emitted_string(v)?;
    }

    // public_boundary only allowed value is exactly "public"; non-public rejected.
    for (k, v) in fm {
        if k == "public_boundary" && v != "public" {
            return Err(ExportError::PublicBoundaryViolation(format!(
                "non-public value for public_boundary: {v}"
            )));
        }
    }
    Ok(())
}

fn has_public_boundary_canary(value: &str) -> bool {
    PUBLIC_BOUNDARY_CANARY_FIXTURE
        .lines()
        .any(|line| !line.is_empty() && value.contains(line))
}

/// Shared emitted-string validator for important_boundary + DLP (canary) policy.
/// Applied to every node/edge identity, source, target, link, path, body, fm key+value.
fn validate_emitted_string(value: &str) -> Result<(), ExportError> {
    if has_public_boundary_canary(value) {
        return Err(ExportError::DlpViolation("synthetic canary".to_owned()));
    }
    if value.contains("important_boundary") {
        return Err(ExportError::PublicBoundaryViolation(
            "important_boundary".to_owned(),
        ));
    }
    Ok(())
}

fn build_node_markdown(node: &ExportNode, edges: &[ExportEdge]) -> String {
    let mut out = String::new();
    out.push_str("---\n");

    // Lexical for determinism. Add bran_node_id only if absent (conflict rejected earlier).
    let mut effective = node.frontmatter.clone();
    if !effective.contains_key("bran_node_id") {
        effective.insert("bran_node_id".to_string(), node.id.as_str().to_string());
    }

    for (k, v) in &effective {
        out.push_str(&format!("{}: {}\n", k, yaml_scalar(v)));
    }

    out.push_str("---\n");
    out.push_str(&node.body);
    if !node.body.ends_with('\n') && !node.body.is_empty() {
        out.push('\n');
    }

    let mut outs: Vec<&ExportEdge> = edges.iter().filter(|e| e.source == node.id).collect();
    if !outs.is_empty() {
        outs.sort_by(|a, b| a.link_target.cmp(&b.link_target));
        out.push('\n');
        for e in outs {
            out.push_str(&format!("[[{}]]\n", e.link_target));
        }
    }

    out
}

fn yaml_scalar(v: &str) -> String {
    format!("\"{}\"", escape_for_yaml(v))
}

fn escape_for_yaml(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn build_graph_markdown(nodes: &[ExportNode], edges: &[ExportEdge]) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("bran_type: graph-representation\n");
    out.push_str(&format!("bran_node_count: {}\n", nodes.len()));
    out.push_str(&format!("bran_edge_count: {}\n", edges.len()));
    // Embed edge identities + declared link targets for independent reparse.
    for (i, e) in edges.iter().enumerate() {
        let val = format!(
            "{}|{}|{}|{}",
            e.id.as_str(),
            e.source.as_str(),
            e.target.as_str(),
            e.link_target
        );
        out.push_str(&format!("bran_edge_{}: {}\n", i, val));
    }
    out.push_str("---\n\n");
    out.push_str("# Bran Graph Representation\n\n");
    out.push_str("## Nodes (wikilinks)\n");
    for n in nodes {
        let link = n.path.strip_suffix(".md").unwrap_or(&n.path);
        out.push_str(&format!("- [[{}]] (id: {})\n", link, n.id.as_str()));
    }
    out.push_str("\n## Edges\n");
    for e in edges {
        out.push_str(&format!(
            "- {}: {} -> [[{}]]\n",
            e.id.as_str(),
            e.source.as_str(),
            e.link_target
        ));
    }
    out
}

/// Result of independent reparse over an exported bundle.
/// Identities and link targets must round-trip exactly for truth.
/// Retains exact edge id/source/target/link records and node frontmatter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reparsed {
    pub node_ids: Vec<NodeId>,
    pub edge_ids: Vec<EdgeId>,
    pub link_targets: Vec<String>,
    /// Exact edge records (id, source, target, link_target) for round-trip.
    pub edge_records: Vec<(EdgeId, NodeId, NodeId, String)>,
    /// Full node frontmatter recovered for frontmatter truth verification.
    pub node_frontmatter: BTreeMap<NodeId, BTreeMap<String, String>>,
}

/// Reparse a produced bundle back to node/edge identities and declared link targets.
/// Recovers exclusively from bran_node_id + bran_edge_* records (no graph nav wikilinks).
/// Validates all four fields in edge records (ids + target syntax) and rejects
/// edges whose endpoints do not exist. Applies emitted-string policy checks.
pub fn reparse_obsidian(bundle: &ObsidianBundle) -> Result<Reparsed, ExportError> {
    let mut node_set: BTreeMap<NodeId, ()> = BTreeMap::new();
    let mut node_fms: BTreeMap<NodeId, BTreeMap<String, String>> = BTreeMap::new();
    let mut edge_map: BTreeMap<EdgeId, (NodeId, NodeId, String)> = BTreeMap::new();
    let mut link_set: BTreeMap<String, ()> = BTreeMap::new();

    for content in bundle.docs().values() {
        let fm = parse_frontmatter_block(content);
        if let Some(id_str) = fm.get("bran_node_id") {
            validate_emitted_string(id_str)?;
            match NodeId::parse(id_str.clone()) {
                Ok(nid) => {
                    node_set.insert(nid.clone(), ());
                    // Validate all frontmatter strings for policy on reparse too.
                    for (k, v) in &fm {
                        validate_emitted_string(k)?;
                        validate_emitted_string(v)?;
                    }
                    node_fms.insert(nid, fm.clone());
                }
                Err(_) => {
                    return Err(ExportError::InvalidIdentity(id_str.clone()));
                }
            }
        }
        for (k, v) in &fm {
            if let Some(suffix) = k.strip_prefix("bran_edge_") {
                if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                    let parts: Vec<&str> = v.splitn(4, '|').collect();
                    if parts.len() != 4 {
                        return Err(ExportError::InvalidIdentity(v.clone()));
                    }
                    // Shared validator + exact record parse for all four fields.
                    validate_emitted_string(parts[0])?;
                    validate_emitted_string(parts[1])?;
                    validate_emitted_string(parts[2])?;
                    validate_emitted_string(parts[3])?;
                    let eid = EdgeId::parse(parts[0].to_owned())
                        .map_err(|_| ExportError::InvalidIdentity(parts[0].to_owned()))?;
                    let src = NodeId::parse(parts[1].to_owned())
                        .map_err(|_| ExportError::InvalidIdentity(parts[1].to_owned()))?;
                    let tgt = NodeId::parse(parts[2].to_owned())
                        .map_err(|_| ExportError::InvalidIdentity(parts[2].to_owned()))?;
                    validate_obsidian_target(parts[3])?;
                    if edge_map.contains_key(&eid) {
                        return Err(ExportError::InvalidIdentity(format!(
                            "duplicate edge id in records: {}",
                            eid.as_str()
                        )));
                    }
                    edge_map.insert(eid, (src, tgt, parts[3].to_string()));
                    link_set.insert(parts[3].to_string(), ());
                }
            }
        }
    }

    // Reparse validation rejects edges whose endpoints do not exist.
    for (eid, (src, tgt, _)) in edge_map.iter() {
        if !node_set.contains_key(src) {
            return Err(ExportError::InvalidIdentity(format!(
                "edge {} source missing: {}",
                eid.as_str(),
                src.as_str()
            )));
        }
        if !node_set.contains_key(tgt) {
            return Err(ExportError::InvalidIdentity(format!(
                "edge {} target missing: {}",
                eid.as_str(),
                tgt.as_str()
            )));
        }
    }

    Ok(Reparsed {
        node_ids: node_set.into_keys().collect(),
        edge_ids: edge_map.keys().cloned().collect(),
        link_targets: link_set.into_keys().collect(),
        edge_records: edge_map
            .iter()
            .map(|(eid, (src, tgt, lnk))| (eid.clone(), src.clone(), tgt.clone(), lnk.clone()))
            .collect(),
        node_frontmatter: node_fms,
    })
}

fn parse_frontmatter_block(content: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Some(rest) = content.split_once("---\n") {
        if let Some((block, _)) = rest.1.split_once("\n---") {
            for line in block.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    let key = k.trim().to_string();
                    if key.is_empty() {
                        continue;
                    }
                    let raw_val = v.trim();
                    let val = if raw_val.starts_with('"') && raw_val.ends_with('"') {
                        raw_val[1..raw_val.len() - 1]
                            .replace("\\\"", "\"")
                            .replace("\\\\", "\\")
                    } else {
                        raw_val.to_string()
                    };
                    map.insert(key, val);
                }
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{EdgeId, NodeId};

    #[test]
    fn p3_obsidian_export() {
        let alpha = NodeId::parse("file:src/alpha.rs").expect("valid node id");
        let beta = NodeId::parse("file:src/beta.rs").expect("valid node id");
        let e1 = EdgeId::parse("edge:alpha-to-beta").expect("valid edge id");

        let mut fm_alpha = BTreeMap::new();
        fm_alpha.insert("type".to_string(), "Concept".to_string());
        fm_alpha.insert("unknown_preserved".to_string(), "true".to_string());
        fm_alpha.insert("status".to_string(), "active".to_string());
        fm_alpha.insert("public_boundary".to_string(), "public".to_string());

        let body_alpha = include_str!("../../../../fixtures/obsidian/p3-body.txt");

        let node_alpha = ExportNode {
            id: alpha.clone(),
            path: "concepts/alpha.md".to_string(),
            frontmatter: fm_alpha,
            body: body_alpha.to_string(),
        };

        let mut fm_beta = BTreeMap::new();
        fm_beta.insert("type".to_string(), "Note".to_string());
        let node_beta = ExportNode {
            id: beta.clone(),
            path: "concepts/beta.md".to_string(),
            frontmatter: fm_beta,
            body: "Beta body text preserved.".to_string(),
        };

        let edge = ExportEdge {
            id: e1.clone(),
            source: alpha.clone(),
            target: beta.clone(),
            link_target: "concepts/beta".to_string(),
        };

        let nodes_in = vec![node_alpha.clone(), node_beta.clone()];
        let edges_in = vec![edge.clone()];

        let b1 = export_to_obsidian(&nodes_in, &edges_in).expect("first export");
        let b2 = export_to_obsidian(&nodes_in, &edges_in).expect("second");
        assert_eq!(b1.as_bytes(), b2.as_bytes());
        assert_eq!(b1.digest(), b2.digest());

        // order independence
        let nodes_rev = vec![node_beta.clone(), node_alpha.clone()];
        let b3 = export_to_obsidian(&nodes_rev, &edges_in).expect("rev order");
        assert_eq!(b1.as_bytes(), b3.as_bytes(), "order independent");

        let docs = b1.docs();
        let alpha_md = docs.get("concepts/alpha.md").expect("alpha");
        assert!(alpha_md.contains("public_boundary: \"public\""));
        assert!(alpha_md.contains("unknown_preserved: \"true\""));
        assert!(alpha_md.contains("Body text must be preserved exactly"));
        assert!(alpha_md.contains("[[concepts/beta]]"));
        assert!(alpha_md.contains("bran_node_id: \"file:src/alpha.rs\""));

        // exact reparse (BTree order by id)
        let reparsed = reparse_obsidian(&b1).expect("reparse");
        assert_eq!(reparsed.node_ids, vec![alpha.clone(), beta.clone()]);
        assert_eq!(reparsed.edge_ids, vec![e1.clone()]);
        assert_eq!(reparsed.link_targets, vec!["concepts/beta".to_string()]);
        // Strengthen: exact edge id/source/target/link records + node frontmatter truth.
        assert_eq!(reparsed.edge_records.len(), 1);
        assert_eq!(reparsed.edge_records[0].0, e1);
        assert_eq!(reparsed.edge_records[0].1, alpha);
        assert_eq!(reparsed.edge_records[0].2, beta);
        assert_eq!(reparsed.edge_records[0].3, "concepts/beta".to_string());
        let fm_a = reparsed.node_frontmatter.get(&alpha).expect("alpha fm");
        assert_eq!(
            fm_a.get("public_boundary").map(String::as_str),
            Some("public")
        );
        assert_eq!(
            fm_a.get("unknown_preserved").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            fm_a.get("bran_node_id").map(String::as_str),
            Some("file:src/alpha.rs")
        );

        // reparse must reject edges with endpoints that do not exist (from records)
        let mut bad_docs: BTreeMap<String, String> = b1.docs().clone();
        if let Some(g) = bad_docs.get_mut("bran-graph.md") {
            let good_rec = format!(
                "{}|{}|{}|{}",
                e1.as_str(),
                alpha.as_str(),
                beta.as_str(),
                "concepts/beta"
            );
            let bad_rec = format!("{}|{}|file:does-not-exist|dne", e1.as_str(), alpha.as_str());
            *g = g.replace(&good_rec, &bad_rec);
        }
        let bad_b = ObsidianBundle { docs: bad_docs };
        assert!(matches!(
            reparse_obsidian(&bad_b),
            Err(ExportError::InvalidIdentity(_))
        ));

        // one rep root rejection
        let bad_root = ExportNode {
            id: alpha.clone(),
            path: "../escape.md".to_string(),
            frontmatter: BTreeMap::new(),
            body: String::new(),
        };
        assert!(matches!(
            export_to_obsidian(&[bad_root], &[]),
            Err(ExportError::RootBoundViolation { .. })
        ));

        // one rep boundary rejection (non-public)
        let mut bad_b = BTreeMap::new();
        bad_b.insert("public_boundary".to_string(), "internal".to_string());
        let bad_b_node = ExportNode {
            id: alpha.clone(),
            path: "b.md".to_string(),
            frontmatter: bad_b,
            body: String::new(),
        };
        assert!(matches!(
            export_to_obsidian(&[bad_b_node], &[]),
            Err(ExportError::PublicBoundaryViolation(_))
        ));

        // identity canary DLP rejection
        let canary_lines: Vec<&str> = PUBLIC_BOUNDARY_CANARY_FIXTURE
            .lines()
            .filter(|l| !l.is_empty())
            .collect();
        let omega_canary = canary_lines
            .get(1)
            .or_else(|| canary_lines.first())
            .copied()
            .expect("fixture must supply canary");
        let bad_c = ExportNode {
            id: NodeId::parse(format!("file:{}", omega_canary))
                .expect("valid canary node id from fixture"),
            path: "c.md".to_string(),
            frontmatter: BTreeMap::new(),
            body: String::new(),
        };
        assert!(matches!(
            export_to_obsidian(&[bad_c], &[]),
            Err(ExportError::DlpViolation(_))
        ));

        // reserved generated graph key rejection
        assert!(matches!(
            export_to_obsidian(
                &[ExportNode {
                    id: alpha.clone(),
                    path: "forged.md".to_string(),
                    frontmatter: BTreeMap::from([(
                        "bran_edge_0".to_string(),
                        "edge:forged|file:a|file:b|forged".to_string(),
                    )]),
                    body: String::new(),
                }],
                &[],
            ),
            Err(ExportError::InvalidIdentity(_))
        ));

        // one rep edge rejection (missing target)
        let bad_edge = ExportEdge {
            id: e1.clone(),
            source: alpha.clone(),
            target: NodeId::parse("file:missing").expect("parse"),
            link_target: "missing".to_string(),
        };
        assert!(matches!(
            export_to_obsidian(&nodes_in, &[bad_edge]),
            Err(ExportError::InvalidIdentity(_))
        ));
    }
}
