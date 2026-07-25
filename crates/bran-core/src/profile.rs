//! Profile validation for OKF v0.1 compatibility and BRAN Strict readiness (Slice 1.2).
//!
//! One call to ProfileValidator produces independent outcomes for both profiles.
//! The selected profile alone governs success/exit; both results remain visible.
//! No source mutation. No link target resolution. Unknown fields and broken links tolerated
//! per the OKF v0.1 floor and approved BRAN contract.

use crate::bundle::{Bundle, DocKind, ParseStatus};
use crate::packet::PacketValidator;
use crate::policy::RepositoryPolicy;
use crate::schema::YamlValue;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Stable identifier for the OKF v0.1 compatibility profile.
pub const OKF_V0_1: &str = "okf-v0.1";

/// Stable identifier for the BRAN Strict readiness profile.
pub const BRAN_STRICT: &str = "bran-strict";

/// A single deterministic diagnostic entry.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Diagnostic {
    pub path: String,
    pub code: String,
    pub message: String,
}

/// Pass or Fail status for a profile outcome.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ValidationStatus {
    Pass,
    Fail,
}

/// Outcome for one profile. Diagnostics are always in deterministic order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileOutcome {
    pub profile: String,
    pub status: ValidationStatus,
    pub diagnostics: Vec<Diagnostic>,
}

/// Dual-profile validation result. Both outcomes are always computed.
/// Only `selected_profile` decides `selected_passed` and `exit_code`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationResult {
    pub okf_compatibility: ProfileOutcome,
    pub bran_strict: ProfileOutcome,
    pub selected_profile: String,
    /// Explicit selection failure, if the caller named an unsupported profile.
    /// Profile outcomes are still present so callers can inspect both results.
    pub selected_profile_error: Option<Diagnostic>,
}

impl ValidationResult {
    /// True only when the selected profile reports Pass.
    /// Unknown selected profile yields false (no panic).
    pub fn selected_passed(&self) -> bool {
        let outcome = match self.selected_profile.as_str() {
            OKF_V0_1 => &self.okf_compatibility,
            BRAN_STRICT => &self.bran_strict,
            _ => return false,
        };
        outcome.status == ValidationStatus::Pass
    }

    /// Explicit exit code for selected profile: 0 on pass, 1 on fail.
    /// Suitable for future CLI integration. Never panics.
    pub fn exit_code(&self) -> i32 {
        if self.selected_passed() {
            0
        } else {
            1
        }
    }
}

/// Owns all profile validation. Stateless.
pub struct ProfileValidator;

impl ProfileValidator {
    /// Validates the bundle for both profiles independently.
    ///
    /// - OKF v0.1 compatibility: requires parseable frontmatter + non-blank string `type`
    ///   only on concept documents. Reserved index.md / log.md follow their own
    ///   portable structures and carry no concept-frontmatter requirement.
    ///   Unknown fields, unknown types, and broken links are tolerated.
    ///
    /// - BRAN Strict: additive field-shape and readiness diagnostics across
    ///   title/status/tag, freshness/authority, citation/source, relationship,
    ///   and public-boundary categories. No source is rewritten. Link targets
    ///   are never validated (permitted broken links from upstream).
    ///
    /// The returned structure always contains both outcomes. Only the
    /// supplied selected_profile controls the overall pass/exit decision.
    /// Validates the bundle for both profiles independently.
    ///
    /// Runs with no policy; intrinsic BRAN strict shape checks use sensible defaults.
    /// For policy-driven checks, use [`validate_with_policy`].
    pub fn validate(bundle: &Bundle, selected_profile: &str) -> ValidationResult {
        Self::validate_with_policy(bundle, selected_profile, None)
    }

    /// Validates the bundle for both profiles independently, driving BRAN strict
    /// status/tags/public-boundary/frontmatter/source-links checks from `policy`
    /// when `Some`.  When `None`, intrinsic BRAN strict shape checks still run
    /// with sensible defaults.
    pub fn validate_with_policy(
        bundle: &Bundle,
        selected_profile: &str,
        policy: Option<&RepositoryPolicy>,
    ) -> ValidationResult {
        let okf = Self::validate_okf_compatibility(bundle, policy);
        let strict = Self::validate_bran_strict(bundle, policy);
        ValidationResult {
            okf_compatibility: okf,
            bran_strict: strict,
            selected_profile: selected_profile.to_owned(),
            selected_profile_error: match selected_profile {
                OKF_V0_1 | BRAN_STRICT => None,
                _ => Some(Diagnostic {
                    path: "<selection>".to_owned(),
                    code: "unknown-profile".to_owned(),
                    message: format!("unknown validation profile: {selected_profile}"),
                }),
            },
        }
    }

    fn validate_okf_compatibility(
        bundle: &Bundle,
        policy: Option<&RepositoryPolicy>,
    ) -> ProfileOutcome {
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let coverage = policy.and_then(|p| p.document_coverage.as_ref());

        // Iteration over BTreeMap yields lexical path order -> deterministic.
        for (path, doc) in bundle.docs() {
            if !portable_document_participates(path, coverage) {
                continue;
            }
            match doc.kind() {
                DocKind::Index => {
                    diagnostics.extend(okf_index_diagnostics(path, doc.body()));
                    continue;
                }
                DocKind::Log => {
                    diagnostics.extend(okf_log_diagnostics(path, doc.body()));
                    continue;
                }
                DocKind::Concept { .. } => {}
            }
            let fm = doc.frontmatter();
            if let Some(diagnostic) = okf_diagnostic(path, fm.status(), fm.parsed()) {
                diagnostics.push(diagnostic);
            }
        }

        let status = if diagnostics.is_empty() {
            ValidationStatus::Pass
        } else {
            ValidationStatus::Fail
        };
        ProfileOutcome {
            profile: OKF_V0_1.to_owned(),
            status,
            diagnostics,
        }
    }

