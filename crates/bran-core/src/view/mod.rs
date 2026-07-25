//! Deterministic, persistent Knowledge Views over an immutable graph.
//!
//! A [`ViewSpec`] is deliberately limited to durable selection and presentation
//! choices. Runtime packet assembly and compression are separate boundaries.

use crate::graph::{KnowledgeGraph, NodeId, NodeRole, Provenance};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Declarative source selection for a persistent knowledge view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ViewSource {
    All,
    NodeIds(Vec<NodeId>),
    Roles(Vec<NodeRole>),
    ProvenanceSourceContains(String),
    SemanticFieldValue { key: String, value: String },
}

/// Deterministic predicates applied after source selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ViewFilter {
    All,
    Role(NodeRole),
    IdPrefix(String),
    ProvenanceSourceContains(String),
    ProvenanceLocatorContains(String),
    MinimumConfidence(u8),
    SemanticFieldValue { key: String, value: String },
    AllOf(Vec<ViewFilter>),
    AnyOf(Vec<ViewFilter>),
    Not(Box<ViewFilter>),
}

/// Stable ordering modes. Every mode breaks ties by stable node identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ViewSort {
    NodeId,
    RoleThenNodeId,
    ProvenanceSourceThenNodeId,
    ConfidenceDescendingThenNodeId,
    SemanticFieldThenNodeId(String),
}

/// The group layout of a compiled view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ViewGrouping {
    None,
    Role,
    ProvenanceSource,
    SemanticField(String),
}

/// Fields copied from graph nodes into a compiled view item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ViewField {
    NodeId,
    Role,
    ProvenanceSource,
    ProvenanceLocator,
    Confidence,
    SemanticField(String),
}

/// A renderer preference recorded with the view without changing selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Presentation {
    Markdown,
    Json,
    Terminal,
    Obsidian,
}

/// Persistent, dependency-free view declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewSpec {
    pub source: ViewSource,
    pub filter: ViewFilter,
    pub sort: ViewSort,
    pub grouping: ViewGrouping,
    pub fields: Vec<ViewField>,
    pub presentation: Presentation,
    pub max_items: usize,
    pub max_bytes: usize,
}

impl ViewSpec {
    pub const MAX_SOURCE_NODE_IDS: usize = 256;
    pub const MAX_SOURCE_ROLES: usize = 16;
    pub const MAX_PROJECTED_FIELDS: usize = 64;
    pub const MAX_FILTER_DEPTH: usize = 16;
    pub const MAX_FILTER_CLAUSES: usize = 128;
    pub const MAX_OUTPUT_ITEMS: usize = 4_096;
    pub const MAX_OUTPUT_BYTES: usize = 1_048_576;
    pub const MAX_DECLARATION_STRING_BYTES: usize = 256;

    /// Validates every caller-controlled collection and recursive shape before
    /// the compiler allocates candidates or descends through predicates.
    pub fn validate(&self) -> Result<(), ViewError> {
        match &self.source {
            ViewSource::NodeIds(ids) if ids.len() > Self::MAX_SOURCE_NODE_IDS => {
                return Err(ViewError::SourceNodeIdLimitExceeded {
                    limit: Self::MAX_SOURCE_NODE_IDS,
                    actual: ids.len(),
                });
            }
            ViewSource::Roles(roles) if roles.len() > Self::MAX_SOURCE_ROLES => {
                return Err(ViewError::SourceRoleLimitExceeded {
                    limit: Self::MAX_SOURCE_ROLES,
                    actual: roles.len(),
                });
            }
            ViewSource::ProvenanceSourceContains(value) => validate_declaration_string(
                value,
                "source.provenance_contains",
                Self::MAX_DECLARATION_STRING_BYTES,
            )?,
            ViewSource::SemanticFieldValue { key, value } => {
                validate_semantic_selector(key, value, "source.semantic")?
            }
            _ => {}
        }
        if self.fields.len() > Self::MAX_PROJECTED_FIELDS {
            return Err(ViewError::ProjectedFieldLimitExceeded {
                limit: Self::MAX_PROJECTED_FIELDS,
                actual: self.fields.len(),
            });
        }
        if self.max_items > Self::MAX_OUTPUT_ITEMS {
            return Err(ViewError::OutputItemLimitExceeded {
                limit: Self::MAX_OUTPUT_ITEMS,
                actual: self.max_items,
            });
        }
        if self.max_bytes > Self::MAX_OUTPUT_BYTES {
            return Err(ViewError::OutputByteLimitExceeded {
                limit: Self::MAX_OUTPUT_BYTES,
                actual: self.max_bytes,
            });
        }
        if let ViewSort::SemanticFieldThenNodeId(key) = &self.sort {
            validate_semantic_key(key, "sort.semantic_key")?;
        }
        if let ViewGrouping::SemanticField(key) = &self.grouping {
            validate_semantic_key(key, "group.semantic_key")?;
        }
        for field in &self.fields {
            if let ViewField::SemanticField(key) = field {
                validate_semantic_key(key, "projection.semantic_key")?;
            }
        }
        let mut clauses = 0;
        validate_filter(&self.filter, 1, &mut clauses)
    }
}

