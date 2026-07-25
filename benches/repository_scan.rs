use bran_core::scan::{RepositoryScanner, ScanConfig, ScanSnapshot};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const CORPUS_ID: &str = "bran-r13-repository-scan";
const CORPUS_VERSION: &str = "1";
const BASELINE_ID: &str = "repository-scanner-supplied-diff-v1";
const FILE_COUNT: usize = 10_000;
const TOTAL_BYTES: usize = 50 * 1024 * 1024;
const LARGE_FILE_BYTES: usize = TOTAL_BYTES.div_ceil(FILE_COUNT);
const LARGE_FILE_COUNT: usize = TOTAL_BYTES % FILE_COUNT;
const SAMPLE_COUNT: usize = 10;
const FULL_SCAN_LIMIT: Duration = Duration::from_secs(5);
const INCREMENTAL_SCAN_LIMIT: Duration = Duration::from_millis(500);
const RSS_LIMIT_BYTES: u64 = 256 * 1024 * 1024;
const CHANGED_PATH: &str = "src/module-00000.rs";
static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

fn benchmark_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "bran-repository-scan-bench-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn file_bytes(index: usize) -> usize {
    if index < LARGE_FILE_COUNT {
        LARGE_FILE_BYTES
    } else {
        LARGE_FILE_BYTES - 1
    }
}

fn make_corpus(root: &Path) {
    fs::create_dir_all(root.join("src")).expect("create benchmark corpus");
    for index in 0..FILE_COUNT {
        let source = vec![b'x'; file_bytes(index)];
        fs::write(root.join(format!("src/module-{index:05}.rs")), source)
            .expect("write benchmark source");
    }
}

fn timed<T>(operation: impl FnOnce() -> T) -> (Duration, T) {
    let started = Instant::now();
    let output = operation();
    (started.elapsed(), output)
}

fn nearest_rank_p95(samples: &[Duration]) -> Duration {
    assert!(!samples.is_empty(), "p95 requires at least one sample");
    let mut ordered = samples.to_vec();
    ordered.sort_unstable();
    let rank = (95 * ordered.len()).div_ceil(100);
    ordered[rank - 1]
}

fn assert_complete_snapshot(snapshot: &ScanSnapshot) {
    assert_eq!(snapshot.entries.len(), FILE_COUNT);
    assert_eq!(snapshot.file_count, FILE_COUNT);
    assert_eq!(snapshot.total_bytes, TOTAL_BYTES);
}

#[cfg(target_os = "linux")]
fn peak_rss_bytes() -> Result<Option<u64>, String> {
    let status = fs::read_to_string("/proc/self/status")
        .map_err(|error| format!("read /proc/self/status: {error}"))?;
    let line = status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .ok_or_else(|| "VmHWM missing from /proc/self/status".to_owned())?;
    let mut fields = line.split_ascii_whitespace();
    if fields.next() != Some("VmHWM:") {
        return Err("malformed VmHWM label".to_owned());
    }
    let kibibytes = fields
        .next()
        .ok_or_else(|| "VmHWM value missing".to_owned())?
        .parse::<u64>()
        .map_err(|error| format!("invalid VmHWM value: {error}"))?;
    if fields.next() != Some("kB") || fields.next().is_some() {
        return Err("malformed VmHWM unit or trailing fields".to_owned());
    }
    let bytes = kibibytes
        .checked_mul(1024)
        .ok_or_else(|| "VmHWM byte conversion overflow".to_owned())?;
    Ok(Some(bytes))
}

#[cfg(not(target_os = "linux"))]
fn peak_rss_bytes() -> Result<Option<u64>, String> {
    Ok(None)
}