    fn validate_bran_strict(bundle: &Bundle, policy: Option<&RepositoryPolicy>) -> ProfileOutcome {
        let mut diagnostics: Vec<Diagnostic> = Vec::new();

        // Policy-driven values (None => use hardcoded defaults matching original Slice 1.1 behavior).
        let allowed_status: Vec<&str> = policy
            .and_then(|p| p.status.as_ref())
            .map(|s| s.allowed.iter().map(String::as_str).collect())
            .unwrap_or_else(|| vec!["draft", "active", "deprecated"]);
        let allowed_tags: Option<Vec<&str>> = policy
            .and_then(|p| p.tags.as_ref())
            .map(|t| t.allowed.iter().map(String::as_str).collect());
        let allowed_boundary: Option<Vec<&str>> = policy
            .and_then(|p| p.public_boundary.as_ref())
            .map(|b| b.values.iter().map(String::as_str).collect());
        let source_prefix: Option<&str> = policy
            .and_then(|p| p.source_links.as_ref())
            .map(|sl| sl.require_prefix.as_str());
        let fm_required: Option<Vec<&str>> = policy
            .and_then(|p| p.frontmatter.as_ref())
            .map(|fm| fm.required.iter().map(String::as_str).collect());
        let fm_optional: Option<Vec<&str>> = policy
            .and_then(|p| p.frontmatter.as_ref())
            .map(|fm| fm.optional.iter().map(String::as_str).collect());
        let fm_allowed: Option<Vec<&str>> = policy
            .and_then(|p| p.frontmatter.as_ref())
            .map(|fm| fm.allowed.iter().map(String::as_str).collect());
        let fm_preserved_canonical: Option<Vec<&str>> = policy
            .and_then(|p| p.frontmatter.as_ref())
            .filter(|fm| !fm.preserved_canonical_keys.is_empty())
            .map(|fm| {
                fm.preserved_canonical_keys
                    .iter()
                    .map(String::as_str)
                    .collect()
            });

        // Effective allowed frontmatter = required ∪ optional ∪ allowed.
        let mut fm_effective_allowed: Vec<&str> = Vec::new();
        if let Some(ref r) = fm_required {
            fm_effective_allowed.extend(r.iter().copied());
        }
        if let Some(ref o) = fm_optional {
            for f in o {
                if !fm_effective_allowed.contains(f) {
                    fm_effective_allowed.push(*f);
                }
            }
        }
        if let Some(ref a) = fm_allowed {
            for f in a {
                if !fm_effective_allowed.contains(f) {
                    fm_effective_allowed.push(*f);
                }
            }
        }

        // --- Document coverage classification (DES-2, DES-4, DES-10) ---
        let dc = policy.and_then(|p| p.document_coverage.as_ref());
        let native_set: Option<BTreeSet<&str>> = dc.map(|d| str_set(&d.native_bundle));
        let canonical_set: Option<BTreeSet<&str>> = dc.map(|d| str_set(&d.canonical_documents));
        let legacy_set: Option<BTreeSet<&str>> = dc.map(|d| str_set(&d.legacy_documents));
        let excluded_set: Option<BTreeSet<&str>> =
            dc.map(|d| d.excluded_documents.keys().map(String::as_str).collect());
        // All classified = native ∪ canonical ∪ legacy (excluded is separate).
        let all_classified: Option<BTreeSet<&str>> = dc.map(|d| {
            let mut set: BTreeSet<&str> = BTreeSet::new();
            for p in &d.native_bundle {
                set.insert(p.as_str());
            }
            for p in &d.canonical_documents {
                set.insert(p.as_str());
            }
            for p in &d.legacy_documents {
                set.insert(p.as_str());
            }
            set
        });
        // Canonical-only = canonical \ native_bundle (for preserved_canonical_keys).
        let canonical_only: Option<BTreeSet<&str>> = match (&canonical_set, &native_set) {
            (Some(cs), Some(ns)) => Some(cs.difference(ns).copied().collect()),
            (Some(cs), None) => Some(cs.clone()),
            _ => None,
        };

        // Extended allowed for canonical-only docs = effective_allowed ∪ preserved_canonical_keys.
        let fm_canonical_allowed: Vec<&str> = match &fm_preserved_canonical {
            Some(pk) if !pk.is_empty() => {
                let mut v = fm_effective_allowed.clone();
                for k in pk {
                    if !v.contains(k) {
                        v.push(*k);
                    }
                }
                v
            }
            _ => fm_effective_allowed.clone(),
        };

        // Public path allowlist prefix set.
        let path_allowlist: Option<Vec<&str>> = policy
            .and_then(|p| p.public_boundary.as_ref())
            .map(|pb| pb.path_allowlist.iter().map(String::as_str).collect());

        for (path, doc) in bundle.docs() {
            if !document_participates(path, dc) {
                continue;
            }
            if matches!(doc.kind(), DocKind::Index | DocKind::Log) {
                continue;
            }
            // Excluded documents are intentionally skipped by strict document checks.
            if let Some(ref excl) = excluded_set {
                if excl.contains(path.as_str()) {
                    continue;
                }
            }
            let fm = doc.frontmatter();
            // Skip every per-document diagnostic for an explicitly legacy-classified
            // document when migration state is legacy_baseline (not strict).
            // Strict state still fails on any legacy entry via the post-loop
            // migration-strict-legacy diagnostic (CONTRACT-10).
            let is_legacy = legacy_set
                .as_ref()
                .is_some_and(|ls| ls.contains(path.as_str()));
            let is_canonical = canonical_set
                .as_ref()
                .is_some_and(|cs| cs.contains(path.as_str()));
            let migration_legacy_baseline = policy
                .and_then(|p| p.frontmatter.as_ref())
                .and_then(|fm| fm.canonical_docs_frontmatter_state.as_deref())
                == Some("legacy_baseline");
            let migration_strict = policy
                .and_then(|p| p.frontmatter.as_ref())
                .and_then(|fm| fm.canonical_docs_frontmatter_state.as_deref())
                == Some("strict");
            if (is_legacy && !migration_strict) || (is_canonical && migration_legacy_baseline) {
                continue;
            }
            if let Some(diagnostic) = okf_diagnostic(path, fm.status(), fm.parsed()) {
                diagnostics.push(diagnostic);
            }
            let map = match fm.parsed() {
                Some(m) => m,
                None => continue,
            };

            // --- intrinsic BRAN strict shape checks (always on; preserved order from Slice 1.1) ---

            // title (shape)
            if !has_nonblank_string(map, "title") {
                diagnostics.push(Diagnostic {
                    path: path.clone(),
                    code: "title".to_owned(),
                    message: "title must be a non-blank string for BRAN strict".to_owned(),
                });
            }

            // status (policy-driven or hardcoded)
            if !has_status_in_policy(map, &allowed_status) {
                let vals = allowed_status.join("/");
                diagnostics.push(Diagnostic {
                    path: path.clone(),
                    code: "status".to_owned(),
                    message: format!("okf_status must be one of {vals} for BRAN strict"),
                });
            }

            // tag (shape check always; value check only when policy is Some)
            if !has_tags_sequence(map) {
                diagnostics.push(Diagnostic {
                    path: path.clone(),
                    code: "tag".to_owned(),
                    message: "tags must be a YAML sequence for BRAN strict".to_owned(),
                });
            } else if let Some(ref allowed) = allowed_tags {
                if let Some(YamlValue::Sequence(ref seq)) = map.get("tags") {
                    for v in seq {
                        let tag_val = yaml_value_to_string(v);
                        if !allowed.contains(&tag_val.as_str()) {
                            diagnostics.push(Diagnostic {
                                path: path.clone(),
                                code: "tag-value".to_owned(),
                                message: format!("tag value not in policy-allowed set: {tag_val}"),
                            });
                        }
                    }
                }
            }

            // freshness
            if !has_nonblank_string(map, "timestamp") && !has_nonblank_string(map, "freshness") {
                diagnostics.push(Diagnostic {
                    path: path.clone(),
                    code: "freshness".to_owned(),
                    message: "freshness requires timestamp or freshness field (non-blank string)"
                        .to_owned(),
                });
            }

            // authority/source (resource field)
            if !has_nonblank_string(map, "resource") {
                diagnostics.push(Diagnostic {
                    path: path.clone(),
                    code: "authority".to_owned(),
                    message: "authority/source requires resource field (non-blank string)"
                        .to_owned(),
                });
            }

            // citation/source
            let has_citation = doc.body().contains("# Citations")
                || doc.body().contains("Citations:")
                || doc.body().contains("[1]");
            if !has_nonblank_string(map, "resource") && !has_citation {
                diagnostics.push(Diagnostic {
                    path: path.clone(),
                    code: "citation-source".to_owned(),
                    message: "citation/source requires resource or # Citations evidence".to_owned(),
                });
            }

            // relationship (presence of link syntax; targets never validated)
            if !has_relationship_links(doc.body()) {
                diagnostics.push(Diagnostic {
                    path: path.clone(),
                    code: "relationship".to_owned(),
                    message:
                        "relationship requires at least one markdown link (targets unvalidated)"
                            .to_owned(),
                });
            }

            // public-boundary (presence always required; value check only when policy is Some)
            if !has_nonblank_string(map, "public_boundary") {
                diagnostics.push(Diagnostic {
                    path: path.clone(),
                    code: "public-boundary".to_owned(),
                    message: "public-boundary requires public_boundary field (non-blank string)"
                        .to_owned(),
                });
            } else if let Some(ref allowed) = allowed_boundary {
                if let Some(YamlValue::String(ref val)) = map.get("public_boundary") {
                    if !allowed.contains(&val.as_str()) {
                        diagnostics.push(Diagnostic {
                            path: path.clone(),
                            code: "boundary-value".to_owned(),
                            message: format!(
                                "public_boundary value not in policy-allowed set: {val}"
                            ),
                        });
                    }
                }
            }

            // --- policy-driven structural checks (only when policy is Some) ---

            // frontmatter required / allowed (DES-1)
            if let Some(ref required) = fm_required {
                for field in required {
                    if !map.contains_key(*field) {
                        diagnostics.push(Diagnostic {
                            path: path.clone(),
                            code: "fm-required".to_owned(),
                            message: format!("frontmatter missing required field: {field}"),
                        });
                    }
                }
            }
            // Per-doc effective allowed set: canonical-only docs also allow preserved_canonical_keys.
            let per_doc_allowed: &[&str] = if canonical_only
                .as_ref()
                .is_some_and(|co| co.contains(path.as_str()))
                && !fm_canonical_allowed.is_empty()
            {
                &fm_canonical_allowed
            } else {
                &fm_effective_allowed
            };
            if !per_doc_allowed.is_empty()
                && (fm_required.is_some() || fm_allowed.is_some() || fm_optional.is_some())
            {
                for key in map.keys() {
                    if !per_doc_allowed.contains(&key.as_str()) {
                        diagnostics.push(Diagnostic {
                            path: path.clone(),
                            code: "fm-disallowed".to_owned(),
                            message: format!("frontmatter field not in allowed set: {key}"),
                        });
                    }
                }
            }

            // public-path-allowlist: a public/public-candidate/external/publishable
            // document outside policy.public_boundary.path_allowlist fails.
            if let Some(ref allowlist) = path_allowlist {
                if let Some(YamlValue::String(ref pb_val)) = map.get("public_boundary") {
                    if PUBLIC_BOUNDARY_VALUES.contains(&pb_val.as_str()) {
                        let in_allowlist =
                            allowlist.iter().any(|prefix| path_under_root(path, prefix));
                        if !in_allowlist {
                            diagnostics.push(Diagnostic {
                                path: path.clone(),
                                code: "public-path-disallowed".to_owned(),
                                message: format!(
                                    "public document path not in path_allowlist: {path}"
                                ),
                            });
                        }
                    }
                }
            }

            // source-links prefix check (policy-driven)
            if let Some(prefix) = source_prefix {
                let links = extract_citation_links(doc.body());
                if links.iter().any(|link| !link.starts_with(prefix)) {
                    diagnostics.push(Diagnostic {
                        path: path.clone(),
                        code: "source-link-prefix".to_owned(),
                        message: format!("source link must start with {prefix}"),
                    });
                }
            }
        }

        // --- Post-loop document coverage checks (DES-2, DES-4) ---
        if let (Some(dc), Some(classified)) = (dc, &all_classified) {
            // Missing configured paths: native_bundle, canonical_documents, legacy_documents.
            for path in classified {
                if !bundle.docs().contains_key(*path) {
                    diagnostics.push(Diagnostic {
                        path: (*path).to_owned(),
                        code: "doc-missing".to_owned(),
                        message: format!("configured document path not found in bundle: {path}"),
                    });
                }
            }
            // Unclassified documents under declared roots (not in any classification, not excluded).
            for doc_path in bundle.docs().keys() {
                if let Some(ref excl) = excluded_set {
                    if excl.contains(doc_path.as_str()) {
                        continue;
                    }
                }
                if classified.contains(doc_path.as_str()) {
                    continue;
                }
                let is_under = dc.roots.iter().any(|root| path_under_root(doc_path, root));
                if is_under {
                    diagnostics.push(Diagnostic {
                        path: doc_path.clone(),
                        code: "doc-unclassified".to_owned(),
                        message: format!(
                            "document under declared root not classified in policy: {doc_path}"
                        ),
                    });
                }
            }

            // Migration state: strict forbids remaining legacy documents (CONTRACT-10).
            if let Some(fm) = policy.and_then(|p| p.frontmatter.as_ref()) {
                if fm.canonical_docs_frontmatter_state.as_deref() == Some("strict")
                    && !dc.legacy_documents.is_empty()
                {
                    let legacy_sorted: BTreeSet<&str> =
                        dc.legacy_documents.iter().map(String::as_str).collect();
                    for path in &legacy_sorted {
                        diagnostics.push(Diagnostic {
                            path: (*path).to_owned(),
                            code: "migration-strict-legacy".to_owned(),
                            message: format!(
                                "strict migration state forbids legacy documents: {path}"
                            ),
                        });
                    }
                }
            }
        }

        // --- Slice 1.2-B: native link, citation, stale, and orphan integrity (DES-2, DES-4, DES-10, CONTRACT-7, CONTRACT-10) ---
        if let Some(dc) = dc {
            // Build set of all valid link targets: native_bundle ∪ canonical_documents ∪ bridge_targets.
            let mut valid_targets: BTreeSet<&str> = BTreeSet::new();
            for p in &dc.native_bundle {
                valid_targets.insert(p.as_str());
            }
            for p in &dc.canonical_documents {
                valid_targets.insert(p.as_str());
            }
            for p in &dc.bridge_targets {
                valid_targets.insert(p.as_str());
            }

            // Track inbound native-bundle links for orphan detection.
            let mut native_inbound: BTreeSet<String> = BTreeSet::new();

            // Deterministic lexical root for orphan detection.
            let native_root = {
                let mut sorted: Vec<&str> = dc.native_bundle.iter().map(String::as_str).collect();
                sorted.sort();
                sorted.first().copied().map(|s| s.to_owned())
            };

            // Native-bundle link validation + orphan tracking.
            for nb_path in &dc.native_bundle {
                if let Some(doc) = bundle.docs().get(nb_path.as_str()) {
                    let native_links = extract_native_links(nb_path, doc.body());
                    for (raw_target, resolved) in &native_links {
                        match resolved {
                            None => {
                                if raw_target.starts_with('/') {
                                    diagnostics.push(Diagnostic {
                                        path: nb_path.clone(),
                                        code: "native-link-absolute".to_owned(),
                                        message: "native bundle link must be repository-relative"
                                            .to_owned(),
                                    });
                                } else {
                                    diagnostics.push(Diagnostic {
                                        path: nb_path.clone(),
                                        code: "native-link-traversal".to_owned(),
                                        message:
                                            "native bundle link unsafe or traverses above repo root"
                                                .to_owned(),
                                    });
                                }
                            }
                            Some(resolved_path) => {
                                if valid_targets.contains(resolved_path.as_str()) {
                                    if resolved_path.as_str() != nb_path.as_str()
                                        && dc.native_bundle.iter().any(|n| n == resolved_path)
                                    {
                                        native_inbound.insert(resolved_path.clone());
                                    }
                                } else {
                                    diagnostics.push(Diagnostic {
                                        path: nb_path.clone(),
                                        code: "native-link-unconfigured".to_owned(),
                                        message: "native bundle link target not in policy"
                                            .to_owned(),
                                    });
                                }
                            }
                        }
                    }
                }
            }

            // Orphan detection: native_bundle pages without inbound links (skip lexical root).
            if dc.native_bundle.len() > 1 {
                for nb_path in &dc.native_bundle {
                    if native_root.as_deref() == Some(nb_path.as_str()) {
                        continue;
                    }
                    if !native_inbound.contains(nb_path.as_str()) {
                        diagnostics.push(Diagnostic {
                            path: nb_path.clone(),
                            code: "native-orphan".to_owned(),
                            message: "native bundle page has no inbound links".to_owned(),
                        });
                    }
                }
            }

            // Citation validation for native_bundle and canonical_documents.
            let canonical_and_bridge: BTreeSet<&str> = dc
                .canonical_documents
                .iter()
                .chain(dc.bridge_targets.iter())
                .map(String::as_str)
                .collect();

            let validate_canonical = policy
                .and_then(|p| p.frontmatter.as_ref())
                .and_then(|fm| fm.canonical_docs_frontmatter_state.as_deref())
                != Some("legacy_baseline");
            for doc_path in dc
                .native_bundle
                .iter()
                .chain(dc.canonical_documents.iter().filter(|_| validate_canonical))
            {
                if let Some(doc) = bundle.docs().get(doc_path.as_str()) {
                    for cit in &extract_citations(doc.body()) {
                        // Unsafe (absolute, drive, UNC, NUL) targets fail without echoing target.
                        if cit.is_unsafe {
                            diagnostics.push(Diagnostic {
                                path: doc_path.clone(),
                                code: "citation-absolute".to_owned(),
                                message: "citation target must be repository-relative".to_owned(),
                            });
                            continue;
                        }
                        // Component-aware traversal check (not substring "..").
                        let has_traversal = Path::new(&cit.target)
                            .components()
                            .any(|c| c == std::path::Component::ParentDir);
                        if has_traversal {
                            diagnostics.push(Diagnostic {
                                path: doc_path.clone(),
                                code: "citation-absolute".to_owned(),
                                message: "citation target must be repository-relative".to_owned(),
                            });
                            continue;
                        }
                        if cit.line == 0 {
                            diagnostics.push(Diagnostic {
                                path: doc_path.clone(),
                                code: "citation-invalid-line".to_owned(),
                                message: "citation line must be a positive integer".to_owned(),
                            });
                            continue;
                        }
                        if !canonical_and_bridge.contains(cit.target.as_str()) {
                            diagnostics.push(Diagnostic {
                                path: doc_path.clone(),
                                code: "citation-unconfigured".to_owned(),
                                message:
                                    "citation target not in canonical_documents or bridge_targets"
                                        .to_owned(),
                            });
                            continue;
                        }
                        // Target is configured. Check Bundle presence and line range.
                        if let Some(target_doc) = bundle.docs().get(&cit.target) {
                            let line_count = target_doc.source().lines().count();
                            if cit.line > line_count {
                                diagnostics.push(Diagnostic {
                                    path: doc_path.clone(),
                                    code: "citation-out-of-range".to_owned(),
                                    message: "citation line exceeds target source line count"
                                        .to_owned(),
                                });
                            }
                        } else if cit.target.ends_with(".md")
                            || !dc.bridge_targets.contains(&cit.target)
                        {
                            // Missing Markdown target (or configured non-bridge target) → fail.
                            diagnostics.push(Diagnostic {
                                path: doc_path.clone(),
                                code: "citation-missing".to_owned(),
                                message: "configured citation target not in bundle".to_owned(),
                            });
                        }
                        // Non-Markdown bridge target accepted without Bundle materialization.
                    }
                }
            }

            // Stale detection for native_bundle and canonical_documents.
            for doc_path in dc
                .native_bundle
                .iter()
                .chain(dc.canonical_documents.iter().filter(|_| validate_canonical))
            {
                if let Some(doc) = bundle.docs().get(doc_path.as_str()) {
                    if doc.body().contains("STALE_CLAIM") {
                        diagnostics.push(Diagnostic {
                            path: doc_path.clone(),
                            code: "stale-body".to_owned(),
                            message: "body contains STALE_CLAIM marker".to_owned(),
                        });
                    }
                    if let Some(map) = doc.frontmatter().parsed() {
                        if let Some(YamlValue::String(s)) = map.get("freshness") {
                            if s.trim().eq_ignore_ascii_case("stale") {
                                diagnostics.push(Diagnostic {
                                    path: doc_path.clone(),
                                    code: "stale-frontmatter".to_owned(),
                                    message: "freshness field is stale".to_owned(),
                                });
                            }
                        }
                    }
                }
            }
        }

        // --- Slice 1.2-D: packet lifecycle & supersession integrity (AC-1, AC-2, AC-5, AC-7) ---
        let packet_report = PacketValidator::validate(bundle);
        for f in packet_report.findings {
            diagnostics.push(Diagnostic {
                path: f.path,
                code: f.code,
                message: f.message,
            });
        }

        let status = if diagnostics.is_empty() {
            ValidationStatus::Pass
        } else {
            ValidationStatus::Fail
        };
        ProfileOutcome {
            profile: BRAN_STRICT.to_owned(),
            status,
            diagnostics,
        }
    }
}

fn okf_diagnostic(
    path: &str,
    status: &ParseStatus,
    map: Option<&BTreeMap<String, YamlValue>>,
) -> Option<Diagnostic> {
    if !status.is_ok() {
        let reason = match status {
            ParseStatus::Malformed { reason } => reason,
            ParseStatus::Ok => unreachable!("successful status already returned"),
        };
        return Some(Diagnostic {
            path: path.to_owned(),
            code: "malformed-frontmatter".to_owned(),
            message: if reason.is_empty() {
                "malformed concept frontmatter".to_owned()
            } else {
                format!("malformed concept frontmatter: {reason}")
            },
        });
    }

    match map.and_then(|frontmatter| frontmatter.get("type")) {
        Some(YamlValue::String(value)) if !value.trim().is_empty() => None,
        Some(YamlValue::String(_)) => Some(Diagnostic {
            path: path.to_owned(),
            code: "blank-type".to_owned(),
            message: "type must be a non-blank string".to_owned(),
        }),
        Some(_) => Some(Diagnostic {
            path: path.to_owned(),
            code: "non-string-type".to_owned(),
            message: "type must be a string".to_owned(),
        }),
        None => Some(Diagnostic {
            path: path.to_owned(),
            code: "missing-type".to_owned(),
            message: "frontmatter must contain a non-empty type field".to_owned(),
        }),
    }
}

// Google Knowledge Catalog OKF SPEC.md §§3.1, 6, 7, 9, 11:
// https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md
// Reserved index.md files are headed link indexes; log.md files are flat,
// newest-first groups headed by ISO dates. These checks deliberately do not
// apply concept-frontmatter, unknown-type, or link-resolution rules.
fn okf_index_diagnostics(path: &str, body: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut saw_heading = false;
    let mut saw_link = false;
    let mut section_has_link = false;

    for line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if markdown_heading_text(line).is_some() {
            if saw_heading && !section_has_link {
                diagnostics.push(Diagnostic {
                    path: path.to_owned(),
                    code: "okf-index-empty-section".to_owned(),
                    message: "each reserved index section must group a markdown link entry"
                        .to_owned(),
                });
            }
            saw_heading = true;
            section_has_link = false;
        } else if markdown_link_entry(line) {
            saw_link = true;
            if saw_heading {
                section_has_link = true;
            } else {
                diagnostics.push(Diagnostic {
                    path: path.to_owned(),
                    code: "okf-index-link-before-heading".to_owned(),
                    message: "reserved index link entries must follow a section heading".to_owned(),
                });
            }
        }
    }

    if !saw_heading {
        diagnostics.push(Diagnostic {
            path: path.to_owned(),
            code: "okf-index-missing-heading".to_owned(),
            message: "reserved index must contain a headed section".to_owned(),
        });
    } else if !section_has_link {
        diagnostics.push(Diagnostic {
            path: path.to_owned(),
            code: "okf-index-empty-section".to_owned(),
            message: "each reserved index section must group a markdown link entry".to_owned(),
        });
    }
    if !saw_link {
        diagnostics.push(Diagnostic {
            path: path.to_owned(),
            code: "okf-index-missing-link".to_owned(),
            message: "reserved index must contain a markdown link entry".to_owned(),
        });
    }
    diagnostics
}