/// A rejected persistent View declaration. Rejections occur before selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ViewError {
    SourceNodeIdLimitExceeded {
        limit: usize,
        actual: usize,
    },
    SourceRoleLimitExceeded {
        limit: usize,
        actual: usize,
    },
    ProjectedFieldLimitExceeded {
        limit: usize,
        actual: usize,
    },
    FilterDepthLimitExceeded {
        limit: usize,
        actual: usize,
    },
    FilterClauseLimitExceeded {
        limit: usize,
        actual: usize,
    },
    OutputItemLimitExceeded {
        limit: usize,
        actual: usize,
    },
    OutputByteLimitExceeded {
        limit: usize,
        actual: usize,
    },
    InvalidDeclarationString {
        field: &'static str,
        limit: usize,
        actual: usize,
    },
}

impl fmt::Display for ViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceNodeIdLimitExceeded { limit, actual } => {
                write!(f, "view source node ID limit {limit} exceeded: {actual}")
            }
            Self::SourceRoleLimitExceeded { limit, actual } => {
                write!(f, "view source role limit {limit} exceeded: {actual}")
            }
            Self::ProjectedFieldLimitExceeded { limit, actual } => {
                write!(f, "view projected field limit {limit} exceeded: {actual}")
            }
            Self::FilterDepthLimitExceeded { limit, actual } => {
                write!(f, "view filter depth limit {limit} exceeded: {actual}")
            }
            Self::FilterClauseLimitExceeded { limit, actual } => {
                write!(f, "view filter clause limit {limit} exceeded: {actual}")
            }
            Self::OutputItemLimitExceeded { limit, actual } => {
                write!(f, "view output item limit {limit} exceeded: {actual}")
            }
            Self::OutputByteLimitExceeded { limit, actual } => {
                write!(f, "view output byte limit {limit} exceeded: {actual}")
            }
            Self::InvalidDeclarationString {
                field,
                limit,
                actual,
            } => write!(
                f,
                "view declaration {field} must be nonblank and at most {limit} UTF-8 bytes: {actual}"
            ),
        }
    }
}

impl std::error::Error for ViewError {}

/// A projected field with its stable wire name and value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedField {
    pub field: ViewField,
    pub name: String,
    pub value: String,
}

/// One selected graph node, retaining its source evidence verbatim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewItem {
    pub id: NodeId,
    pub group: Option<String>,
    pub provenance: Provenance,
    pub fields: Vec<ProjectedField>,
    rendered_bytes: usize,
}

impl ViewItem {
    /// Returns the canonical byte contribution used by this view's byte limit.
    pub fn rendered_bytes(&self) -> usize {
        self.rendered_bytes
    }
}

/// Immutable grouped output shared by every renderer projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledView {
    spec: ViewSpec,
    items: Vec<ViewItem>,
    groups: BTreeMap<String, Vec<NodeId>>,
    selected_bytes: usize,
    truncated: bool,
}

impl CompiledView {
    pub fn spec(&self) -> &ViewSpec {
        &self.spec
    }

    pub fn items(&self) -> &[ViewItem] {
        &self.items
    }

