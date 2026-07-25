//! Derived-state self-healing: rebuild only BRAN-owned derived artifacts
//! (index, cache, snapshot, report, generated-validator) while refusing
//! automatic source, metadata, classification, configuration, policy, or
//! source-link repair.
//!
//! AC-6, DES-3, DES-5, CONTRACT-9, SEIT-7.

use crate::repair::{MaintainerAuthority, RepairCoordinator, RepairTerminal};
use std::fmt;

/// Schema version for derived-state rebuild receipts.
pub const DERIVED_REBUILD_SCHEMA_VERSION: &str = "1.0.0";

/// Types of BRAN-owned derived artifacts that may be self-healed.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DerivedArtifactKind {
    /// Deterministic content index (path→digest map).
    Index,
    /// Deterministic key→value cache.
    Cache,
    /// State snapshot (serialised scan/graph state).
    Snapshot,
    /// Generated report artifact.
    Report,
    /// Generated validator artifact (compiled or derived check).
    GeneratedValidator,
}

impl DerivedArtifactKind {
    /// All recognised kinds, ordered for deterministic iteration.
    pub fn all() -> [Self; 5] {
        [
            Self::Index,
            Self::Cache,
            Self::Snapshot,
            Self::Report,
            Self::GeneratedValidator,
        ]
    }

    /// Stable string label for receipt and fixture identity.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::Cache => "cache",
            Self::Snapshot => "snapshot",
            Self::Report => "report",
            Self::GeneratedValidator => "generated-validator",
        }
    }
}

impl fmt::Display for DerivedArtifactKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Reasons a derived-state rebuild is refused without mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DerivedRebuildRefusal {
    /// The target path is not a BRAN-owned derived artifact.
    NotDerivedArtifact { path: String, reason: String },
    /// The proposed path targets a forbidden category (source, config, policy, etc.).
    ForbiddenCategory { path: String, category: String },
    /// Rebuild inputs are empty or malformed for the requested kind.
    InvalidInputs {
        kind: DerivedArtifactKind,
        reason: String,
    },
    /// One key has multiple distinct input values, so rebuilding would be ambiguous.
    ConflictingInput { key: String },
    /// Underlying repair coordinator refused (authorisation, digest, stale, etc.).
    RepairRefused(RepairTerminal),
}

impl fmt::Display for DerivedRebuildRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotDerivedArtifact { path, reason } => {
                write!(f, "not a BRAN-owned derived artifact: {path} ({reason})")
            }
            Self::ForbiddenCategory { path, category } => {
                write!(
                    f,
                    "forbidden category {category}: automatic repair refused for {path}"
                )
            }
            Self::InvalidInputs { kind, reason } => {
                write!(f, "invalid rebuild inputs for {kind}: {reason}")
            }
            Self::ConflictingInput { key } => {
                write!(f, "conflicting duplicate rebuild input key: {key}")
            }
            Self::RepairRefused(term) => {
                write!(f, "repair coordinator refused: {term:?}")
            }
        }
    }
}

/// Successful derived-state rebuild receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DerivedRebuildReceipt {
    pub schema_version: &'static str,
    pub artifact_kind: DerivedArtifactKind,
    pub target: String,
    pub input_digest: String,
    pub output_hash: String,
    pub authority_tag: String,
}

/// Path prefixes that signal forbidden category: source, metadata,
/// classification, configuration, policy, or source-link targets.
const FORBIDDEN_MARKERS: &[(&str, &str)] = &[
    ("source", "source code or document"),
    ("src/", "source directory"),
    ("Cargo.toml", "package manifest"),
    ("Cargo.lock", "lockfile"),
    ("config", "configuration"),
    ("policy", "policy"),
    ("metadata", "metadata"),
    ("classification", "classification"),
    (".git/", "version-control metadata"),
    ("schemas/", "schema definition"),
    ("fixtures/", "fixture data (unless derived-state fixture)"),
    ("docs/", "documentation"),
    ("skill/", "agent skill"),
    ("tools/ci/", "CI scripts"),
    ("AGENTS.md", "agent instructions"),
    ("README.md", "project readme"),
    ("LICENSE", "license"),
];

/// BRAN-owned derived-artifact namespace prefixes (component-aware).
/// Each entry is the exact `.bran/<ns>` prefix; descendants match
/// when the next character is `/` — never a bare substring.
const DERIVED_NAMESPACES: &[(&str, DerivedArtifactKind)] = &[
    (".bran/index", DerivedArtifactKind::Index),
    (".bran/cache", DerivedArtifactKind::Cache),
    (".bran/snapshot", DerivedArtifactKind::Snapshot),
    (".bran/report", DerivedArtifactKind::Report),
    (".bran/validator", DerivedArtifactKind::GeneratedValidator),
];

