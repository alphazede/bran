//! Read-only database evidence: typed extraction outcomes, canonical
//! provenance, and entry into packets as ordinary graph evidence.

use super::sql::{gate, project, GatedQuery, RelationName, SqlRejection};
use super::{AccessMode, EvidenceSourceDescriptor};
use crate::agent::result_store::ResultId;
use crate::graph::{Confidence, NodeFacts, NodeId, NodeInput, NodeRole, Provenance};
use crate::packet::{EvidenceContent, EvidencePriority, PreservationAnchor};
use crate::schema::write_escaped_string_for_bundle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Provenance source recorded on every database evidence node.
pub const DATABASE_EVIDENCE_SOURCE: &str = "database-evidence";

/// The exactly-one result of an accepted extraction request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceOutcome {
    SchemaOnly,
    BoundedRows,
    Truncated,
    Cancelled,
    Timeout,
    Unavailable,
    StaleSnapshot,
    DlpRejected,
}

impl EvidenceOutcome {
    pub const ALL: [Self; 8] = [
        Self::SchemaOnly,
        Self::BoundedRows,
        Self::Truncated,
        Self::Cancelled,
        Self::Timeout,
        Self::Unavailable,
        Self::StaleSnapshot,
        Self::DlpRejected,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SchemaOnly => "schema-only",
            Self::BoundedRows => "bounded-rows",
            Self::Truncated => "truncated",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::Unavailable => "unavailable",
            Self::StaleSnapshot => "stale-snapshot",
            Self::DlpRejected => "dlp-rejected",
        }
    }

    /// Only these outcomes may become packet evidence.
    pub const fn admitted(self) -> bool {
        matches!(self, Self::SchemaOnly | Self::BoundedRows | Self::Truncated)
    }
}

/// One result cell. Adapters encode other engine types as canonical text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cell {
    Null,
    Integer(i64),
    Text(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelationKind {
    Table,
    View,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnSchema {
    pub name: String,
    pub declared_type: String,
}

/// Catalog evidence for one allowlisted relation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelationSchema {
    pub relation: RelationName,
    pub kind: RelationKind,
    pub columns: Vec<ColumnSchema>,
}

/// Content-free engine failures. Any other engine error is `Unavailable`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineFailure {
    Unavailable,
    Cancelled,
    Timeout,
}

/// Cancellation and the effective deadline. Adapters must stop in-flight
/// engine work when either fires (SQLite: a progress handler); BRAN also
/// rechecks both after every engine call.
pub struct Interrupt<'a> {
    pub cancel: &'a AtomicBool,
    pub deadline: Instant,
}

/// The evidence-source port. An adapter must also enforce read-only access in
/// the engine itself (see `docs/database-boundary.md`).
pub trait EvidenceEngine {
    /// Adapter name and version, for example `sqlite-reference/1`.
    fn adapter(&self) -> &str;
    /// Engine-attested snapshot identity, or `None` when the engine cannot
    /// attest one. Anything other than a `sha256:` digest is sealed as its digest.
    fn snapshot(&mut self) -> Result<Option<String>, EngineFailure>;
    /// Catalog for the allowlisted relations only.
    fn catalog(
        &mut self,
        relations: &[RelationName],
        interrupt: &Interrupt<'_>,
    ) -> Result<Vec<RelationSchema>, EngineFailure>;
    /// Streams rows until exhausted or until `sink` returns `false`.
    fn rows(
        &mut self,
        query: &GatedQuery,
        interrupt: &Interrupt<'_>,
        sink: &mut dyn FnMut(Vec<Cell>) -> bool,
    ) -> Result<(), EngineFailure>;
}

/// A row request. `Sql` may come from an operator or a model; both are gated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RowRequest {
    Projection {
        relation: RelationName,
        columns: Vec<String>,
    },
    Sql(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DlpStatus {
    NotEvaluated,
    Passed,
    Findings,
}

impl DlpStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotEvaluated => "not-evaluated",
            Self::Passed => "passed",
            Self::Findings => "findings",
        }
    }
}

/// One extraction attempt. The effective deadline is the earlier of `deadline`
/// and the descriptor's `timeout_ms`. `extracted_at` is caller-supplied so
/// replays are deterministic.
pub struct ExtractionRequest<'a> {
    pub source: &'a EvidenceSourceDescriptor,
    pub rows: Option<&'a RowRequest>,
    pub cancel: &'a AtomicBool,
    pub deadline: Instant,
    pub extracted_at: &'a str,
    pub dlp: &'a dyn Fn(&str) -> DlpStatus,
}

/// Policy refusals. The engine is never contacted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtractionRefusal {
    RowsNotPermitted,
    Sql(SqlRejection),
    QueryNotAllowlisted,
}

/// Returned when a non-admitted outcome is offered to a packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotAdmitted(pub EvidenceOutcome);

/// The result of one accepted extraction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseEvidence {
    source_id: String,
    outcome: EvidenceOutcome,
    record: String,
    evidence_digest: String,
    result_digest: String,
    rows: Vec<Vec<Cell>>,
    truncated: bool,
}

impl DatabaseEvidence {
    pub const fn outcome(&self) -> EvidenceOutcome {
        self.outcome
    }
    /// Canonical provenance JSON. It never holds rows or credential material.
    pub fn record(&self) -> &str {
        &self.record
    }
    pub fn evidence_digest(&self) -> &str {
        &self.evidence_digest
    }
    pub fn result_digest(&self) -> &str {
        &self.result_digest
    }
    pub fn rows(&self) -> &[Vec<Cell>] {
        &self.rows
    }
    pub const fn truncated(&self) -> bool {
        self.truncated
    }

