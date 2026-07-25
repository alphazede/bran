use super::{RepositoryScanner, ScanChange, ScanFailure, ScanSnapshot};
use crate::agent::result_store::ResultId;
use crate::graph::{
    GraphBuildFailureKind, GraphBuilder, Provenance, StructuralAuthority, StructuralEdge,
    StructuralEdgeCertainty, StructuralGeneratedStatus, StructuralGraphCandidate,
    StructuralGraphSnapshot, StructuralLifecycle, StructuralNode,
};
use crate::metadata::{MetadataParserRegistry, StructuralParseResult, StructuralParseStatus};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Stable change meaning emitted before candidate graph construction.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum IndexDeltaKind {
    Create,
    Change,
    Delete,
    Rename,
}

/// One ordered repository delta. Rename records carry both paths.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IndexDelta {
    kind: IndexDeltaKind,
    old_path: Option<String>,
    new_path: Option<String>,
}

impl IndexDelta {
    pub const fn kind(&self) -> IndexDeltaKind {
        self.kind
    }
    pub fn old_path(&self) -> Option<&str> {
        self.old_path.as_deref()
    }
    pub fn new_path(&self) -> Option<&str> {
        self.new_path.as_deref()
    }
}

/// Equal-byte rename evidence that was not unique enough to collapse.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RenameAmbiguity {
    content_digest: String,
    removed_paths: Vec<String>,
    added_paths: Vec<String>,
}

impl RenameAmbiguity {
    pub fn content_digest(&self) -> &str {
        &self.content_digest
    }
    pub fn removed_paths(&self) -> &[String] {
        &self.removed_paths
    }
    pub fn added_paths(&self) -> &[String] {
        &self.added_paths
    }
}

/// Exact deterministic identity of one refresh request and observed scan input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshInputIdentity {
    prior_digest: String,
    changed_paths: Vec<String>,
    scan_input_digest: String,
}

impl RefreshInputIdentity {
    pub fn prior_digest(&self) -> &str {
        &self.prior_digest
    }
    pub fn changed_paths(&self) -> &[String] {
        &self.changed_paths
    }
    pub fn scan_input_digest(&self) -> &str {
        &self.scan_input_digest
    }
}

/// Candidate validation outcome retained in both success and failure receipts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexValidationState {
    NotRun,
    Passed,
    Failed,
}

/// Deterministic publication receipt kept outside portable snapshot v1.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexReceipt {
    prior_digest: String,
    candidate_digest: Option<String>,
    published_digest: String,
    deltas: Vec<IndexDelta>,
    reused_paths: Vec<String>,
    rename_ambiguities: Vec<RenameAmbiguity>,
    validation_state: IndexValidationState,
    diagnostics: Vec<String>,
    refresh_input: RefreshInputIdentity,
}

impl IndexReceipt {
    pub fn prior_digest(&self) -> &str {
        &self.prior_digest
    }
    pub fn candidate_digest(&self) -> Option<&str> {
        self.candidate_digest.as_deref()
    }
    pub fn published_digest(&self) -> &str {
        &self.published_digest
    }
    pub fn deltas(&self) -> &[IndexDelta] {
        &self.deltas
    }
    pub fn reused_paths(&self) -> &[String] {
        &self.reused_paths
    }
    pub fn rename_ambiguities(&self) -> &[RenameAmbiguity] {
        &self.rename_ambiguities
    }
    pub const fn validation_state(&self) -> IndexValidationState {
        self.validation_state
    }
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
    pub fn refresh_input(&self) -> &RefreshInputIdentity {
        &self.refresh_input
    }
}

/// Stable failure class for a rejected candidate publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexFailureKind {
    Scan,
    Parse,
    Graph,
    Limit,
    Receipt,
    Validation,
}

/// Failed publication plus its complete deterministic receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexFailure {
    kind: IndexFailureKind,
    receipt: Box<IndexReceipt>,
}

impl IndexFailure {
    fn new(kind: IndexFailureKind, receipt: IndexReceipt) -> Self {
        Self {
            kind,
            receipt: Box::new(receipt),
        }
    }

    pub const fn kind(&self) -> IndexFailureKind {
        self.kind
    }
    pub fn receipt(&self) -> &IndexReceipt {
        &self.receipt
    }
}