fn okf_log_diagnostics(path: &str, body: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut prior_date: Option<&str> = None;
    let mut saw_date_heading = false;
    let mut saw_title = false;
    let mut group_has_entry = false;

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(heading) = markdown_heading_text(line) {
            if !is_iso_date(heading) {
                if !saw_date_heading && !saw_title {
                    saw_title = true;
                } else {
                    diagnostics.push(Diagnostic {
                        path: path.to_owned(),
                        code: "okf-log-invalid-date-heading".to_owned(),
                        message: "reserved log headings must be ISO YYYY-MM-DD dates".to_owned(),
                    });
                }
                continue;
            }
            if saw_date_heading && !group_has_entry {
                diagnostics.push(Diagnostic {
                    path: path.to_owned(),
                    code: "okf-log-empty-date-group".to_owned(),
                    message: "each reserved log date heading must group an entry".to_owned(),
                });
            }
            if prior_date.is_some_and(|prior| heading >= prior) {
                diagnostics.push(Diagnostic {
                    path: path.to_owned(),
                    code: "okf-log-date-order".to_owned(),
                    message: "reserved log dates must be newest first".to_owned(),
                });
            }
            prior_date = Some(heading);
            saw_date_heading = true;
            group_has_entry = false;
        } else if saw_date_heading {
            group_has_entry = true;
        } else {
            diagnostics.push(Diagnostic {
                path: path.to_owned(),
                code: "okf-log-entry-before-date".to_owned(),
                message: "reserved log entries must follow an ISO date heading".to_owned(),
            });
        }
    }

    if !saw_date_heading {
        diagnostics.push(Diagnostic {
            path: path.to_owned(),
            code: "okf-log-missing-date-heading".to_owned(),
            message: "reserved log must contain an ISO date heading".to_owned(),
        });
    } else if !group_has_entry {
        diagnostics.push(Diagnostic {
            path: path.to_owned(),
            code: "okf-log-empty-date-group".to_owned(),
            message: "each reserved log date heading must group an entry".to_owned(),
        });
    }
    diagnostics
}

fn markdown_heading_text(line: &str) -> Option<&str> {
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    let text = line.get(hashes..)?.strip_prefix(' ')?.trim();
    (1..=6)
        .contains(&hashes)
        .then_some(text)
        .filter(|text| !text.is_empty())
}

fn markdown_link_entry(line: &str) -> bool {
    let Some(start) = line.find('[') else {
        return false;
    };
    let Some(label_end) = line[start + 1..].find(']') else {
        return false;
    };
    let target_start = start + label_end + 2;
    let Some(target) = line
        .get(target_start..)
        .and_then(|tail| tail.strip_prefix('('))
    else {
        return false;
    };
    target
        .find(')')
        .is_some_and(|end| !target[..end].trim().is_empty())
}

fn is_iso_date(date: &str) -> bool {
    let bytes = date.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    if [0, 1, 2, 3, 5, 6, 8, 9]
        .iter()
        .any(|index| !bytes[*index].is_ascii_digit())
    {
        return false;
    }
    let year = (bytes[0] - b'0') as u16 * 1000
        + (bytes[1] - b'0') as u16 * 100
        + (bytes[2] - b'0') as u16 * 10
        + (bytes[3] - b'0') as u16;
    let month = (bytes[5] - b'0') * 10 + bytes[6] - b'0';
    let day = (bytes[8] - b'0') * 10 + bytes[9] - b'0';
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

fn has_nonblank_string(map: &BTreeMap<String, YamlValue>, key: &str) -> bool {
    matches!(map.get(key), Some(YamlValue::String(s)) if !s.trim().is_empty())
}

fn has_status_in_policy(map: &BTreeMap<String, YamlValue>, allowed: &[&str]) -> bool {
    matches!(
        map.get("okf_status"),
        Some(YamlValue::String(s)) if allowed.contains(&s.as_str())
    )
}

/// Converts a YamlValue leaf to its string representation for tag-value matching.
fn yaml_value_to_string(v: &YamlValue) -> String {
    match v {
        YamlValue::String(s) => s.clone(),
        YamlValue::Number(n) => n.clone(),
        YamlValue::Bool(true) => "true".to_owned(),
        YamlValue::Bool(false) => "false".to_owned(),
        _ => String::new(),
    }
}

/// Extracts HTTP(S) markdown targets from the document's citations section.
fn extract_citation_links(body: &str) -> Vec<String> {
    let citations = body
        .find("# Citations")
        .or_else(|| body.find("Citations:"))
        .map_or("", |start| &body[start..]);
    let mut links = Vec::new();
    let bytes = citations.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Look for `](` pattern
        if bytes[i] == b']' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            let start = i + 2;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b')' {
                end += 1;
            }
            let link = std::str::from_utf8(&bytes[start..end]).unwrap_or("");
            // Only collect http/https links for source-link checking
            if link.starts_with("http") {
                links.push(link.to_owned());
            }
            i = end;
        }
        i += 1;
    }
    links
}

/// Extracts non-HTTP(S) Markdown link targets from native-bundle body text.
/// Returns (raw_target, resolved_path_or_none_if_invalid).
/// Skips HTTP(S), fragments, mailto, and image links.
fn extract_native_links(source_path: &str, body: &str) -> Vec<(String, Option<String>)> {
    let mut links = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b']' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            // Check if this is an image link: search back for '[' and check for preceding '!'.
            let mut bracket_start = i;
            let mut is_image = false;
            while bracket_start > 0 {
                bracket_start -= 1;
                if bytes[bracket_start] == b'[' {
                    is_image = bracket_start > 0 && bytes[bracket_start - 1] == b'!';
                    break;
                }
                if bytes[bracket_start] == b']' || bytes[bracket_start] == b'\n' {
                    break;
                }
            }
            if is_image {
                i += 1;
                continue;
            }
            let start = i + 2;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b')' {
                end += 1;
            }
            let raw = std::str::from_utf8(&bytes[start..end]).unwrap_or("");
            // Skip HTTP(S), fragments, mailto.
            if !raw.starts_with("http") && !raw.starts_with('#') && !raw.starts_with("mailto:") {
                let resolved = resolve_relative_link(source_path, raw);
                links.push((raw.to_owned(), resolved));
            }
            i = end;
        }
        i += 1;
    }
    links
}

/// Platform-neutral path safety check matching policy.rs rules:
/// rejects empty, NUL, Windows drive/UNC, and absolute paths.
fn is_path_safe(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    if path.contains('\0') {
        return false;
    }
    // Windows drive-letter absolute (C:\..., D:/...).
    if path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':' {
        return false;
    }
    // UNC (\\server\share...).
    if path.starts_with('\\') {
        return false;
    }
    // Unix absolute.
    if path.starts_with('/') {
        return false;
    }
    true
}

/// Resolve a repo-relative link target against the source document's directory.
/// Returns the normalized repo-relative path, or None for unsafe/invalid.
/// Uses a component stack: `../` is allowed as long as the stack never empties
/// (escape above repo root).  Windows drive/UNC/NUL targets are rejected.
fn resolve_relative_link(source_path: &str, target: &str) -> Option<String> {
    // Strip fragment before resolving.
    let path_part = target.split('#').next().unwrap_or(target);
    if !is_path_safe(path_part) {
        return None;
    }
    if path_part.is_empty() {
        return None;
    }
    // Resolve relative to the source document's directory.
    let source_dir = Path::new(source_path).parent().unwrap_or(Path::new("."));
    let resolved = source_dir.join(path_part);
    // Component stack: Pop on ParentDir, push on Normal.  Escape above root
    // (pop from empty stack) is the only rejection.
    let mut stack: Vec<&str> = Vec::new();
    for comp in resolved.components() {
        match comp {
            std::path::Component::ParentDir => {
                stack.pop()?;
            }
            std::path::Component::Normal(s) => {
                stack.push(s.to_str()?);
            }
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    if stack.is_empty() {
        None
    } else {
        Some(stack.join("/"))
    }
}

/// A parsed citation reference: `path.ext:line` with a positive integer line (0 = invalid).
/// `is_unsafe` is true when the target path is absolute, Windows-drive, UNC, or contains NUL.
struct Citation {
    target: String,
    line: usize,
    is_unsafe: bool,
}

/// Valid file extensions for citation targets.
const CITATION_EXTS: &[&str] = &[
    "md", "py", "ts", "tsx", "js", "mjs", "rs", "c", "h", "yaml", "yml", "json", "sh",
];

/// Extract citation references from body text: `repo-relative-path.ext:positive-line`.
/// Line zero means invalid (non-numeric, zero, or out of range).
/// Also captures `path.ext:` (trailing colon) and `path.ext:<non-numeric>` with line=0.
/// Unsafe (NUL, drive, UNC, absolute) targets are captured with `is_unsafe = true`.
fn extract_citations(body: &str) -> Vec<Citation> {
    let mut citations = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            // Backtrack to find path characters.
            let mut k = i;
            while k > 0 && is_citation_path_char(bytes[k - 1]) {
                k -= 1;
            }
            let path_str = std::str::from_utf8(&bytes[k..i]).unwrap_or("");
            if let Some(ext) = path_ext(path_str) {
                if CITATION_EXTS.contains(&ext) {
                    // Look ahead for digits.
                    let mut j = i + 1;
                    while j < bytes.len() && bytes[j].is_ascii_digit() {
                        j += 1;
                    }
                    let line: usize = if j > i + 1 {
                        let line_str = std::str::from_utf8(&bytes[i + 1..j]).unwrap_or("");
                        line_str.parse().unwrap_or(0)
                    } else {
                        0
                    };
                    citations.push(Citation {
                        target: path_str.to_owned(),
                        line,
                        is_unsafe: !is_path_safe(path_str),
                    });
                    i = j;
                    continue;
                }
            }
        }
        i += 1;
    }
    citations
}

fn is_citation_path_char(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || b == b'/'
        || b == b'.'
        || b == b'-'
        || b == b'_'
        || b == b'\\'
        || b == b':'
        || b == 0 // NUL
}

fn path_ext(path: &str) -> Option<&str> {
    let dot = path.rfind('.')?;
    let ext = &path[dot + 1..];
    if ext.is_empty() {
        None
    } else {
        Some(ext)
    }
}

fn has_tags_sequence(map: &BTreeMap<String, YamlValue>) -> bool {
    matches!(map.get("tags"), Some(YamlValue::Sequence(_)))
}

fn has_relationship_links(body: &str) -> bool {
    // Detect link syntax expressing relationships. Explicitly do not resolve or
    // validate targets: upstream OKF tolerates broken links; graph scope is later.
    body.contains("](")
}

/// Portable OKF conformance applies to the declared native bundle, not to
/// neighboring canonical or legacy documents that BRAN indexes for routing.
fn portable_document_participates(
    path: &str,
    coverage: Option<&crate::policy::DocumentCoveragePolicy>,
) -> bool {
    match coverage {
        Some(coverage) if !coverage.native_bundle.is_empty() => {
            coverage.native_bundle.iter().any(|item| item == path)
        }
        _ => document_participates(path, coverage),
    }
}

/// Return whether a scanned document belongs to the policy-selected bundle.
///
/// Repository scanning is intentionally broader than validation so query and
/// evidence commands can still see neighboring files.  Once coverage is
/// declared, profile conformance applies only to documents under a declared
/// root or documents explicitly classified by the policy.  Unrelated files
/// beside the bundle do not participate unless the policy names them.
fn document_participates(
    path: &str,
    coverage: Option<&crate::policy::DocumentCoveragePolicy>,
) -> bool {
    let Some(coverage) = coverage else {
        return true;
    };
    if coverage.excluded_documents.contains_key(path) {
        return false;
    }
    coverage.native_bundle.iter().any(|item| item == path)
        || coverage.canonical_documents.iter().any(|item| item == path)
        || coverage.legacy_documents.iter().any(|item| item == path)
        || coverage
            .roots
            .iter()
            .any(|root| path_under_root(path, root))
}

/// Component-aware repository-relative prefix match.
/// `root` of `"."` matches every path. Otherwise checks that path components
/// start with root components (directory-by-directory, not raw string prefix).
fn path_under_root(path: &str, root: &str) -> bool {
    if root == "." {
        return true;
    }
    let path_comps: Vec<_> = Path::new(path).components().collect();
    let root_comps: Vec<_> = Path::new(root).components().collect();
    path_comps.len() >= root_comps.len() && path_comps.starts_with(&root_comps)
}

/// Values of public_boundary that indicate a document crosses the public boundary.
const PUBLIC_BOUNDARY_VALUES: &[&str] = &["public", "public-candidate", "external", "publishable"];