    pub fn groups(&self) -> &BTreeMap<String, Vec<NodeId>> {
        &self.groups
    }

    pub fn selected_bytes(&self) -> usize {
        self.selected_bytes
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Projects the same already-selected item identities for a renderer.
    pub fn render_model(&self, presentation: Presentation) -> RenderModel {
        RenderModel {
            presentation,
            node_ids: self.items.iter().map(|item| item.id.clone()).collect(),
            items: self.items.clone(),
            groups: self.groups.clone(),
            selected_bytes: self.selected_bytes,
            truncated: self.truncated,
        }
    }

    /// Projects using the preference persisted by the view declaration.
    pub fn preferred_render_model(&self) -> RenderModel {
        self.render_model(self.spec.presentation)
    }
}

/// Renderer-neutral model derived only from one [`CompiledView`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderModel {
    pub presentation: Presentation,
    pub node_ids: Vec<NodeId>,
    pub items: Vec<ViewItem>,
    pub groups: BTreeMap<String, Vec<NodeId>>,
    pub selected_bytes: usize,
    pub truncated: bool,
}

/// Stateless compiler for deterministic graph views.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ViewCompiler;

impl ViewCompiler {
    pub fn new() -> Self {
        Self
    }

    /// Compiles selection, ordering, grouping, projection, and explicit bounds.
    ///
    /// Candidates are filtered before one stable `O(k log k)` sort. Group keys
    /// use a `BTreeMap`, while item order continues to follow the requested
    /// sort rather than being reordered by group name.
    pub fn compile(
        &self,
        spec: &ViewSpec,
        graph: &KnowledgeGraph,
    ) -> Result<CompiledView, ViewError> {
        spec.validate()?;
        let wanted_ids = match &spec.source {
            ViewSource::NodeIds(ids) => Some(ids.iter().cloned().collect::<BTreeSet<_>>()),
            _ => None,
        };
        let mut candidates = graph
            .node_ids()
            .into_iter()
            .filter_map(|id| graph.node(&id))
            .filter(|node| source_matches(&spec.source, wanted_ids.as_ref(), node))
            .filter(|node| filter_matches(&spec.filter, node))
            .collect::<Vec<_>>();
        stable_sort(&mut candidates, &spec.sort);

        let mut items = Vec::new();
        let mut groups = BTreeMap::new();
        let mut selected_bytes = 0usize;
        let mut truncated = false;
        for node in candidates {
            let key = group_key(&spec.grouping, node);
            let item = project_item(node, group_value(&spec.grouping, &key), &spec.fields);
            let Some(total) = selected_bytes.checked_add(item.rendered_bytes()) else {
                truncated = true;
                break;
            };
            if items.len() == spec.max_items || total > spec.max_bytes {
                truncated = true;
                break;
            }
            groups
                .entry(key)
                .or_insert_with(Vec::new)
                .push(item.id.clone());
            selected_bytes = total;
            items.push(item);
        }

        Ok(CompiledView {
            spec: spec.clone(),
            items,
            groups,
            selected_bytes,
            truncated,
        })
    }
}

fn source_matches(
    source: &ViewSource,
    wanted_ids: Option<&BTreeSet<NodeId>>,
    node: &crate::graph::NodeInput,
) -> bool {
    match source {
        ViewSource::All => true,
        ViewSource::NodeIds(_) => wanted_ids.is_some_and(|ids| ids.contains(node.id())),
        ViewSource::Roles(roles) => roles.contains(&node.role()),
        ViewSource::ProvenanceSourceContains(value) => node.provenance().source().contains(value),
        ViewSource::SemanticFieldValue { key, value } => node.facts().contains_value(key, value),
    }
}