    /// Converts admitted evidence into packet inputs.
    pub fn packet_evidence(
        &self,
        priority: EvidencePriority,
    ) -> Result<(NodeInput, EvidenceContent), NotAdmitted> {
        if !self.outcome.admitted() {
            return Err(NotAdmitted(self.outcome));
        }
        let hex = &self.evidence_digest["sha256:".len()..];
        let id = NodeId::parse(format!("db:{}:{}", self.source_id, &hex[..32]))
            .expect("source ids and digests are valid node identity characters");
        let provenance = Provenance::new(
            DATABASE_EVIDENCE_SOURCE,
            format!("database:{}#{}", self.source_id, self.evidence_digest),
        )
        .expect("database locators are bounded and non-empty");
        let facts = [
            ("db.outcome", self.outcome.as_str().to_owned()),
            ("db.truncated", self.truncated.to_string()),
            ("db.row_count", self.rows.len().to_string()),
            ("db.result_digest", self.result_digest.clone()),
        ];
        let mut node_facts = NodeFacts::default();
        let mut anchors = Vec::with_capacity(facts.len());
        for (key, value) in facts {
            node_facts = node_facts
                .with_field_value(&key[3..], value.as_str())
                .expect("database facts are bounded");
            anchors
                .push(PreservationAnchor::new(key, value).expect("database anchors are bounded"));
        }
        let node = NodeInput::new(
            id.clone(),
            NodeRole::External,
            provenance,
            Confidence::new(50).expect("50 is a valid confidence"),
        )
        .with_facts(node_facts);
        let content = format!("{}\n{}", self.record, rows_text(&self.rows));
        Ok((
            node,
            EvidenceContent::new(id, content, priority, 0, 0, anchors),
        ))
    }
}

/// Checks policy, then extracts with BRAN-enforced bounds. `engine` is `None`
/// when no adapter for the descriptor's engine is compiled in.
pub fn extract(
    engine: Option<&mut dyn EvidenceEngine>,
    request: &ExtractionRequest<'_>,
) -> Result<DatabaseEvidence, ExtractionRefusal> {
    let source = request.source;
    let query = match request.rows {
        None => None,
        Some(_) if source.access() == AccessMode::Introspection => {
            return Err(ExtractionRefusal::RowsNotPermitted)
        }
        Some(RowRequest::Projection { relation, columns }) => {
            let columns = columns.iter().map(String::as_str).collect::<Vec<_>>();
            Some(project(relation, &columns, source.relations()).map_err(ExtractionRefusal::Sql)?)
        }
        Some(RowRequest::Sql(sql)) => {
            let query = gate(sql, source.relations()).map_err(ExtractionRefusal::Sql)?;
            if !source
                .queries()
                .iter()
                .any(|digest| digest == query.digest())
            {
                return Err(ExtractionRefusal::QueryNotAllowlisted);
            }
            Some(query)
        }
    };
    let mut run = Run::default();
    let outcome = match engine {
        None => EvidenceOutcome::Unavailable,
        Some(engine) => {
            run.adapter = Some(engine.adapter().to_owned());
            collect(engine, request, query.as_ref(), &mut run)
        }
    };
    Ok(finish(request, query.as_ref(), outcome, run))
}

#[derive(Default)]
struct Run {
    adapter: Option<String>,
    snapshot: Option<String>,
    catalog: Vec<RelationSchema>,
    rows: Vec<Vec<Cell>>,
    truncated: bool,
}

fn failure(failure: EngineFailure) -> EvidenceOutcome {
    match failure {
        EngineFailure::Unavailable => EvidenceOutcome::Unavailable,
        EngineFailure::Cancelled => EvidenceOutcome::Cancelled,
        EngineFailure::Timeout => EvidenceOutcome::Timeout,
    }
}

