use bran_core::adapters::sqz::{
    DlpStatus, FidelityStatus, SqzAdapter, SqzAdapterConfig, SqzIdentity, SqzPolicy, SqzPort,
    SqzPortError, SqzPortOutput, SqzStatus,
};
use bran_core::graph::{
    Confidence, GraphInput, GraphLimits, KnowledgeGraph, NodeId, NodeInput, NodeRole, Provenance,
};
use bran_core::packet::{
    DependencyClosureLimits, EvidenceContent, EvidencePriority, PacketAssembler,
    PacketAssemblyRequest, PacketLimits, PreservationAnchor,
};
use bran_core::view::{
    CompiledView, Presentation, ViewCompiler, ViewField, ViewFilter, ViewGrouping, ViewSort,
    ViewSource, ViewSpec,
};
use std::time::{Duration, Instant};

const CORPUS_ID: &str = "bran-controlled-context";
const CORPUS_VERSION: &str = "1";
const BASELINE_ID: &str = "r13-context-v1";
const CANDIDATE_COUNT: usize = 6_000;
const VIEW_ITEM_LIMIT: usize = 128;
const VIEW_BYTE_LIMIT: usize = 1_000_000;
const PACKET_ITEM_LIMIT: usize = 48;
const PACKET_BYTE_LIMIT: usize = 64 * 1024;
const PACKET_TOKEN_LIMIT: usize = 16 * 1024;
const SQZ_OUTPUT_LIMIT: usize = 1_024;
const WARM_SAMPLE_COUNT: usize = 15;
const PEAK_RSS_LIMIT_BYTES: u64 = 256 * 1024 * 1024;
const FIDELITY_ANCHOR: &str = "architecture-contract";

struct InMemorySqzPort;

impl SqzPort for InMemorySqzPort {
    fn compress(&self, input: &str) -> Result<SqzPortOutput, SqzPortError> {
        Ok(SqzPortOutput::new(
            format!(
                "{FIDELITY_ANCHOR}\nrequired-fact-0\nrequired-fact-1\nrequired-fact-2\nrequired-fact-3\nrequired-fact-4\nrequired-fact-5\nrequired-fact-6\nrequired-fact-7\nfocused_context_source_bytes={}\n",
                input.len()
            ),
            SqzIdentity::approved(),
        ))
    }
}

struct Sample {
    view_compile: Duration,
    packet_assembly: Duration,
    sqz_evaluation: Duration,
    view_ids: Vec<NodeId>,
    selected_ids: Vec<NodeId>,
    raw_bytes: usize,
    returned_bytes: usize,
    raw_token_estimate: usize,
    returned_token_estimate: usize,
}

fn node_id(index: usize) -> NodeId {
    NodeId::parse(format!("repository.node.{index:05}"))
        .expect("valid context-packet benchmark node identity")
}

fn controlled_graph_input() -> GraphInput {
    let mut nodes = Vec::with_capacity(CANDIDATE_COUNT);
    for index in 0..CANDIDATE_COUNT {
        let role = match index % 4 {
            0 => NodeRole::Document,
            1 => NodeRole::Symbol,
            2 => NodeRole::Test,
            _ => NodeRole::Entrypoint,
        };
        nodes.push(NodeInput::new(
            node_id(index),
            role,
            Provenance::new("controlled-repository", format!("src/module-{index:05}.rs"))
                .expect("valid context-packet benchmark provenance"),
            Confidence::new(80 + (index % 21) as u8)
                .expect("valid context-packet benchmark confidence"),
        ));
    }
    GraphInput::new(nodes, Vec::new())
}

fn controlled_spec() -> ViewSpec {
    ViewSpec {
        source: ViewSource::All,
        filter: ViewFilter::All,
        sort: ViewSort::ConfidenceDescendingThenNodeId,
        grouping: ViewGrouping::Role,
        fields: vec![
            ViewField::NodeId,
            ViewField::Role,
            ViewField::ProvenanceLocator,
            ViewField::Confidence,
        ],
        presentation: Presentation::Terminal,
        max_items: VIEW_ITEM_LIMIT,
        max_bytes: VIEW_BYTE_LIMIT,
    }
}