fn filter_matches(filter: &ViewFilter, node: &crate::graph::NodeInput) -> bool {
    match filter {
        ViewFilter::All => true,
        ViewFilter::Role(role) => node.role() == *role,
        ViewFilter::IdPrefix(value) => node.id().as_str().starts_with(value),
        ViewFilter::ProvenanceSourceContains(value) => node.provenance().source().contains(value),
        ViewFilter::ProvenanceLocatorContains(value) => node.provenance().locator().contains(value),
        ViewFilter::MinimumConfidence(value) => node.confidence().value() >= *value,
        ViewFilter::SemanticFieldValue { key, value } => node.facts().contains_value(key, value),
        ViewFilter::AllOf(filters) => filters.iter().all(|filter| filter_matches(filter, node)),
        ViewFilter::AnyOf(filters) => filters.iter().any(|filter| filter_matches(filter, node)),
        ViewFilter::Not(filter) => !filter_matches(filter, node),
    }
}

fn stable_sort(nodes: &mut [&crate::graph::NodeInput], sort: &ViewSort) {
    match sort {
        ViewSort::NodeId => nodes.sort_by_key(|node| node.id().clone()),
        ViewSort::RoleThenNodeId => {
            nodes.sort_by_key(|node| (role_name(node.role()), node.id().clone()))
        }
        ViewSort::ProvenanceSourceThenNodeId => {
            nodes.sort_by_key(|node| (node.provenance().source().to_owned(), node.id().clone()))
        }
        ViewSort::ConfidenceDescendingThenNodeId => {
            nodes.sort_by_key(|node| (Reverse(node.confidence().value()), node.id().clone()))
        }
        ViewSort::SemanticFieldThenNodeId(key) => nodes.sort_by_key(|node| {
            (
                node.facts()
                    .values(key)
                    .and_then(|values| values.first())
                    .cloned(),
                node.id().clone(),
            )
        }),
    }
}

fn group_key(grouping: &ViewGrouping, node: &crate::graph::NodeInput) -> String {
    match grouping {
        ViewGrouping::None => String::new(),
        ViewGrouping::Role => role_name(node.role()).to_owned(),
        ViewGrouping::ProvenanceSource => node.provenance().source().to_owned(),
        ViewGrouping::SemanticField(key) => node
            .facts()
            .values(key)
            .and_then(|values| values.first())
            .cloned()
            .unwrap_or_default(),
    }
}

fn group_value(grouping: &ViewGrouping, key: &str) -> Option<String> {
    match grouping {
        ViewGrouping::None => None,
        ViewGrouping::Role | ViewGrouping::ProvenanceSource | ViewGrouping::SemanticField(_) => {
            Some(key.to_owned())
        }
    }
}

fn project_item(
    node: &crate::graph::NodeInput,
    group: Option<String>,
    fields: &[ViewField],
) -> ViewItem {
    let fields = fields
        .iter()
        .map(|field| ProjectedField {
            field: field.clone(),
            name: field_name(field),
            value: field_value(field, node),
        })
        .collect::<Vec<_>>();
    let rendered_bytes =
        canonical_item_bytes(node.id(), group.as_deref(), node.provenance(), &fields);
    ViewItem {
        id: node.id().clone(),
        group,
        provenance: node.provenance().clone(),
        fields,
        rendered_bytes,
    }
}

fn canonical_item_bytes(
    id: &NodeId,
    group: Option<&str>,
    provenance: &Provenance,
    fields: &[ProjectedField],
) -> usize {
    canonical_item_utf8(id, group, provenance, fields).len()
}

/// Canonical portable item bytes are UTF-8 records of the form
/// `label:byte_length:value\n`, in this fixed component order: ID, group,
/// provenance source/locator, then projected field name/value pairs. A missing
/// group is `g:-\n`; all present values are length-delimited. This is
/// independent of renderer, host, and Rust debug formatting.
fn canonical_item_utf8(
    id: &NodeId,
    group: Option<&str>,
    provenance: &Provenance,
    fields: &[ProjectedField],
) -> String {
    let mut output = String::new();
    append_canonical_component(&mut output, "i", id.as_str());
    if let Some(group) = group {
        append_canonical_component(&mut output, "g", group);
    } else {
        output.push_str("g:-\n");
    }
    append_canonical_component(&mut output, "s", provenance.source());
    append_canonical_component(&mut output, "l", provenance.locator());
    for field in fields {
        append_canonical_component(&mut output, "n", &field.name);
        append_canonical_component(&mut output, "v", &field.value);
    }
    output
}

