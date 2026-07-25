use bran_core::graph::query::{NodeQuery, QueryBounds, TrailStatus};
use bran_core::graph::{
    Confidence, EdgeCertainty, EdgeId, EdgeInput, GraphInput, GraphLimits, KnowledgeGraph, NodeId,
    NodeInput, NodeRole, Provenance,
};
use std::time::{Duration, Instant};

const CORPUS_ID: &str = "bran-controlled-graph";
const CORPUS_VERSION: &str = "1";
const BASELINE_ID: &str = "r13-graph-v1";
const NODE_COUNT: usize = 6_000;
const EDGE_COUNT: usize = 100_000;
const CANDIDATE_COUNT: usize = 6_000;
const WARM_SAMPLE_COUNT: usize = 15;
const WARM_QUERY_LIMIT: Duration = Duration::from_millis(150);
const PEAK_RSS_LIMIT_BYTES: u64 = 256 * 1024 * 1024;

fn node_id(index: usize) -> NodeId {
    NodeId::parse(format!("node.{index:05}")).expect("valid benchmark node identity")
}

fn edge_id(index: usize) -> EdgeId {
    EdgeId::parse(format!("edge.{index:06}")).expect("valid benchmark edge identity")
}

fn provenance(kind: &str, index: usize) -> Provenance {
    Provenance::new("controlled-benchmark", format!("{kind}:{index:06}"))
        .expect("valid benchmark provenance")
}

fn controlled_input() -> GraphInput {
    let mut nodes = Vec::with_capacity(NODE_COUNT);
    for index in 0..NODE_COUNT {
        let role = if index == 0 {
            NodeRole::Entrypoint
        } else {
            NodeRole::Symbol
        };
        nodes.push(NodeInput::new(
            node_id(index),
            role,
            provenance("node", index),
            Confidence::new(100).expect("valid benchmark confidence"),
        ));
    }

    let mut edges = Vec::with_capacity(EDGE_COUNT);
    for index in 0..EDGE_COUNT {
        let (source, target) = if index < NODE_COUNT - 1 {
            (node_id(0), node_id(index + 1))
        } else {
            let source_index = 2 + ((index - (NODE_COUNT - 1)) % (NODE_COUNT - 2));
            (node_id(source_index), node_id(1))
        };
        edges.push(EdgeInput::new(
            edge_id(index),
            source,
            target,
            provenance("edge", index),
            Confidence::new(100).expect("valid benchmark confidence"),
            EdgeCertainty::Known,
        ));
    }
    GraphInput::new(nodes, edges)
}

fn elapsed<T>(operation: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let result = operation();
    (result, started.elapsed())
}

fn assert_queries(graph: &KnowledgeGraph) {
    let found = graph.find("node.", CANDIDATE_COUNT);
    assert_eq!(found.items.len(), CANDIDATE_COUNT);
    assert!(!found.truncated);
    assert_eq!(found.items.first().expect("first candidate").id, node_id(0));
    assert_eq!(
        found.items.last().expect("last candidate").id,
        node_id(NODE_COUNT - 1)
    );

    let impact = graph.impact(&node_id(1), QueryBounds::new(CANDIDATE_COUNT, 2));
    let NodeQuery::Found(impact) = impact else {
        panic!("controlled impact target must exist");
    };
    assert_eq!(impact.items.len(), NODE_COUNT - 1);
    assert!(!impact.truncated);
    assert!(impact.items.iter().all(|item| item.depth == 1));

    let trail = graph.trail(
        &node_id(0),
        &node_id(NODE_COUNT - 1),
        QueryBounds::new(CANDIDATE_COUNT, 1),
    );
    let TrailStatus::Found(trail) = trail else {
        panic!("controlled direct trail must exist");
    };
    assert_eq!(trail.nodes.len(), 2);
    assert_eq!(trail.edges.len(), 1);
    assert_eq!(trail.nodes[0].id, node_id(0));
    assert_eq!(trail.nodes[1].id, node_id(NODE_COUNT - 1));
    assert!(!trail.truncated);
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
    let (input, input_setup) = elapsed(controlled_input);
    assert_eq!(input.nodes().len(), NODE_COUNT);
    assert_eq!(input.edges().len(), EDGE_COUNT);
    let (graph, cold_index) = elapsed(|| {
        KnowledgeGraph::build(
            input,
            GraphLimits::new(NODE_COUNT, EDGE_COUNT).expect("valid benchmark limits"),
        )
        .expect("build controlled benchmark graph")
    });

    // Correctness warmup is deliberately excluded from the reported sample set.
    assert_queries(&graph);
    let mut warm_samples = Vec::with_capacity(WARM_SAMPLE_COUNT);
    for _ in 0..WARM_SAMPLE_COUNT {
        let (_, duration) = elapsed(|| assert_queries(&graph));
        warm_samples.push(duration);
    }
    let warm_p95 = nearest_rank_p95(&mut warm_samples);
    assert!(
        warm_p95 <= WARM_QUERY_LIMIT,
        "controlled warm-query p95 exceeded the 150ms threshold: {warm_p95:?}"
    );

    let rss = peak_rss_bytes();
    if let Some(bytes) = rss {
        assert!(
            bytes <= PEAK_RSS_LIMIT_BYTES,
            "controlled graph peak RSS exceeded 256MiB: {bytes} bytes"
        );
    }
    let (rss_value, rss_status) = match rss {
        Some(bytes) => (bytes.to_string(), "available"),
        None => ("unavailable".to_string(), "unsupported-platform"),
    };

    println!(
        "benchmark=knowledge_graph controlled=true corpus_id={CORPUS_ID} corpus_version={CORPUS_VERSION} baseline_id={BASELINE_ID} nodes={NODE_COUNT} edges={EDGE_COUNT} query_candidates={CANDIDATE_COUNT} input_setup_ns={} cold_graph_index_ns={} warmup=untimed warm_query_samples={WARM_SAMPLE_COUNT} warm_query_p95_method=nearest-rank warm_query_p95_ns={} warm_query_threshold_ns={} peak_rss_bytes={rss_value} peak_rss_threshold_bytes={PEAK_RSS_LIMIT_BYTES} peak_rss_status={rss_status} status=pass",
        input_setup.as_nanos(),
        cold_index.as_nanos(),
        warm_p95.as_nanos(),
        WARM_QUERY_LIMIT.as_nanos(),
    );
}