fn main() {
    assert_eq!(
        LARGE_FILE_COUNT * LARGE_FILE_BYTES
            + (FILE_COUNT - LARGE_FILE_COUNT) * (LARGE_FILE_BYTES - 1),
        TOTAL_BYTES
    );

    let root = benchmark_root();
    let (corpus_setup, ()) = timed(|| make_corpus(&root));
    let (scanner_initialization, scanner) = timed(|| {
        RepositoryScanner::new(
            &root,
            ScanConfig::new(FILE_COUNT, LARGE_FILE_BYTES, TOTAL_BYTES),
        )
        .expect("construct bounded scanner")
    });

    let (cold_full, initial_snapshot) = timed(|| scanner.scan().expect("cold full scan"));
    assert_complete_snapshot(&initial_snapshot);

    let mut warm_full_samples = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let (sample, snapshot) = timed(|| scanner.scan().expect("warm full scan"));
        assert_complete_snapshot(&snapshot);
        warm_full_samples.push(sample);
    }
    let warm_full_p95 = nearest_rank_p95(&warm_full_samples);

    let changed_paths = BTreeSet::from([CHANGED_PATH.to_owned()]);
    let changed_file = root.join(CHANGED_PATH);
    let changed_file_bytes = file_bytes(0);
    let mut snapshot = initial_snapshot;
    let mut incremental_samples = Vec::with_capacity(SAMPLE_COUNT);
    for sample_index in 0..SAMPLE_COUNT {
        let mut source = vec![b'x'; changed_file_bytes];
        source[0] = b'a' + u8::try_from(sample_index).expect("bounded sample index");
        fs::write(&changed_file, source).expect("mutate one same-sized source file");

        let (sample, change) = timed(|| {
            scanner
                .scan_changed(&snapshot, &changed_paths)
                .expect("one-file supplied-diff scan")
        });
        assert_eq!(change.changed, [CHANGED_PATH]);
        assert_eq!(change.reused.len(), FILE_COUNT - 1);
        assert!(change.added.is_empty());
        assert!(change.removed.is_empty());
        assert_complete_snapshot(&change.snapshot);
        snapshot = change.snapshot;
        incremental_samples.push(sample);
    }
    let incremental_p95 = nearest_rank_p95(&incremental_samples);

    let peak_rss = peak_rss_bytes().unwrap_or_else(|error| panic!("peak RSS unavailable: {error}"));
    fs::remove_dir_all(&root).expect("remove exact benchmark corpus");

    let rss_within_limit = peak_rss.is_none_or(|bytes| bytes <= RSS_LIMIT_BYTES);
    let status = if cold_full <= FULL_SCAN_LIMIT
        && warm_full_p95 <= FULL_SCAN_LIMIT
        && incremental_p95 <= INCREMENTAL_SCAN_LIMIT
        && rss_within_limit
    {
        "pass"
    } else {
        "fail"
    };
    let (rss_peak_bytes, rss_status) = peak_rss.map_or_else(
        || ("unavailable".to_owned(), "unsupported-platform"),
        |bytes| (bytes.to_string(), "actual"),
    );

    println!(
        "benchmark=repository_scan controlled=true corpus_id={CORPUS_ID} corpus_version={CORPUS_VERSION} baseline_id={BASELINE_ID} file_count={FILE_COUNT} eligible_bytes={TOTAL_BYTES} corpus_setup_ns={} scanner_initialization_ns={} cold_full_samples=1 cold_full_ns={} warm_full_samples={} warm_full_p95_ns={} incremental_samples={} incremental_p95_ns={} changed_path={CHANGED_PATH} rss_peak_bytes={rss_peak_bytes} rss_status={rss_status} actual_provider_tokens=unavailable provider=not_applicable model=not_applicable full_limit_ns={} incremental_limit_ns={} rss_limit_bytes={RSS_LIMIT_BYTES} status={status}",
        corpus_setup.as_nanos(),
        scanner_initialization.as_nanos(),
        cold_full.as_nanos(),
        warm_full_samples.len(),
        warm_full_p95.as_nanos(),
        incremental_samples.len(),
        incremental_p95.as_nanos(),
        FULL_SCAN_LIMIT.as_nanos(),
        INCREMENTAL_SCAN_LIMIT.as_nanos(),
    );

    assert!(
        cold_full <= FULL_SCAN_LIMIT,
        "cold full scan exceeded {} ns: {} ns",
        FULL_SCAN_LIMIT.as_nanos(),
        cold_full.as_nanos()
    );
    assert!(
        warm_full_p95 <= FULL_SCAN_LIMIT,
        "warm full p95 exceeded {} ns: {} ns",
        FULL_SCAN_LIMIT.as_nanos(),
        warm_full_p95.as_nanos()
    );
    assert!(
        incremental_p95 <= INCREMENTAL_SCAN_LIMIT,
        "incremental p95 exceeded {} ns: {} ns",
        INCREMENTAL_SCAN_LIMIT.as_nanos(),
        incremental_p95.as_nanos()
    );
    if let Some(bytes) = peak_rss {
        assert!(
            bytes <= RSS_LIMIT_BYTES,
            "peak RSS exceeded {RSS_LIMIT_BYTES} bytes: {bytes} bytes"
        );
    }
}