fn append_canonical_component(output: &mut String, label: &str, value: &str) {
    output.push_str(label);
    output.push(':');
    output.push_str(&value.len().to_string());
    output.push(':');
    output.push_str(value);
    output.push('\n');
}

fn field_name(field: &ViewField) -> String {
    match field {
        ViewField::NodeId => "node_id".to_owned(),
        ViewField::Role => "role".to_owned(),
        ViewField::ProvenanceSource => "provenance_source".to_owned(),
        ViewField::ProvenanceLocator => "provenance_locator".to_owned(),
        ViewField::Confidence => "confidence".to_owned(),
        ViewField::SemanticField(key) => key.clone(),
    }
}

fn field_value(field: &ViewField, node: &crate::graph::NodeInput) -> String {
    match field {
        ViewField::NodeId => node.id().as_str().to_owned(),
        ViewField::Role => role_name(node.role()).to_owned(),
        ViewField::ProvenanceSource => node.provenance().source().to_owned(),
        ViewField::ProvenanceLocator => node.provenance().locator().to_owned(),
        ViewField::Confidence => node.confidence().value().to_string(),
        ViewField::SemanticField(key) => node
            .facts()
            .values(key)
            .map(|values| values.join(", "))
            .unwrap_or_default(),
    }
}

fn validate_filter(
    filter: &ViewFilter,
    depth: usize,
    clauses: &mut usize,
) -> Result<(), ViewError> {
    if depth > ViewSpec::MAX_FILTER_DEPTH {
        return Err(ViewError::FilterDepthLimitExceeded {
            limit: ViewSpec::MAX_FILTER_DEPTH,
            actual: depth,
        });
    }
    *clauses = clauses.saturating_add(1);
    if *clauses > ViewSpec::MAX_FILTER_CLAUSES {
        return Err(ViewError::FilterClauseLimitExceeded {
            limit: ViewSpec::MAX_FILTER_CLAUSES,
            actual: *clauses,
        });
    }
    match filter {
        ViewFilter::IdPrefix(value) => validate_declaration_string(
            value,
            "filter.id_prefix",
            ViewSpec::MAX_DECLARATION_STRING_BYTES,
        )?,
        ViewFilter::ProvenanceSourceContains(value) => validate_declaration_string(
            value,
            "filter.provenance_source_contains",
            ViewSpec::MAX_DECLARATION_STRING_BYTES,
        )?,
        ViewFilter::ProvenanceLocatorContains(value) => validate_declaration_string(
            value,
            "filter.provenance_locator_contains",
            ViewSpec::MAX_DECLARATION_STRING_BYTES,
        )?,
        ViewFilter::SemanticFieldValue { key, value } => {
            validate_semantic_selector(key, value, "filter.semantic")?
        }
        ViewFilter::AllOf(filters) | ViewFilter::AnyOf(filters) => {
            for filter in filters {
                validate_filter(filter, depth + 1, clauses)?;
            }
        }
        ViewFilter::Not(filter) => validate_filter(filter, depth + 1, clauses)?,
        _ => {}
    }
    Ok(())
}

fn validate_semantic_selector(
    key: &str,
    value: &str,
    field: &'static str,
) -> Result<(), ViewError> {
    validate_semantic_key(key, field)?;
    validate_declaration_string(value, field, crate::graph::NodeFacts::MAX_VALUE_BYTES)
}

fn validate_semantic_key(key: &str, field: &'static str) -> Result<(), ViewError> {
    validate_declaration_string(key, field, crate::graph::NodeFacts::MAX_FIELD_KEY_BYTES)
}

fn validate_declaration_string(
    value: &str,
    field: &'static str,
    limit: usize,
) -> Result<(), ViewError> {
    if value.trim().is_empty() || value.len() > limit {
        return Err(ViewError::InvalidDeclarationString {
            field,
            limit,
            actual: value.len(),
        });
    }
    Ok(())
}