/// Build a BTreeSet of &str from a slice of Strings for O(log n) membership.
fn str_set(items: &[String]) -> BTreeSet<&str> {
    items.iter().map(String::as_str).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{Bundle, Doc, Frontmatter};
    use crate::schema::YamlValue;
    use std::collections::BTreeMap;

    fn strict_gap_document(path: &str) -> Doc {
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        let raw = "---\ntype: Concept\n---\n";
        let body = "No strict readiness evidence.\n";
        Doc::new(
            path,
            format!("{raw}{body}"),
            body,
            Frontmatter::from_parsed(raw, fields),
        )
    }

    fn reserved_document(path: &str, body: &str) -> Doc {
        Doc::new(path, body, body, Frontmatter::empty())
    }

    #[test]
    fn p1_profiles() {
        let bundle = Bundle::from_documents([strict_gap_document("concepts/profile-gap.md")])
            .expect("single profile document");
        let result = ProfileValidator::validate(&bundle, BRAN_STRICT);
        let diagnostics = &result.bran_strict.diagnostics;

        assert_eq!(result.okf_compatibility.profile, OKF_V0_1);
        assert_eq!(result.okf_compatibility.status, ValidationStatus::Pass);
        assert!(result.okf_compatibility.diagnostics.is_empty());
        assert_eq!(result.bran_strict.profile, BRAN_STRICT);
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        assert_eq!(diagnostics.len(), 8);
        assert_eq!(diagnostics[0].code, "title");
        assert_eq!(diagnostics[1].code, "status");
        assert_eq!(diagnostics[2].code, "tag");
        assert_eq!(diagnostics[3].code, "freshness");
        assert_eq!(diagnostics[4].code, "authority");
        assert_eq!(diagnostics[5].code, "citation-source");
        assert_eq!(diagnostics[6].code, "relationship");
        assert_eq!(diagnostics[7].code, "public-boundary");
        assert!(!result.selected_passed());
        assert_eq!(result.exit_code(), 1);

        let strict_only = Bundle::from_documents([
            strict_gap_document("concepts/profile-gap.md"),
            reserved_document(
                "index.md",
                "# Contents\n\n- [Concept](concepts/concept.md)\n",
            ),
        ])
        .expect("strict-only bundle");
        let strict_only_result = ProfileValidator::validate(&strict_only, BRAN_STRICT);
        assert_eq!(
            strict_only_result.okf_compatibility.status,
            ValidationStatus::Pass
        );
        assert_eq!(
            strict_only_result.bran_strict.status,
            ValidationStatus::Fail
        );

        let portable_failure = Bundle::from_documents([
            strict_gap_document("concepts/profile-gap.md"),
            reserved_document("nested/index.md", "# Contents\n"),
        ])
        .expect("portable-failure bundle");
        let portable_failure_result = ProfileValidator::validate(&portable_failure, BRAN_STRICT);
        assert_eq!(
            portable_failure_result.okf_compatibility.status,
            ValidationStatus::Fail
        );
        assert_eq!(
            portable_failure_result.bran_strict.status,
            ValidationStatus::Fail
        );
        assert_eq!(
            portable_failure_result.okf_compatibility.diagnostics[0].code,
            "okf-index-empty-section"
        );
    }

    #[test]
    fn p1_conformance() {
        let fixture =
            include_str!("../../../fixtures/conformance/strict-gap-strict-selected.fixture");
        assert_eq!(
            fixture,
            concat!(
                "# Frozen normalized fixture syntax consumed only by profile.rs conformance tests.\n",
                "name=strict-gap-strict-selected\n",
                "selected_profile=bran-strict\n",
                "doc.path=concepts/strict-gap.md\n",
                "doc.source=---\\ntype: Concept\\n---\\nNo strict readiness evidence.\\n\n",
                "doc.body=No strict readiness evidence.\\n\n",
                "frontmatter.raw=---\\ntype: Concept\\n---\\n\n",
                "frontmatter.status=ok\n",
                "frontmatter.type=string:Concept\n",
                "expected.okf.status=pass\n",
                "expected.okf.codes=\n",
                "expected.strict.status=fail\n",
                "expected.strict.codes=title,status,tag,freshness,authority,citation-source,relationship,public-boundary\n",
                "expected.selected_exit=1\n",
                "expected.selection_code=\n"
            )
        );

        let bundle = Bundle::from_documents([strict_gap_document("concepts/strict-gap.md")])
            .expect("single frozen conformance document");
        let result = ProfileValidator::validate(&bundle, BRAN_STRICT);
        let diagnostics = &result.bran_strict.diagnostics;

        assert_eq!(result.okf_compatibility.status, ValidationStatus::Pass);
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        assert_eq!(result.exit_code(), 1);
        assert_eq!(diagnostics.len(), 8);
        assert_eq!(diagnostics[0].code, "title");
        assert_eq!(diagnostics[1].code, "status");
        assert_eq!(diagnostics[2].code, "tag");
        assert_eq!(diagnostics[3].code, "freshness");
        assert_eq!(diagnostics[4].code, "authority");
        assert_eq!(diagnostics[5].code, "citation-source");
        assert_eq!(diagnostics[6].code, "relationship");
        assert_eq!(diagnostics[7].code, "public-boundary");

        assert_eq!(
            include_str!("../../../fixtures/conformance/okf-v0.1-index-valid.fixture"),
            "# Root\n\n- [Nested index](missing/nested-index.md)\n\n## Concepts\n\n- [Concept](missing/concept.md)\n"
        );
        assert_eq!(
            include_str!("../../../fixtures/conformance/okf-v0.1-index-invalid.fixture"),
            "- [Before section](missing/before.md)\n\n# Empty section\n\n## Populated section\n\n- [After section](missing/after.md)\n"
        );
        assert_eq!(
            include_str!("../../../fixtures/conformance/okf-v0.1-log-valid.fixture"),
            "# Directory Update Log\n\n## 2026-07-23\n\n- Latest entry.\n\n## 2026-07-22\n\n- Earlier entry.\n"
        );
        assert_eq!(
            include_str!("../../../fixtures/conformance/okf-v0.1-log-invalid.fixture"),
            "- Entry before a date group.\n\n# Directory Update Log\n\n## July 23, 2026\n\n## 2026-07-20\n\n- Earlier entry.\n\n### Not a date\n\n## 2026-07-21\n\n- Out of order entry.\n"
        );

        let valid = Bundle::from_documents([
            reserved_document(
                "index.md",
                include_str!("../../../fixtures/conformance/okf-v0.1-index-valid.fixture"),
            ),
            reserved_document("nested/index.md", "# Nested\n\n- [Concept](concept.md)\n"),
            reserved_document(
                "log.md",
                include_str!("../../../fixtures/conformance/okf-v0.1-log-valid.fixture"),
            ),
        ])
        .expect("valid reserved documents");
        assert_eq!(
            ProfileValidator::validate(&valid, OKF_V0_1)
                .okf_compatibility
                .status,
            ValidationStatus::Pass
        );

        let invalid = Bundle::from_documents([
            reserved_document(
                "index.md",
                include_str!("../../../fixtures/conformance/okf-v0.1-index-invalid.fixture"),
            ),
            reserved_document(
                "log.md",
                include_str!("../../../fixtures/conformance/okf-v0.1-log-invalid.fixture"),
            ),
        ])
        .expect("invalid reserved documents");
        let invalid_result = ProfileValidator::validate(&invalid, OKF_V0_1);
        assert_eq!(
            invalid_result.okf_compatibility.status,
            ValidationStatus::Fail
        );
        let codes: Vec<_> = invalid_result
            .okf_compatibility
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert_eq!(
            codes,
            vec![
                "okf-index-link-before-heading",
                "okf-index-empty-section",
                "okf-log-entry-before-date",
                "okf-log-invalid-date-heading",
                "okf-log-invalid-date-heading",
                "okf-log-date-order"
            ]
        );

        let headingless_index = Bundle::from_documents([reserved_document(
            "nested/index.md",
            "- [Concept](concept.md)\n",
        )])
        .expect("headingless index");
        let headingless_result = ProfileValidator::validate(&headingless_index, OKF_V0_1);
        assert_eq!(
            headingless_result.okf_compatibility.diagnostics[0].code,
            "okf-index-link-before-heading"
        );
        assert_eq!(
            headingless_result.okf_compatibility.diagnostics[1].code,
            "okf-index-missing-heading"
        );
    }

    // ===================================================================
    // Slice 1.2 policy-driven tests (SEIT-3, SEIT-6)
    // ===================================================================

    /// Build a compliant concept document that satisfies the valid-v1 policy.
    fn compliant_document() -> Doc {
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        fields.insert(
            "tags".to_owned(),
            YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
        );
        fields.insert(
            "public_boundary".to_owned(),
            YamlValue::String("private".to_owned()),
        );
        fields.insert(
            "title".to_owned(),
            YamlValue::String("Test Concept".to_owned()),
        );
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://github.com/alphazede/repo".to_owned()),
        );
        fields.insert(
            "timestamp".to_owned(),
            YamlValue::String("2026-01-01".to_owned()),
        );
        let raw = "---\ntype: Concept\nokf_status: active\ntags:\n  - internal\npublic_boundary: private\ntitle: Test Concept\nresource: https://github.com/alphazede/repo\ntimestamp: 2026-01-01\n---\n";
        let body = "Body with a relationship [design](design.md).\n# Citations\n- [source](https://github.com/alphazede/repo)\n";
        Doc::new(
            "concepts/compliant.md",
            format!("{raw}{body}"),
            body,
            Frontmatter::from_parsed(raw, fields),
        )
    }

    fn valid_v1_policy() -> RepositoryPolicy {
        RepositoryPolicy {
            schema_version: "1".to_owned(),
            frontmatter: Some(crate::policy::FrontmatterPolicy {
                required: vec!["type".to_owned(), "okf_status".to_owned()],
                optional: vec![],
                allowed: vec![
                    "type".to_owned(),
                    "okf_status".to_owned(),
                    "tags".to_owned(),
                    "public_boundary".to_owned(),
                    "title".to_owned(),
                    "resource".to_owned(),
                    "timestamp".to_owned(),
                ],
                preserved_canonical_keys: vec![],
                canonical_docs_frontmatter_state: None,
            }),
            coverage: Some(vec!["canonical".to_owned()]),
            document_coverage: None,
            source_links: Some(crate::policy::SourceLinksPolicy {
                require_prefix: "https://github.com/alphazede/".to_owned(),
            }),
            tags: Some(crate::policy::TagsPolicy {
                allowed: vec!["internal".to_owned(), "public".to_owned()],
            }),
            status: Some(crate::policy::StatusPolicy {
                allowed: vec![
                    "draft".to_owned(),
                    "active".to_owned(),
                    "deprecated".to_owned(),
                ],
            }),
            public_boundary: Some(crate::policy::PublicBoundaryPolicy {
                values: vec!["public".to_owned(), "private".to_owned()],
                path_allowlist: vec![],
                exact_allowlist: BTreeMap::new(),
            }),
        }
    }

    #[test]
    fn policy_compliant_document_passes() {
        let policy = valid_v1_policy();
        let bundle = Bundle::from_documents([compliant_document()]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Pass);
        assert!(result.bran_strict.diagnostics.is_empty());
        assert!(result.selected_passed());
        assert_eq!(result.exit_code(), 0);
    }

    #[test]
    fn policy_excluded_document_skips_okf_compatibility() {
        let mut policy = valid_v1_policy();
        policy.document_coverage = Some(crate::policy::DocumentCoveragePolicy {
            roots: vec!["concepts".to_owned()],
            native_bundle: vec![],
            canonical_documents: vec![],
            legacy_documents: vec![],
            excluded_documents: [("concepts/excluded.md".to_owned(), "fixture".to_owned())]
                .into_iter()
                .collect(),
            bridge_targets: vec![],
        });
        let doc = Doc::new(
            "concepts/excluded.md",
            "body",
            "body",
            Frontmatter::from_parsed("", BTreeMap::new()),
        );
        let bundle = Bundle::from_documents([doc]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, OKF_V0_1, Some(&policy));
        assert_eq!(result.okf_compatibility.status, ValidationStatus::Pass);
    }

    #[test]
    fn policy_ignores_unrelated_documents_outside_declared_roots() {
        let mut policy = valid_v1_policy();
        policy.document_coverage = Some(crate::policy::DocumentCoveragePolicy {
            roots: vec!["docs/bran".to_owned()],
            native_bundle: vec![],
            canonical_documents: vec![],
            legacy_documents: vec![],
            excluded_documents: BTreeMap::new(),
            bridge_targets: vec![],
        });
        let unrelated = Doc::new(
            "notes/unrelated.md",
            "plain markdown without frontmatter",
            "plain markdown without frontmatter",
            Frontmatter::from_parsed("", BTreeMap::new()),
        );
        let bundle = Bundle::from_documents([unrelated]).expect("unique");

        let okf = ProfileValidator::validate_with_policy(&bundle, OKF_V0_1, Some(&policy));
        assert_eq!(okf.okf_compatibility.status, ValidationStatus::Pass);
        let strict = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(strict.bran_strict.status, ValidationStatus::Pass);
    }

    #[test]
    fn policy_validates_explicit_document_outside_declared_roots() {
        let mut policy = valid_v1_policy();
        policy.document_coverage = Some(crate::policy::DocumentCoveragePolicy {
            roots: vec!["docs/bran".to_owned()],
            native_bundle: vec![],
            canonical_documents: vec!["notes/explicit.md".to_owned()],
            legacy_documents: vec![],
            excluded_documents: BTreeMap::new(),
            bridge_targets: vec![],
        });
        let explicit = Doc::new(
            "notes/explicit.md",
            "plain markdown without frontmatter",
            "plain markdown without frontmatter",
            Frontmatter::from_parsed("", BTreeMap::new()),
        );
        let bundle = Bundle::from_documents([explicit]).expect("unique");

        let okf = ProfileValidator::validate_with_policy(&bundle, OKF_V0_1, Some(&policy));
        assert_eq!(okf.okf_compatibility.status, ValidationStatus::Fail);
        let strict = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(strict.bran_strict.status, ValidationStatus::Fail);
    }

    #[test]
    fn policy_frontmatter_disallowed_field_fails() {
        let policy = valid_v1_policy();
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        fields.insert("bogus".to_owned(), YamlValue::String("nope".to_owned()));
        fields.insert("title".to_owned(), YamlValue::String("X".to_owned()));
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://github.com/alphazede/r".to_owned()),
        );
        fields.insert(
            "timestamp".to_owned(),
            YamlValue::String("2026-01-01".to_owned()),
        );
        fields.insert(
            "tags".to_owned(),
            YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
        );
        fields.insert(
            "public_boundary".to_owned(),
            YamlValue::String("private".to_owned()),
        );
        let raw = "---\ntype: Concept\nokf_status: active\nbogus: nope\n---\n";
        let doc = Doc::new(
            "concepts/bogus.md",
            format!("{raw}Body with [link](https://github.com/alphazede/r).\n# Citations\n"),
            "Body with [link](https://github.com/alphazede/r).\n# Citations\n",
            Frontmatter::from_parsed(raw, fields),
        );
        let bundle = Bundle::from_documents([doc]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"fm-disallowed"),
            "expected fm-disallowed in {:?}",
            codes
        );
    }

    #[test]
    fn policy_source_link_prefix_violation_fails() {
        let policy = valid_v1_policy();
        let body = "Body with [design](design.md).\n# Citations\n- [bad source](https://evil.com/payload)\n";
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        fields.insert(
            "tags".to_owned(),
            YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
        );
        fields.insert(
            "public_boundary".to_owned(),
            YamlValue::String("private".to_owned()),
        );
        fields.insert("title".to_owned(), YamlValue::String("Bad Link".to_owned()));
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://github.com/alphazede/repo".to_owned()),
        );
        fields.insert(
            "timestamp".to_owned(),
            YamlValue::String("2026-01-01".to_owned()),
        );
        let doc = Doc::new(
            "concepts/bad-link.md",
            format!("---\ntype: Concept\nokf_status: active\n---\n{body}"),
            body,
            Frontmatter::from_parsed("---\ntype: Concept\nokf_status: active\n---\n", fields),
        );
        let bundle = Bundle::from_documents([doc]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"source-link-prefix"),
            "expected source-link-prefix in {:?}",
            codes
        );
    }

    #[test]
    fn policy_public_boundary_value_violation_fails() {
        let policy = valid_v1_policy();
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        fields.insert(
            "tags".to_owned(),
            YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
        );
        fields.insert(
            "public_boundary".to_owned(),
            YamlValue::String("classified".to_owned()),
        );
        fields.insert(
            "title".to_owned(),
            YamlValue::String("Bad Boundary".to_owned()),
        );
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://github.com/alphazede/r".to_owned()),
        );
        fields.insert(
            "timestamp".to_owned(),
            YamlValue::String("2026-01-01".to_owned()),
        );
        let doc = Doc::new(
            "concepts/bad-boundary.md",
            "---\ntype: Concept\n---\nBody with [link](https://github.com/alphazede/r).\n"
                .to_owned(),
            "Body with [link](https://github.com/alphazede/r).\n",
            Frontmatter::from_parsed("---\ntype: Concept\n---\n", fields),
        );
        let bundle = Bundle::from_documents([doc]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"boundary-value"),
            "expected boundary-value in {:?}",
            codes
        );
    }

    #[test]
    fn policy_tag_value_violation_fails() {
        let policy = valid_v1_policy();
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        fields.insert(
            "tags".to_owned(),
            YamlValue::Sequence(vec![YamlValue::String("secret".to_owned())]),
        );
        fields.insert(
            "public_boundary".to_owned(),
            YamlValue::String("private".to_owned()),
        );
        fields.insert("title".to_owned(), YamlValue::String("Bad Tag".to_owned()));
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://github.com/alphazede/r".to_owned()),
        );
        fields.insert(
            "timestamp".to_owned(),
            YamlValue::String("2026-01-01".to_owned()),
        );
        let doc = Doc::new(
            "concepts/bad-tag.md",
            "---\ntype: Concept\n---\nBody with [link](https://github.com/alphazede/r).\n# Citations\n".to_owned(),
            "Body with [link](https://github.com/alphazede/r).\n# Citations\n",
            Frontmatter::from_parsed("---\ntype: Concept\n---\n", fields),
        );
        let bundle = Bundle::from_documents([doc]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"tag-value"),
            "expected tag-value in {:?}",
            codes
        );
    }

    /// Deterministic diagnostics: repeated validation yields identical ordered output.
    #[test]
    fn policy_diagnostics_deterministic() {
        let policy = valid_v1_policy();
        let mut docs = vec![compliant_document()];
        // Add a second document with violations (different path for lexical determinism)
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("retired".to_owned()),
        );
        fields.insert(
            "title".to_owned(),
            YamlValue::String("Violation".to_owned()),
        );
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://evil.com/x".to_owned()),
        );
        fields.insert(
            "timestamp".to_owned(),
            YamlValue::String("2026-01-01".to_owned()),
        );
        let vdoc = Doc::new(
            "concepts/violation.md",
            "---\ntype: Concept\n---\nBody with [bad](https://evil.com/x).\n",
            "Body with [bad](https://evil.com/x).\n",
            Frontmatter::from_parsed("---\ntype: Concept\n---\n", fields),
        );
        docs.push(vdoc);
        let bundle = Bundle::from_documents(docs).expect("unique");
        let r1 = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        let r2 = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(r1, r2, "validation must be deterministic");
        // Diagnostics sorted by path then code
        let codes1: Vec<&str> = r1
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        let codes2: Vec<&str> = r2
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert_eq!(codes1, codes2);
    }

    // ===================================================================
    // Slice 1.2-A document coverage, migration, preserved-canonical,
    // and public-path-allowlist tests (DES-2, DES-4, DES-10, CONTRACT-7, CONTRACT-10)
    // ===================================================================

    fn policy_with_coverage() -> RepositoryPolicy {
        let mut p = valid_v1_policy();
        p.document_coverage = Some(crate::policy::DocumentCoveragePolicy {
            roots: vec!["concepts".to_owned()],
            native_bundle: vec!["concepts/compliant.md".to_owned()],
            canonical_documents: vec!["concepts/canon.md".to_owned()],
            legacy_documents: vec!["concepts/old.md".to_owned()],
            excluded_documents: {
                let mut m = BTreeMap::new();
                m.insert(
                    "concepts/skip.md".to_owned(),
                    "excluded for testing".to_owned(),
                );
                m
            },
            bridge_targets: vec![],
        });
        p
    }

    fn concept_doc(path: &str, title: &str, pb_val: &str) -> Doc {
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        fields.insert(
            "tags".to_owned(),
            YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
        );
        fields.insert(
            "public_boundary".to_owned(),
            YamlValue::String(pb_val.to_owned()),
        );
        fields.insert("title".to_owned(), YamlValue::String(title.to_owned()));
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://github.com/alphazede/repo".to_owned()),
        );
        fields.insert(
            "timestamp".to_owned(),
            YamlValue::String("2026-01-01".to_owned()),
        );
        let raw = format!(
            "---\ntype: Concept\nokf_status: active\ntags:\n  - internal\npublic_boundary: {pb_val}\ntitle: {title}\nresource: https://github.com/alphazede/repo\ntimestamp: 2026-01-01\n---\n"
        );
        let body = format!(
            "Body for {title} with [link](https://github.com/alphazede/repo).\n# Citations\n"
        );
        Doc::new(
            path,
            format!("{raw}{body}"),
            body,
            Frontmatter::from_parsed(&raw, fields),
        )
    }

    /// Positive: fully classified canonical/native documents pass.
    #[test]
    fn coverage_fully_classified_passes() {
        let policy = policy_with_coverage();
        let bundle = Bundle::from_documents([
            concept_doc("concepts/compliant.md", "Compliant", "private"),
            concept_doc("concepts/canon.md", "Canonical", "private"),
            concept_doc("concepts/old.md", "Old", "private"),
        ])
        .expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Pass);
        assert!(result.bran_strict.diagnostics.is_empty());
    }

    /// Negative: unclassified in-scope path fails with doc-unclassified.
    #[test]
    fn coverage_unclassified_fails() {
        let policy = policy_with_coverage();
        let bundle = Bundle::from_documents([
            concept_doc("concepts/compliant.md", "Compliant", "private"),
            concept_doc("concepts/unlisted.md", "Unlisted", "private"),
        ])
        .expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"doc-unclassified"),
            "expected doc-unclassified in {:?}",
            codes
        );
    }

    /// Negative: configured missing path fails with doc-missing.
    #[test]
    fn coverage_missing_configured_fails() {
        let policy = policy_with_coverage();
        let bundle =
            Bundle::from_documents([concept_doc("concepts/compliant.md", "Compliant", "private")])
                .expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"doc-missing"),
            "expected doc-missing for configured canonical path not in bundle: {:?}",
            codes
        );
    }

    /// Positive: excluded documents do not fail strict checks.
    #[test]
    fn coverage_excluded_skipped() {
        let policy = policy_with_coverage();
        // Create an excluded doc with zero strict readiness (no title, no resource, etc.).
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        let skip_doc = Doc::new(
            "concepts/skip.md",
            "---\ntype: Concept\n---\nNo strict evidence.\n",
            "No strict evidence.\n",
            Frontmatter::from_parsed("---\ntype: Concept\n---\n", fields),
        );
        let bundle = Bundle::from_documents([
            concept_doc("concepts/compliant.md", "Compliant", "private"),
            concept_doc("concepts/canon.md", "Canonical", "private"),
            concept_doc("concepts/old.md", "Old", "private"),
            skip_doc,
        ])
        .expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        // skip_doc is excluded — no diagnostic on it.
        assert_eq!(result.bran_strict.status, ValidationStatus::Pass);
        assert!(result.bran_strict.diagnostics.is_empty());
    }

    /// Negative: strict migration state with legacy documents fails.
    #[test]
    fn migration_strict_with_legacy_fails() {
        let mut policy = policy_with_coverage();
        if let Some(ref mut fm) = policy.frontmatter {
            fm.canonical_docs_frontmatter_state = Some("strict".to_owned());
        }
        let bundle = Bundle::from_documents([
            concept_doc("concepts/compliant.md", "Compliant", "private"),
            concept_doc("concepts/canon.md", "Canonical", "private"),
            concept_doc("concepts/old.md", "Old", "private"),
        ])
        .expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"migration-strict-legacy"),
            "expected migration-strict-legacy in {:?}",
            codes
        );
    }

    /// Permutation proof: two strict policies with differently ordered legacy_documents
    /// produce identical strict diagnostics (SEIT-17).
    #[test]
    fn legacy_documents_permutation_identical_strict_diagnostics() {
        // Policy A: legacy_documents in one order.
        let mut policy_a = policy_with_coverage();
        if let Some(ref mut dc) = policy_a.document_coverage {
            dc.legacy_documents =
                vec!["concepts/old.md".to_owned(), "concepts/other.md".to_owned()];
        }
        // Policy B: same legacy_documents, reversed order.
        let mut policy_b = policy_with_coverage();
        if let Some(ref mut dc) = policy_b.document_coverage {
            dc.legacy_documents =
                vec!["concepts/other.md".to_owned(), "concepts/old.md".to_owned()];
        }
        // Strict state so legacy documents trigger diagnostics.
        if let Some(ref mut fm) = policy_a.frontmatter {
            fm.canonical_docs_frontmatter_state = Some("strict".to_owned());
        }
        if let Some(ref mut fm) = policy_b.frontmatter {
            fm.canonical_docs_frontmatter_state = Some("strict".to_owned());
        }
        let bundle = Bundle::from_documents([
            concept_doc("concepts/compliant.md", "Compliant", "private"),
            concept_doc("concepts/canon.md", "Canonical", "private"),
            concept_doc("concepts/old.md", "Old", "private"),
            concept_doc("concepts/other.md", "Other", "private"),
        ])
        .expect("unique");
        let result_a =
            ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy_a));
        let result_b =
            ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy_b));
        assert_eq!(
            result_a.bran_strict, result_b.bran_strict,
            "strict diagnostics must be identical regardless of legacy_documents ordering"
        );
        // Also verify both fail as expected.
        assert_eq!(result_a.bran_strict.status, ValidationStatus::Fail);
        assert_eq!(result_b.bran_strict.status, ValidationStatus::Fail);
    }

    /// Positive: legacy_baseline state can represent legacy documents.
    #[test]
    fn migration_legacy_baseline_with_legacy_passes() {
        let policy = policy_with_coverage(); // already legacy_baseline from valid_v1_policy
        let bundle = Bundle::from_documents([
            concept_doc("concepts/compliant.md", "Compliant", "private"),
            concept_doc("concepts/canon.md", "Canonical", "private"),
            concept_doc("concepts/old.md", "Old", "private"),
        ])
        .expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Pass);
        assert!(result.bran_strict.diagnostics.is_empty());
    }

    /// Positive: canonical-only doc with preserved_canonical_keys passes.
    #[test]
    fn preserved_canonical_keys_allowed_for_canonical_only() {
        let mut policy = policy_with_coverage();
        if let Some(ref mut fm) = policy.frontmatter {
            fm.preserved_canonical_keys =
                vec!["description".to_owned(), "allowed-tools".to_owned()];
        }
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        fields.insert(
            "tags".to_owned(),
            YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
        );
        fields.insert(
            "public_boundary".to_owned(),
            YamlValue::String("private".to_owned()),
        );
        fields.insert("title".to_owned(), YamlValue::String("Canon".to_owned()));
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://github.com/alphazede/repo".to_owned()),
        );
        fields.insert(
            "timestamp".to_owned(),
            YamlValue::String("2026-01-01".to_owned()),
        );
        fields.insert(
            "description".to_owned(),
            YamlValue::String("preserved key".to_owned()),
        );
        fields.insert(
            "allowed-tools".to_owned(),
            YamlValue::String("bash".to_owned()),
        );
        let raw = "---\ntype: Concept\nokf_status: active\ntags:\n  - internal\npublic_boundary: private\ntitle: Canon\nresource: https://github.com/alphazede/repo\ntimestamp: 2026-01-01\ndescription: preserved key\nallowed-tools: bash\n---\n";
        let body = "Body with [link](https://github.com/alphazede/repo).\n# Citations\n";
        let canon_doc = Doc::new(
            "concepts/canon.md",
            format!("{raw}{body}"),
            body,
            Frontmatter::from_parsed(raw, fields),
        );
        let bundle = Bundle::from_documents([
            concept_doc("concepts/compliant.md", "Compliant", "private"),
            concept_doc("concepts/old.md", "Old", "private"),
            canon_doc,
        ])
        .expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Pass);
        assert!(result.bran_strict.diagnostics.is_empty());
    }

    /// Negative: non-canonical (native bundle) doc with preserved_canonical_keys field fails.
    #[test]
    fn preserved_canonical_keys_disallowed_for_native() {
        let mut policy = policy_with_coverage();
        if let Some(ref mut fm) = policy.frontmatter {
            fm.preserved_canonical_keys = vec!["description".to_owned()];
        }
        // Add `description` field to a native_bundle doc (compliant.md).
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        fields.insert(
            "tags".to_owned(),
            YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
        );
        fields.insert(
            "public_boundary".to_owned(),
            YamlValue::String("private".to_owned()),
        );
        fields.insert("title".to_owned(), YamlValue::String("Native".to_owned()));
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://github.com/alphazede/repo".to_owned()),
        );
        fields.insert(
            "timestamp".to_owned(),
            YamlValue::String("2026-01-01".to_owned()),
        );
        fields.insert(
            "description".to_owned(),
            YamlValue::String("not allowed here".to_owned()),
        );
        let raw = "---\ntype: Concept\nokf_status: active\ntags:\n  - internal\npublic_boundary: private\ntitle: Native\nresource: https://github.com/alphazede/repo\ntimestamp: 2026-01-01\ndescription: not allowed here\n---\n";
        let body = "Body with [link](https://github.com/alphazede/repo).\n# Citations\n";
        let native_doc = Doc::new(
            "concepts/compliant.md",
            format!("{raw}{body}"),
            body,
            Frontmatter::from_parsed(raw, fields),
        );
        let bundle = Bundle::from_documents([native_doc]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"fm-disallowed"),
            "expected fm-disallowed for preserved key on native bundle doc: {:?}",
            codes
        );
    }

    /// Positive: public document in path_allowlist passes.
    #[test]
    fn public_path_allowlisted_passes() {
        let mut policy = policy_with_coverage();
        policy.public_boundary = Some(crate::policy::PublicBoundaryPolicy {
            values: vec!["public".to_owned(), "private".to_owned()],
            path_allowlist: vec!["concepts".to_owned()],
            exact_allowlist: BTreeMap::new(),
        });
        // Clear other classification lists; only test path-allowlist for a single native_bundle doc.
        if let Some(ref mut dc) = policy.document_coverage {
            dc.native_bundle = vec!["concepts/compliant.md".to_owned()];
            dc.canonical_documents.clear();
            dc.legacy_documents.clear();
        }
        let bundle =
            Bundle::from_documents([concept_doc("concepts/compliant.md", "Compliant", "public")])
                .expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Pass);
        assert!(result.bran_strict.diagnostics.is_empty());
    }

    /// Negative: public document outside path_allowlist fails.
    #[test]
    fn public_path_disallowed_fails() {
        let mut policy = policy_with_coverage();
        policy.public_boundary = Some(crate::policy::PublicBoundaryPolicy {
            values: vec!["public".to_owned(), "private".to_owned()],
            path_allowlist: vec!["other".to_owned()],
            exact_allowlist: BTreeMap::new(),
        });
        if let Some(ref mut dc) = policy.document_coverage {
            dc.native_bundle = vec!["concepts/compliant.md".to_owned()];
            dc.canonical_documents.clear();
            dc.legacy_documents.clear();
        }
        let bundle =
            Bundle::from_documents([concept_doc("concepts/compliant.md", "Compliant", "public")])
                .expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"public-path-disallowed"),
            "expected public-path-disallowed in {:?}",
            codes
        );
    }

    /// Negative: public document with empty path_allowlist fails (defect 1 fix).
    #[test]
    fn public_path_empty_allowlist_fails() {
        let mut policy = policy_with_coverage();
        policy.public_boundary = Some(crate::policy::PublicBoundaryPolicy {
            values: vec!["public".to_owned(), "private".to_owned()],
            path_allowlist: vec![],
            exact_allowlist: BTreeMap::new(),
        });
        if let Some(ref mut dc) = policy.document_coverage {
            dc.native_bundle = vec!["concepts/compliant.md".to_owned()];
            dc.canonical_documents.clear();
            dc.legacy_documents.clear();
        }
        let bundle =
            Bundle::from_documents([concept_doc("concepts/compliant.md", "Compliant", "public")])
                .expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"public-path-disallowed"),
            "expected public-path-disallowed for empty allowlist, got {:?}",
            codes
        );
    }

    /// Negative: unclassified reserved doc (index.md) under coverage root fails (defect 2 fix).
    #[test]
    fn coverage_unclassified_reserved_fails() {
        let mut policy = policy_with_coverage();
        if let Some(ref mut dc) = policy.document_coverage {
            dc.native_bundle = vec!["concepts/compliant.md".to_owned()];
            dc.canonical_documents.clear();
            dc.legacy_documents.clear();
        }
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        let index_doc = Doc::new(
            "concepts/index.md",
            "---\ntype: Concept\n---\nreserved index body.\n",
            "reserved index body.\n",
            Frontmatter::from_parsed("---\ntype: Concept\n---\n", fields),
        );
        let bundle = Bundle::from_documents([
            concept_doc("concepts/compliant.md", "Compliant", "private"),
            index_doc,
        ])
        .expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"doc-unclassified"),
            "expected doc-unclassified for reserved unclassified index.md, got {:?}",
            codes
        );
    }

    /// Positive: a legacy-baseline legacy document with absent frontmatter is
    /// outside both native strict and portable bundle conformance.
    #[test]
    fn migration_legacy_baseline_absent_metadata_passes() {
        let policy = policy_with_coverage();
        // Legacy doc with Frontmatter::empty() — no raw, no fields, Ok parse status.
        let legacy_doc = Doc::new(
            "concepts/old.md",
            "Minimal legacy body.\n",
            "Minimal legacy body.\n",
            Frontmatter::empty(),
        );
        let bundle = Bundle::from_documents([
            concept_doc("concepts/compliant.md", "Compliant", "private"),
            concept_doc("concepts/canon.md", "Canonical", "private"),
            legacy_doc,
        ])
        .expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        // BRAN strict: legacy_baseline skips every per-document diagnostic.
        assert_eq!(result.bran_strict.status, ValidationStatus::Pass);
        assert!(result.bran_strict.diagnostics.is_empty());
        assert_eq!(result.okf_compatibility.status, ValidationStatus::Pass);
        assert!(result.okf_compatibility.diagnostics.is_empty());
    }

    #[test]
    fn migration_legacy_baseline_canonical_absent_metadata_passes() {
        let mut policy = policy_with_coverage();
        policy
            .frontmatter
            .as_mut()
            .unwrap()
            .canonical_docs_frontmatter_state = Some("legacy_baseline".to_owned());
        let canonical_doc = Doc::new(
            "concepts/canon.md",
            "Canonical body without migrated metadata.\n",
            "Canonical body without migrated metadata.\n",
            Frontmatter::empty(),
        );
        let bundle = Bundle::from_documents([
            concept_doc("concepts/compliant.md", "Compliant", "private"),
            canonical_doc,
            concept_doc("concepts/old.md", "Old", "private"),
        ])
        .expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Pass);
        assert!(result.bran_strict.diagnostics.is_empty());
        assert_eq!(result.okf_compatibility.status, ValidationStatus::Pass);
    }

    /// Policy parity conformance: frozen fixture dictates expected outcomes.
    #[test]
    fn policy_parity_conformance() {
        let fixture = include_str!("../../../fixtures/conformance/bran-policy-parity.fixture");
        assert_eq!(
            fixture,
            concat!(
                "# Frozen policy-parity conformance fixture (Slice 1.2 SEIT-3, SEIT-6, SEIT-17).\n",
                "# consumed only by profile.rs policy_parity_conformance test.\n",
                "name=bran-policy-parity\n",
                "selected_profile=bran-strict\n",
                "policy.schema_version=1\n",
                "policy.frontmatter.required=type,okf_status\n",
                "policy.frontmatter.allowed=type,okf_status,tags,public_boundary,title,resource,timestamp\n",
                "policy.coverage=canonical\n",
                "policy.source_links.require_prefix=https://github.com/alphazede/\n",
                "policy.tags.allowed=internal,public\n",
                "policy.status.allowed=draft,active,deprecated\n",
                "policy.public_boundary.values=public,private\n",
                "doc.compliant.path=concepts/compliant.md\n",
                "doc.compliant.expected=pass\n",
                "doc.compliant.codes=\n",
                "doc.disallowed.path=concepts/disallowed.md\n",
                "doc.disallowed.expected=fail\n",
                "doc.disallowed.codes=fm-disallowed\n",
                "doc.bad-link.path=concepts/bad-link.md\n",
                "doc.bad-link.expected=fail\n",
                "doc.bad-link.codes=source-link-prefix\n",
                "doc.bad-boundary.path=concepts/bad-boundary.md\n",
                "doc.bad-boundary.expected=fail\n",
                "doc.bad-boundary.codes=boundary-value\n",
                "doc.bad-tag.path=concepts/bad-tag.md\n",
                "doc.bad-tag.expected=fail\n",
                "doc.bad-tag.codes=tag-value\n",
                "doc.coverage-classified.path=concepts/classified.md\n",
                "doc.coverage-classified.expected=pass\n",
                "doc.coverage-classified.codes=\n",
                "doc.unclassified.path=concepts/unlisted.md\n",
                "doc.unclassified.expected=fail\n",
                "doc.unclassified.codes=doc-unclassified\n",
                "doc.missing-configured.path=concepts/missing.md\n",
                "doc.missing-configured.expected=fail\n",
                "doc.missing-configured.codes=doc-missing\n",
                "doc.excluded-skipped.path=concepts/skip.md\n",
                "doc.excluded-skipped.expected=pass\n",
                "doc.excluded-skipped.codes=\n",
                "doc.strict-legacy.path=concepts/old.md\n",
                "doc.strict-legacy.expected=fail\n",
                "doc.strict-legacy.codes=migration-strict-legacy\n",
                "doc.legacy-baseline.path=concepts/old.md\n",
                "doc.legacy-baseline.expected=pass\n",
                "doc.legacy-baseline.codes=\n",
                "doc.canonical-preserved-keys.path=concepts/canon.md\n",
                "doc.canonical-preserved-keys.expected=pass\n",
                "doc.canonical-preserved-keys.codes=\n",
                "doc.native-preserved-key.path=concepts/compliant.md\n",
                "doc.native-preserved-key.expected=fail\n",
                "doc.native-preserved-key.codes=fm-disallowed\n",
                "doc.public-allowlisted.path=concepts/public-ok.md\n",
                "doc.public-allowlisted.expected=pass\n",
                "doc.public-allowlisted.codes=\n",
                "doc.public-disallowed.path=concepts/public-bad.md\n",
                "doc.public-disallowed.expected=fail\n",
                "doc.public-disallowed.codes=public-path-disallowed\n",
                "doc.empty-allowlist.path=concepts/public-bad-empty.md\n",
                "doc.empty-allowlist.expected=fail\n",
                "doc.empty-allowlist.codes=public-path-disallowed\n",
                "doc.reserved-unclassified.path=concepts/index.md\n",
                "doc.reserved-unclassified.expected=fail\n",
                "doc.reserved-unclassified.codes=doc-unclassified\n",
                "doc.legacy-baseline-skip.path=concepts/old.md\n",
                "doc.legacy-baseline-skip.expected=pass\n",
                "doc.legacy-baseline-skip.codes=\n",
                "doc.native-linked.path=concepts/linked.md\n",
                "doc.native-linked.expected=pass\n",
                "doc.native-linked.codes=\n",
                "doc.native-link-unconfigured.path=concepts/bad-native-link.md\n",
                "doc.native-link-unconfigured.expected=fail\n",
                "doc.native-link-unconfigured.codes=native-link-unconfigured\n",
                "doc.native-link-traversal.path=concepts/bad-native-traversal.md\n",
                "doc.native-link-traversal.expected=fail\n",
                "doc.native-link-traversal.codes=native-link-traversal\n",
                "doc.native-link-absolute.path=concepts/bad-native-absolute.md\n",
                "doc.native-link-absolute.expected=fail\n",
                "doc.native-link-absolute.codes=native-link-absolute\n",
                "doc.native-orphan.path=concepts/orphan.md\n",
                "doc.native-orphan.expected=fail\n",
                "doc.native-orphan.codes=native-orphan\n",
                "doc.citation-ok.path=concepts/citation-ok.md\n",
                "doc.citation-ok.expected=pass\n",
                "doc.citation-ok.codes=\n",
                "doc.citation-bridge-nonmd.path=concepts/citation-bridge.md\n",
                "doc.citation-bridge-nonmd.expected=pass\n",
                "doc.citation-bridge-nonmd.codes=\n",
                "doc.citation-unconfigured.path=concepts/citation-bad.md\n",
                "doc.citation-unconfigured.expected=fail\n",
                "doc.citation-unconfigured.codes=citation-unconfigured\n",
                "doc.citation-missing.path=concepts/citation-missing.md\n",
                "doc.citation-missing.expected=fail\n",
                "doc.citation-missing.codes=citation-missing\n",
                "doc.citation-range.path=concepts/citation-range.md\n",
                "doc.citation-range.expected=fail\n",
                "doc.citation-range.codes=citation-out-of-range\n",
                "doc.stale-body.path=concepts/stale-body.md\n",
                "doc.stale-body.expected=fail\n",
                "doc.stale-body.codes=stale-body\n",
                "doc.stale-frontmatter.path=concepts/stale-fm.md\n",
                "doc.stale-frontmatter.expected=fail\n",
                "doc.stale-frontmatter.codes=stale-frontmatter\n",
                "doc.image-links-ignored.path=concepts/image-links.md\n",
                "doc.image-links-ignored.expected=pass\n",
                "doc.image-links-ignored.codes=\n",
                "expected.selected_exit=1\n",
                "expected.deterministic=true\n",
            )
        );
    }

    // ===================================================================
    // Slice 1.2-B: native link, citation, stale, and orphan integrity tests
    // ===================================================================

    fn policy_for_native_tests() -> RepositoryPolicy {
        let mut p = policy_with_coverage();
        // Root native doc is root.md
        if let Some(ref mut dc) = p.document_coverage {
            dc.native_bundle = vec![
                "concepts/root.md".to_owned(),
                "concepts/linked.md".to_owned(),
            ];
            dc.canonical_documents = vec!["concepts/canon.md".to_owned()];
            dc.legacy_documents.clear();
            dc.bridge_targets = vec!["src/main.rs".to_owned()];
        }
        p
    }

    fn native_doc(path: &str, title: &str, body: &str) -> Doc {
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        fields.insert(
            "tags".to_owned(),
            YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
        );
        fields.insert(
            "public_boundary".to_owned(),
            YamlValue::String("private".to_owned()),
        );
        fields.insert("title".to_owned(), YamlValue::String(title.to_owned()));
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://github.com/alphazede/repo".to_owned()),
        );
        fields.insert(
            "timestamp".to_owned(),
            YamlValue::String("2026-01-01".to_owned()),
        );
        let raw = "---\ntype: Concept\nokf_status: active\ntags:\n  - internal\npublic_boundary: private\ntitle: ".to_owned() + title + "\nresource: https://github.com/alphazede/repo\ntimestamp: 2026-01-01\n---\n";
        // Always include a relationship link for strict compliance.
        let body = if body.contains("](") {
            body.to_owned()
        } else {
            format!("{body}[link](https://github.com/alphazede/repo)\n")
        };
        Doc::new(
            path,
            format!("{raw}{body}"),
            &body,
            Frontmatter::from_parsed(&raw, fields),
        )
    }

    /// Positive: native bundle page links to another configured native bundle page — passes.
    #[test]
    fn native_link_valid_passes() {
        let mut policy = policy_for_native_tests();
        if let Some(ref mut dc) = policy.document_coverage {
            dc.canonical_documents.clear();
            dc.bridge_targets.clear();
        }
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "Link to [linked page](linked.md).\n# Citations\n",
        );
        let linked = native_doc(
            "concepts/linked.md",
            "Linked",
            "Linked page content, back to [root](root.md).\n# Citations\n",
        );
        let bundle = Bundle::from_documents([root, linked]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Pass);
        assert!(result.bran_strict.diagnostics.is_empty());
    }

    /// Negative: native bundle link to unconfigured target fails.
    #[test]
    fn native_link_unconfigured_fails() {
        let policy = policy_for_native_tests();
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "Link to [missing](other.md).\n# Citations\n",
        );
        let linked = native_doc(
            "concepts/linked.md",
            "Linked",
            "Linked page.\n# Citations\n",
        );
        let bundle = Bundle::from_documents([root, linked]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"native-link-unconfigured"),
            "expected native-link-unconfigured in {:?}",
            codes
        );
    }

    /// Negative: native bundle link with traversal fails.
    #[test]
    fn native_link_traversal_fails() {
        let policy = policy_for_native_tests();
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "Link to [up](../../secret/passwd).\n# Citations\n",
        );
        let linked = native_doc(
            "concepts/linked.md",
            "Linked",
            "Linked page.\n# Citations\n",
        );
        let bundle = Bundle::from_documents([root, linked]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"native-link-traversal"),
            "expected native-link-traversal in {:?}",
            codes
        );
    }

    /// Negative: native bundle link with absolute path fails.
    #[test]
    fn native_link_absolute_fails() {
        let policy = policy_for_native_tests();
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "Link to [/etc/passwd](/etc/passwd).\n# Citations\n",
        );
        let linked = native_doc(
            "concepts/linked.md",
            "Linked",
            "Linked page.\n# Citations\n",
        );
        let bundle = Bundle::from_documents([root, linked]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"native-link-absolute"),
            "expected native-link-absolute in {:?}",
            codes
        );
    }

    /// Negative: orphan native bundle page (not the root, no inbound links) fails.
    #[test]
    fn native_orphan_fails() {
        let mut policy = policy_for_native_tests();
        if let Some(ref mut dc) = policy.document_coverage {
            dc.canonical_documents.clear();
            dc.bridge_targets.clear();
        }
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "Root page, no link to linked.md.\n# Citations\n",
        );
        let linked = native_doc(
            "concepts/linked.md",
            "Linked",
            "Linked page, no one links here.\n# Citations\n",
        );
        let bundle = Bundle::from_documents([root, linked]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"native-orphan"),
            "expected native-orphan in {:?}",
            codes
        );
    }

    /// Positive: canonical citation in range passes.
    #[test]
    fn citation_in_range_passes() {
        let mut policy = policy_for_native_tests();
        if let Some(ref mut dc) = policy.document_coverage {
            dc.native_bundle = vec!["concepts/root.md".to_owned()];
            dc.canonical_documents = vec!["concepts/canon.md".to_owned()];
        }
        let canon_body = "Line 1\nLine 2\nLine 3\n[link](https://github.com/alphazede/repo)\n";
        let canon_raw = format!(
            "---\ntype: Concept\nokf_status: active\ntags:\n  - internal\npublic_boundary: private\ntitle: Canon\nresource: https://github.com/alphazede/repo\ntimestamp: 2026-01-01\n---\n{canon_body}"
        );
        let canon_fields = {
            let mut f = BTreeMap::new();
            f.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
            f.insert(
                "okf_status".to_owned(),
                YamlValue::String("active".to_owned()),
            );
            f.insert(
                "tags".to_owned(),
                YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
            );
            f.insert(
                "public_boundary".to_owned(),
                YamlValue::String("private".to_owned()),
            );
            f.insert("title".to_owned(), YamlValue::String("Canon".to_owned()));
            f.insert(
                "resource".to_owned(),
                YamlValue::String("https://github.com/alphazede/repo".to_owned()),
            );
            f.insert(
                "timestamp".to_owned(),
                YamlValue::String("2026-01-01".to_owned()),
            );
            f
        };
        let canon_doc = Doc::new(
            "concepts/canon.md",
            canon_raw.clone(),
            canon_body,
            Frontmatter::from_parsed(&canon_raw, canon_fields),
        );
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "See concepts/canon.md:3 for detail.\n# Citations\n",
        );
        let bundle = Bundle::from_documents([root, canon_doc]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Pass);
        assert!(result.bran_strict.diagnostics.is_empty());
    }

    /// Positive: non-Markdown bridge target citation accepted without Bundle materialization.
    #[test]
    fn citation_bridge_nonmd_passes() {
        let mut policy = policy_for_native_tests();
        if let Some(ref mut dc) = policy.document_coverage {
            dc.canonical_documents.clear();
            dc.native_bundle = vec!["concepts/root.md".to_owned()];
        }
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "See src/main.rs:42 for the entry point.\n# Citations\n",
        );
        let bundle = Bundle::from_documents([root]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Pass);
        assert!(result.bran_strict.diagnostics.is_empty());
    }

    /// Negative: citation target not in canonical_documents or bridge_targets fails.
    #[test]
    fn citation_unconfigured_fails() {
        let policy = policy_for_native_tests();
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "See unknown/thing.py:10.\n# Citations\n",
        );
        let linked = native_doc(
            "concepts/linked.md",
            "Linked",
            "Linked page.\n# Citations\n",
        );
        let bundle = Bundle::from_documents([root, linked]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"citation-unconfigured"),
            "expected citation-unconfigured in {:?}",
            codes
        );
    }

    /// Negative: configured Markdown citation not in Bundle fails.
    #[test]
    fn citation_missing_fails() {
        let mut policy = policy_for_native_tests();
        if let Some(ref mut dc) = policy.document_coverage {
            dc.canonical_documents = vec!["concepts/absent.md".to_owned()];
        }
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "See concepts/absent.md:1.\n# Citations\n",
        );
        let linked = native_doc(
            "concepts/linked.md",
            "Linked",
            "Linked page.\n# Citations\n",
        );
        let bundle = Bundle::from_documents([root, linked]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"citation-missing"),
            "expected citation-missing in {:?}",
            codes
        );
    }

    /// Negative: citation line exceeds target source line count fails.
    #[test]
    fn citation_out_of_range_fails() {
        let mut policy = policy_for_native_tests();
        if let Some(ref mut dc) = policy.document_coverage {
            dc.native_bundle = vec!["concepts/root.md".to_owned()];
            dc.canonical_documents = vec!["concepts/canon.md".to_owned()];
        }
        let canon_body = "Only one line.\n[link](https://github.com/alphazede/repo)\n";
        let canon_raw = format!(
            "---\ntype: Concept\nokf_status: active\ntags:\n  - internal\npublic_boundary: private\ntitle: Canon\nresource: https://github.com/alphazede/repo\ntimestamp: 2026-01-01\n---\n{canon_body}"
        );
        let canon_fields = {
            let mut f = BTreeMap::new();
            f.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
            f.insert(
                "okf_status".to_owned(),
                YamlValue::String("active".to_owned()),
            );
            f.insert(
                "tags".to_owned(),
                YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
            );
            f.insert(
                "public_boundary".to_owned(),
                YamlValue::String("private".to_owned()),
            );
            f.insert("title".to_owned(), YamlValue::String("Canon".to_owned()));
            f.insert(
                "resource".to_owned(),
                YamlValue::String("https://github.com/alphazede/repo".to_owned()),
            );
            f.insert(
                "timestamp".to_owned(),
                YamlValue::String("2026-01-01".to_owned()),
            );
            f
        };
        let canon_doc = Doc::new(
            "concepts/canon.md",
            canon_raw.clone(),
            canon_body,
            Frontmatter::from_parsed(&canon_raw, canon_fields),
        );
        // Canon doc source has ~12 lines. Line 99 definitely exceeds.
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "See concepts/canon.md:99.\n# Citations\n",
        );
        let bundle = Bundle::from_documents([root, canon_doc]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"citation-out-of-range"),
            "expected citation-out-of-range in {:?}",
            codes
        );
    }

    /// Negative: STALE_CLAIM in body fails.
    #[test]
    fn stale_body_fails() {
        let policy = policy_for_native_tests();
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "This document is STALE_CLAIM.\n# Citations\n",
        );
        let linked = native_doc(
            "concepts/linked.md",
            "Linked",
            "Linked page.\n# Citations\n",
        );
        let bundle = Bundle::from_documents([root, linked]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"stale-body"),
            "expected stale-body in {:?}",
            codes
        );
    }

    /// Negative: freshness: stale in frontmatter fails.
    #[test]
    fn stale_frontmatter_fails() {
        let policy = policy_for_native_tests();
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        fields.insert(
            "tags".to_owned(),
            YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
        );
        fields.insert(
            "public_boundary".to_owned(),
            YamlValue::String("private".to_owned()),
        );
        fields.insert("title".to_owned(), YamlValue::String("Stale".to_owned()));
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://github.com/alphazede/repo".to_owned()),
        );
        fields.insert(
            "freshness".to_owned(),
            YamlValue::String("stale".to_owned()),
        );
        let raw = "---\ntype: Concept\nokf_status: active\ntags:\n  - internal\npublic_boundary: private\ntitle: Stale\nresource: https://github.com/alphazede/repo\nfreshness: stale\n---\n";
        let body = "Body.\n# Citations\n";
        let stale_doc = Doc::new(
            "concepts/root.md",
            format!("{raw}{body}"),
            body,
            Frontmatter::from_parsed(raw, fields),
        );
        let linked = native_doc(
            "concepts/linked.md",
            "Linked",
            "Linked page.\n# Citations\n",
        );
        let bundle = Bundle::from_documents([stale_doc, linked]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"stale-frontmatter"),
            "expected stale-frontmatter in {:?}",
            codes
        );
    }

    /// Positive: image links do not create edges and are ignored.
    #[test]
    fn image_links_ignored() {
        let mut policy = policy_for_native_tests();
        if let Some(ref mut dc) = policy.document_coverage {
            dc.native_bundle = vec![
                "concepts/root.md".to_owned(),
                "concepts/linked.md".to_owned(),
            ];
        }
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "See ![diagram](linked.md) and ![other](other.png).\n# Citations\n",
        );
        let linked = native_doc(
            "concepts/linked.md",
            "Linked",
            "Linked page.\n# Citations\n",
        );
        let bundle = Bundle::from_documents([root, linked]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        // Image link to linked.md does NOT count as inbound link, so linked.md is orphan.
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"native-orphan"),
            "expected native-orphan because image links are ignored: {:?}",
            codes
        );
    }

    /// Positive: repeated bundle construction produces identical diagnostic output.
    #[test]
    fn native_deterministic_identical_outcomes() {
        let policy = policy_for_native_tests();
        let build = || {
            let root = native_doc(
                "concepts/root.md",
                "Root",
                "Link to [linked page](linked.md).\n# Citations\n",
            );
            let linked = native_doc(
                "concepts/linked.md",
                "Linked",
                "Linked page.\n# Citations\n",
            );
            Bundle::from_documents([root, linked]).expect("unique")
        };
        let r1 = ProfileValidator::validate_with_policy(&build(), BRAN_STRICT, Some(&policy));
        let r2 = ProfileValidator::validate_with_policy(&build(), BRAN_STRICT, Some(&policy));
        assert_eq!(r1, r2);
    }

    /// Positive: permuted native_bundle ordering in policy produces identical diagnostics.
    #[test]
    fn native_permuted_policy_identical_outcomes() {
        let mut policy_a = policy_for_native_tests();
        if let Some(ref mut dc) = policy_a.document_coverage {
            dc.canonical_documents.clear();
            dc.bridge_targets.clear();
            dc.native_bundle = vec![
                "concepts/root.md".to_owned(),
                "concepts/linked.md".to_owned(),
            ];
        }
        let mut policy_b = policy_for_native_tests();
        if let Some(ref mut dc) = policy_b.document_coverage {
            dc.canonical_documents.clear();
            dc.bridge_targets.clear();
            dc.native_bundle = vec![
                "concepts/linked.md".to_owned(),
                "concepts/root.md".to_owned(),
            ];
        }
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "Link to [linked page](linked.md).\n# Citations\n",
        );
        let linked = native_doc(
            "concepts/linked.md",
            "Linked",
            "Link back to [root](root.md).\n# Citations\n",
        );
        let bundle = Bundle::from_documents([root, linked]).expect("unique");
        let r_a = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy_a));
        let r_b = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy_b));
        // Both should pass (linked.md gets inbound from root.md).
        assert_eq!(r_a.bran_strict.status, ValidationStatus::Pass);
        assert_eq!(r_b.bran_strict.status, ValidationStatus::Pass);
        // Diagnostics identical regardless of policy ordering.
        assert_eq!(r_a.bran_strict.diagnostics, r_b.bran_strict.diagnostics);
    }

    // ===================================================================
    // Slice 1.2-B defect-remediation focused tests
    // ===================================================================

    /// Defect 1 fix: safe in-repo parent resolution using component stack.
    #[test]
    fn resolve_safe_parent_within_repo() {
        // From concepts/sub/root.md, link ../canon.md resolves to concepts/canon.md.
        let resolved = resolve_relative_link("concepts/sub/root.md", "../canon.md");
        assert_eq!(resolved, Some("concepts/canon.md".to_owned()));
        // Multiple parent dirs that stay within repo.
        let resolved2 = resolve_relative_link("a/b/c/doc.md", "../../d/other.md");
        assert_eq!(resolved2, Some("a/d/other.md".to_owned()));
    }

    /// Defect 1 fix: above-root traversal rejected.
    #[test]
    fn resolve_above_root_traversal_rejected() {
        // From concepts/root.md, ../../secret escapes above repo root.
        assert_eq!(
            resolve_relative_link("concepts/root.md", "../../secret/passwd"),
            None
        );
        // From root-level doc, any ../ escapes.
        assert_eq!(resolve_relative_link("root.md", "../evil"), None);
    }

    /// Defect 2 fix: Windows/UNC/NUL link and citation paths rejected.
    #[test]
    fn path_safety_rejects_unsafe() {
        assert!(!is_path_safe(""));
        assert!(!is_path_safe("C:\\windows\\system32"));
        assert!(!is_path_safe("D:/foo/bar"));
        assert!(!is_path_safe("\\\\server\\share"));
        assert!(!is_path_safe("/etc/passwd"));
        assert!(!is_path_safe("foo\0bar"));
        // Safe paths.
        assert!(is_path_safe("concepts/doc.md"));
        assert!(is_path_safe("src/main.rs"));
        assert!(is_path_safe("../canon.md"));
    }

    /// Defect 2 fix: resolve_relative_link rejects Windows/UNC/NUL targets.
    #[test]
    fn resolve_relative_link_rejects_unsafe() {
        assert_eq!(
            resolve_relative_link("concepts/root.md", "C:\\windows"),
            None
        );
        assert_eq!(
            resolve_relative_link("concepts/root.md", "\\\\server\\share"),
            None
        );
        assert_eq!(
            resolve_relative_link("concepts/root.md", "/etc/passwd"),
            None
        );
    }

    /// Defect 4 fix: self-links do not count as inbound (orphan requires another native page).
    #[test]
    fn self_link_does_not_prevent_orphan() {
        let mut policy = policy_for_native_tests();
        if let Some(ref mut dc) = policy.document_coverage {
            dc.canonical_documents.clear();
            dc.bridge_targets.clear();
            dc.native_bundle = vec![
                "concepts/alpha.md".to_owned(),
                "concepts/beta.md".to_owned(),
            ];
        }
        // alpha.md links only to itself; beta.md has no links.
        let alpha = native_doc(
            "concepts/alpha.md",
            "Alpha",
            "Self-link [here](alpha.md).\n# Citations\n",
        );
        let beta = native_doc(
            "concepts/beta.md",
            "Beta",
            "No outgoing links.\n# Citations\n",
        );
        let bundle = Bundle::from_documents([alpha, beta]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        // Lexical root = concepts/alpha.md. Beta has no inbound → orphan.
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"native-orphan"),
            "self-link should not count as inbound for orphan: {:?}",
            codes
        );
    }

    /// Defect 5 fix: lexical-root permutation produces identical orphan outcomes.
    #[test]
    fn lexical_root_permutation_identical() {
        let mut policy_a = policy_for_native_tests();
        if let Some(ref mut dc) = policy_a.document_coverage {
            dc.canonical_documents.clear();
            dc.bridge_targets.clear();
            dc.native_bundle = vec!["concepts/xxx.md".to_owned(), "concepts/aaa.md".to_owned()];
        }
        // Same set, reversed order.
        let mut policy_b = policy_for_native_tests();
        if let Some(ref mut dc) = policy_b.document_coverage {
            dc.canonical_documents.clear();
            dc.bridge_targets.clear();
            dc.native_bundle = vec!["concepts/aaa.md".to_owned(), "concepts/xxx.md".to_owned()];
        }
        let xxx = native_doc("concepts/xxx.md", "Xxx", "Content.\n# Citations\n");
        let aaa = native_doc("concepts/aaa.md", "Aaa", "Content.\n# Citations\n");
        let bundle = Bundle::from_documents([xxx, aaa]).expect("unique");
        let r_a = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy_a));
        let r_b = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy_b));
        assert_eq!(
            r_a.bran_strict.diagnostics, r_b.bran_strict.diagnostics,
            "lexical root must be deterministic regardless of permutation"
        );
    }

    /// Defect 6 fix: non-numeric and zero citation line emits citation-invalid-line.
    #[test]
    fn citation_non_numeric_or_zero_line_fails() {
        let mut policy = policy_for_native_tests();
        if let Some(ref mut dc) = policy.document_coverage {
            dc.native_bundle = vec!["concepts/root.md".to_owned()];
            dc.canonical_documents = vec!["concepts/canon.md".to_owned()];
            dc.bridge_targets.clear();
        }
        // Build canonical doc.
        let canon_body = "Line 1\nLine 2\n";
        let canon_raw = format!(
            "---\ntype: Concept\nokf_status: active\ntags:\n  - internal\npublic_boundary: private\ntitle: Canon\nresource: https://github.com/alphazede/repo\ntimestamp: 2026-01-01\n---\n{canon_body}"
        );
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        fields.insert(
            "tags".to_owned(),
            YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
        );
        fields.insert(
            "public_boundary".to_owned(),
            YamlValue::String("private".to_owned()),
        );
        fields.insert("title".to_owned(), YamlValue::String("Canon".to_owned()));
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://github.com/alphazede/repo".to_owned()),
        );
        fields.insert(
            "timestamp".to_owned(),
            YamlValue::String("2026-01-01".to_owned()),
        );
        let canon_doc = Doc::new(
            "concepts/canon.md",
            canon_raw.clone(),
            canon_body,
            Frontmatter::from_parsed(&canon_raw, fields),
        );

        // Body contains: trailing-colon, non-numeric, zero-line citations.
        let body = "See concepts/canon.md: for detail. Also concepts/canon.md:abc. Also concepts/canon.md:0.\n# Citations\n";
        let root = native_doc("concepts/root.md", "Root", body);
        let bundle = Bundle::from_documents([root, canon_doc]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        let invalid_lines: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .filter(|d| d.code == "citation-invalid-line")
            .collect();
        assert_eq!(
            invalid_lines.len(),
            3,
            "expected 3 citation-invalid-line diagnostics, got {:?}",
            result.bran_strict.diagnostics
        );
    }

    /// Defect 3 fix: dotted filenames without traversal are not flagged.
    #[test]
    fn dotted_filename_no_false_traversal() {
        // Extract a citation from body where the filename contains ".." but no traversal.
        let body = "See v2..changelog.md:42 for details.\n# Citations\n";
        let citations = extract_citations(body);
        // Should be recognized as a valid citation with line 42 (not rejected as traversal).
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].target, "v2..changelog.md");
        assert_eq!(citations[0].line, 42);
    }

    /// Defect 8 fix: diagnostics must not echo raw URLs/targets (secret-bearing).
    #[test]
    fn diagnostics_do_not_echo_raw_urls() {
        let policy = policy_for_native_tests();
        // Create a doc with a source link containing query/fragment secrets.
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        fields.insert(
            "tags".to_owned(),
            YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
        );
        fields.insert(
            "public_boundary".to_owned(),
            YamlValue::String("private".to_owned()),
        );
        fields.insert(
            "title".to_owned(),
            YamlValue::String("Secret Link".to_owned()),
        );
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://github.com/alphazede/repo".to_owned()),
        );
        fields.insert(
            "timestamp".to_owned(),
            YamlValue::String("2026-01-01".to_owned()),
        );
        let body =
            "Body with [secret](https://evil.com/path?token=abc123&secret=xyz).\n# Citations\n";
        let doc = Doc::new(
            "concepts/root.md",
            format!("---\ntype: Concept\nokf_status: active\ntags:\n  - internal\npublic_boundary: private\ntitle: Secret Link\nresource: https://github.com/alphazede/repo\ntimestamp: 2026-01-01\n---\n{body}"),
            body,
            Frontmatter::from_parsed("---\ntype: Concept\nokf_status: active\ntags:\n  - internal\npublic_boundary: private\ntitle: Secret Link\nresource: https://github.com/alphazede/repo\ntimestamp: 2026-01-01\n---\n", fields),
        );
        let linked = native_doc(
            "concepts/linked.md",
            "Linked",
            "Linked page.\n# Citations\n",
        );
        let bundle = Bundle::from_documents([doc, linked]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        // No diagnostic message should contain the raw secret URL.
        for d in &result.bran_strict.diagnostics {
            assert!(
                !d.message.contains("evil.com"),
                "diagnostic must not echo raw URL: {}",
                d.message
            );
            assert!(
                !d.message.contains("token=abc123"),
                "diagnostic must not echo query secrets: {}",
                d.message
            );
        }
    }

    /// Defect 7 fix: stale frontmatter is ASCII case-insensitive after trim.
    #[test]
    fn stale_frontmatter_case_insensitive() {
        let mut policy = policy_for_native_tests();
        if let Some(ref mut dc) = policy.document_coverage {
            dc.native_bundle = vec!["concepts/root.md".to_owned()];
            dc.canonical_documents.clear();
            dc.legacy_documents.clear();
            dc.bridge_targets.clear();
        }
        for (freshness_val, label) in &[
            ("Stale", "title-case"),
            ("STALE", "upper-case"),
            (" stale ", "whitespace-padded"),
        ] {
            let mut fields = BTreeMap::new();
            fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
            fields.insert(
                "okf_status".to_owned(),
                YamlValue::String("active".to_owned()),
            );
            fields.insert(
                "tags".to_owned(),
                YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
            );
            fields.insert(
                "public_boundary".to_owned(),
                YamlValue::String("private".to_owned()),
            );
            fields.insert(
                "title".to_owned(),
                YamlValue::String(format!("Stale {label}")),
            );
            fields.insert(
                "resource".to_owned(),
                YamlValue::String("https://github.com/alphazede/repo".to_owned()),
            );
            fields.insert(
                "freshness".to_owned(),
                YamlValue::String((*freshness_val).to_owned()),
            );
            let raw = format!("---\ntype: Concept\nokf_status: active\ntags:\n  - internal\npublic_boundary: private\ntitle: Stale {label}\nresource: https://github.com/alphazede/repo\nfreshness: {freshness_val}\n---\n");
            let body = "Body.\n# Citations\n";
            let doc = Doc::new(
                "concepts/root.md",
                format!("{raw}{body}"),
                body,
                Frontmatter::from_parsed(&raw, fields),
            );
            let bundle = Bundle::from_documents([doc]).expect("unique");
            let result =
                ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
            assert!(
                result
                    .bran_strict
                    .diagnostics
                    .iter()
                    .any(|d| d.code == "stale-frontmatter"),
                "expected stale-frontmatter for freshness={label}: {:?}",
                result
                    .bran_strict
                    .diagnostics
                    .iter()
                    .map(|d| &d.code)
                    .collect::<Vec<_>>()
            );
        }
    }

    // ===================================================================
    // Slice 1.2-B3: unsafe citation observability tests (AC-2, AC-5)
    // ===================================================================

    fn policy_for_citation_tests() -> RepositoryPolicy {
        let mut p = policy_with_coverage();
        if let Some(ref mut dc) = p.document_coverage {
            dc.native_bundle = vec!["concepts/root.md".to_owned()];
            dc.canonical_documents = vec!["concepts/canon.md".to_owned()];
            dc.legacy_documents.clear();
            dc.bridge_targets.clear();
        }
        p
    }

    fn canon_doc_for_citation(body: &str) -> Doc {
        let canon_body = if body.contains("](") {
            body.to_owned()
        } else {
            format!("{body}[link](https://github.com/alphazede/repo)\n")
        };
        let raw = format!(
            "---\ntype: Concept\nokf_status: active\ntags:\n  - internal\npublic_boundary: private\ntitle: Canon\nresource: https://github.com/alphazede/repo\ntimestamp: 2026-01-01\n---\n{canon_body}"
        );
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String("Concept".to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        fields.insert(
            "tags".to_owned(),
            YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
        );
        fields.insert(
            "public_boundary".to_owned(),
            YamlValue::String("private".to_owned()),
        );
        fields.insert("title".to_owned(), YamlValue::String("Canon".to_owned()));
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://github.com/alphazede/repo".to_owned()),
        );
        fields.insert(
            "timestamp".to_owned(),
            YamlValue::String("2026-01-01".to_owned()),
        );
        Doc::new(
            "concepts/canon.md",
            raw.clone(),
            canon_body,
            Frontmatter::from_parsed(&raw, fields),
        )
    }

    /// Unix absolute citation: /etc/passwd.md:1 must fail with citation-absolute.
    #[test]
    fn citation_unsafe_unix_absolute_fails() {
        let policy = policy_for_citation_tests();
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "See /etc/passwd.md:1 for secret.\n# Citations\n",
        );
        let canon = canon_doc_for_citation("Line 1\nLine 2\n");
        let bundle = Bundle::from_documents([root, canon]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"citation-absolute"),
            "expected citation-absolute for Unix absolute path, got {:?}",
            codes
        );
        // No diagnostic message may contain the secret-bearing target.
        for d in &result.bran_strict.diagnostics {
            assert!(
                !d.message.contains("passwd"),
                "diagnostic must not echo target: {}",
                d.message
            );
        }
    }

    /// Windows drive with slash citation: D:/foo.md:1 must fail with citation-absolute.
    #[test]
    fn citation_unsafe_windows_drive_slash_fails() {
        let policy = policy_for_citation_tests();
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "See D:/secret/foo.md:1 for details.\n# Citations\n",
        );
        let canon = canon_doc_for_citation("Line 1\n");
        let bundle = Bundle::from_documents([root, canon]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"citation-absolute"),
            "expected citation-absolute for Windows drive slash path, got {:?}",
            codes
        );
        for d in &result.bran_strict.diagnostics {
            assert!(
                !d.message.contains("secret"),
                "diagnostic must not echo target: {}",
                d.message
            );
        }
    }

    /// Windows drive with backslash citation: C:\foo.md:1 must fail with citation-absolute.
    #[test]
    fn citation_unsafe_windows_drive_backslash_fails() {
        let policy = policy_for_citation_tests();
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "See C:\\secrets\\bar.md:1 for details.\n# Citations\n",
        );
        let canon = canon_doc_for_citation("Line 1\n");
        let bundle = Bundle::from_documents([root, canon]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"citation-absolute"),
            "expected citation-absolute for Windows drive backslash path, got {:?}",
            codes
        );
        for d in &result.bran_strict.diagnostics {
            assert!(
                !d.message.contains("secrets"),
                "diagnostic must not echo target: {}",
                d.message
            );
        }
    }

    /// UNC citation: \\server\share\foo.md:1 must fail with citation-absolute.
    #[test]
    fn citation_unsafe_unc_fails() {
        let policy = policy_for_citation_tests();
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "See \\\\evil\\share\\foo.md:1 for details.\n# Citations\n",
        );
        let canon = canon_doc_for_citation("Line 1\n");
        let bundle = Bundle::from_documents([root, canon]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"citation-absolute"),
            "expected citation-absolute for UNC path, got {:?}",
            codes
        );
        for d in &result.bran_strict.diagnostics {
            assert!(
                !d.message.contains("evil"),
                "diagnostic must not echo target: {}",
                d.message
            );
        }
    }

    /// NUL citation: foo\0bar.md:1 must fail with citation-absolute.
    #[test]
    fn citation_unsafe_nul_fails() {
        let policy = policy_for_citation_tests();
        let body = "See foo\0bar.md:1 for details.\n# Citations\n".to_owned();
        let root = native_doc("concepts/root.md", "Root", &body);
        let canon = canon_doc_for_citation("Line 1\n");
        let bundle = Bundle::from_documents([root, canon]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"citation-absolute"),
            "expected citation-absolute for NUL path, got {:?}",
            codes
        );
        for d in &result.bran_strict.diagnostics {
            assert!(
                !d.message.contains("bar"),
                "diagnostic must not echo target: {}",
                d.message
            );
        }
    }

    /// Parent traversal citation: ../../secret.md:1 must fail with citation-absolute.
    #[test]
    fn citation_unsafe_parent_traversal_fails() {
        let policy = policy_for_citation_tests();
        let root = native_doc(
            "concepts/root.md",
            "Root",
            "See ../../secret.md:1 for details.\n# Citations\n",
        );
        let canon = canon_doc_for_citation("Line 1\n");
        let bundle = Bundle::from_documents([root, canon]).expect("unique");
        let result = ProfileValidator::validate_with_policy(&bundle, BRAN_STRICT, Some(&policy));
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"citation-absolute"),
            "expected citation-absolute for parent traversal, got {:?}",
            codes
        );
        for d in &result.bran_strict.diagnostics {
            assert!(
                !d.message.contains("secret"),
                "diagnostic must not echo target: {}",
                d.message
            );
        }
    }

    // ===================================================================
    // Slice 1.2-D: profile-level packet integration tests (AC-1, AC-2, AC-5, AC-7)
    // ===================================================================

    /// Minimal doc that passes BRAN strict shape checks (no policy).
    fn bran_strict_compliant_minimal(path: &str, status: &str, type_val: &str, body: &str) -> Doc {
        let mut fields = BTreeMap::new();
        fields.insert("type".to_owned(), YamlValue::String(type_val.to_owned()));
        fields.insert(
            "okf_status".to_owned(),
            YamlValue::String("active".to_owned()),
        );
        fields.insert("title".to_owned(), YamlValue::String("Test Doc".to_owned()));
        fields.insert(
            "resource".to_owned(),
            YamlValue::String("https://github.com/alphazede/repo".to_owned()),
        );
        fields.insert(
            "timestamp".to_owned(),
            YamlValue::String("2026-01-01".to_owned()),
        );
        fields.insert(
            "tags".to_owned(),
            YamlValue::Sequence(vec![YamlValue::String("internal".to_owned())]),
        );
        fields.insert(
            "public_boundary".to_owned(),
            YamlValue::String("private".to_owned()),
        );
        fields.insert("status".to_owned(), YamlValue::String(status.to_owned()));
        let raw = format!(
            "---\ntype: {type_val}\nokf_status: active\ntitle: Test Doc\nresource: https://github.com/alphazede/repo\ntimestamp: 2026-01-01\ntags:\n  - internal\npublic_boundary: private\nstatus: {status}\n---\n"
        );
        let body = if body.contains("](") {
            body.to_owned()
        } else {
            format!("{body}[link](https://github.com/alphazede/repo)\n# Citations\n")
        };
        Doc::new(
            path,
            format!("{raw}{body}"),
            &body,
            Frontmatter::from_parsed(&raw, fields),
        )
    }

    /// Test 1: compliant active helper referencing active prompt has no packet diagnostic.
    #[test]
    fn profile_packet_active_references_active_no_packet_finding() {
        let active = bran_strict_compliant_minimal(
            "docs/plans/a/prompts/agent.md",
            "active",
            "research-assistant",
            "See [helper](helper.md).",
        );
        let helper = bran_strict_compliant_minimal(
            "docs/plans/a/prompts/helper.md",
            "active",
            "Concept",
            "",
        );
        let bundle = Bundle::from_documents([active, helper]).expect("unique");
        let result = ProfileValidator::validate(&bundle, BRAN_STRICT);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            !codes.contains(&"packet_missing_authority_field"),
            "no packet_missing_authority_field expected, got {:?}",
            codes
        );
        assert!(
            !codes.contains(&"superseded_prompt_reference"),
            "no superseded_prompt_reference expected, got {:?}",
            codes
        );
    }

    /// Test 2: helper missing status surfaces packet_missing_authority_field and makes
    /// bran_strict fail.
    #[test]
    fn profile_packet_helper_missing_status_fails_bran_strict() {
        let mut fields = BTreeMap::new();
        fields.insert(
            "type".to_owned(),
            YamlValue::String("research-assistant".to_owned()),
        );
        // No status field.
        let raw = "---\ntype: research-assistant\n---\n";
        let doc = Doc::new(
            "docs/plans/test/prompts/agent.md",
            format!("{raw}body"),
            "body",
            Frontmatter::from_parsed(raw, fields),
        );
        let bundle = Bundle::from_documents([doc]).expect("unique");
        let result = ProfileValidator::validate(&bundle, BRAN_STRICT);
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"packet_missing_authority_field"),
            "expected packet_missing_authority_field in {:?}",
            codes
        );
    }

    /// Test 3: active packet referencing superseded prompt surfaces
    /// superseded_prompt_reference and selecting BRAN_STRICT fails.
    #[test]
    fn profile_packet_active_references_superseded_fails_bran_strict() {
        let active = bran_strict_compliant_minimal(
            "docs/plans/current/prompts/agent.md",
            "active",
            "research-assistant",
            "See [old approach](../../old/prompts/old.md).",
        );
        let superseded = bran_strict_compliant_minimal(
            "docs/plans/old/prompts/old.md",
            "superseded",
            "Concept",
            "",
        );
        let bundle = Bundle::from_documents([active, superseded]).expect("unique");
        let result = ProfileValidator::validate(&bundle, BRAN_STRICT);
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"superseded_prompt_reference"),
            "expected superseded_prompt_reference in {:?}",
            codes
        );
    }

    /// Test 4: selecting OKF_V0_1 remains independent/passing for a fixture whose only
    /// BRAN-specific failure is packet supersession.
    #[test]
    fn profile_packet_supersession_okf_passes_bran_fails() {
        let active = bran_strict_compliant_minimal(
            "docs/plans/current/prompts/agent.md",
            "active",
            "research-assistant",
            "See [old plan](../../old/prompts/old.md).",
        );
        let superseded = bran_strict_compliant_minimal(
            "docs/plans/old/prompts/old.md",
            "superseded",
            "Concept",
            "",
        );
        let bundle = Bundle::from_documents([active, superseded]).expect("unique");
        let result = ProfileValidator::validate(&bundle, OKF_V0_1);
        // OKF compatibility passes: both docs have valid type frontmatter.
        assert_eq!(result.okf_compatibility.status, ValidationStatus::Pass);
        assert!(result.okf_compatibility.diagnostics.is_empty());
        // BRAN strict fails due to superseded_prompt_reference.
        assert_eq!(result.bran_strict.status, ValidationStatus::Fail);
        let strict_codes: Vec<_> = result
            .bran_strict
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            strict_codes.contains(&"superseded_prompt_reference"),
            "expected superseded_prompt_reference in {:?}",
            strict_codes
        );
        // No other BRAN strict failures (shape checks all pass).
        assert_eq!(
            strict_codes
                .iter()
                .filter(|&&c| c != "superseded_prompt_reference")
                .count(),
            0,
            "only packet failure expected, got {:?}",
            strict_codes
        );
        // selected profile is OKF_V0_1 → passes.
        assert!(result.selected_passed());
        assert_eq!(result.exit_code(), 0);
    }

    /// Test 5: permuted Bundle construction produces byte-for-byte equal ordered
    /// BRAN_STRICT diagnostics.
    #[test]
    fn profile_packet_permuted_bundle_equal_diagnostics() {
        let active = bran_strict_compliant_minimal(
            "docs/plans/current/prompts/agent.md",
            "active",
            "research-assistant",
            "See [old approach](../../old/prompts/old.md).",
        );
        let superseded = bran_strict_compliant_minimal(
            "docs/plans/old/prompts/old.md",
            "superseded",
            "Concept",
            "",
        );
        let bundle1 = Bundle::from_documents([active.clone(), superseded.clone()]).expect("unique");
        let bundle2 = Bundle::from_documents([superseded, active]).expect("unique");
        let r1 = ProfileValidator::validate(&bundle1, BRAN_STRICT);
        let r2 = ProfileValidator::validate(&bundle2, BRAN_STRICT);
        assert_eq!(r1, r2, "permuted bundle must produce identical results");
        assert_eq!(
            r1.bran_strict.diagnostics, r2.bran_strict.diagnostics,
            "permuted bundle must produce identical ordered diagnostics"
        );
    }
}