fn controlled_evidence(view: &CompiledView) -> Vec<EvidenceContent> {
    view.items()
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let required_fact = format!("required-fact-{index}");
            let priority = if index < 8 {
                EvidencePriority::Required
            } else if index < PACKET_ITEM_LIMIT {
                EvidencePriority::Recommended
            } else {
                EvidencePriority::Related
            };
            EvidenceContent::new(
                item.id.clone(),
                format!(
                    "{FIDELITY_ANCHOR} {required_fact} evidence for {}: deterministic repository context {}",
                    item.id.as_str(),
                    "supports bounded review and planning. ".repeat(12)
                ),
                priority,
                10_000 - index as u64,
                20_000 - index as u64,
                if priority == EvidencePriority::Required {
                    vec![
                        PreservationAnchor::new(
                            format!("required.fact.{index}"),
                            required_fact,
                        )
                        .expect("valid context-packet benchmark preservation anchor"),
                    ]
                } else {
                    Vec::new()
                },
            )
        })
        .collect()
}

fn controlled_adapter() -> SqzAdapter<InMemorySqzPort> {
    SqzAdapter::new(
        InMemorySqzPort,
        SqzAdapterConfig::new(
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            SQZ_OUTPUT_LIMIT,
            vec![
                PreservationAnchor::new("global.architecture", FIDELITY_ANCHOR)
                    .expect("valid global context-packet benchmark preservation anchor"),
            ],
        ),
    )
}

fn elapsed<T>(operation: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let result = operation();
    (result, started.elapsed())
}

fn run_sample(
    graph: &KnowledgeGraph,
    spec: &ViewSpec,
    evidence: &[EvidenceContent],
    adapter: &SqzAdapter<InMemorySqzPort>,
) -> Sample {
    let (view, view_compile) = elapsed(|| {
        ViewCompiler::new()
            .compile(spec, graph)
            .expect("compile controlled context view")
    });
    assert_eq!(view.items().len(), VIEW_ITEM_LIMIT);
    assert!(view.truncated());
    assert!(view.selected_bytes() <= VIEW_BYTE_LIMIT);
    let view_ids = view.items().iter().map(|item| item.id.clone()).collect();

    let request = PacketAssemblyRequest {
        view: &view,
        graph,
        evidence,
        limits: PacketLimits::new(
            PACKET_ITEM_LIMIT,
            PACKET_BYTE_LIMIT,
            Some(PACKET_TOKEN_LIMIT),
        ),
        dependency_limits: DependencyClosureLimits::new(4, VIEW_ITEM_LIMIT)
            .expect("valid context-packet dependency limits"),
    };
    let (packet, packet_assembly) = elapsed(|| {
        PacketAssembler::new()
            .assemble(&request)
            .expect("assemble bounded context packet")
    });
    assert_eq!(packet.items.len(), PACKET_ITEM_LIMIT);
    assert!(packet.receipt.truncated);
    assert_eq!(packet.receipt.raw_bytes, packet.payload.len());
    assert!(packet.receipt.raw_bytes <= PACKET_BYTE_LIMIT);
    assert!(packet.receipt.estimated_tokens <= PACKET_TOKEN_LIMIT);
    assert_eq!(packet.receipt.effective_max_items, PACKET_ITEM_LIMIT);
    assert_eq!(packet.receipt.effective_max_bytes, PACKET_BYTE_LIMIT);
    assert_eq!(
        packet.receipt.runtime_token_ceiling,
        Some(PACKET_TOKEN_LIMIT)
    );
    for index in 0..8 {
        let anchor = format!("required-fact-{index}");
        assert!(packet.payload.contains(&anchor));
    }
    let selected_ids = packet.receipt.selected_ids.clone();
    let raw_packet_bytes = packet.payload.len();

    let (evaluation, sqz_evaluation) = elapsed(|| {
        adapter
            .evaluate(packet, SQZ_OUTPUT_LIMIT)
            .expect("evaluate packet through controlled in-memory SQZ port")
    });
    assert_eq!(evaluation.receipt.status, SqzStatus::Applied);
    assert_eq!(evaluation.receipt.fidelity_status, FidelityStatus::Passed);
    assert_eq!(evaluation.receipt.dlp_status, DlpStatus::Passed);
    assert!(evaluation.receipt.dlp_findings.is_empty());
    assert_eq!(evaluation.receipt.actual_input_tokens, None);
    assert_eq!(evaluation.receipt.actual_output_tokens, None);
    assert_eq!(evaluation.receipt.raw_bytes, raw_packet_bytes);
    assert!(evaluation.receipt.returned_bytes < raw_packet_bytes);
    assert!(evaluation.receipt.returned_bytes <= SQZ_OUTPUT_LIMIT);
    assert_eq!(
        evaluation
            .receipt
            .raw_token_estimate_bytes_divided_by_four_ceiling,
        evaluation.receipt.raw_bytes.div_ceil(4)
    );
    assert_eq!(
        evaluation
            .receipt
            .returned_token_estimate_bytes_divided_by_four_ceiling,
        evaluation.receipt.returned_bytes.div_ceil(4)
    );
    assert_eq!(evaluation.packet.items.len(), PACKET_ITEM_LIMIT);
    assert!(evaluation.packet.payload.contains(FIDELITY_ANCHOR));
    for index in 0..8 {
        assert!(evaluation
            .packet
            .payload
            .contains(&format!("required-fact-{index}")));
    }

    Sample {
        view_compile,
        packet_assembly,
        sqz_evaluation,
        view_ids,
        selected_ids,
        raw_bytes: evaluation.receipt.raw_bytes,
        returned_bytes: evaluation.receipt.returned_bytes,
        raw_token_estimate: evaluation
            .receipt
            .raw_token_estimate_bytes_divided_by_four_ceiling,
        returned_token_estimate: evaluation
            .receipt
            .returned_token_estimate_bytes_divided_by_four_ceiling,
    }
}