fn role_name(role: NodeRole) -> &'static str {
    match role {
        NodeRole::Document => "document",
        NodeRole::Section => "section",
        NodeRole::Symbol => "symbol",
        NodeRole::External => "external",
        NodeRole::Entrypoint => "entrypoint",
        NodeRole::Test => "test",
        NodeRole::Generated => "generated",
        NodeRole::Archived => "archived",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Confidence, GraphInput, GraphLimits, NodeFacts, NodeInput};

    #[test]
    fn p2_view() {
        let public_fixture = include_str!("../../../../fixtures/views/compiled-view-v1.json");
        assert!(public_fixture.contains("\"schema_version\": \"1.0.0\""));
        assert!(public_fixture.contains("\"id\": \"file:src/lib.rs\""));
        assert!(public_fixture.contains("\"key\": \"subsystem\""));
        assert!(public_fixture.contains("\"canonical_item_bytes\": 151"));
        assert!(public_fixture.contains("\"selected_bytes\": 151"));
        assert!(public_fixture.contains("\"truncated\": true"));
        let graph = KnowledgeGraph::build(
            GraphInput::new(
                vec![
                    NodeInput::new(
                        NodeId::parse("node.alpha").unwrap(),
                        NodeRole::Document,
                        Provenance::new("docs", "README.md").unwrap(),
                        Confidence::new(80).unwrap(),
                    )
                    .with_facts(
                        NodeFacts::default()
                            .with_field_value("status", "active")
                            .unwrap()
                            .with_field_value("tags", "reference")
                            .unwrap()
                            .with_field_value("freshness", "current")
                            .unwrap()
                            .with_field_value("subsystem", "graph")
                            .unwrap()
                            .with_field_value("purpose", "topology")
                            .unwrap()
                            .with_field_value("task", "analysis")
                            .unwrap()
                            .with_field_value("audience", "engineers")
                            .unwrap(),
                    ),
                    NodeInput::new(
                        NodeId::parse("node.beta").unwrap(),
                        NodeRole::Document,
                        Provenance::new("docs", "guide.md").unwrap(),
                        Confidence::new(80).unwrap(),
                    )
                    .with_facts(
                        NodeFacts::default()
                            .with_status("active")
                            .unwrap()
                            .with_tag("reference")
                            .unwrap()
                            .with_freshness("current")
                            .unwrap()
                            .with_subsystem("graph")
                            .unwrap()
                            .with_purpose("guide")
                            .unwrap()
                            .with_task("onboarding")
                            .unwrap()
                            .with_audience("engineers")
                            .unwrap(),
                    ),
                    NodeInput::new(
                        NodeId::parse("node.gamma").unwrap(),
                        NodeRole::Symbol,
                        Provenance::new("code", "src/lib.rs").unwrap(),
                        Confidence::new(95).unwrap(),
                    ),
                    NodeInput::new(
                        NodeId::parse("node.delta").unwrap(),
                        NodeRole::Symbol,
                        Provenance::new("code", "src/view.rs").unwrap(),
                        Confidence::new(80).unwrap(),
                    )
                    .with_facts(
                        NodeFacts::default()
                            .with_status("active")
                            .unwrap()
                            .with_tag("reference")
                            .unwrap()
                            .with_freshness("current")
                            .unwrap()
                            .with_purpose("routing")
                            .unwrap()
                            .with_task("dispatch")
                            .unwrap()
                            .with_audience("engineers")
                            .unwrap(),
                    ),
                ],
                vec![],
            ),
            GraphLimits::new(4, 1).unwrap(),
        )
        .unwrap();
        let spec = ViewSpec {
            source: ViewSource::Roles(vec![NodeRole::Document]),
            filter: ViewFilter::AllOf(vec![
                ViewFilter::ProvenanceSourceContains("docs".to_owned()),
                ViewFilter::MinimumConfidence(80),
            ]),
            sort: ViewSort::ConfidenceDescendingThenNodeId,
            grouping: ViewGrouping::ProvenanceSource,
            fields: vec![ViewField::NodeId, ViewField::ProvenanceLocator],
            presentation: Presentation::Markdown,
            max_items: 2,
            max_bytes: 256,
        };
        let compiler = ViewCompiler::new();
        let compiled = compiler.compile(&spec, &graph).unwrap();
        let replay = compiler.compile(&spec, &graph).unwrap();
        let markdown = compiled.preferred_render_model();
        let json = compiled.render_model(Presentation::Json);
        let terminal = compiled.render_model(Presentation::Terminal);
        let item_bound = compiler
            .compile(
                &ViewSpec {
                    max_items: 1,
                    ..spec.clone()
                },
                &graph,
            )
            .unwrap();
        let byte_bound = compiler
            .compile(
                &ViewSpec {
                    max_bytes: compiled.items()[0].rendered_bytes(),
                    ..spec.clone()
                },
                &graph,
            )
            .unwrap();
        let byte_cutoff = compiler
            .compile(
                &ViewSpec {
                    max_bytes: compiled.items()[0].rendered_bytes() - 1,
                    ..spec.clone()
                },
                &graph,
            )
            .unwrap();
        let zero_bound = compiler
            .compile(
                &ViewSpec {
                    max_items: 0,
                    ..spec
                },
                &graph,
            )
            .unwrap();
        let semantic = compiler
            .compile(
                &ViewSpec {
                    source: ViewSource::SemanticFieldValue {
                        key: "status".to_owned(),
                        value: "active".to_owned(),
                    },
                    filter: ViewFilter::AllOf(vec![
                        ViewFilter::SemanticFieldValue {
                            key: "tags".to_owned(),
                            value: "reference".to_owned(),
                        },
                        ViewFilter::SemanticFieldValue {
                            key: "freshness".to_owned(),
                            value: "current".to_owned(),
                        },
                        ViewFilter::SemanticFieldValue {
                            key: "audience".to_owned(),
                            value: "engineers".to_owned(),
                        },
                    ]),
                    sort: ViewSort::SemanticFieldThenNodeId("subsystem".to_owned()),
                    grouping: ViewGrouping::SemanticField("purpose".to_owned()),
                    fields: vec![
                        ViewField::SemanticField("task".to_owned()),
                        ViewField::SemanticField("audience".to_owned()),
                    ],
                    presentation: Presentation::Obsidian,
                    max_items: 3,
                    max_bytes: 1_024,
                },
                &graph,
            )
            .unwrap();
        let depth_1 = ViewFilter::Not(Box::new(ViewFilter::All));
        let depth_2 = ViewFilter::Not(Box::new(depth_1));
        let depth_3 = ViewFilter::Not(Box::new(depth_2));
        let depth_4 = ViewFilter::Not(Box::new(depth_3));
        let depth_5 = ViewFilter::Not(Box::new(depth_4));
        let depth_6 = ViewFilter::Not(Box::new(depth_5));
        let depth_7 = ViewFilter::Not(Box::new(depth_6));
        let depth_8 = ViewFilter::Not(Box::new(depth_7));
        let depth_9 = ViewFilter::Not(Box::new(depth_8));
        let depth_10 = ViewFilter::Not(Box::new(depth_9));
        let depth_11 = ViewFilter::Not(Box::new(depth_10));
        let depth_12 = ViewFilter::Not(Box::new(depth_11));
        let depth_13 = ViewFilter::Not(Box::new(depth_12));
        let depth_14 = ViewFilter::Not(Box::new(depth_13));
        let depth_15 = ViewFilter::Not(Box::new(depth_14));
        let depth_16 = ViewFilter::Not(Box::new(depth_15));
        let depth_17 = ViewFilter::Not(Box::new(depth_16));
        let recursive_limit = compiler.compile(
            &ViewSpec {
                filter: depth_17,
                ..semantic.spec().clone()
            },
            &graph,
        );
        let source_id_limit = compiler.compile(
            &ViewSpec {
                source: ViewSource::NodeIds(vec![
                    NodeId::parse("node.alpha").unwrap();
                    ViewSpec::MAX_SOURCE_NODE_IDS + 1
                ]),
                ..semantic.spec().clone()
            },
            &graph,
        );
        let source_role_limit = compiler.compile(
            &ViewSpec {
                source: ViewSource::Roles(vec![NodeRole::Document; ViewSpec::MAX_SOURCE_ROLES + 1]),
                ..semantic.spec().clone()
            },
            &graph,
        );
        let field_limit = compiler.compile(
            &ViewSpec {
                fields: vec![ViewField::NodeId; ViewSpec::MAX_PROJECTED_FIELDS + 1],
                ..semantic.spec().clone()
            },
            &graph,
        );
        let output_item_limit = compiler.compile(
            &ViewSpec {
                max_items: ViewSpec::MAX_OUTPUT_ITEMS + 1,
                ..semantic.spec().clone()
            },
            &graph,
        );
        let output_byte_limit = compiler.compile(
            &ViewSpec {
                max_bytes: ViewSpec::MAX_OUTPUT_BYTES + 1,
                ..semantic.spec().clone()
            },
            &graph,
        );
        let declaration_string_limit = compiler.compile(
            &ViewSpec {
                source: ViewSource::ProvenanceSourceContains(
                    "x".repeat(ViewSpec::MAX_DECLARATION_STRING_BYTES + 1),
                ),
                ..semantic.spec().clone()
            },
            &graph,
        );
        let semantic_key_limit = compiler.compile(
            &ViewSpec {
                fields: vec![ViewField::SemanticField(
                    "x".repeat(NodeFacts::MAX_FIELD_KEY_BYTES + 1),
                )],
                ..semantic.spec().clone()
            },
            &graph,
        );

        assert_eq!(compiled.items().len(), 2);
        assert_eq!(compiled.items()[0].id.as_str(), "node.alpha");
        assert_eq!(compiled.items()[1].id.as_str(), "node.beta");
        assert_eq!(compiled.groups().get("docs").unwrap().len(), 2);
        assert_eq!(compiled.items()[0].fields[0].name, "node_id");
        assert_eq!(compiled.items()[0].fields[1].value, "README.md");
        assert_eq!(markdown.presentation, Presentation::Markdown);
        assert_eq!(markdown.node_ids, json.node_ids);
        assert_eq!(json.node_ids, terminal.node_ids);
        assert_eq!(compiled, replay);
        assert_eq!(item_bound.items().len(), 1);
        assert!(item_bound.truncated());
        assert_eq!(byte_bound.items().len(), 1);
        assert!(byte_bound.truncated());
        assert!(byte_cutoff.items().is_empty());
        assert_eq!(byte_cutoff.selected_bytes(), 0);
        assert!(byte_cutoff.truncated());
        assert!(zero_bound.items().is_empty());
        assert!(zero_bound.truncated());
        assert!(compiled.selected_bytes() <= 256);
        assert_eq!(semantic.items()[0].id.as_str(), "node.delta");
        assert_eq!(semantic.items()[1].id.as_str(), "node.alpha");
        assert_eq!(semantic.items()[2].id.as_str(), "node.beta");
        assert_eq!(semantic.items()[1].group.as_deref(), Some("topology"));
        assert_eq!(semantic.items()[1].fields[0].name, "task");
        assert_eq!(semantic.items()[1].fields[0].value, "analysis");
        assert_eq!(semantic.items()[1].fields[1].value, "engineers");
        assert_eq!(
            semantic.render_model(Presentation::Markdown).node_ids,
            semantic.render_model(Presentation::Json).node_ids
        );
        assert_eq!(
            semantic.render_model(Presentation::Json).node_ids,
            semantic.render_model(Presentation::Obsidian).node_ids
        );
        assert!(matches!(
            recursive_limit,
            Err(ViewError::FilterDepthLimitExceeded { .. })
        ));
        assert!(matches!(
            source_id_limit,
            Err(ViewError::SourceNodeIdLimitExceeded { .. })
        ));
        assert!(matches!(
            source_role_limit,
            Err(ViewError::SourceRoleLimitExceeded { .. })
        ));
        assert!(matches!(
            field_limit,
            Err(ViewError::ProjectedFieldLimitExceeded { .. })
        ));
        assert!(matches!(
            output_item_limit,
            Err(ViewError::OutputItemLimitExceeded { .. })
        ));
        assert!(matches!(
            output_byte_limit,
            Err(ViewError::OutputByteLimitExceeded { .. })
        ));
        assert!(matches!(
            declaration_string_limit,
            Err(ViewError::InvalidDeclarationString { .. })
        ));
        assert!(matches!(
            semantic_key_limit,
            Err(ViewError::InvalidDeclarationString { .. })
        ));
    }
}