/// Exact filenames directly under `.bran/` that are recognised derived
/// artifacts.  These match only the precise `.bran/<file>` path, never
/// a bare filename or suffix elsewhere in the tree.
const DERIVED_FILES_UNDER_BRAN: &[(&str, DerivedArtifactKind)] = &[
    ("index.json", DerivedArtifactKind::Index),
    ("cache.json", DerivedArtifactKind::Cache),
    ("snapshot.json", DerivedArtifactKind::Snapshot),
    ("report.json", DerivedArtifactKind::Report),
    (
        "generated-validator",
        DerivedArtifactKind::GeneratedValidator,
    ),
];

/// Returns `true` if the relative path appears to target a BRAN-owned derived
/// artifact.  Delegates to [`classify`] for component-aware matching.
pub fn is_derived_artifact(path: &str) -> bool {
    classify(path).is_ok()
}

/// Classify a path for derived-state rebuild eligibility.
///
/// Returns `Ok(kind)` if the path matches a known derived-artifact pattern,
/// or an `Err(refusal)` describing why automatic rebuild is refused.
#[allow(clippy::result_large_err)]
pub fn classify(path: &str) -> Result<DerivedArtifactKind, DerivedRebuildRefusal> {
    // 1. Reject absolute paths (containment boundary).
    if path.starts_with('/') {
        return Err(DerivedRebuildRefusal::NotDerivedArtifact {
            path: path.to_owned(),
            reason: "absolute paths are not valid derived-artifact targets".to_owned(),
        });
    }

    // 2. Reject parent traversal (containment boundary).
    if path.contains("..") {
        return Err(DerivedRebuildRefusal::NotDerivedArtifact {
            path: path.to_owned(),
            reason: "parent traversal not allowed in derived-artifact targets".to_owned(),
        });
    }

    // 3. Normalize: strip leading "./" and trailing "/".
    let normalized = path
        .strip_prefix("./")
        .unwrap_or(path)
        .trim_end_matches('/');

    // 4. Reject known forbidden categories for paths not already in .bran/.
    if !normalized.starts_with(".bran/") {
        for &(marker, category) in FORBIDDEN_MARKERS {
            if normalized.contains(marker) && !normalized.contains("fixtures/derived-state/") {
                return Err(DerivedRebuildRefusal::ForbiddenCategory {
                    path: path.to_owned(),
                    category: category.to_owned(),
                });
            }
        }
    }

    // 5. Must start with ".bran/" for derived artifacts.
    if !normalized.starts_with(".bran/") {
        return Err(DerivedRebuildRefusal::NotDerivedArtifact {
            path: path.to_owned(),
            reason: "not a BRAN-owned derived path: must be under .bran/".to_owned(),
        });
    }

    // 6. Match known namespace prefixes (component-aware — never substring).
    for &(prefix, kind) in DERIVED_NAMESPACES {
        if normalized == prefix {
            return Ok(kind);
        }
        // Descendant: prefix must be followed by '/' so .bran/indexevil won't match.
        if let Some(suffix) = normalized.strip_prefix(prefix) {
            if suffix.starts_with('/') {
                return Ok(kind);
            }
        }
    }

    // 7. Match exact filenames directly under .bran/.
    for &(file, kind) in DERIVED_FILES_UNDER_BRAN {
        let exact = format!(".bran/{file}");
        if normalized == exact {
            return Ok(kind);
        }
    }

    Err(DerivedRebuildRefusal::NotDerivedArtifact {
        path: path.to_owned(),
        reason: "no recognised BRAN derived-artifact namespace in path".to_owned(),
    })
}

/// Pair an input name with its content bytes for deterministic rebuild.
pub type RebuildInput<'a> = (&'a str, &'a [u8]);