/// The sole mutable owner of the published structural snapshot reference.
pub struct IndexTransaction {
    published: Arc<StructuralGraphSnapshot>,
    scan: ScanSnapshot,
    parsed: BTreeMap<String, StructuralParseResult>,
}

impl IndexTransaction {
    /// Starts from a caller-supplied immutable publication and matching scan baseline.
    pub fn new(
        published: Arc<StructuralGraphSnapshot>,
        scan: ScanSnapshot,
        parser: &MetadataParserRegistry,
    ) -> Result<Self, IndexFailure> {
        let mut parsed = BTreeMap::new();
        let diagnostics = parse_paths(&scan, scan.entries.keys(), parser, &mut parsed);
        if !diagnostics.is_empty() {
            let prior = published.snapshot_digest().to_owned();
            let receipt = receipt(
                &prior,
                None,
                &prior,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                IndexValidationState::NotRun,
                diagnostics,
                RefreshInputIdentity {
                    prior_digest: prior.clone(),
                    changed_paths: Vec::new(),
                    scan_input_digest: scan_input_digest(&scan),
                },
            );
            return Err(IndexFailure::new(IndexFailureKind::Parse, receipt));
        }
        Ok(Self {
            published,
            scan,
            parsed,
        })
    }

    /// Returns the currently visible immutable publication.
    pub fn published(&self) -> Arc<StructuralGraphSnapshot> {
        Arc::clone(&self.published)
    }

