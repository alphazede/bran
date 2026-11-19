mod digest;
mod graph_input;
mod ignore;
mod transaction;

pub use digest::{ContentIdentity, IdentityComparison};
pub use graph_input::{AffectedGraphNodes, ScanGraphError};
pub use ignore::{
    IgnoreDiagnostic, IgnoreMatcher, RuleError, MAX_BRANIGNORE_BYTES, MAX_OKFIGNORE_BYTES,
};
pub use transaction::{
    IndexDelta, IndexDeltaKind, IndexFailure, IndexFailureKind, IndexReceipt, IndexTransaction,
    IndexValidationState, RefreshInputIdentity, RenameAmbiguity,
};

use crate::metadata::{MetadataParserRegistry, MetadataReport, PackageDefaults};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, DirEntry};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const DEFAULT_MAX_FILES: usize = 10_000;
const DEFAULT_MAX_FILE_BYTES: usize = 1024 * 1024;
const DEFAULT_MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;
const MAX_SCAN_DEPTH: usize = 64;
const MIN_PARALLEL_FILES: usize = 16;
const MAX_PARALLEL_THREADS: usize = 16;
const BRANIGNORE_FILENAME: &str = ".branignore";
const LEGACY_OKFIGNORE_FILENAME: &str = ".okfignore";
const GITIGNORE_FILENAME: &str = ".gitignore";
pub(crate) const MAX_OVERSIZE_PROBE_BYTES: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanConfig {
    pub max_files: usize,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
    package_defaults: BTreeMap<String, PackageDefaults>,
    profile: String,
    knowledge_documents_only: bool,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_FILES,
            DEFAULT_MAX_FILE_BYTES,
            DEFAULT_MAX_TOTAL_BYTES,
        )
    }
}

impl ScanConfig {
    pub fn new(max_files: usize, max_file_bytes: usize, max_total_bytes: usize) -> Self {
        Self {
            max_files,
            max_file_bytes,
            max_total_bytes,
            package_defaults: BTreeMap::new(),
            profile: "default".to_owned(),
            knowledge_documents_only: false,
        }
    }

    /// Scans only BRAN knowledge-document candidates.
    pub fn knowledge_documents() -> Self {
        Self {
            knowledge_documents_only: true,
            ..Self::default()
        }
    }

    pub fn set_package_defaults(
        &mut self,
        directory: impl AsRef<str>,
        defaults: PackageDefaults,
    ) -> Result<(), ScanFailure> {
        self.package_defaults
            .insert(safe_directory(directory.as_ref())?, defaults);
        Ok(())
    }