/// Deterministically rebuild a derived artifact from ordered inputs.
///
/// The rebuild is deterministic: inputs are sorted and identical duplicate keys
/// are deduplicated, while conflicting duplicate keys are rejected.
#[allow(clippy::result_large_err)]
pub fn rebuild(
    kind: DerivedArtifactKind,
    inputs: &[RebuildInput<'_>],
) -> Result<Vec<u8>, DerivedRebuildRefusal> {
    if inputs.is_empty() {
        return Err(DerivedRebuildRefusal::InvalidInputs {
            kind,
            reason: "no inputs supplied for deterministic rebuild".to_owned(),
        });
    }

    let mut sorted = inputs.to_vec();
    sorted.sort_by(|(left_key, left_value), (right_key, right_value)| {
        left_key
            .cmp(right_key)
            .then_with(|| left_value.cmp(right_value))
    });
    let mut deduped = Vec::new();
    for (key, value) in sorted {
        if let Some((previous_key, previous_value)) = deduped.last() {
            if previous_key == &key {
                if previous_value != &value {
                    return Err(DerivedRebuildRefusal::ConflictingInput {
                        key: key.to_owned(),
                    });
                }
                continue;
            }
        }
        deduped.push((key, value));
    }

    match kind {
        DerivedArtifactKind::Index => {
            // Index: key\tdigest\n per entry, sorted by key.
            let mut out = Vec::new();
            for (key, val) in &deduped {
                let digest = crate::scan::ContentIdentity::from_bytes(val);
                out.extend_from_slice(key.as_bytes());
                out.push(b'\t');
                let hex = format!(
                    "{:016x}-{:016x}-{:016x}",
                    digest.lanes[0], digest.lanes[1], digest.lanes[2]
                );
                out.extend_from_slice(hex.as_bytes());
                out.push(b'\n');
            }
            Ok(out)
        }
        DerivedArtifactKind::Cache => {
            // Cache: same as index format (key\tdigest\n).
            // ponytail: reuse index format until cache needs differ.
            let mut out = Vec::new();
            for (key, val) in &deduped {
                let digest = crate::scan::ContentIdentity::from_bytes(val);
                out.extend_from_slice(key.as_bytes());
                out.push(b'\t');
                let hex = format!(
                    "{:016x}-{:016x}-{:016x}",
                    digest.lanes[0], digest.lanes[1], digest.lanes[2]
                );
                out.extend_from_slice(hex.as_bytes());
                out.push(b'\n');
            }
            Ok(out)
        }
        DerivedArtifactKind::Snapshot => {
            // Snapshot: key\nlen\ncontent\n per entry.
            let mut out = Vec::new();
            for (key, val) in &deduped {
                out.extend_from_slice(key.as_bytes());
                out.push(b'\n');
                let len = val.len().to_string();
                out.extend_from_slice(len.as_bytes());
                out.push(b'\n');
                out.extend_from_slice(val);
                out.push(b'\n');
            }
            Ok(out)
        }
        DerivedArtifactKind::Report => {
            // Report: header line + sorted key→content entries.
            let kind_label = kind.as_str();
            let mut out: Vec<u8> = format!(
                "# BRAN Derived {kind_label}\nschema: {DERIVED_REBUILD_SCHEMA_VERSION}\n\n"
            )
            .into_bytes();
            for (key, val) in &deduped {
                out.extend_from_slice(key.as_bytes());
                out.extend_from_slice(b": ");
                // Truncate value display for report readability.
                let display = if val.len() > 128 {
                    format!(
                        "{}...({} bytes)",
                        String::from_utf8_lossy(&val[..128]),
                        val.len()
                    )
                } else {
                    String::from_utf8_lossy(val).into_owned()
                };
                out.extend_from_slice(display.as_bytes());
                out.push(b'\n');
            }
            Ok(out)
        }
        DerivedArtifactKind::GeneratedValidator => {
            // Generated-validator: sorted key→content, with content-identity footer.
            let mut out = Vec::new();
            for (key, val) in &deduped {
                out.extend_from_slice(key.as_bytes());
                out.push(b'\n');
                out.extend_from_slice(val);
                out.push(b'\n');
            }
            let footer = crate::scan::ContentIdentity::from_bytes(&out);
            let footer_line = format!(
                "\n--validator-footer:{:016x}-{:016x}-{:016x}\n",
                footer.lanes[0], footer.lanes[1], footer.lanes[2]
            );
            out.extend_from_slice(footer_line.as_bytes());
            Ok(out)
        }
    }
}

/// Compute an input digest for rebuild receipt identity.
pub fn input_digest(inputs: &[RebuildInput<'_>]) -> String {
    let mut sorted: Vec<_> = inputs.iter().collect();
    sorted.sort_by_key(|(k, _)| *k);
    let mut combined = Vec::new();
    for (k, v) in &sorted {
        combined.extend_from_slice(k.as_bytes());
        combined.push(0);
        combined.extend_from_slice(v);
        combined.push(0);
    }
    let id = crate::scan::ContentIdentity::from_bytes(&combined);
    format!(
        "{:016x}-{:016x}-{:016x}",
        id.lanes[0], id.lanes[1], id.lanes[2]
    )
}

/// Rebuild a derived artifact at `target` under `root`, using a
/// [`RepairCoordinator`] for safe staged writes with rollback.
///
/// Returns a [`DerivedRebuildReceipt`] on success, or a
/// [`DerivedRebuildRefusal`] describing why the rebuild was refused.
///
/// # Safety
///
/// - Only BRAN-owned derived artifact paths are accepted (checked via
///   [`classify`]). Source, config, policy, metadata, classification, and
///   source-link paths are refused without any mutation.
/// - Failed writes preserve the prior artifact through the coordinator's
///   rollback mechanism.
#[allow(clippy::result_large_err)]
pub fn rebuild_artifact(
    coordinator: &RepairCoordinator,
    authority: Option<MaintainerAuthority>,
    target: &str,
    inputs: &[RebuildInput<'_>],
) -> Result<DerivedRebuildReceipt, DerivedRebuildRefusal> {
    let kind = classify(target)?;

    let rebuilt = rebuild(kind, inputs)?;
    let inp_digest = input_digest(inputs);
    let out_id = crate::scan::ContentIdentity::from_bytes(&rebuilt);
    let output_hash = format!(
        "{:016x}-{:016x}-{:016x}",
        out_id.lanes[0], out_id.lanes[1], out_id.lanes[2]
    );

    let proposal = match coordinator.propose(target.to_string(), rebuilt) {
        RepairTerminal::Proposed(p) => p,
        other => return Err(DerivedRebuildRefusal::RepairRefused(other)),
    };
    let digest = proposal.digest().to_owned();
    let authority = authority.unwrap_or_else(|| MaintainerAuthority::new("derived-rebuild-auto"));
    let auth_tag = authority.reason.clone();
    match coordinator.apply(Some(authority), proposal, &digest) {
        RepairTerminal::ValidationPassed(_receipt) => Ok(DerivedRebuildReceipt {
            schema_version: DERIVED_REBUILD_SCHEMA_VERSION,
            artifact_kind: kind,
            target: target.to_owned(),
            input_digest: inp_digest,
            output_hash,
            authority_tag: auth_tag,
        }),
        other => Err(DerivedRebuildRefusal::RepairRefused(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repair::{MaintainerAuthority, RepairCoordinator};
    use std::fs;
    use std::path::Path;

    struct TempDirGuard(std::path::PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn passing_validator(_root: &Path) -> Result<(), String> {
        Ok(())
    }

    // ── positive tests ──

    #[test]
    fn derived_state_rebuild_index_deterministic() {
        // Prove: identical inputs → identical rebuild output (deterministic).
        let inputs: &[RebuildInput] =
            &[("a.txt", b"hello"), ("b.txt", b"world"), ("z.json", b"{}")];
        let out1 = rebuild(DerivedArtifactKind::Index, inputs).unwrap();
        let out2 = rebuild(DerivedArtifactKind::Index, inputs).unwrap();
        assert_eq!(out1, out2, "rebuild must be deterministic");

        // Order-independent: same inputs in different order produce same output.
        let inputs2: &[RebuildInput] =
            &[("z.json", b"{}"), ("b.txt", b"world"), ("a.txt", b"hello")];
        let out3 = rebuild(DerivedArtifactKind::Index, inputs2).unwrap();
        assert_eq!(out1, out3, "rebuild must be order-independent");
    }

    #[test]
    fn derived_state_rebuild_all_kinds_deterministic() {
        let inputs: &[RebuildInput] =
            &[("entry-a", b"payload alpha"), ("entry-b", b"payload beta")];
        for kind in DerivedArtifactKind::all() {
            let out1 = rebuild(kind, inputs).unwrap();
            let out2 = rebuild(kind, inputs).unwrap();
            assert_eq!(out1, out2, "kind {kind:?} rebuild must be deterministic");
            assert!(
                !out1.is_empty(),
                "kind {kind:?} must produce non-empty output"
            );
        }
    }

    #[test]
    fn derived_state_rebuild_absent_artifact() {
        // Prove: an absent derived artifact can be rebuilt into existence.
        let base = std::env::temp_dir();
        let unique = format!(
            "bran-derived-rebuild-absent-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = base.join(&unique);
        fs::create_dir_all(&root).unwrap();
        let _guard = TempDirGuard(root.clone());

        let coord = RepairCoordinator::new(&root, passing_validator).unwrap();
        let target = ".bran/index/main.idx";
        fs::create_dir_all(root.join(".bran/index")).unwrap();
        let inputs: &[RebuildInput] = &[("f1.rs", b"fn main(){}"), ("f2.rs", b"mod test;")];

        // Target does not exist initially.
        assert!(!root.join(target).exists());

        let receipt = rebuild_artifact(
            &coord,
            Some(MaintainerAuthority::new("test-rebuild-absent")),
            target,
            inputs,
        )
        .unwrap();

        assert_eq!(receipt.artifact_kind, DerivedArtifactKind::Index);
        assert_eq!(receipt.target, target);
        assert!(!receipt.input_digest.is_empty());
        assert!(!receipt.output_hash.is_empty());

        // File now exists with deterministic content.
        let written = fs::read(root.join(target)).unwrap();
        let expected = rebuild(DerivedArtifactKind::Index, inputs).unwrap();
        assert_eq!(written, expected);
    }

    #[test]
    fn derived_state_rebuild_without_authority_uses_derived_receipt() {
        let root = std::env::temp_dir().join(format!("bran-derived-auto-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".bran/index")).unwrap();
        let _guard = TempDirGuard(root.clone());
        let coordinator = RepairCoordinator::new(&root, passing_validator).unwrap();

        let receipt = rebuild_artifact(
            &coordinator,
            None,
            ".bran/index/auto.idx",
            &[("key", b"value")],
        )
        .unwrap();

        assert_eq!(receipt.authority_tag, "derived-rebuild-auto");
    }

    #[test]
    fn derived_state_rebuild_corrupted_artifact() {
        // Prove: a corrupted artifact is rebuilt correctly from unchanged inputs.
        let base = std::env::temp_dir();
        let unique = format!(
            "bran-derived-rebuild-corrupt-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = base.join(&unique);
        fs::create_dir_all(&root).unwrap();
        let _guard = TempDirGuard(root.clone());

        let target = ".bran/cache/data.cache";
        fs::create_dir_all(root.join(".bran/cache")).unwrap();
        // Write corrupted content first.
        fs::write(root.join(target), b"garbage-corrupted-data!!").unwrap();

        let coord = RepairCoordinator::new(&root, passing_validator).unwrap();
        let inputs: &[RebuildInput] = &[("k1", b"v1"), ("k2", b"v2")];

        let receipt = rebuild_artifact(
            &coord,
            Some(MaintainerAuthority::new("test-rebuild-corrupt")),
            target,
            inputs,
        )
        .unwrap();

        assert_eq!(receipt.artifact_kind, DerivedArtifactKind::Cache);

        let written = fs::read(root.join(target)).unwrap();
        let expected = rebuild(DerivedArtifactKind::Cache, inputs).unwrap();
        assert_eq!(written, expected);
        // Must not contain the old corrupted data.
        assert!(!written.windows(b"garbage".len()).any(|w| w == b"garbage"));
    }

    #[test]
    fn derived_state_rebuild_failed_preserves_prior() {
        // Prove: a failed replacement preserves the prior usable derived artifact.
        let base = std::env::temp_dir();
        let unique = format!(
            "bran-derived-rollback-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = base.join(&unique);
        fs::create_dir_all(&root).unwrap();
        let _guard = TempDirGuard(root.clone());

        let target = ".bran/index/main.idx";
        fs::create_dir_all(root.join(".bran/index")).unwrap();
        let prior = b"prior-valid-index-content\n";
        fs::write(root.join(target), prior).unwrap();

        // Use a failing validator to force rollback.
        fn failing_validator(_root: &Path) -> Result<(), String> {
            Err("injected validator failure".to_owned())
        }
        let coord = RepairCoordinator::new(&root, failing_validator).unwrap();
        let inputs: &[RebuildInput] = &[("x.rs", b"// x")];

        let result = rebuild_artifact(
            &coord,
            Some(MaintainerAuthority::new("test-rollback")),
            target,
            inputs,
        );

        // Must be refused (validator failed → rollback).
        assert!(result.is_err(), "rollback must refuse on validator failure");

        // Prior content must be preserved.
        let after = fs::read(root.join(target)).unwrap();
        assert_eq!(after, prior, "prior artifact must survive failed rebuild");
    }

    #[test]
    fn derived_state_rebuild_missing_inputs_refused() {
        let result = rebuild(DerivedArtifactKind::Index, &[]);
        assert!(matches!(
            result,
            Err(DerivedRebuildRefusal::InvalidInputs { .. })
        ));
    }

    #[test]
    fn derived_state_rebuild_duplicate_inputs_are_deterministic_or_refused() {
        let identical = [("key", b"value" as &[u8]), ("key", b"value")];
        assert_eq!(
            rebuild(DerivedArtifactKind::Index, &identical).unwrap(),
            rebuild(DerivedArtifactKind::Index, &[("key", b"value")]).unwrap()
        );
        for inputs in [
            [("key", b"first" as &[u8]), ("key", b"second")],
            [("key", b"second" as &[u8]), ("key", b"first")],
        ] {
            assert!(matches!(
                rebuild(DerivedArtifactKind::Index, &inputs),
                Err(DerivedRebuildRefusal::ConflictingInput { .. })
            ));
        }
    }

    // ── negative tests: refusal without mutation ──

    #[test]
    fn derived_state_refuses_source_path() {
        for path in &[
            "src/main.rs",
            "src/lib.rs",
            "crates/bran-core/src/lib.rs",
            "source/app.rs",
        ] {
            let result = classify(path);
            assert!(
                matches!(
                    result,
                    Err(DerivedRebuildRefusal::ForbiddenCategory { .. })
                        | Err(DerivedRebuildRefusal::NotDerivedArtifact { .. })
                ),
                "source path '{path}' must be refused, got {result:?}"
            );
        }
    }

    #[test]
    fn derived_state_refuses_config_path() {
        for path in &[
            "Cargo.toml",
            "Cargo.lock",
            "config/settings.toml",
            "tools/ci/config.yaml",
        ] {
            let result = classify(path);
            assert!(
                result.is_err(),
                "config path '{path}' must be refused, got {result:?}"
            );
        }
    }

    #[test]
    fn derived_state_refuses_policy_and_metadata_path() {
        for path in &[
            "policy/rules.json",
            "metadata/classifications.yaml",
            "metadata.yaml",
            "classification/schema.json",
            ".git/HEAD",
            ".git/config",
        ] {
            let result = classify(path);
            assert!(
                result.is_err(),
                "policy/metadata path '{path}' must be refused, got {result:?}"
            );
        }
    }

    #[test]
    fn derived_state_refuses_docs_and_skills() {
        for path in &[
            "docs/plans/plan.md",
            "AGENTS.md",
            "README.md",
            "LICENSE",
            "skill/use-bran/SKILL.md",
            "schemas/schema.json",
        ] {
            let result = classify(path);
            assert!(
                result.is_err(),
                "docs/skill path '{path}' must be refused, got {result:?}"
            );
        }
    }

    #[test]
    fn derived_state_accepts_derived_paths() {
        for path in &[
            ".bran/index/main.idx",
            ".bran/cache/data.cache",
            ".bran/snapshot/state.snap",
            ".bran/report/scan.rpt",
            ".bran/validator/check.gen",
            ".bran/index.json",
            ".bran/cache.json",
            ".bran/snapshot.json",
            ".bran/report.json",
            ".bran/generated-validator",
        ] {
            let result = classify(path);
            assert!(
                result.is_ok(),
                "derived path '{path}' must be accepted, got {result:?}"
            );
        }
    }

    #[test]
    fn derived_state_rebuild_refuses_non_derived_target() {
        let base = std::env::temp_dir();
        let unique = format!(
            "bran-derived-refuse-non-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = base.join(&unique);
        fs::create_dir_all(&root).unwrap();
        let _guard = TempDirGuard(root.clone());

        let coord = RepairCoordinator::new(&root, passing_validator).unwrap();

        // Attempt rebuild of a source file through rebuild_artifact.
        let result = rebuild_artifact(&coord, None, "src/main.rs", &[("k", b"v")]);
        assert!(
            result.is_err(),
            "non-derived target must be refused by rebuild_artifact"
        );

        // Verify no file was created.
        assert!(
            !root.join("src/main.rs").exists(),
            "non-derived target must not be mutated"
        );
    }

    #[test]
    fn derived_state_refuses_lookalike_paths() {
        // Negative proof: substring and suffix lookalikes outside .bran/
        // must be refused without mutation.
        for path in &[
            "random/index.json",
            ".bran/indexevil/file",
            "x/.bran/index/file",
            "../.bran/index/file",
            "/.bran/index/file",
        ] {
            let result = classify(path);
            assert!(
                result.is_err(),
                "lookalike path '{path}' must be refused, got {result:?}"
            );
        }
    }
}