/// Runs the engine under BRAN-enforced cancellation, deadline, and bounds.
/// Interruption is rechecked after every engine call, so a blocking call that
/// overruns or is cancelled never yields admissible evidence.
fn collect(
    engine: &mut dyn EvidenceEngine,
    request: &ExtractionRequest<'_>,
    query: Option<&GatedQuery>,
    run: &mut Run,
) -> EvidenceOutcome {
    let timeout = Duration::from_millis(request.source.limits().timeout_ms);
    let interrupt = Interrupt {
        cancel: request.cancel,
        deadline: Instant::now()
            .checked_add(timeout)
            .map_or(request.deadline, |limit| limit.min(request.deadline)),
    };
    let interrupted = || {
        if interrupt.cancel.load(Ordering::SeqCst) {
            Some(EvidenceOutcome::Cancelled)
        } else if Instant::now() >= interrupt.deadline {
            Some(EvidenceOutcome::Timeout)
        } else {
            None
        }
    };
    if let Some(outcome) = interrupted() {
        return outcome;
    }
    run.snapshot = match engine.snapshot() {
        Ok(snapshot) => snapshot.map(sealed_snapshot),
        Err(error) => return failure(error),
    };
    if let Some(outcome) = interrupted() {
        return outcome;
    }
    if let Some(pinned) = request.source.snapshot() {
        if run.snapshot.as_deref() != Some(pinned) {
            return EvidenceOutcome::StaleSnapshot;
        }
    }
    run.catalog = match engine.catalog(request.source.relations(), &interrupt) {
        Ok(catalog) => catalog,
        Err(error) => return failure(error),
    };
    if let Some(outcome) = interrupted() {
        return outcome;
    }
    run.catalog
        .sort_by(|left, right| left.relation.cmp(&right.relation));
    let outcome = match query {
        None => EvidenceOutcome::SchemaOnly,
        Some(query) => {
            let limits = request.source.limits();
            let (rows, truncated) = (&mut run.rows, &mut run.truncated);
            let mut stopped = None;
            let mut bytes = 0_usize;
            let result = engine.rows(query, &interrupt, &mut |row| {
                if let Some(outcome) = interrupted() {
                    stopped = Some(outcome);
                    return false;
                }
                let size = row_json(&row).len() + 1;
                if rows.len() == limits.max_rows || bytes + size > limits.max_bytes {
                    *truncated = true;
                    return false;
                }
                bytes += size;
                rows.push(row);
                true
            });
            match (stopped.or_else(interrupted), result) {
                (Some(outcome), _) => outcome,
                (None, Err(error)) => failure(error),
                (None, Ok(())) if run.truncated => EvidenceOutcome::Truncated,
                (None, Ok(())) => EvidenceOutcome::BoundedRows,
            }
        }
    };
    if !outcome.admitted() {
        return outcome;
    }
    let after = match engine.snapshot() {
        Ok(snapshot) => snapshot.map(sealed_snapshot),
        Err(error) => return failure(error),
    };
    if let Some(interrupted) = interrupted() {
        return interrupted;
    }
    if after == run.snapshot {
        outcome
    } else {
        EvidenceOutcome::StaleSnapshot
    }
}

/// Engine snapshot text is untrusted: only a `sha256:` digest is sealed.
fn sealed_snapshot(snapshot: String) -> String {
    if super::is_digest(&snapshot) {
        snapshot
    } else {
        digest(&snapshot)
    }
}

/// Applies DLP, drops rows from refused outcomes, and seals the provenance record.
fn finish(
    request: &ExtractionRequest<'_>,
    query: Option<&GatedQuery>,
    mut outcome: EvidenceOutcome,
    mut run: Run,
) -> DatabaseEvidence {
    let source = request.source;
    let mut dlp = DlpStatus::NotEvaluated;
    if outcome.admitted() {
        dlp = (request.dlp)(&format!(
            "{}\n{}",
            catalog_json(&run.catalog),
            rows_text(&run.rows)
        ));
        if dlp == DlpStatus::Findings {
            outcome = EvidenceOutcome::DlpRejected;
        }
    }
    // Refused outcomes keep no engine metadata: an unchecked or DLP-flagged
    // column name must not reach the sealed record.
    if !outcome.admitted() {
        run.catalog.clear();
        run.rows.clear();
        run.truncated = false;
    }
    let catalog = catalog_json(&run.catalog);
    let result_digest = digest(&rows_text(&run.rows));
    let limits = source.limits();
    let mut record = String::from("{\"access\":");
    write_escaped_string_for_bundle(&mut record, source.access().as_str());
    record.push_str(",\"adapter\":");
    write_optional(&mut record, run.adapter.as_deref());
    record.push_str(",\"classification\":\"unavailable\",\"columns\":[");
    let columns = run.catalog.iter().flat_map(|schema| {
        schema
            .columns
            .iter()
            .map(move |column| format!("{}.{}", schema.relation, column.name))
    });
    write_strings(&mut record, columns);
    record.push_str("],\"credential\":");
    write_optional(
        &mut record,
        source.credential().map(ToString::to_string).as_deref(),
    );
    record.push_str(",\"database\":");
    write_escaped_string_for_bundle(&mut record, source.database());
    record.push_str(",\"derivation\":");
    write_escaped_string_for_bundle(
        &mut record,
        query.map_or("schema-only", |query| query.derivation().as_str()),
    );
    record.push_str(",\"dlp\":");
    write_escaped_string_for_bundle(&mut record, dlp.as_str());
    record.push_str(",\"engine\":");
    write_escaped_string_for_bundle(&mut record, source.engine().as_str());
    record.push_str(",\"extracted_at\":");
    write_escaped_string_for_bundle(&mut record, request.extracted_at);
    record.push_str(&format!(
        ",\"limits\":{{\"max_bytes\":{},\"max_rows\":{},\"timeout_ms\":{}}},\"outcome\":",
        limits.max_bytes, limits.max_rows, limits.timeout_ms
    ));
    write_escaped_string_for_bundle(&mut record, outcome.as_str());
    record.push_str(",\"query_digest\":");
    write_optional(&mut record, query.map(GatedQuery::digest));
    record.push_str(",\"redaction\":\"unavailable\",\"relations\":[");
    write_strings(
        &mut record,
        source.relations().iter().map(ToString::to_string),
    );
    record.push_str(&format!(
        "],\"result_digest\":\"{result_digest}\",\"row_count\":{},\"snapshot\":",
        run.rows.len()
    ));
    match run.snapshot.as_deref() {
        Some(value) => {
            record.push_str("{\"state\":\"attested\",\"value\":");
            write_escaped_string_for_bundle(&mut record, value);
            record.push('}');
        }
        None => record.push_str("{\"state\":\"unavailable\",\"value\":null}"),
    }
    record.push_str(&format!(
        ",\"source_digest\":\"{}\",\"source_id\":",
        digest(&catalog)
    ));
    write_escaped_string_for_bundle(&mut record, source.id());
    record.push_str(&format!(",\"truncated\":{}}}", run.truncated));
    DatabaseEvidence {
        source_id: source.id().to_owned(),
        outcome,
        evidence_digest: digest(&record),
        record,
        result_digest,
        rows: run.rows,
        truncated: run.truncated,
    }
}