    /// Sets the caller-selected metadata profile recorded in snapshot inputs.
    pub fn set_profile(&mut self, profile: impl AsRef<str>) -> Result<(), ScanFailure> {
        let profile = profile.as_ref();
        if profile.is_empty() || profile.len() > 256 || profile.contains(['/', '\\', '\0']) {
            return Err(ScanFailure::InvalidProfile {
                profile: profile.to_owned(),
            });
        }
        self.profile = profile.to_owned();
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanEntry {
    pub identity: ContentIdentity,
    pub source: Arc<[u8]>,
    pub metadata: MetadataReport,
}

/// All scanner inputs that can alter parsed metadata independently of source
/// bytes. Snapshots may reuse entries only when this value is identical.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanInputFingerprint {
    pub schema: ContentIdentity,
    pub parser: ContentIdentity,
    pub profile: ContentIdentity,
    pub config: ContentIdentity,
    pub package_defaults: ContentIdentity,
    pub ignore: ContentIdentity,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ScanDiagnostic {
    Symlink {
        path: String,
    },
    Unreadable {
        path: String,
        reason: String,
    },
    NonUtf8Path {
        path: String,
    },
    UnsupportedInput {
        path: String,
    },
    OversizedInput {
        path: String,
        actual: usize,
        limit: usize,
    },
    WeakIdentityMismatch {
        path: String,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanSnapshot {
    pub entries: BTreeMap<String, Arc<ScanEntry>>,
    pub diagnostics: Vec<ScanDiagnostic>,
    pub inputs: ScanInputFingerprint,
    pub file_count: usize,
    pub total_bytes: usize,
    pub observed_bytes: BTreeMap<String, usize>,
}

/// The old and new identity for one path requiring graph-neighbor closure.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AffectedNode {
    pub path: String,
    pub previous: Option<ContentIdentity>,
    pub current: Option<ContentIdentity>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanChange {
    pub snapshot: ScanSnapshot,
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    pub reused: Vec<String>,
    pub affected: Vec<AffectedNode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScanFailure {
    InvalidRoot {
        path: PathBuf,
        reason: String,
    },
    InvalidPackageDirectory {
        directory: String,
    },
    InvalidProfile {
        profile: String,
    },
    InvalidIgnore(IgnoreDiagnostic),
    UnreadableIgnore {
        reason: String,
    },
    LimitExceeded {
        path: String,
        actual: usize,
        limit: usize,
    },
    DepthExceeded {
        path: String,
        actual: usize,
        limit: usize,
    },
    RootEscape {
        path: String,
    },
    InvalidChangedPath {
        path: String,
        reason: String,
    },
}

#[derive(Clone, Debug)]
pub struct RepositoryScanner {
    root: PathBuf,
    config: ScanConfig,
    parser: MetadataParserRegistry,
}

impl RepositoryScanner {
    pub fn new(root: impl AsRef<Path>, config: ScanConfig) -> Result<Self, ScanFailure> {
        let requested = root.as_ref().to_path_buf();
        let root = fs::canonicalize(&requested).map_err(|error| ScanFailure::InvalidRoot {
            path: requested.clone(),
            reason: error.to_string(),
        })?;
        if !root.is_dir() {
            return Err(ScanFailure::InvalidRoot {
                path: requested,
                reason: "not a directory".to_owned(),
            });
        }
        Ok(Self {
            parser: MetadataParserRegistry::new(config.max_file_bytes),
            root,
            config,
        })
    }

    pub fn scan(&self) -> Result<ScanSnapshot, ScanFailure> {
        self.collect(None, &BTreeSet::new())
    }

    /// Incrementally scans using caller-supplied repository change evidence.
    /// Paths in `changed_paths` are reparsed even when their bytes match, so a
    /// caller's observed change is never silently treated as unchanged.
    pub fn scan_changed(
        &self,
        previous: &ScanSnapshot,
        changed_paths: &BTreeSet<String>,
    ) -> Result<ScanChange, ScanFailure> {
        for path in changed_paths {
            validate_changed_path(path)?;
        }
        let (matcher, ignore) = self.read_ignore()?;
        let inputs = self.inputs(ignore);
        let mut snapshot = if inputs != previous.inputs {
            self.collect(None, &BTreeSet::new())?
        } else {
            let mut snapshot = previous.clone();
            snapshot.diagnostics.retain(|diagnostic| {
                let path = match diagnostic {
                    ScanDiagnostic::Symlink { path }
                    | ScanDiagnostic::Unreadable { path, .. }
                    | ScanDiagnostic::NonUtf8Path { path }
                    | ScanDiagnostic::UnsupportedInput { path }
                    | ScanDiagnostic::OversizedInput { path, .. }
                    | ScanDiagnostic::WeakIdentityMismatch { path } => path,
                };
                !changed_paths.contains(path)
            });
            snapshot.inputs = inputs;
            for relative in changed_paths {
                if is_ignore_control_path(relative) || matcher.is_ignored_relative(relative) {
                    snapshot.entries.remove(relative);
                    remove_observed(&mut snapshot, relative);
                } else {
                    self.apply_changed_path(relative, &mut snapshot)?;
                }
            }
            if snapshot.file_count > self.config.max_files {
                return Err(limit(
                    "<snapshot>",
                    snapshot.file_count,
                    self.config.max_files,
                ));
            }
            if snapshot.total_bytes > self.config.max_total_bytes {
                return Err(limit(
                    "<snapshot>",
                    snapshot.total_bytes,
                    self.config.max_total_bytes,
                ));
            }
            snapshot
        };
        let mut change = ScanChange::default();
        let input_changed = snapshot.inputs != previous.inputs;
        for (path, entry) in &snapshot.entries {
            match previous.entries.get(path) {
                Some(prior)
                    if !input_changed
                        && !changed_paths.contains(path)
                        && prior.identity == entry.identity
                        && prior.source == entry.source =>
                {
                    change.reused.push(path.clone());
                }
                Some(prior) => {
                    if prior.identity == entry.identity && prior.source != entry.source {
                        snapshot
                            .diagnostics
                            .push(ScanDiagnostic::WeakIdentityMismatch { path: path.clone() });
                    }
                    change.changed.push(path.clone());
                    change.affected.push(AffectedNode {
                        path: path.clone(),
                        previous: Some(prior.identity.clone()),
                        current: Some(entry.identity.clone()),
                    });
                }
                None => {
                    change.added.push(path.clone());
                    change.affected.push(AffectedNode {
                        path: path.clone(),
                        previous: None,
                        current: Some(entry.identity.clone()),
                    });
                }
            }
        }
        for path in previous.entries.keys() {
            if !snapshot.entries.contains_key(path) {
                change.removed.push(path.clone());
                change.affected.push(AffectedNode {
                    path: path.clone(),
                    previous: previous
                        .entries
                        .get(path)
                        .map(|entry| entry.identity.clone()),
                    current: None,
                });
            }
        }
        snapshot.diagnostics.sort();
        change.affected.sort();
        change.snapshot = snapshot;
        Ok(change)
    }

    fn apply_changed_path(
        &self,
        relative: &str,
        snapshot: &mut ScanSnapshot,
    ) -> Result<(), ScanFailure> {
        validate_changed_path(relative)?;
        let path = self.root.join(relative);
        if let Some(prior) = snapshot.entries.remove(relative) {
            let _ = prior;
        }
        remove_observed(snapshot, relative);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(());
            }
            Err(error) => {
                unreadable(snapshot, relative, error);
                return Ok(());
            }
        };
        if metadata.file_type().is_symlink() {
            snapshot.diagnostics.push(ScanDiagnostic::Symlink {
                path: relative.to_owned(),
            });
            return Ok(());
        }
        if metadata.is_dir() {
            return Err(ScanFailure::InvalidChangedPath {
                path: relative.to_owned(),
                reason: "changed path must not be a directory".to_owned(),
            });
        }
        if !metadata.is_file() {
            snapshot.diagnostics.push(ScanDiagnostic::UnsupportedInput {
                path: relative.to_owned(),
            });
            return Ok(());
        }
        if !self.accepts_file(relative) {
            return Ok(());
        }
        let file_size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        let canonical = match fs::canonicalize(&path) {
            Ok(canonical) => canonical,
            Err(error) => {
                unreadable(snapshot, relative, error);
                return Ok(());
            }
        };
        if !canonical.starts_with(&self.root) {
            return Err(ScanFailure::RootEscape {
                path: relative.to_owned(),
            });
        }
        if file_size > self.config.max_file_bytes {
            if let Some(diag) = self.probe_oversized(&canonical, relative) {
                snapshot.diagnostics.push(diag);
                return Ok(());
            }
        }
        let source = match fs::read(&canonical) {
            Ok(source) => source,
            Err(error) => {
                unreadable(snapshot, relative, error);
                return Ok(());
            }
        };
        if fs::symlink_metadata(&path).is_ok_and(|item| item.file_type().is_symlink()) {
            snapshot.diagnostics.push(ScanDiagnostic::Symlink {
                path: relative.to_owned(),
            });
            return Ok(());
        }
        let text = match std::str::from_utf8(&source) {
            Ok(text) => text,
            Err(_) => {
                snapshot.diagnostics.push(ScanDiagnostic::UnsupportedInput {
                    path: relative.to_owned(),
                });
                return Ok(());
            }
        };
        if source.len() > self.config.max_file_bytes {
            snapshot.diagnostics.push(ScanDiagnostic::OversizedInput {
                path: relative.to_owned(),
                actual: source.len(),
                limit: self.config.max_file_bytes,
            });
            return Ok(());
        }
        self.check_bytes(relative, source.len(), snapshot.total_bytes)?;
        replace_observed(snapshot, relative, source.len());
        let parsed = self
            .parser
            .parse(relative, text, &self.defaults_for(relative));
        snapshot.entries.insert(
            relative.to_owned(),
            Arc::new(ScanEntry {
                identity: ContentIdentity::from_bytes(&source),
                source: Arc::from(source),
                metadata: parsed,
            }),
        );
        Ok(())
    }

    fn collect(
        &self,
        previous: Option<&ScanSnapshot>,
        changed_paths: &BTreeSet<String>,
    ) -> Result<ScanSnapshot, ScanFailure> {
        let (matcher, ignore) = self.read_ignore()?;
        let mut snapshot = ScanSnapshot {
            inputs: self.inputs(ignore),
            ..ScanSnapshot::default()
        };
        let reusable = previous.is_some_and(|prior| prior.inputs == snapshot.inputs);
        let prior = reusable.then_some(previous).flatten();
        let mut limits = ScanLimits::default();
        let mut stack = Vec::new();
        if let Some(root_frame) =
            self.open_dir_frame(&self.root, "", 0, &mut snapshot.diagnostics)?
        {
            stack.push(root_frame);
        }
        let mut batch = Vec::new();
        loop {
            let batch_limit = self
                .config
                .max_files
                .saturating_sub(limits.files)
                .saturating_add(1);
            let remaining_bytes = self.config.max_total_bytes.saturating_sub(limits.bytes);
            let step = self.walk_batch(
                &mut stack,
                &matcher,
                &mut snapshot.diagnostics,
                &mut limits.paths,
                &mut batch,
                batch_limit,
                remaining_bytes,
            );
            if !batch.is_empty() {
                self.process_file_batch(
                    &batch,
                    prior,
                    changed_paths,
                    remaining_bytes,
                    &mut snapshot,
                    &mut limits,
                )?;
                batch.clear();
            }
            match step {
                WalkStep::Done => break,
                WalkStep::BatchFull => continue,
                WalkStep::Failed(failure) => return Err(failure),
            }
        }
        snapshot.file_count = snapshot.observed_bytes.len();
        snapshot.total_bytes = snapshot.observed_bytes.values().sum();
        snapshot.diagnostics.sort();
        Ok(snapshot)
    }

    fn read_ignore(&self) -> Result<(IgnoreMatcher, ContentIdentity), ScanFailure> {
        let native_path = self.root.join(BRANIGNORE_FILENAME);
        let configured = match fs::symlink_metadata(&native_path) {
            Ok(metadata) => Some((
                BRANIGNORE_FILENAME,
                b"branignore-v1:\0".as_slice(),
                native_path,
                metadata,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let legacy_path = self.root.join(LEGACY_OKFIGNORE_FILENAME);
                match fs::symlink_metadata(&legacy_path) {
                    Ok(metadata) => Some((
                        LEGACY_OKFIGNORE_FILENAME,
                        b"okfignore-v1:\0".as_slice(),
                        legacy_path,
                        metadata,
                    )),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => {
                        return Err(unreadable_ignore(LEGACY_OKFIGNORE_FILENAME, error));
                    }
                }
            }
            Err(error) => return Err(unreadable_ignore(BRANIGNORE_FILENAME, error)),
        };
        let (mut matcher, mut input) = if let Some((filename, prefix, path, metadata)) = configured
        {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(ScanFailure::UnreadableIgnore {
                    reason: format!("root {filename} must be a regular file, not a symlink"),
                });
            }
            let advertised = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
            if advertised > MAX_BRANIGNORE_BYTES {
                return Err(ScanFailure::InvalidIgnore(
                    IgnoreDiagnostic::InputTooLarge {
                        actual_bytes: advertised,
                    },
                ));
            }
            let bytes = fs::read(path).map_err(|error| unreadable_ignore(filename, error))?;
            let matcher =
                IgnoreMatcher::from_branignore(&bytes).map_err(ScanFailure::InvalidIgnore)?;
            let mut input = prefix.to_vec();
            input.extend_from_slice(&bytes);
            (matcher, input)
        } else {
            (
                IgnoreMatcher::default(),
                b"branignore-v1:missing;okfignore-v1:missing".to_vec(),
            )
        };

        let gitignore_path = self.root.join(GITIGNORE_FILENAME);
        match fs::symlink_metadata(&gitignore_path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                let advertised = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
                if advertised > MAX_BRANIGNORE_BYTES {
                    input.extend_from_slice(b";gitignore-v1:oversized");
                } else if let Ok(bytes) = fs::read(gitignore_path) {
                    matcher.add_gitignore(&bytes);
                    input.extend_from_slice(b";gitignore-v1:\0");
                    input.extend_from_slice(&bytes);
                } else {
                    input.extend_from_slice(b";gitignore-v1:unreadable");
                }
            }
            Ok(_) => input.extend_from_slice(b";gitignore-v1:unsupported"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                input.extend_from_slice(b";gitignore-v1:missing");
            }
            Err(_) => input.extend_from_slice(b";gitignore-v1:unreadable"),
        }
        Ok((matcher, ContentIdentity::from_bytes(&input)))
    }

    fn open_dir_frame(
        &self,
        directory: &Path,
        prefix: &str,
        depth: usize,
        diagnostics: &mut Vec<ScanDiagnostic>,
    ) -> Result<Option<DirFrame>, ScanFailure> {
        if depth > MAX_SCAN_DEPTH {
            return Err(ScanFailure::DepthExceeded {
                path: prefix.to_owned(),
                actual: depth,
                limit: MAX_SCAN_DEPTH,
            });
        }
        let current = match fs::symlink_metadata(directory) {
            Ok(metadata) => metadata,
            Err(error) => {
                diagnostics.push(ScanDiagnostic::Unreadable {
                    path: prefix.to_owned(),
                    reason: error.to_string(),
                });
                return Ok(None);
            }
        };
        if current.file_type().is_symlink() {
            diagnostics.push(ScanDiagnostic::Symlink {
                path: prefix.to_owned(),
            });
            return Ok(None);
        }
        let canonical = match fs::canonicalize(directory) {
            Ok(path) => path,
            Err(error) => {
                diagnostics.push(ScanDiagnostic::Unreadable {
                    path: prefix.to_owned(),
                    reason: error.to_string(),
                });
                return Ok(None);
            }
        };
        if !canonical.starts_with(&self.root) {
            return Err(ScanFailure::RootEscape {
                path: prefix.to_owned(),
            });
        }
        let entries = match fs::read_dir(canonical) {
            Ok(entries) => sorted_entries(entries, prefix, diagnostics),
            Err(error) => {
                diagnostics.push(ScanDiagnostic::Unreadable {
                    path: prefix.to_owned(),
                    reason: error.to_string(),
                });
                return Ok(None);
            }
        };
        Ok(Some(DirFrame {
            prefix: prefix.to_owned(),
            depth,
            entries,
            next_idx: 0,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn walk_batch(
        &self,
        stack: &mut Vec<DirFrame>,
        matcher: &IgnoreMatcher,
        diagnostics: &mut Vec<ScanDiagnostic>,
        path_count: &mut usize,
        batch: &mut Vec<CandidateFile>,
        batch_limit: usize,
        remaining_bytes: usize,
    ) -> WalkStep {
        let path_limit = self
            .config
            .max_files
            .saturating_mul(4)
            .saturating_add(MAX_SCAN_DEPTH);
        let mut batch_bytes = 0usize;
        while let Some(frame) = stack.last_mut() {
            if frame.next_idx >= frame.entries.len() {
                stack.pop();
                continue;
            }
            let entry = &frame.entries[frame.next_idx];
            frame.next_idx += 1;
            let prefix = frame.prefix.as_str();
            let depth = frame.depth;
            let name = match entry.file_name().into_string() {
                Ok(name) => name,
                Err(_) => {
                    diagnostics.push(ScanDiagnostic::NonUtf8Path {
                        path: path_hint(prefix),
                    });
                    continue;
                }
            };
            let relative = join_relative(prefix, &name);
            if is_ignore_control_path(&relative) || matcher.is_ignored_relative(&relative) {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    diagnostics.push(ScanDiagnostic::Unreadable {
                        path: relative,
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
            if !self.config.knowledge_documents_only
                || !file_type.is_file()
                || self.accepts_file(&relative)
            {
                *path_count = path_count.saturating_add(1);
                if *path_count > path_limit {
                    return WalkStep::Failed(limit(&relative, *path_count, path_limit));
                }
            }
            if file_type.is_symlink() {
                diagnostics.push(ScanDiagnostic::Symlink { path: relative });
            } else if file_type.is_dir() {
                match self.open_dir_frame(&entry.path(), &relative, depth + 1, diagnostics) {
                    Ok(Some(child)) => stack.push(child),
                    Ok(None) => {}
                    Err(failure) => return WalkStep::Failed(failure),
                }
            } else if file_type.is_file() {
                if self.accepts_file(&relative) {
                    let estimated_bytes = entry
                        .metadata()
                        .map(|meta| usize::try_from(meta.len()).unwrap_or(usize::MAX))
                        .unwrap_or(0)
                        .min(self.config.max_file_bytes.saturating_add(1));
                    batch_bytes = batch_bytes.saturating_add(estimated_bytes);
                    batch.push(CandidateFile {
                        path: entry.path(),
                        relative,
                    });
                    if batch.len() >= batch_limit || batch_bytes > remaining_bytes {
                        return WalkStep::BatchFull;
                    }
                }
            } else {
                diagnostics.push(ScanDiagnostic::UnsupportedInput { path: relative });
            }
        }
        WalkStep::Done
    }

    fn process_file_batch(
        &self,
        batch: &[CandidateFile],
        previous: Option<&ScanSnapshot>,
        changed_paths: &BTreeSet<String>,
        remaining_bytes: usize,
        snapshot: &mut ScanSnapshot,
        limits: &mut ScanLimits,
    ) -> Result<(), ScanFailure> {
        let workers = if batch.len() < MIN_PARALLEL_FILES {
            1
        } else {
            let hw = std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .unwrap_or(4);
            hw.min(MAX_PARALLEL_THREADS).min(batch.len() / 4).max(1)
        };
        if workers <= 1 {
            for candidate in batch {
                let outcome =
                    self.read_file_outcome(candidate, previous, changed_paths, remaining_bytes);
                self.apply_file_outcome(outcome, snapshot, limits)?;
            }
            return Ok(());
        }

        let mut outcomes: Vec<Option<FileOutcome>> = (0..batch.len()).map(|_| None).collect();
        let next_index = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(workers);
            for _ in 0..workers {
                let spawned = std::thread::Builder::new().spawn_scoped(scope, || {
                    let mut local = Vec::new();
                    loop {
                        let index = next_index.fetch_add(1, Ordering::Relaxed);
                        let Some(candidate) = batch.get(index) else {
                            break;
                        };
                        local.push((
                            index,
                            self.read_file_outcome(
                                candidate,
                                previous,
                                changed_paths,
                                remaining_bytes,
                            ),
                        ));
                    }
                    local
                });
                match spawned {
                    Ok(handle) => handles.push(handle),
                    Err(_) => break,
                }
            }
            for handle in handles {
                if let Ok(local) = handle.join() {
                    for (index, outcome) in local {
                        outcomes[index] = Some(outcome);
                    }
                }
            }
        });
        for (candidate, outcome) in batch.iter().zip(outcomes) {
            let outcome = outcome.unwrap_or_else(|| {
                self.read_file_outcome(candidate, previous, changed_paths, remaining_bytes)
            });
            self.apply_file_outcome(outcome, snapshot, limits)?;
        }
        Ok(())
    }

    fn apply_file_outcome(
        &self,
        outcome: FileOutcome,
        snapshot: &mut ScanSnapshot,
        limits: &mut ScanLimits,
    ) -> Result<(), ScanFailure> {
        match outcome {
            FileOutcome::Error(failure) => Err(failure),
            FileOutcome::Diagnostic(diag) => {
                snapshot.diagnostics.push(diag);
                Ok(())
            }
            FileOutcome::ByteLimitCandidate { relative, bytes } => {
                limits.files = limits.files.saturating_add(1);
                if limits.files > self.config.max_files {
                    return Err(limit(&relative, limits.files, self.config.max_files));
                }
                self.check_bytes(&relative, bytes, limits.bytes)?;
                Err(limit(
                    &relative,
                    limits.bytes.saturating_add(bytes),
                    self.config.max_total_bytes,
                ))
            }
            FileOutcome::Entry {
                relative,
                bytes,
                entry,
            } => {
                limits.files = limits.files.saturating_add(1);
                if limits.files > self.config.max_files {
                    return Err(limit(&relative, limits.files, self.config.max_files));
                }
                self.check_bytes(&relative, bytes, limits.bytes)?;
                replace_observed(snapshot, &relative, bytes);
                limits.bytes += bytes;
                snapshot.entries.insert(relative, entry);
                Ok(())
            }
        }
    }

    fn read_file_outcome(
        &self,
        candidate: &CandidateFile,
        previous: Option<&ScanSnapshot>,
        changed_paths: &BTreeSet<String>,
        remaining_bytes: usize,
    ) -> FileOutcome {
        let path = &candidate.path;
        let relative = &candidate.relative;
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                return FileOutcome::Diagnostic(ScanDiagnostic::Unreadable {
                    path: relative.clone(),
                    reason: error.to_string(),
                });
            }
        };
        if metadata.file_type().is_symlink() {
            return FileOutcome::Diagnostic(ScanDiagnostic::Symlink {
                path: relative.clone(),
            });
        }
        let canonical = match fs::canonicalize(path) {
            Ok(path) if path.starts_with(&self.root) => path,
            Ok(_) => {
                return FileOutcome::Error(ScanFailure::RootEscape {
                    path: relative.clone(),
                });
            }
            Err(error) => {
                return FileOutcome::Diagnostic(ScanDiagnostic::Unreadable {
                    path: relative.clone(),
                    reason: error.to_string(),
                });
            }
        };
        let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if size > self.config.max_file_bytes {
            if let Some(diag) = self.probe_oversized(&canonical, relative) {
                return FileOutcome::Diagnostic(diag);
            }
        }
        let source = match fs::read(&canonical) {
            Ok(source) => source,
            Err(error) => {
                return FileOutcome::Diagnostic(ScanDiagnostic::Unreadable {
                    path: relative.clone(),
                    reason: error.to_string(),
                });
            }
        };
        if fs::symlink_metadata(path).is_ok_and(|item| item.file_type().is_symlink()) {
            return FileOutcome::Diagnostic(ScanDiagnostic::Symlink {
                path: relative.clone(),
            });
        }
        let text = match std::str::from_utf8(&source) {
            Ok(text) => text,
            Err(_) => {
                return FileOutcome::Diagnostic(ScanDiagnostic::UnsupportedInput {
                    path: relative.clone(),
                });
            }
        };
        if source.len() > self.config.max_file_bytes {
            return FileOutcome::Diagnostic(ScanDiagnostic::OversizedInput {
                path: relative.clone(),
                actual: source.len(),
                limit: self.config.max_file_bytes,
            });
        }
        if source.len() > remaining_bytes {
            return FileOutcome::ByteLimitCandidate {
                relative: relative.clone(),
                bytes: source.len(),
            };
        }
        let identity = ContentIdentity::from_bytes(&source);
        let reused = previous
            .and_then(|prior| prior.entries.get(relative.as_str()))
            .filter(|prior| {
                !changed_paths.contains(relative.as_str())
                    && prior.identity == identity
                    && prior.source.as_ref() == source.as_slice()
            });
        let metadata = if let Some(prior) = reused {
            prior.metadata.clone()
        } else {
            self.parser
                .parse(relative, text, &self.defaults_for(relative))
        };
        let bytes = source.len();
        FileOutcome::Entry {
            relative: relative.clone(),
            bytes,
            entry: Arc::new(ScanEntry {
                identity,
                source: Arc::from(source),
                metadata,
            }),
        }
    }

    fn check_bytes(&self, path: &str, size: usize, prior: usize) -> Result<(), ScanFailure> {
        if size > self.config.max_file_bytes {
            return Err(limit(path, size, self.config.max_file_bytes));
        }
        if size > self.config.max_total_bytes.saturating_sub(prior) {
            return Err(limit(
                path,
                prior.saturating_add(size),
                self.config.max_total_bytes,
            ));
        }
        Ok(())
    }

    fn accepts_file(&self, relative: &str) -> bool {
        !self.config.knowledge_documents_only || is_knowledge_document_path(relative)
    }

    fn probe_oversized(&self, canonical: &Path, relative: &str) -> Option<ScanDiagnostic> {
        let mut file = fs::File::open(canonical).ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file()
            || usize::try_from(metadata.len()).unwrap_or(usize::MAX) <= self.config.max_file_bytes
        {
            return None;
        }
        let mut buf = [0u8; MAX_OVERSIZE_PROBE_BYTES];
        let n = match file.read(&mut buf) {
            Ok(n) => n,
            Err(_) => return None,
        };
        let probe = &buf[..n];
        if probe.contains(&0) || std::str::from_utf8(probe).is_err() {
            Some(ScanDiagnostic::UnsupportedInput {
                path: relative.to_owned(),
            })
        } else {
            Some(ScanDiagnostic::OversizedInput {
                path: relative.to_owned(),
                actual: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
                limit: self.config.max_file_bytes,
            })
        }
    }

    fn defaults_for(&self, relative: &str) -> PackageDefaults {
        let parent = relative.rsplit_once('/').map_or("", |(parent, _)| parent);
        let mut applicable: Vec<_> = self
            .config
            .package_defaults
            .iter()
            .filter(|(directory, _)| {
                directory.is_empty()
                    || parent == directory.as_str()
                    || parent.starts_with(&format!("{directory}/"))
            })
            .collect();
        applicable.sort_by_key(|(directory, _)| directory.len());
        let mut defaults = PackageDefaults::default();
        for (_, package) in applicable {
            defaults.overlay(package);
        }
        defaults
    }

    fn inputs(&self, ignore: ContentIdentity) -> ScanInputFingerprint {
        ScanInputFingerprint {
            schema: fingerprint("source-metadata-schema-v2"),
            parser: fingerprint(&self.parser.fingerprint()),
            profile: fingerprint(&self.config.profile),
            config: fingerprint(&format!(
                "max-files={};max-file-bytes={};max-total-bytes={};knowledge-documents-only={}",
                self.config.max_files,
                self.config.max_file_bytes,
                self.config.max_total_bytes,
                self.config.knowledge_documents_only
            )),
            package_defaults: fingerprint(&package_defaults_fingerprint(
                &self.config.package_defaults,
            )),
            ignore,
        }
    }
}

/// Returns whether a repository-relative path is a BRAN knowledge-document candidate.
pub fn is_knowledge_document_path(path: &str) -> bool {
    path.ends_with(".md") || path.ends_with(".markdown")
}

#[derive(Default)]
struct ScanLimits {
    paths: usize,
    files: usize,
    bytes: usize,
}

struct DirFrame {
    prefix: String,
    depth: usize,
    entries: Vec<DirEntry>,
    next_idx: usize,
}

struct CandidateFile {
    path: PathBuf,
    relative: String,
}

enum WalkStep {
    Done,
    BatchFull,
    Failed(ScanFailure),
}

enum FileOutcome {
    Error(ScanFailure),
    Diagnostic(ScanDiagnostic),
    ByteLimitCandidate {
        relative: String,
        bytes: usize,
    },
    Entry {
        relative: String,
        bytes: usize,
        entry: Arc<ScanEntry>,
    },
}

fn safe_directory(directory: &str) -> Result<String, ScanFailure> {
    if directory.is_empty() {
        return Ok(String::new());
    }
    if directory.starts_with('/')
        || directory.contains('\\')
        || directory
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(ScanFailure::InvalidPackageDirectory {
            directory: directory.to_owned(),
        });
    }
    Ok(directory.to_owned())
}

fn validate_changed_path(path: &str) -> Result<(), ScanFailure> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path.len() > 4096
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(ScanFailure::InvalidChangedPath {
            path: path.to_owned(),
            reason: "path must be a normalized relative file path".to_owned(),
        });
    }
    Ok(())
}

fn remove_observed(snapshot: &mut ScanSnapshot, path: &str) {
    if let Some(bytes) = snapshot.observed_bytes.remove(path) {
        snapshot.file_count = snapshot.file_count.saturating_sub(1);
        snapshot.total_bytes = snapshot.total_bytes.saturating_sub(bytes);
    }
}

fn replace_observed(snapshot: &mut ScanSnapshot, path: &str, bytes: usize) {
    remove_observed(snapshot, path);
    snapshot.observed_bytes.insert(path.to_owned(), bytes);
    snapshot.file_count += 1;
    snapshot.total_bytes = snapshot.total_bytes.saturating_add(bytes);
}

fn fingerprint(value: &str) -> ContentIdentity {
    ContentIdentity::from_bytes(value.as_bytes())
}

fn package_defaults_fingerprint(defaults: &BTreeMap<String, PackageDefaults>) -> String {
    format!("{defaults:?}")
}

fn sorted_entries(
    entries: fs::ReadDir,
    prefix: &str,
    diagnostics: &mut Vec<ScanDiagnostic>,
) -> Vec<DirEntry> {
    let mut values = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => values.push(entry),
            Err(error) => diagnostics.push(ScanDiagnostic::Unreadable {
                path: prefix.to_owned(),
                reason: error.to_string(),
            }),
        }
    }
    values.sort_by_key(|entry| entry.file_name().to_string_lossy().into_owned());
    values
}