fn nearest_rank_p95(samples: &mut [Duration]) -> Duration {
    assert!(!samples.is_empty(), "p95 requires at least one sample");
    samples.sort_unstable();
    let rank = (95 * samples.len()).div_ceil(100);
    samples[rank - 1]
}

#[cfg(target_os = "linux")]
fn peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status")
        .expect("Linux performance gate requires readable /proc/self/status");
    let line = status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .expect("Linux performance gate requires VmHWM in /proc/self/status");
    let mut fields = line.split_whitespace();
    assert_eq!(fields.next(), Some("VmHWM:"));
    let kib = fields
        .next()
        .expect("VmHWM requires a numeric value")
        .parse::<u64>()
        .expect("VmHWM must be an unsigned integer");
    assert_eq!(fields.next(), Some("kB"), "VmHWM must use kB units");
    assert_eq!(fields.next(), None, "VmHWM must have exactly three fields");
    Some(kib.checked_mul(1024).expect("VmHWM byte count overflow"))
}

#[cfg(not(target_os = "linux"))]
fn peak_rss_bytes() -> Option<u64> {
    None
}

fn main() {
    let (graph_input, graph_input_setup) = elapsed(controlled_graph_input);
    assert_eq!(graph_input.nodes().len(), CANDIDATE_COUNT);
    assert!(graph_input.edges().is_empty());
    let (graph, cold_graph_index) = elapsed(|| {
        KnowledgeGraph::build(
            graph_input,
            GraphLimits::new(CANDIDATE_COUNT, 1).expect("valid context-packet graph limits"),
        )
        .expect("build controlled context-packet graph")
    });
    let spec = controlled_spec();
    let seed_view = ViewCompiler::new()
        .compile(&spec, &graph)
        .expect("compile controlled evidence seed view");
    let (evidence, evidence_setup) = elapsed(|| controlled_evidence(&seed_view));
    assert_eq!(evidence.len(), VIEW_ITEM_LIMIT);
    let adapter = controlled_adapter();

    // Correctness warmup is deliberately excluded from the reported sample set.
    let warmup = run_sample(&graph, &spec, &evidence, &adapter);
    let expected_view_ids = warmup.view_ids;
    let expected_selected_ids = warmup.selected_ids;
    let expected_raw_bytes = warmup.raw_bytes;
    let expected_returned_bytes = warmup.returned_bytes;
    let expected_raw_tokens = warmup.raw_token_estimate;
    let expected_returned_tokens = warmup.returned_token_estimate;

    let mut view_samples = Vec::with_capacity(WARM_SAMPLE_COUNT);
    let mut packet_samples = Vec::with_capacity(WARM_SAMPLE_COUNT);
    let mut sqz_samples = Vec::with_capacity(WARM_SAMPLE_COUNT);
    for _ in 0..WARM_SAMPLE_COUNT {
        let sample = run_sample(&graph, &spec, &evidence, &adapter);
        assert_eq!(sample.view_ids, expected_view_ids);
        assert_eq!(sample.selected_ids, expected_selected_ids);
        assert_eq!(sample.raw_bytes, expected_raw_bytes);
        assert_eq!(sample.returned_bytes, expected_returned_bytes);
        assert_eq!(sample.raw_token_estimate, expected_raw_tokens);
        assert_eq!(sample.returned_token_estimate, expected_returned_tokens);
        view_samples.push(sample.view_compile);
        packet_samples.push(sample.packet_assembly);
        sqz_samples.push(sample.sqz_evaluation);
    }
    let view_p95 = nearest_rank_p95(&mut view_samples);
    let packet_p95 = nearest_rank_p95(&mut packet_samples);
    let sqz_p95 = nearest_rank_p95(&mut sqz_samples);

    let rss = peak_rss_bytes();
    if let Some(bytes) = rss {
        assert!(
            bytes <= PEAK_RSS_LIMIT_BYTES,
            "controlled context benchmark peak RSS exceeded 256MiB: {bytes} bytes"
        );
    }
    let (rss_value, rss_status) = match rss {
        Some(bytes) => (bytes.to_string(), "available"),
        None => ("unavailable".to_string(), "unsupported-platform"),
    };

    println!(
        "benchmark=context_packet controlled=true corpus_id={CORPUS_ID} corpus_version={CORPUS_VERSION} baseline_id={BASELINE_ID} sqz_port=controlled-in-memory sqz_provider_telemetry=false candidates={CANDIDATE_COUNT} view_item_ceiling={VIEW_ITEM_LIMIT} view_byte_ceiling={VIEW_BYTE_LIMIT} packet_item_ceiling={PACKET_ITEM_LIMIT} packet_byte_ceiling={PACKET_BYTE_LIMIT} packet_token_estimate_ceiling={PACKET_TOKEN_LIMIT} token_estimate_method=bytes-divided-by-four-ceiling raw_bytes={expected_raw_bytes} returned_bytes={expected_returned_bytes} estimated_raw_tokens={expected_raw_tokens} estimated_returned_tokens={expected_returned_tokens} actual_input_tokens=unavailable actual_output_tokens=unavailable graph_input_setup_ns={} cold_graph_index_ns={} evidence_setup_ns={} warmup=untimed warm_samples={WARM_SAMPLE_COUNT} p95_method=nearest-rank warm_view_compile_p95_ns={} warm_packet_assembly_p95_ns={} warm_sqz_evaluation_p95_ns={} peak_rss_bytes={rss_value} peak_rss_threshold_bytes={PEAK_RSS_LIMIT_BYTES} peak_rss_status={rss_status} status=pass",
        graph_input_setup.as_nanos(),
        cold_graph_index.as_nanos(),
        evidence_setup.as_nanos(),
        view_p95.as_nanos(),
        packet_p95.as_nanos(),
        sqz_p95.as_nanos(),
    );
}