fn digest(text: &str) -> String {
    ResultId::sha256(text.as_bytes()).to_string()
}

fn write_optional(out: &mut String, value: Option<&str>) {
    match value {
        Some(value) => write_escaped_string_for_bundle(out, value),
        None => out.push_str("null"),
    }
}

fn write_strings(out: &mut String, values: impl Iterator<Item = String>) {
    for (index, value) in values.enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_escaped_string_for_bundle(out, &value);
    }
}

fn row_json(row: &[Cell]) -> String {
    let mut out = String::from("[");
    for (index, cell) in row.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        match cell {
            Cell::Null => out.push_str("null"),
            Cell::Integer(value) => out.push_str(&value.to_string()),
            Cell::Text(value) => write_escaped_string_for_bundle(&mut out, value),
        }
    }
    out.push(']');
    out
}

/// One canonical JSON array per row, each ending in a newline.
fn rows_text(rows: &[Vec<Cell>]) -> String {
    rows.iter().map(|row| row_json(row) + "\n").collect()
}

fn catalog_json(catalog: &[RelationSchema]) -> String {
    let mut out = String::from("[");
    for (index, schema) in catalog.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("{\"columns\":[");
        for (position, column) in schema.columns.iter().enumerate() {
            if position > 0 {
                out.push(',');
            }
            out.push_str("{\"name\":");
            write_escaped_string_for_bundle(&mut out, &column.name);
            out.push_str(",\"type\":");
            write_escaped_string_for_bundle(&mut out, &column.declared_type);
            out.push('}');
        }
        out.push_str("],\"kind\":");
        out.push_str(match schema.kind {
            RelationKind::Table => "\"table\"",
            RelationKind::View => "\"view\"",
        });
        out.push_str(",\"relation\":");
        write_escaped_string_for_bundle(&mut out, &schema.relation.to_string());
        out.push('}');
    }
    out.push(']');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphInput, GraphLimits, KnowledgeGraph};
    use crate::packet::{
        DependencyClosureLimits, PacketAssembler, PacketAssemblyRequest, PacketLimits,
    };
    use crate::view::{
        Presentation, ViewCompiler, ViewField, ViewFilter, ViewGrouping, ViewSort, ViewSource,
        ViewSpec,
    };
    use std::sync::Arc;
    use std::time::Duration;

    const SOURCE: &str = "\
kind: evidence-source
id: synthetic-orders
engine: sqlite
database: data/orders.sqlite
access: bounded-rows
relations: [main.customers, main.orders]
max_rows: 3
max_bytes: 4096
";
    const SNAPSHOT: &str =
        "sha256:1111111111111111111111111111111111111111111111111111111111111111";
    const OTHER_SNAPSHOT: &str =
        "sha256:2222222222222222222222222222222222222222222222222222222222222222";

    /// Synthetic in-memory engine standing in for an adapter in unit tests.
    struct SyntheticEngine {
        snapshots: Vec<Option<String>>,
        rows: Vec<Vec<Cell>>,
        failure: Option<EngineFailure>,
        cancel_after: Option<(usize, Arc<AtomicBool>)>,
        delay: Duration,
        catalog_delay: Duration,
        catalog_cancel: Option<Arc<AtomicBool>>,
        snapshot_cancel: Option<(usize, Arc<AtomicBool>)>,
        extra_column: Option<String>,
        calls: usize,
        last_sql: Option<String>,
    }

    impl SyntheticEngine {
        fn new(row_count: usize) -> Self {
            let statuses = ["paid", "shipped", "void", "pending", "paid"];
            Self {
                snapshots: vec![Some(SNAPSHOT.to_owned())],
                rows: (1..=row_count)
                    .map(|id| {
                        vec![
                            Cell::Integer(id as i64),
                            Cell::Text(statuses[(id - 1) % statuses.len()].to_owned()),
                        ]
                    })
                    .collect(),
                failure: None,
                cancel_after: None,
                delay: Duration::ZERO,
                catalog_delay: Duration::ZERO,
                catalog_cancel: None,
                snapshot_cancel: None,
                extra_column: None,
                calls: 0,
                last_sql: None,
            }
        }
    }

    impl EvidenceEngine for SyntheticEngine {
        fn adapter(&self) -> &str {
            "synthetic/1"
        }
        fn snapshot(&mut self) -> Result<Option<String>, EngineFailure> {
            self.calls += 1;
            if let Some((call, flag)) = &self.snapshot_cancel {
                if self.calls == *call {
                    flag.store(true, Ordering::SeqCst);
                }
            }
            let next = if self.snapshots.len() > 1 {
                self.snapshots.remove(0)
            } else {
                self.snapshots[0].clone()
            };
            Ok(next)
        }
        fn catalog(
            &mut self,
            relations: &[RelationName],
            _interrupt: &Interrupt<'_>,
        ) -> Result<Vec<RelationSchema>, EngineFailure> {
            self.calls += 1;
            std::thread::sleep(self.catalog_delay);
            if let Some(flag) = &self.catalog_cancel {
                flag.store(true, Ordering::SeqCst);
            }
            Ok(relations
                .iter()
                .map(|relation| RelationSchema {
                    relation: relation.clone(),
                    kind: RelationKind::Table,
                    columns: ["id", "status"]
                        .into_iter()
                        .map(str::to_owned)
                        .chain(self.extra_column.clone())
                        .map(|name| ColumnSchema {
                            name,
                            declared_type: "TEXT".into(),
                        })
                        .collect(),
                })
                .collect())
        }
        fn rows(
            &mut self,
            query: &GatedQuery,
            _interrupt: &Interrupt<'_>,
            sink: &mut dyn FnMut(Vec<Cell>) -> bool,
        ) -> Result<(), EngineFailure> {
            self.calls += 1;
            self.last_sql = Some(query.sql().to_owned());
            for (index, row) in self.rows.iter().enumerate() {
                std::thread::sleep(self.delay);
                if let Some((after, flag)) = &self.cancel_after {
                    if index == *after {
                        flag.store(true, Ordering::SeqCst);
                    }
                }
                if !sink(row.clone()) {
                    return Ok(());
                }
            }
            // An index past the last row cancels after the stream ends.
            if let Some((after, flag)) = &self.cancel_after {
                if *after >= self.rows.len() {
                    flag.store(true, Ordering::SeqCst);
                }
            }
            self.failure.map_or(Ok(()), Err)
        }
    }

    fn projection() -> RowRequest {
        RowRequest::Projection {
            relation: RelationName::parse("main.orders").unwrap(),
            columns: vec!["id".into(), "status".into()],
        }
    }

    fn run(
        engine: Option<&mut dyn EvidenceEngine>,
        source: &EvidenceSourceDescriptor,
        rows: Option<&RowRequest>,
        cancel: &AtomicBool,
        deadline: Instant,
        dlp: &dyn Fn(&str) -> DlpStatus,
    ) -> Result<DatabaseEvidence, ExtractionRefusal> {
        extract(
            engine,
            &ExtractionRequest {
                source,
                rows,
                cancel,
                deadline,
                extracted_at: "2026-09-29T00:00:00Z",
                dlp,
            },
        )
    }

    #[test]
    fn p8_database_evidence_packet() {
        let source = EvidenceSourceDescriptor::parse(SOURCE).unwrap();
        let later = Instant::now() + Duration::from_secs(3_600);
        let idle = AtomicBool::new(false);
        let passed = |_: &str| DlpStatus::Passed;
        let projection = projection();
        let mut seen = Vec::new();

        // schema-only: introspection is the default and needs no row request.
        let schema = run(
            Some(&mut SyntheticEngine::new(2)),
            &source,
            None,
            &idle,
            later,
            &passed,
        )
        .unwrap();
        assert_eq!(schema.outcome(), EvidenceOutcome::SchemaOnly);
        assert!(schema.rows().is_empty());
        assert!(schema.record().contains(r#""derivation":"schema-only""#));
        assert!(schema.record().contains(r#""columns":["main.customers.id","main.customers.status","main.orders.id","main.orders.status"]"#));
        seen.push(schema.outcome());

        // bounded-rows: everything fits.
        let mut engine = SyntheticEngine::new(2);
        let bounded = run(
            Some(&mut engine),
            &source,
            Some(&projection),
            &idle,
            later,
            &passed,
        )
        .unwrap();
        assert_eq!(bounded.outcome(), EvidenceOutcome::BoundedRows);
        assert_eq!(bounded.rows().len(), 2);
        assert!(!bounded.truncated());
        assert_eq!(
            engine.last_sql.as_deref(),
            Some(r#"select "id" , "status" from "main" . "orders" order by 1 , 2"#)
        );
        assert!(bounded.record().contains(r#""derivation":"projection""#));
        assert!(bounded
            .record()
            .contains(r#""snapshot":{"state":"attested","value":"sha256:1111"#));
        seen.push(bounded.outcome());

        // truncated: max_rows (3) and max_bytes both bound the prefix.
        let truncated = run(
            Some(&mut SyntheticEngine::new(5)),
            &source,
            Some(&projection),
            &idle,
            later,
            &passed,
        )
        .unwrap();
        assert_eq!(truncated.outcome(), EvidenceOutcome::Truncated);
        assert_eq!(truncated.rows().len(), 3);
        assert!(truncated.truncated());
        assert!(truncated.record().contains(r#""row_count":3"#));
        assert!(truncated.record().contains(r#""truncated":true"#));
        let small =
            EvidenceSourceDescriptor::parse(&SOURCE.replace("max_bytes: 4096", "max_bytes: 20"))
                .unwrap();
        let byte_bound = run(
            Some(&mut SyntheticEngine::new(5)),
            &small,
            Some(&projection),
            &idle,
            later,
            &passed,
        )
        .unwrap();
        assert_eq!(byte_bound.outcome(), EvidenceOutcome::Truncated);
        assert_eq!(byte_bound.rows().len(), 1);
        seen.push(truncated.outcome());

        // cancelled: the flag flips mid-stream; no partial rows survive.
        let flag = Arc::new(AtomicBool::new(false));
        let mut engine = SyntheticEngine::new(3);
        engine.cancel_after = Some((1, flag.clone()));
        let cancelled = run(
            Some(&mut engine),
            &source,
            Some(&projection),
            &flag,
            later,
            &passed,
        )
        .unwrap();
        assert_eq!(cancelled.outcome(), EvidenceOutcome::Cancelled);
        assert!(cancelled.rows().is_empty());
        let mut engine = SyntheticEngine::new(1);
        engine.failure = Some(EngineFailure::Cancelled);
        assert_eq!(
            run(
                Some(&mut engine),
                &source,
                Some(&projection),
                &idle,
                later,
                &passed
            )
            .unwrap()
            .outcome(),
            EvidenceOutcome::Cancelled
        );
        seen.push(cancelled.outcome());

        // timeout: an expired deadline, or an engine-reported interrupt.
        let expired = Instant::now();
        let timeout = run(
            Some(&mut SyntheticEngine::new(2)),
            &source,
            Some(&projection),
            &idle,
            expired,
            &passed,
        )
        .unwrap();
        assert_eq!(timeout.outcome(), EvidenceOutcome::Timeout);
        let mut engine = SyntheticEngine::new(2);
        engine.failure = Some(EngineFailure::Timeout);
        let interrupted = run(
            Some(&mut engine),
            &source,
            Some(&projection),
            &idle,
            later,
            &passed,
        )
        .unwrap();
        assert_eq!(interrupted.outcome(), EvidenceOutcome::Timeout);
        assert!(interrupted.rows().is_empty());
        // The descriptor's timeout_ms bounds extraction even when the caller's deadline is later.
        let quick = EvidenceSourceDescriptor::parse(&format!("{SOURCE}timeout_ms: 1\n")).unwrap();
        let mut engine = SyntheticEngine::new(2);
        engine.delay = Duration::from_millis(5);
        let bounded_time = run(
            Some(&mut engine),
            &quick,
            Some(&projection),
            &idle,
            later,
            &passed,
        )
        .unwrap();
        assert_eq!(bounded_time.outcome(), EvidenceOutcome::Timeout);
        seen.push(timeout.outcome());

        // unavailable: no adapter compiled in (PostgreSQL today) is never simulated.
        let postgres = EvidenceSourceDescriptor::parse(
            "kind: evidence-source\nid: pg-orders\nengine: postgresql\ndatabase: orders-replica\nrelations: [public.orders]\ncredential: credref:evidence/orders-reader\n",
        )
        .unwrap();
        let unavailable = run(None, &postgres, None, &idle, later, &passed).unwrap();
        assert_eq!(unavailable.outcome(), EvidenceOutcome::Unavailable);
        assert!(unavailable.record().contains(r#""adapter":null"#));
        assert!(unavailable
            .record()
            .contains(r#""credential":"credref:evidence/orders-reader""#));
        assert!(unavailable
            .record()
            .contains(r#""snapshot":{"state":"unavailable","value":null}"#));
        let mut engine = SyntheticEngine::new(2);
        engine.failure = Some(EngineFailure::Unavailable);
        assert_eq!(
            run(
                Some(&mut engine),
                &source,
                Some(&projection),
                &idle,
                later,
                &passed
            )
            .unwrap()
            .outcome(),
            EvidenceOutcome::Unavailable
        );
        seen.push(unavailable.outcome());

        // stale-snapshot: a pinned identity mismatch, or a change during extraction.
        let pinned =
            EvidenceSourceDescriptor::parse(&format!("{SOURCE}snapshot: {OTHER_SNAPSHOT}\n"))
                .unwrap();
        let stale = run(
            Some(&mut SyntheticEngine::new(2)),
            &pinned,
            Some(&projection),
            &idle,
            later,
            &passed,
        )
        .unwrap();
        assert_eq!(stale.outcome(), EvidenceOutcome::StaleSnapshot);
        assert!(stale.rows().is_empty());
        let mut engine = SyntheticEngine::new(2);
        engine.snapshots = vec![Some(SNAPSHOT.into()), Some(OTHER_SNAPSHOT.into())];
        assert_eq!(
            run(
                Some(&mut engine),
                &source,
                Some(&projection),
                &idle,
                later,
                &passed
            )
            .unwrap()
            .outcome(),
            EvidenceOutcome::StaleSnapshot
        );
        seen.push(stale.outcome());

        // dlp-rejected: findings drop the rows and block packet admission.
        let findings = |text: &str| {
            if text.contains("void") {
                DlpStatus::Findings
            } else {
                DlpStatus::Passed
            }
        };
        let rejected = run(
            Some(&mut SyntheticEngine::new(3)),
            &source,
            Some(&projection),
            &idle,
            later,
            &findings,
        )
        .unwrap();
        assert_eq!(rejected.outcome(), EvidenceOutcome::DlpRejected);
        assert!(rejected.rows().is_empty());
        assert!(rejected.record().contains(r#""dlp":"findings""#));
        seen.push(rejected.outcome());

        // Every outcome is produced and typed separately.
        assert_eq!(seen, EvidenceOutcome::ALL);
        let mut labels = EvidenceOutcome::ALL.map(EvidenceOutcome::as_str).to_vec();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), 8);

        // Policy refusals never contact the engine.
        let introspection =
            EvidenceSourceDescriptor::parse(&SOURCE.replace("access: bounded-rows\n", "")).unwrap();
        let mut engine = SyntheticEngine::new(2);
        assert_eq!(
            run(
                Some(&mut engine),
                &introspection,
                Some(&projection),
                &idle,
                later,
                &passed
            ),
            Err(ExtractionRefusal::RowsNotPermitted)
        );
        let model_sql = RowRequest::Sql(
            "SELECT id FROM main.orders ORDER BY id; DROP TABLE main.orders".into(),
        );
        assert_eq!(
            run(
                Some(&mut engine),
                &source,
                Some(&model_sql),
                &idle,
                later,
                &passed
            ),
            Err(ExtractionRefusal::Sql(SqlRejection::MultiStatement))
        );
        let unlisted = RowRequest::Sql("SELECT id FROM main.orders ORDER BY id".into());
        assert_eq!(
            run(
                Some(&mut engine),
                &source,
                Some(&unlisted),
                &idle,
                later,
                &passed
            ),
            Err(ExtractionRefusal::QueryNotAllowlisted)
        );
        assert_eq!(engine.calls, 0);
        let digest = gate("SELECT id FROM main.orders ORDER BY id", source.relations()).unwrap();
        let listed =
            EvidenceSourceDescriptor::parse(&format!("{SOURCE}queries: [{}]\n", digest.digest()))
                .unwrap();
        let query = run(
            Some(&mut engine),
            &listed,
            Some(&unlisted),
            &idle,
            later,
            &passed,
        )
        .unwrap();
        assert_eq!(query.outcome(), EvidenceOutcome::BoundedRows);
        assert!(query
            .record()
            .contains(&format!(r#""query_digest":"{}""#, digest.digest())));
        assert!(query.record().contains(r#""derivation":"query""#));

        // Provenance is stable: the same source, request, and snapshot give the same record.
        let again = run(
            Some(&mut SyntheticEngine::new(5)),
            &source,
            Some(&projection),
            &idle,
            later,
            &passed,
        )
        .unwrap();
        assert_eq!(again, truncated);
        assert!(truncated.evidence_digest().starts_with("sha256:"));
        assert!(
            !truncated.record().contains("paid"),
            "rows stay out of the provenance record"
        );

        // Admitted evidence enters a packet with provenance and truncation state.
        let admitted = [&schema, &bounded, &truncated];
        let (nodes, evidence): (Vec<_>, Vec<_>) = admitted
            .iter()
            .map(|item| item.packet_evidence(EvidencePriority::Required).unwrap())
            .unzip();
        let graph = KnowledgeGraph::build(
            GraphInput::new(nodes, vec![]),
            GraphLimits::new(3, 1).unwrap(),
        )
        .unwrap();
        let view = ViewCompiler::new()
            .compile(
                &ViewSpec {
                    source: ViewSource::ProvenanceSourceContains(DATABASE_EVIDENCE_SOURCE.into()),
                    filter: ViewFilter::All,
                    sort: ViewSort::NodeId,
                    grouping: ViewGrouping::None,
                    fields: vec![ViewField::NodeId],
                    presentation: Presentation::Json,
                    max_items: 3,
                    max_bytes: 65_536,
                },
                &graph,
            )
            .unwrap();
        let assemble = || {
            PacketAssembler::new()
                .assemble(&PacketAssemblyRequest {
                    view: &view,
                    graph: &graph,
                    evidence: &evidence,
                    limits: PacketLimits::new(3, 64 * 1024, None),
                    dependency_limits: DependencyClosureLimits::new(0, 1).unwrap(),
                })
                .unwrap()
        };
        let packet = assemble();
        assert_eq!(packet.items.len(), 3);
        assert_eq!(packet, assemble());
        for item in &packet.items {
            assert_eq!(item.provenance.source(), DATABASE_EVIDENCE_SOURCE);
            assert!(item
                .provenance
                .locator()
                .starts_with("database:synthetic-orders#sha256:"));
            assert!(item.id.as_str().starts_with("db:synthetic-orders:"));
        }
        let truncated_item = packet
            .items
            .iter()
            .find(|item| {
                item.provenance
                    .locator()
                    .ends_with(truncated.evidence_digest())
            })
            .unwrap();
        let anchors = truncated_item
            .preservation_anchors
            .iter()
            .map(|anchor| (anchor.id(), anchor.value()))
            .collect::<Vec<_>>();
        assert_eq!(
            anchors,
            [
                ("db.outcome", "truncated"),
                ("db.truncated", "true"),
                ("db.row_count", "3"),
                ("db.result_digest", truncated.result_digest()),
            ]
        );
        assert!(packet.payload.contains(r#""truncated":true"#));
        assert!(packet.payload.contains(r#"[3,"void"]"#));
        let (node, _) = truncated
            .packet_evidence(EvidencePriority::Required)
            .unwrap();
        assert_eq!(
            node.facts().values("outcome"),
            Some(&["truncated".to_owned()][..])
        );

        // Non-admitted outcomes never become packet evidence.
        for refused in [&cancelled, &timeout, &unavailable, &stale, &rejected] {
            assert_eq!(
                refused
                    .packet_evidence(EvidencePriority::Related)
                    .unwrap_err(),
                NotAdmitted(refused.outcome())
            );
        }
    }

    #[test]
    fn dlp_rejection_drops_catalog_locators() {
        // Review F2: a detected secret in engine metadata must not reach provenance.
        let source = EvidenceSourceDescriptor::parse(SOURCE).unwrap();
        let later = Instant::now() + Duration::from_secs(3_600);
        let idle = AtomicBool::new(false);
        let detect = |text: &str| {
            if text.contains("synthetic-review-secret") {
                DlpStatus::Findings
            } else {
                DlpStatus::Passed
            }
        };
        let mut engine = SyntheticEngine::new(2);
        engine.extra_column = Some("password=synthetic-review-secret".into());
        let rejected = run(Some(&mut engine), &source, None, &idle, later, &detect).unwrap();
        assert_eq!(rejected.outcome(), EvidenceOutcome::DlpRejected);
        assert!(
            !rejected.record().contains("synthetic-review-secret"),
            "{}",
            rejected.record()
        );
        assert!(
            rejected.record().contains(r#""columns":[]"#),
            "{}",
            rejected.record()
        );
        assert!(rejected.packet_evidence(EvidencePriority::Related).is_err());
        // Engine-supplied snapshot text is untrusted too: only a digest is sealed.
        let mut engine = SyntheticEngine::new(2);
        engine.snapshots = vec![Some("token=synthetic-snapshot-secret".into())];
        let passed = |_: &str| DlpStatus::Passed;
        let evidence = run(Some(&mut engine), &source, None, &idle, later, &passed).unwrap();
        assert!(
            !evidence.record().contains("synthetic-snapshot-secret"),
            "{}",
            evidence.record()
        );
        assert!(evidence
            .record()
            .contains(r#""snapshot":{"state":"attested","value":"sha256:"#));
    }

    #[test]
    fn catalog_interruption_is_not_admitted() {
        // Review F5: a slow or cancelled catalog read must not become schema-only evidence.
        let passed = |_: &str| DlpStatus::Passed;
        let later = Instant::now() + Duration::from_secs(3_600);
        let idle = AtomicBool::new(false);
        let quick = EvidenceSourceDescriptor::parse(&format!("{SOURCE}timeout_ms: 1\n")).unwrap();
        let mut engine = SyntheticEngine::new(2);
        engine.catalog_delay = Duration::from_millis(20);
        let slow = run(Some(&mut engine), &quick, None, &idle, later, &passed).unwrap();
        assert_eq!(slow.outcome(), EvidenceOutcome::Timeout);
        assert!(slow.packet_evidence(EvidencePriority::Related).is_err());

        let source = EvidenceSourceDescriptor::parse(SOURCE).unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        let mut engine = SyntheticEngine::new(2);
        engine.catalog_cancel = Some(flag.clone());
        let cancelled = run(Some(&mut engine), &source, None, &flag, later, &passed).unwrap();
        assert_eq!(cancelled.outcome(), EvidenceOutcome::Cancelled);
        assert!(cancelled
            .packet_evidence(EvidencePriority::Related)
            .is_err());
        // After an interruption the engine is not contacted again.
        let flag = Arc::new(AtomicBool::new(false));
        let mut engine = SyntheticEngine::new(2);
        engine.catalog_cancel = Some(flag.clone());
        let before_rows = run(
            Some(&mut engine),
            &source,
            Some(&projection()),
            &flag,
            later,
            &passed,
        )
        .unwrap();
        assert_eq!(before_rows.outcome(), EvidenceOutcome::Cancelled);
        assert_eq!(engine.last_sql, None);
        assert_eq!(engine.calls, 2);

        // Cancellation after the last row is still a cancellation.
        let flag = Arc::new(AtomicBool::new(false));
        let mut engine = SyntheticEngine::new(2);
        engine.cancel_after = Some((2, flag.clone()));
        let late = run(
            Some(&mut engine),
            &source,
            Some(&projection()),
            &flag,
            later,
            &passed,
        )
        .unwrap();
        assert_eq!(late.outcome(), EvidenceOutcome::Cancelled);
        assert!(late.rows().is_empty());
        assert_eq!(engine.calls, 3, "no final snapshot after cancellation");

        // Cancellation during the closing snapshot read is not admitted either.
        let flag = Arc::new(AtomicBool::new(false));
        let mut engine = SyntheticEngine::new(2);
        engine.snapshot_cancel = Some((3, flag.clone()));
        let closing = run(Some(&mut engine), &source, None, &flag, later, &passed).unwrap();
        assert_eq!(closing.outcome(), EvidenceOutcome::Cancelled);
    }

    #[test]
    fn provenance_binds_database_origin() {
        // Review F7: the configured database handle is part of the evidence identity.
        let passed = |_: &str| DlpStatus::Passed;
        let later = Instant::now() + Duration::from_secs(3_600);
        let idle = AtomicBool::new(false);
        let extract_from = |database: &str| {
            let source =
                EvidenceSourceDescriptor::parse(&SOURCE.replace("data/orders.sqlite", database))
                    .unwrap();
            let mut engine = SyntheticEngine::new(2);
            engine.snapshots = vec![None];
            run(Some(&mut engine), &source, None, &idle, later, &passed).unwrap()
        };
        let one = extract_from("data/one.db");
        let two = extract_from("data/two.db");
        assert!(
            one.record().contains(r#""database":"data/one.db""#),
            "{}",
            one.record()
        );
        assert_ne!(one.record(), two.record());
        assert_ne!(one.evidence_digest(), two.evidence_digest());
        let node = |evidence: &DatabaseEvidence| {
            evidence
                .packet_evidence(EvidencePriority::Required)
                .unwrap()
                .0
                .id()
                .clone()
        };
        assert_ne!(node(&one), node(&two));
    }
}