fn unreadable(snapshot: &mut ScanSnapshot, path: &str, error: std::io::Error) {
    snapshot.diagnostics.push(ScanDiagnostic::Unreadable {
        path: path.to_owned(),
        reason: error.to_string(),
    });
}

fn unreadable_ignore(filename: &str, error: std::io::Error) -> ScanFailure {
    ScanFailure::UnreadableIgnore {
        reason: format!("{filename}: {error}"),
    }
}

fn is_ignore_control_path(path: &str) -> bool {
    matches!(
        path,
        BRANIGNORE_FILENAME | LEGACY_OKFIGNORE_FILENAME | GITIGNORE_FILENAME
    )
}

fn join_relative(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    }
}

fn path_hint(prefix: &str) -> String {
    join_relative(prefix, "<non-utf8>")
}

fn limit(path: &str, actual: usize, limit: usize) -> ScanFailure {
    ScanFailure::LimitExceeded {
        path: path.to_owned(),
        actual,
        limit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{FactProvenance, PackageDefaults};
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bran-scanner-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write(root: &Path, relative: &str, source: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }

    fn paths(snapshot: &ScanSnapshot) -> Vec<String> {
        snapshot.entries.keys().cloned().collect()
    }
    fn fact(report: &MetadataReport, key: &str, value: &str) -> bool {
        report.facts.iter().any(|item| {
            item.key == key
                && item.value == value
                && item.provenance == FactProvenance::PackageDefault
        })
    }
    fn boundary(report: &MetadataReport) -> bool {
        report
            .facts
            .iter()
            .any(|item| item.key == "important_boundary" && item.value == "true")
    }
    fn symlink(snapshot: &ScanSnapshot) -> bool {
        snapshot
            .diagnostics
            .iter()
            .any(|item| matches!(item, ScanDiagnostic::Symlink { path } if path == "outside.rs"))
    }
    fn file_limit(error: ScanFailure) -> bool {
        matches!(error, ScanFailure::LimitExceeded { limit: 2, .. })
    }

    #[cfg(unix)]
    fn outside_link(root: &Path) -> bool {
        let target = root.with_extension("outside");
        fs::write(&target, b"outside").unwrap();
        std::os::unix::fs::symlink(&target, root.join("outside.rs")).is_ok()
    }
    #[cfg(not(unix))]
    fn outside_link(_: &Path) -> bool {
        false
    }

    #[test]
    #[rustfmt::skip]
    fn p2_scanner() {
        let public_fixture = include_str!("../../../../fixtures/scanner/scan-snapshot-v1.json");
        assert!(public_fixture.contains("\"schema_version\": \"1.0.0\""));
        assert!(public_fixture.contains("\"path\": \"src/lib.rs\""));
        assert!(public_fixture.contains("\"byte_len\": 72"));
        assert!(public_fixture.contains("\"metadata_status\": \"warning\""));
        let root = root();
        write(&root, ".okfignore", b"skip/\n");
        write(&root, "docs/readme.md", b"---\ntype: guide\n---\n# Readme\n");
        write(&root, "packages/core/infra/security.rs", b"pub fn locked() {}\n");
        write(&root, "src/a.rs", b"pub fn alpha() {}\n");
        write(&root, "src/z.rs", b"pub fn zulu() {}\n");
        write(&root, "node_modules/hidden.rs", b"pub fn hidden() {}\n");
        write(&root, "skip/ignored.rs", b"pub fn ignored() {}\n");
        let mut config = ScanConfig::new(16, 1024, 4096);
        config.set_package_defaults("", PackageDefaults::new([("owner".into(), "root".into())])).unwrap();
        config.set_package_defaults("packages", PackageDefaults::new([("owner".into(), "product".into())])).unwrap();
        config.set_package_defaults("packages/core", PackageDefaults::new([("kind".into(), "core".into())])).unwrap();
        let scanner = RepositoryScanner::new(&root, config.clone()).unwrap();
        let full = scanner.scan().unwrap();
        assert_eq!(paths(&full), vec!["docs/readme.md", "packages/core/infra/security.rs", "src/a.rs", "src/z.rs"]);
        assert!(!full.entries.contains_key("node_modules/hidden.rs"));
        assert!(!full.entries.contains_key("skip/ignored.rs"));
        assert!(fact(&full.entries["packages/core/infra/security.rs"].metadata, "kind", "core"));
        assert!(boundary(&full.entries["packages/core/infra/security.rs"].metadata));
        write(&root, "src/a.rs", b"pub fn altered() {}\n");
        write(&root, "src/z.rs", b"pub fn unseen() {}\n");
        let changed_paths = BTreeSet::from(["src/a.rs".to_owned()]);
        let changed = scanner.scan_changed(&full, &changed_paths).unwrap();
        assert_eq!(changed.changed, vec!["src/a.rs"]);
        assert_eq!(changed.reused, vec!["docs/readme.md", "packages/core/infra/security.rs", "src/z.rs"]);
        assert_eq!(changed.affected[0].path, "src/a.rs");
        assert_eq!(changed.snapshot.entries["src/z.rs"].source, full.entries["src/z.rs"].source);
        let observed = scanner.scan_changed(&changed.snapshot, &BTreeSet::from(["src/z.rs".to_owned()])).unwrap();
        assert_eq!(observed.changed, vec!["src/z.rs"]);
        write(&root, "src/new.rs", b"pub fn new_file() {}\n");
        let added = scanner.scan_changed(&observed.snapshot, &BTreeSet::from(["src/new.rs".to_owned()])).unwrap();
        assert_eq!(added.added, vec!["src/new.rs"]);
        fs::remove_file(root.join("src/z.rs")).unwrap();
        let removed_paths = BTreeSet::from(["src/z.rs".to_owned()]);
        let removed = scanner.scan_changed(&added.snapshot, &removed_paths).unwrap();
        assert_eq!(removed.removed, vec!["src/z.rs"]);
        assert_eq!(removed.affected[0].path, "src/z.rs");
        assert_eq!(removed.snapshot.entries, scanner.scan().unwrap().entries);
        assert!(matches!(scanner.scan_changed(&removed.snapshot, &BTreeSet::from(["../escape.rs".to_owned()])), Err(ScanFailure::InvalidChangedPath { .. })));
        let mut refreshed_config = config;
        refreshed_config.set_profile("strict").unwrap();
        let refreshed = RepositoryScanner::new(&root, refreshed_config).unwrap();
        let profile_changed = refreshed.scan_changed(&removed.snapshot, &BTreeSet::new()).unwrap();
        assert_eq!(profile_changed.reused, Vec::<String>::new());
        assert_eq!(profile_changed.snapshot, refreshed.scan().unwrap());
        let forced_paths = BTreeSet::from(["src/a.rs".to_owned()]);
        let forced = refreshed
            .scan_changed(&profile_changed.snapshot, &forced_paths)
            .unwrap();
        assert_eq!(forced.changed, vec!["src/a.rs"]);
        assert_eq!(
            forced.reused,
            vec!["docs/readme.md", "packages/core/infra/security.rs", "src/new.rs"]
        );
        write(&root, "src/a.rs", &[0xff, 0xfe]);
        let non_utf = refreshed.scan_changed(&forced.snapshot, &forced_paths).unwrap();
        assert_eq!(non_utf.snapshot, refreshed.scan().unwrap());
        write(&root, "src/a.rs", b"pub fn restored() {}\n");
        let recovered = refreshed.scan_changed(&non_utf.snapshot, &forced_paths).unwrap();
        assert_eq!(recovered.snapshot, refreshed.scan().unwrap());
        write(&root, "too-large.rs", &vec![b'x'; 1025]);
        let oversized = scanner.scan().unwrap();
        assert!(!oversized.entries.contains_key("too-large.rs"));
        assert!(oversized.diagnostics.iter().any(|diagnostic| {
            matches!(diagnostic, ScanDiagnostic::OversizedInput { path, .. } if path == "too-large.rs")
        }));
        fs::remove_file(root.join("too-large.rs")).unwrap();
        if outside_link(&root) {
            let linked = refreshed.scan_changed(&recovered.snapshot, &BTreeSet::from(["outside.rs".to_owned()])).unwrap();
            assert_eq!(linked.snapshot, refreshed.scan().unwrap());
            assert!(symlink(&linked.snapshot));
            fs::remove_file(root.join("outside.rs")).unwrap();
        }
        let maxed = RepositoryScanner::new(&root, ScanConfig::new(2, 1024, 4096)).unwrap();
        assert!(file_limit(maxed.scan().unwrap_err()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_ignore_precedes_legacy_and_neither_control_file_is_scanned() {
        let root = root();
        write(&root, BRANIGNORE_FILENAME, b"native-only.rs\n");
        write(&root, LEGACY_OKFIGNORE_FILENAME, b"../invalid-fallback\n");
        write(&root, "native-only.rs", b"native\n");
        write(&root, "legacy-visible.rs", b"legacy\n");

        let scanner = RepositoryScanner::new(&root, ScanConfig::default()).unwrap();
        let snapshot = scanner.scan().unwrap();

        assert!(!snapshot.entries.contains_key("native-only.rs"));
        assert!(snapshot.entries.contains_key("legacy-visible.rs"));
        assert!(!snapshot.entries.contains_key(BRANIGNORE_FILENAME));
        assert!(!snapshot.entries.contains_key(LEGACY_OKFIGNORE_FILENAME));

        write(&root, LEGACY_OKFIGNORE_FILENAME, b"/still-invalid\n");
        let changed = scanner
            .scan_changed(
                &snapshot,
                &BTreeSet::from([LEGACY_OKFIGNORE_FILENAME.to_owned()]),
            )
            .unwrap();
        assert_eq!(changed.snapshot.inputs.ignore, snapshot.inputs.ignore);
        assert_eq!(changed.reused, vec!["legacy-visible.rs"]);
        assert!(changed.changed.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_ignore_is_used_only_when_native_input_is_absent() {
        let root = root();
        write(&root, LEGACY_OKFIGNORE_FILENAME, b"legacy-only.rs\n");
        write(&root, "legacy-only.rs", b"legacy\n");
        write(&root, "visible.rs", b"visible\n");

        let scanner = RepositoryScanner::new(&root, ScanConfig::default()).unwrap();
        let snapshot = scanner.scan().unwrap();

        assert!(!snapshot.entries.contains_key("legacy-only.rs"));
        assert!(snapshot.entries.contains_key("visible.rs"));
        assert!(!snapshot.entries.contains_key(LEGACY_OKFIGNORE_FILENAME));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_native_ignore_fails_without_legacy_fallback() {
        let root = root();
        write(&root, BRANIGNORE_FILENAME, b"../escape\n");
        write(&root, LEGACY_OKFIGNORE_FILENAME, b"visible.rs\n");
        write(&root, "visible.rs", b"visible\n");

        let scanner = RepositoryScanner::new(&root, ScanConfig::default()).unwrap();
        assert!(matches!(
            scanner.scan(),
            Err(ScanFailure::InvalidIgnore(IgnoreDiagnostic::InvalidRule {
                line: 1,
                reason: RuleError::Traversal,
            }))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_native_ignore_fails_at_the_bounded_limit() {
        let root = root();
        write(
            &root,
            BRANIGNORE_FILENAME,
            &vec![b'x'; MAX_BRANIGNORE_BYTES + 1],
        );

        let scanner = RepositoryScanner::new(&root, ScanConfig::default()).unwrap();
        assert!(matches!(
            scanner.scan(),
            Err(ScanFailure::InvalidIgnore(
                IgnoreDiagnostic::InputTooLarge { actual_bytes }
            )) if actual_bytes == MAX_BRANIGNORE_BYTES + 1
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn knowledge_document_scan_filters_before_limits_and_matches_incremental() {
        let root = root();
        write(&root, "source.rs", b"pub fn retained_by_default() {}\n");
        let default = RepositoryScanner::new(&root, ScanConfig::default())
            .unwrap()
            .scan()
            .unwrap();
        assert!(default.entries.contains_key("source.rs"));

        write(&root, "guide.md", b"# Guide\n");
        write(
            &root,
            "review.html",
            &vec![b'x'; DEFAULT_MAX_FILE_BYTES + 1],
        );
        let scanner = RepositoryScanner::new(&root, ScanConfig::knowledge_documents()).unwrap();
        let full = scanner.scan().unwrap();
        assert_eq!(paths(&full), vec!["guide.md"]);
        assert_eq!(full.file_count, 1);
        assert_eq!(full.total_bytes, full.entries["guide.md"].source.len());
        assert!(!full.observed_bytes.contains_key("source.rs"));
        assert!(!full.observed_bytes.contains_key("review.html"));
        assert!(full.diagnostics.is_empty());

        fs::rename(root.join("guide.md"), root.join("guide.rs")).unwrap();
        let changed = scanner
            .scan_changed(
                &full,
                &BTreeSet::from(["guide.md".to_owned(), "guide.rs".to_owned()]),
            )
            .unwrap();
        assert_eq!(changed.removed, vec!["guide.md"]);
        assert_eq!(changed.snapshot, scanner.scan().unwrap());

        write(
            &root,
            "too-large.md",
            &vec![b'x'; DEFAULT_MAX_FILE_BYTES + 1],
        );
        let oversized = scanner.scan().unwrap();
        assert!(!oversized.entries.contains_key("too-large.md"));
        assert!(oversized.diagnostics.iter().any(|diagnostic| {
            matches!(diagnostic, ScanDiagnostic::OversizedInput { path, .. } if path == "too-large.md")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_binary_is_unsupported_and_skipped() {
        let root = root();
        write(&root, "ok.rs", b"pub fn ok() {}\n");
        let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        png.resize(1024, 0);
        write(&root, "image.png", &png);
        let config = ScanConfig::new(16, 512, 4096);
        let scanner = RepositoryScanner::new(&root, config).unwrap();
        let snapshot = scanner.scan().unwrap();
        assert_eq!(paths(&snapshot), vec!["ok.rs"]);
        assert!(snapshot.diagnostics.iter().any(|d| {
            matches!(d, ScanDiagnostic::UnsupportedInput { path } if path == "image.png")
        }));
        assert!(!snapshot.entries.contains_key("image.png"));
        assert!(!snapshot.observed_bytes.contains_key("image.png"));
        assert_eq!(snapshot.total_bytes, snapshot.entries["ok.rs"].source.len());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_nul_bearing_prefix_is_unsupported() {
        let root = root();
        let mut data = b"start text then \0 nul".to_vec();
        data.resize(1024, b'x');
        write(&root, "hasnul.txt", &data);
        let config = ScanConfig::new(16, 512, 4096);
        let scanner = RepositoryScanner::new(&root, config).unwrap();
        let snapshot = scanner.scan().unwrap();
        assert!(snapshot.diagnostics.iter().any(|d| {
            matches!(d, ScanDiagnostic::UnsupportedInput { path } if path == "hasnul.txt")
        }));
        assert!(!snapshot.entries.contains_key("hasnul.txt"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_valid_utf8_is_skipped_with_diagnostic() {
        let root = root();
        write(&root, "big.rs", &vec![b'a'; 1024]);
        let config = ScanConfig::new(16, 512, 4096);
        let scanner = RepositoryScanner::new(&root, config).unwrap();
        let snapshot = scanner.scan().expect("oversized files are skipped");
        assert!(!snapshot.entries.contains_key("big.rs"));
        assert!(snapshot.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic,
                ScanDiagnostic::OversizedInput {
                    path,
                    actual: 1024,
                    limit: 512,
                } if path == "big.rs"
            )
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn root_gitignore_and_bearing_state_are_skipped() {
        let root = root();
        write(&root, ".gitignore", b"ignored/\n");
        write(&root, "ignored/private.md", b"ignored\n");
        write(&root, ".bearing/focus/log.txt", b"tool state\n");
        write(&root, "visible.md", b"visible\n");

        let snapshot = RepositoryScanner::new(&root, ScanConfig::default())
            .unwrap()
            .scan()
            .unwrap();

        assert_eq!(paths(&snapshot), vec!["visible.md"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_gitignore_is_skipped_without_being_read() {
        let root = root();
        let mut gitignore = b"ignored/\n".to_vec();
        gitignore.resize(MAX_BRANIGNORE_BYTES + 1, b'x');
        write(&root, GITIGNORE_FILENAME, &gitignore);
        write(&root, "ignored/visible.md", b"visible\n");

        let snapshot = RepositoryScanner::new(&root, ScanConfig::default())
            .unwrap()
            .scan()
            .unwrap();

        assert_eq!(paths(&snapshot), vec!["ignored/visible.md"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn probe_requires_the_opened_file_to_remain_oversized() {
        let root = root();
        write(&root, "replaced.bin", b"\0");
        let scanner = RepositoryScanner::new(&root, ScanConfig::new(16, 512, 4096)).unwrap();
        assert_eq!(
            scanner.probe_oversized(&root.join("replaced.bin"), "replaced.bin"),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_valid_prefix_later_binary_is_still_skipped() {
        let root = root();
        let mut data = vec![b'a'; 1024];
        data[600] = 0xff;
        write(&root, "ambiguous.rs", &data);
        let config = ScanConfig::new(16, 512, 4096);
        let scanner = RepositoryScanner::new(&root, config).unwrap();
        let snapshot = scanner.scan().unwrap();
        assert!(!snapshot.entries.contains_key("ambiguous.rs"));
        assert!(snapshot.diagnostics.iter().any(|diagnostic| {
            matches!(diagnostic, ScanDiagnostic::OversizedInput { path, .. } if path == "ambiguous.rs")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_binary_changed_path_parity() {
        let root = root();
        write(&root, "ok.rs", b"pub fn ok() {}\n");
        let config = ScanConfig::new(16, 512, 4096);
        let scanner = RepositoryScanner::new(&root, config).unwrap();
        let full = scanner.scan().unwrap();
        let mut png = vec![0x89, b'P', b'N', b'G'];
        png.resize(1024, 0);
        write(&root, "img.png", &png);
        let changed = scanner
            .scan_changed(&full, &BTreeSet::from(["img.png".to_owned()]))
            .unwrap();
        assert!(!changed.snapshot.entries.contains_key("img.png"));
        assert!(changed.snapshot.diagnostics.iter().any(|d| {
            matches!(d, ScanDiagnostic::UnsupportedInput { path } if path == "img.png")
        }));
        assert_eq!(changed.added, Vec::<String>::new());
        assert_eq!(changed.snapshot, scanner.scan().unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_limit_still_accepted() {
        let root = root();
        write(&root, "exact.rs", &vec![b'x'; 512]);
        let config = ScanConfig::new(16, 512, 4096);
        let scanner = RepositoryScanner::new(&root, config).unwrap();
        let snapshot = scanner.scan().unwrap();
        assert!(snapshot.entries.contains_key("exact.rs"));
        assert_eq!(snapshot.entries["exact.rs"].source.len(), 512);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_file_limit_accepts_one_mib_and_skips_larger() {
        let root = root();
        write(&root, "canonical.md", &vec![b'x'; DEFAULT_MAX_FILE_BYTES]);
        let scanner = RepositoryScanner::new(&root, ScanConfig::knowledge_documents()).unwrap();
        let snapshot = scanner.scan().unwrap();
        assert_eq!(
            snapshot.entries["canonical.md"].source.len(),
            DEFAULT_MAX_FILE_BYTES
        );

        write(
            &root,
            "canonical.md",
            &vec![b'x'; DEFAULT_MAX_FILE_BYTES + 1],
        );
        let oversized = scanner.scan().unwrap();
        assert!(!oversized.entries.contains_key("canonical.md"));
        assert!(oversized.diagnostics.iter().any(|diagnostic| {
            matches!(diagnostic, ScanDiagnostic::OversizedInput { path, .. } if path == "canonical.md")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_total_limit_accepts_256_mib_and_rejects_more() {
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        const MIB: usize = 1024 * 1024;
        let root = Cleanup(root());
        // A full 256 MiB tree takes over 15 s in debug builds, so the upper
        // boundary is checked on the default scanner's byte budget directly.
        let chunk = vec![b'x'; MIB];
        for index in 0..65 {
            write(&root.0, &format!("data/{index:03}.txt"), &chunk);
        }
        let scanner = RepositoryScanner::new(&root.0, ScanConfig::default()).unwrap();
        let snapshot = scanner.scan().unwrap();
        assert_eq!(snapshot.file_count, 65);
        assert_eq!(snapshot.total_bytes, 65 * MIB);

        assert_eq!(ScanConfig::default().max_total_bytes, 256 * MIB);
        assert!(scanner.check_bytes("last.txt", MIB, 255 * MIB).is_ok());
        assert!(matches!(
            scanner.check_bytes("over.txt", 1, 256 * MIB),
            Err(ScanFailure::LimitExceeded { actual, limit, .. })
                if actual == 256 * MIB + 1 && limit == 256 * MIB
        ));

        // Exercise small aggregate byte-budget enforcement across parallel batches
        // (40 files >= MIN_PARALLEL_FILES) and multi-batch continuation past skipped inputs.
        let small_root = Cleanup(self::root());
        for index in 0..40 {
            write(&small_root.0, &format!("data/{index:03}.txt"), &[b'a'; 20]);
        }
        let small_scanner =
            RepositoryScanner::new(&small_root.0, ScanConfig::new(100, 1024, 400)).unwrap();
        assert!(matches!(
            small_scanner.scan(),
            Err(ScanFailure::LimitExceeded {
                ref path,
                actual: 420,
                limit: 400,
            }) if path == "data/020.txt"
        ));

        let mixed_root = Cleanup(self::root());
        for index in 0..10 {
            write(&mixed_root.0, &format!("data/a{index:02}.bin"), &[0xff; 20]);
        }
        for index in 0..20 {
            write(&mixed_root.0, &format!("data/b{index:02}.txt"), &[b'b'; 20]);
        }
        let mixed_scanner =
            RepositoryScanner::new(&mixed_root.0, ScanConfig::new(100, 1024, 400)).unwrap();
        let mixed_ok = mixed_scanner.scan().unwrap();
        assert_eq!(mixed_ok.file_count, 20);
        assert_eq!(mixed_ok.total_bytes, 400);
        assert_eq!(mixed_ok.diagnostics.len(), 10);

        write(&mixed_root.0, "data/b20.txt", &[b'b'; 20]);
        assert!(matches!(
            mixed_scanner.scan(),
            Err(ScanFailure::LimitExceeded {
                ref path,
                actual: 420,
                limit: 400,
            }) if path == "data/b20.txt"
        ));

        let single_over_root = Cleanup(self::root());
        for index in 0..20 {
            write(
                &single_over_root.0,
                &format!("data/s{index:02}.txt"),
                &[b'c'; 20],
            );
        }
        write(&single_over_root.0, "data/s20.txt", &[b'c'; 500]);
        let single_over_scanner =
            RepositoryScanner::new(&single_over_root.0, ScanConfig::new(100, 1024, 400)).unwrap();
        assert!(matches!(
            single_over_scanner.scan(),
            Err(ScanFailure::LimitExceeded {
                ref path,
                actual: 900,
                limit: 400,
            }) if path == "data/s20.txt"
        ));
    }

    #[test]
    fn probe_constant_is_bounded() {
        const {
            assert!(MAX_OVERSIZE_PROBE_BYTES >= 256);
            assert!(MAX_OVERSIZE_PROBE_BYTES <= 1024);
        }
    }
}