    /// Scans, reparses affected inputs, validates one complete candidate, then swaps once.
    pub fn refresh(
        &mut self,
        scanner: &RepositoryScanner,
        changed_paths: &BTreeSet<String>,
        parser: &MetadataParserRegistry,
        builder: &GraphBuilder,
        operation_id: &str,
    ) -> Result<IndexReceipt, IndexFailure> {
        let prior = self.published.snapshot_digest().to_owned();
        let requested_paths = changed_paths.iter().cloned().collect::<Vec<_>>();
        let change = match scanner.scan_changed(&self.scan, changed_paths) {
            Ok(change) => change,
            Err(error) => {
                let kind = if matches!(
                    error,
                    ScanFailure::LimitExceeded { .. } | ScanFailure::DepthExceeded { .. }
                ) {
                    IndexFailureKind::Limit
                } else {
                    IndexFailureKind::Scan
                };
                let receipt = receipt(
                    &prior,
                    None,
                    &prior,
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    IndexValidationState::NotRun,
                    vec![format!("scan: {error:?}")],
                    RefreshInputIdentity {
                        prior_digest: prior.clone(),
                        changed_paths: requested_paths,
                        scan_input_digest: scan_input_digest(&self.scan),
                    },
                );
                return Err(IndexFailure::new(kind, receipt));
            }
        };
        let input_digest = scan_input_digest(&change.snapshot);
        let refresh_input = RefreshInputIdentity {
            prior_digest: prior.clone(),
            changed_paths: requested_paths,
            scan_input_digest: input_digest.clone(),
        };
        let mut candidate_parsed = self.parsed.clone();
        for path in &change.removed {
            candidate_parsed.remove(path);
        }
        let reparsed = change
            .added
            .iter()
            .chain(change.changed.iter())
            .collect::<Vec<_>>();
        let parse_diagnostics =
            parse_paths(&change.snapshot, reparsed, parser, &mut candidate_parsed);
        let (deltas, rename_ambiguities) = classify_deltas(
            &change,
            &self.scan,
            &change.snapshot,
            &self.parsed,
            &candidate_parsed,
        );
        if !parse_diagnostics.is_empty() {
            let receipt = receipt(
                &prior,
                None,
                &prior,
                deltas,
                change.reused,
                rename_ambiguities,
                IndexValidationState::NotRun,
                parse_diagnostics,
                refresh_input,
            );
            return Err(IndexFailure::new(IndexFailureKind::Parse, receipt));
        }
        let candidate = match graph_candidate(&change.snapshot, &candidate_parsed, input_digest) {
            Ok(candidate) => candidate,
            Err(diagnostic) => {
                let receipt = receipt(
                    &prior,
                    None,
                    &prior,
                    deltas,
                    change.reused,
                    rename_ambiguities,
                    IndexValidationState::NotRun,
                    vec![diagnostic],
                    refresh_input,
                );
                return Err(IndexFailure::new(IndexFailureKind::Graph, receipt));
            }
        };
        let candidate = match builder.build(candidate, Some(prior.clone()), operation_id) {
            Ok(candidate) => candidate,
            Err(error) => {
                let (kind, state) = match error.kind() {
                    GraphBuildFailureKind::Limit => {
                        (IndexFailureKind::Limit, IndexValidationState::NotRun)
                    }
                    GraphBuildFailureKind::Receipt => {
                        (IndexFailureKind::Receipt, IndexValidationState::NotRun)
                    }
                    GraphBuildFailureKind::Validation => {
                        (IndexFailureKind::Validation, IndexValidationState::Failed)
                    }
                };
                let receipt = receipt(
                    &prior,
                    Some(error.candidate_digest().to_owned()),
                    &prior,
                    deltas,
                    change.reused,
                    rename_ambiguities,
                    state,
                    vec![error.diagnostic().to_owned()],
                    refresh_input,
                );
                return Err(IndexFailure::new(kind, receipt));
            }
        };
        let candidate_digest = candidate.snapshot_digest().to_owned();
        let unchanged = candidate.graph_digest() == self.published.graph_digest()
            && candidate.scan_input_digest() == self.published.scan_input_digest()
            && candidate.frozen_frequency_digest() == self.published.frozen_frequency_digest()
            && candidate.nodes() == self.published.nodes()
            && candidate.edges() == self.published.edges();
        if !unchanged {
            self.published = Arc::new(candidate);
        }
        self.scan = change.snapshot;
        self.parsed = candidate_parsed;
        let published_digest = self.published.snapshot_digest().to_owned();
        Ok(receipt(
            &prior,
            Some(candidate_digest),
            &published_digest,
            deltas,
            change.reused,
            rename_ambiguities,
            IndexValidationState::Passed,
            vec![if unchanged {
                "candidate-identical-publication-retained".to_owned()
            } else {
                "candidate-atomically-published".to_owned()
            }],
            refresh_input,
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn receipt(
    prior_digest: &str,
    candidate_digest: Option<String>,
    published_digest: &str,
    mut deltas: Vec<IndexDelta>,
    mut reused_paths: Vec<String>,
    mut rename_ambiguities: Vec<RenameAmbiguity>,
    validation_state: IndexValidationState,
    mut diagnostics: Vec<String>,
    refresh_input: RefreshInputIdentity,
) -> IndexReceipt {
    deltas.sort_by(|left, right| delta_key(left).cmp(&delta_key(right)));
    reused_paths.sort();
    reused_paths.dedup();
    rename_ambiguities.sort();
    diagnostics.sort();
    diagnostics.dedup();
    IndexReceipt {
        prior_digest: prior_digest.to_owned(),
        candidate_digest,
        published_digest: published_digest.to_owned(),
        deltas,
        reused_paths,
        rename_ambiguities,
        validation_state,
        diagnostics,
        refresh_input,
    }
}

fn delta_key(delta: &IndexDelta) -> (&str, &str, IndexDeltaKind) {
    (
        delta
            .old_path
            .as_deref()
            .or(delta.new_path.as_deref())
            .unwrap_or(""),
        delta.new_path.as_deref().unwrap_or(""),
        delta.kind,
    )
}

fn parse_paths<'a>(
    scan: &ScanSnapshot,
    paths: impl IntoIterator<Item = &'a String>,
    parser: &MetadataParserRegistry,
    parsed: &mut BTreeMap<String, StructuralParseResult>,
) -> Vec<String> {
    let mut diagnostics = Vec::new();
    for path in paths {
        let Some(entry) = scan.entries.get(path) else {
            continue;
        };
        let result = parser.parse_structural(path, &entry.source, None);
        if result.status() == StructuralParseStatus::Rejected {
            diagnostics.extend(
                result
                    .diagnostics()
                    .iter()
                    .map(|diagnostic| format!("parse {path}: {diagnostic}")),
            );
        }
        parsed.insert(path.clone(), result);
    }
    diagnostics
}

fn classify_deltas(
    change: &ScanChange,
    previous: &ScanSnapshot,
    current: &ScanSnapshot,
    previous_parsed: &BTreeMap<String, StructuralParseResult>,
    current_parsed: &BTreeMap<String, StructuralParseResult>,
) -> (Vec<IndexDelta>, Vec<RenameAmbiguity>) {
    let mut eligible = BTreeSet::new();
    for old_path in &change.removed {
        for new_path in &change.added {
            let Some(old) = previous.entries.get(old_path) else {
                continue;
            };
            let Some(new) = current.entries.get(new_path) else {
                continue;
            };
            let same_evidence = previous_parsed
                .get(old_path)
                .zip(current_parsed.get(new_path))
                .is_some_and(|(old, new)| {
                    old.source_type() == new.source_type() && old.provenance() == new.provenance()
                });
            if old.source.as_ref() == new.source.as_ref() && same_evidence {
                eligible.insert((old_path.clone(), new_path.clone()));
            }
        }
    }
    let mut renames = BTreeSet::new();
    let mut ambiguity_by_digest: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)> =
        BTreeMap::new();
    for (old_path, new_path) in &eligible {
        let old_count = eligible.iter().filter(|(old, _)| old == old_path).count();
        let new_count = eligible.iter().filter(|(_, new)| new == new_path).count();
        if old_count == 1 && new_count == 1 {
            renames.insert((old_path.clone(), new_path.clone()));
        } else if let Some(entry) = previous.entries.get(old_path) {
            let group = ambiguity_by_digest
                .entry(sha256(&entry.source))
                .or_default();
            group.0.insert(old_path.clone());
            group.1.insert(new_path.clone());
        }
    }
    let renamed_old: BTreeSet<_> = renames.iter().map(|(old, _)| old.clone()).collect();
    let renamed_new: BTreeSet<_> = renames.iter().map(|(_, new)| new.clone()).collect();
    let mut deltas = Vec::new();
    deltas.extend(
        change
            .added
            .iter()
            .filter(|path| !renamed_new.contains(*path))
            .map(|path| IndexDelta {
                kind: IndexDeltaKind::Create,
                old_path: None,
                new_path: Some(path.clone()),
            }),
    );
    deltas.extend(change.changed.iter().map(|path| IndexDelta {
        kind: IndexDeltaKind::Change,
        old_path: Some(path.clone()),
        new_path: Some(path.clone()),
    }));
    deltas.extend(
        change
            .removed
            .iter()
            .filter(|path| !renamed_old.contains(*path))
            .map(|path| IndexDelta {
                kind: IndexDeltaKind::Delete,
                old_path: Some(path.clone()),
                new_path: None,
            }),
    );
    deltas.extend(renames.into_iter().map(|(old_path, new_path)| IndexDelta {
        kind: IndexDeltaKind::Rename,
        old_path: Some(old_path),
        new_path: Some(new_path),
    }));
    let ambiguities = ambiguity_by_digest
        .into_iter()
        .map(
            |(content_digest, (removed_paths, added_paths))| RenameAmbiguity {
                content_digest,
                removed_paths: removed_paths.into_iter().collect(),
                added_paths: added_paths.into_iter().collect(),
            },
        )
        .collect();
    (deltas, ambiguities)
}

fn graph_candidate(
    scan: &ScanSnapshot,
    parsed: &BTreeMap<String, StructuralParseResult>,
    scan_input_digest: String,
) -> Result<StructuralGraphCandidate, String> {
    let mut nodes = Vec::new();
    for (path, result) in parsed {
        let entry = scan
            .entries
            .get(path)
            .ok_or_else(|| format!("graph: parsed path missing from scan: {path}"))?;
        let source_type = result
            .source_type()
            .ok_or_else(|| format!("graph: accepted source type missing: {path}"))?;
        let domains = fact_values(result, "domain");
        let owner = fact_values(result, "owner").into_iter().next();
        let lifecycle = lifecycle(result, path)?;
        let authority = authority(result, path)?;
        let generated = generated_status(result, path)?;
        nodes.push(
            StructuralNode::new(
                path,
                sha256(&entry.source),
                source_type,
                domains,
                owner,
                lifecycle,
                authority,
                generated,
                vec![node_provenance(path, result)?],
            )
            .map_err(|error| format!("graph node {path}: {error}"))?,
        );
    }
    let node_ids: BTreeMap<_, _> = nodes
        .iter()
        .map(|node| (node.path().to_owned(), node.id().to_owned()))
        .collect();
    let mut edges = Vec::new();
    for (path, result) in parsed {
        let source = node_ids
            .get(path)
            .ok_or_else(|| format!("graph: source node missing: {path}"))?;
        for edge in result.edges() {
            let (target, certainty) = node_ids.get(edge.target()).map_or_else(
                || {
                    (
                        sha256(format!("missing\n{}", edge.target()).as_bytes()),
                        StructuralEdgeCertainty::MissingTarget,
                    )
                },
                |target| (target.clone(), StructuralEdgeCertainty::Known),
            );
            edges.push(
                StructuralEdge::new(
                    edge.kind(),
                    source,
                    target,
                    edge.locator(),
                    vec![Provenance::new("structural-parser-v1", edge.locator())
                        .map_err(|error| format!("graph edge {path}: {error}"))?],
                    certainty,
                )
                .map_err(|error| format!("graph edge {path}: {error}"))?,
            );
        }
    }
    Ok(StructuralGraphCandidate::new(
        scan_input_digest,
        nodes,
        edges,
    ))
}

fn fact_values(result: &StructuralParseResult, key: &str) -> Vec<String> {
    result
        .facts()
        .iter()
        .filter(|fact| fact.key() == key)
        .map(|fact| fact.value().to_owned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn lifecycle(result: &StructuralParseResult, path: &str) -> Result<StructuralLifecycle, String> {
    match fact_values(result, "lifecycle").first().map(String::as_str) {
        None | Some("current") => Ok(StructuralLifecycle::Current),
        Some("superseded") => Ok(StructuralLifecycle::Superseded),
        Some("draft") => Ok(StructuralLifecycle::Draft),
        Some("archive") => Ok(StructuralLifecycle::Archive),
        Some("rejected") => Ok(StructuralLifecycle::Rejected),
        Some(value) => Err(format!("graph {path}: invalid lifecycle {value}")),
    }
}

fn authority(result: &StructuralParseResult, path: &str) -> Result<StructuralAuthority, String> {
    match fact_values(result, "authority").first().map(String::as_str) {
        None | Some("unknown") => Ok(StructuralAuthority::Unknown),
        Some("canonical") => Ok(StructuralAuthority::Canonical),
        Some("approved") => Ok(StructuralAuthority::Approved),
        Some("supporting") => Ok(StructuralAuthority::Supporting),
        Some(value) => Err(format!("graph {path}: invalid authority {value}")),
    }
}

fn generated_status(
    result: &StructuralParseResult,
    path: &str,
) -> Result<StructuralGeneratedStatus, String> {
    let explicit = fact_values(result, "generated_status");
    let value = explicit.first().map(String::as_str).or_else(|| {
        fact_values(result, "type")
            .into_iter()
            .find(|value| value == "generated")
            .map(|_| "generated")
    });
    match value {
        None | Some("source") => Ok(StructuralGeneratedStatus::Source),
        Some("generated") => Ok(StructuralGeneratedStatus::Generated),
        Some("preextracted") => Ok(StructuralGeneratedStatus::Preextracted),
        Some(value) => Err(format!("graph {path}: invalid generated status {value}")),
    }
}

fn node_provenance(path: &str, result: &StructuralParseResult) -> Result<Provenance, String> {
    if let Some(extraction) = result.provenance() {
        Provenance::new(
            "preextracted-text",
            format!(
                "{}:{}@{}:{}",
                extraction.original_artifact_digest(),
                extraction.extractor_identity(),
                extraction.extractor_version(),
                extraction.extraction_digest()
            ),
        )
    } else {
        Provenance::new("repository-scanner", path)
    }
    .map_err(|error| format!("graph provenance {path}: {error}"))
}

fn scan_input_digest(scan: &ScanSnapshot) -> String {
    let mut bytes = format!("inputs:{:?}\n", scan.inputs).into_bytes();
    for (path, entry) in &scan.entries {
        bytes.extend_from_slice(path.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(sha256(&entry.source).as_bytes());
        bytes.push(b'\n');
    }
    sha256(&bytes)
}

fn sha256(bytes: &[u8]) -> String {
    ResultId::sha256(bytes).value().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::StructuralEdgeKind;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bran-p8-index-{}-{}",
                std::process::id(),
                NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, relative: &str, source: &[u8]) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source).unwrap();
        }

        fn remove(&self, relative: &str) {
            fs::remove_file(self.0.join(relative)).unwrap();
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn graph_builder() -> GraphBuilder {
        GraphBuilder::new(128, 256, "0".repeat(64))
    }

    fn parser() -> MetadataParserRegistry {
        MetadataParserRegistry::new(4096)
    }

    fn blank(builder: &GraphBuilder) -> Arc<StructuralGraphSnapshot> {
        Arc::new(
            builder
                .build(
                    StructuralGraphCandidate::new("0".repeat(64), Vec::new(), Vec::new()),
                    None,
                    "blank",
                )
                .unwrap(),
        )
    }

    fn scanner(root: &TestRoot) -> RepositoryScanner {
        RepositoryScanner::new(
            root.path(),
            super::super::ScanConfig::new(128, 4096, 131_072),
        )
        .unwrap()
    }

    fn clean_index(
        root: &TestRoot,
        parser: &MetadataParserRegistry,
        builder: &GraphBuilder,
    ) -> IndexTransaction {
        let scanner = scanner(root);
        let mut index =
            IndexTransaction::new(blank(builder), ScanSnapshot::default(), parser).unwrap();
        index
            .refresh(&scanner, &BTreeSet::new(), parser, builder, "clean")
            .unwrap();
        index
    }

    fn changed_paths() -> BTreeSet<String> {
        BTreeSet::from([
            "docs/amb-new-a.md".to_owned(),
            "docs/amb-new-b.md".to_owned(),
            "docs/amb-old-a.md".to_owned(),
            "docs/amb-old-b.md".to_owned(),
            "docs/change.md".to_owned(),
            "docs/create.md".to_owned(),
            "docs/delete.md".to_owned(),
            "docs/unique-new.md".to_owned(),
            "docs/unique-old.md".to_owned(),
        ])
    }

    fn has_delta(
        receipt: &IndexReceipt,
        kind: IndexDeltaKind,
        old_path: Option<&str>,
        new_path: Option<&str>,
    ) -> bool {
        receipt.deltas().iter().any(|delta| {
            delta.kind() == kind && delta.old_path() == old_path && delta.new_path() == new_path
        })
    }

    fn has_path(snapshot: &StructuralGraphSnapshot, path: &str) -> bool {
        snapshot.nodes().iter().any(|node| node.path() == path)
    }

    fn relationship_count(snapshot: &StructuralGraphSnapshot, kind: StructuralEdgeKind) -> usize {
        snapshot
            .edges()
            .iter()
            .filter(|edge| edge.kind() == kind)
            .count()
    }

    fn graph_content_matches(
        left: &StructuralGraphSnapshot,
        right: &StructuralGraphSnapshot,
    ) -> bool {
        left.graph_digest() == right.graph_digest()
            && left.nodes() == right.nodes()
            && left.edges() == right.edges()
    }

    fn ambiguity_matches(receipt: &IndexReceipt) -> bool {
        receipt.rename_ambiguities()
            == [RenameAmbiguity {
                content_digest: sha256(b"# Shared\n"),
                removed_paths: vec![
                    "docs/amb-old-a.md".to_owned(),
                    "docs/amb-old-b.md".to_owned(),
                ],
                added_paths: vec![
                    "docs/amb-new-a.md".to_owned(),
                    "docs/amb-new-b.md".to_owned(),
                ],
            }]
    }

    fn seed(root: &TestRoot) {
        root.write("docs/keep.md", b"# Keep\n");
        root.write("docs/change.md", b"# Before\n");
        root.write("docs/delete.md", b"# Delete\n");
        root.write("docs/unique-old.md", b"# Unique\n");
        root.write("docs/amb-old-a.md", b"# Shared\n");
        root.write("docs/amb-old-b.md", b"# Shared\n");
        root.write(
            "docs/cycle-a.md",
            b"---\ndependency: docs/cycle-b.md\nimpact: docs/cycle-b.md\n---\n# A\n",
        );
        root.write(
            "docs/cycle-b.md",
            b"---\ndependency: docs/cycle-a.md\nimpact: docs/cycle-a.md\n---\n# B\n",
        );
    }

    fn mutate(root: &TestRoot) {
        root.write("docs/change.md", b"# After\n");
        root.write("docs/create.md", b"# Create\n");
        root.remove("docs/delete.md");
        root.remove("docs/unique-old.md");
        root.write("docs/unique-new.md", b"# Unique\n");
        root.remove("docs/amb-old-a.md");
        root.remove("docs/amb-old-b.md");
        root.write("docs/amb-new-a.md", b"# Shared\n");
        root.write("docs/amb-new-b.md", b"# Shared\n");
    }

    fn add_bad_lineage(root: &TestRoot) {
        root.write(
            "docs/life-a.md",
            b"---\nsupersedes: docs/life-b.md\npredecessor: docs/life-b.md\n---\n# A\n",
        );
        root.write(
            "docs/life-b.md",
            b"---\nsupersedes: docs/life-a.md\npredecessor: docs/life-a.md\n---\n# B\n",
        );
    }

    fn lifecycle_paths() -> BTreeSet<String> {
        BTreeSet::from(["docs/life-a.md".to_owned(), "docs/life-b.md".to_owned()])
    }

    fn authored_paths() -> BTreeSet<String> {
        BTreeSet::from([
            "docs/design.md".to_owned(),
            "docs/life-a.md".to_owned(),
            "docs/life-b.md".to_owned(),
        ])
    }

    #[test]
    fn p8_incremental_publish() {
        let root = TestRoot::new();
        seed(&root);
        let parser = parser();
        let builder = graph_builder();
        let mut index = clean_index(&root, &parser, &builder);
        let prior = index.published();
        mutate(&root);
        let receipt = index
            .refresh(
                &scanner(&root),
                &changed_paths(),
                &parser,
                &builder,
                "incremental",
            )
            .unwrap();
        assert!(has_delta(
            &receipt,
            IndexDeltaKind::Create,
            None,
            Some("docs/create.md")
        ));
        assert!(has_delta(
            &receipt,
            IndexDeltaKind::Change,
            Some("docs/change.md"),
            Some("docs/change.md")
        ));
        assert!(has_delta(
            &receipt,
            IndexDeltaKind::Delete,
            Some("docs/delete.md"),
            None
        ));
        assert!(has_delta(
            &receipt,
            IndexDeltaKind::Rename,
            Some("docs/unique-old.md"),
            Some("docs/unique-new.md")
        ));
        assert!(has_delta(
            &receipt,
            IndexDeltaKind::Delete,
            Some("docs/amb-old-a.md"),
            None
        ));
        assert!(has_delta(
            &receipt,
            IndexDeltaKind::Create,
            None,
            Some("docs/amb-new-a.md")
        ));
        assert!(ambiguity_matches(&receipt));
        assert!(receipt.reused_paths().contains(&"docs/keep.md".to_owned()));
        assert_eq!(receipt.validation_state(), IndexValidationState::Passed);
        assert_ne!(prior.snapshot_digest(), index.published().snapshot_digest());
        assert_eq!(
            receipt.published_digest(),
            index.published().snapshot_digest()
        );
        assert_eq!(
            relationship_count(&index.published(), StructuralEdgeKind::Dependency),
            2
        );
        assert_eq!(
            relationship_count(&index.published(), StructuralEdgeKind::Impact),
            2
        );
        assert!(graph_content_matches(
            &index.published(),
            &clean_index(&root, &parser, &builder).published()
        ));
        add_bad_lineage(&root);
        let retained = index.published();
        let failed = index
            .refresh(
                &scanner(&root),
                &lifecycle_paths(),
                &parser,
                &builder,
                "invalid-lineage",
            )
            .unwrap_err();
        assert_eq!(failed.kind(), IndexFailureKind::Validation);
        assert_eq!(
            failed.receipt().validation_state(),
            IndexValidationState::Failed
        );
        assert!(failed.receipt().candidate_digest().is_some());
        assert!(failed.receipt().diagnostics()[0].contains("structural lineage cycle"));
        assert_eq!(
            retained.snapshot_digest(),
            index.published().snapshot_digest()
        );
        assert_eq!(
            retained.canonical_json(),
            index.published().canonical_json()
        );
        root.write("docs/design.md", b"# Newly Authored Design\n");
        assert!(!has_path(&index.published(), "docs/design.md"));
        root.remove("docs/life-a.md");
        root.remove("docs/life-b.md");
        index
            .refresh(
                &scanner(&root),
                &authored_paths(),
                &parser,
                &builder,
                "explicit-refresh",
            )
            .unwrap();
        assert!(has_path(&index.published(), "docs/design.md"));
        assert!(graph_content_matches(
            &index.published(),
            &clean_index(&root, &parser, &builder).published()
        ));
        let stable = index.published();
        let first = index
            .refresh(
                &scanner(&root),
                &authored_paths(),
                &parser,
                &builder,
                "idempotent",
            )
            .unwrap();
        let second = index
            .refresh(
                &scanner(&root),
                &authored_paths(),
                &parser,
                &builder,
                "idempotent",
            )
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            stable.snapshot_digest(),
            index.published().snapshot_digest()
        );
    }
}
