//! Immutable delegation request contract.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::adapters::is_public_dlp_safe;
use crate::agent::result_store::ResultId;
use crate::agent::runtime::{valid_sha256, MIN_CLAIM_SUPPORT_BYTES};
use crate::packet::PreservationAnchor;

use super::{ReasoningLevel, ToolPolicy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelegationRequestError {
    _p: (),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroundingContractError {
    _p: (),
}

/// Bounded evidence admission contract for one provider invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedEvidence {
    locator: String,
    content_digest: String,
}

impl AdmittedEvidence {
    pub fn new(
        locator: impl Into<String>,
        content_digest: impl Into<String>,
    ) -> Result<Self, GroundingContractError> {
        let locator = locator.into();
        let content_digest = content_digest.into();
        if !valid_locator(&locator) || !valid_sha256(&content_digest) {
            return Err(GroundingContractError { _p: () });
        }
        Ok(Self {
            locator,
            content_digest,
        })
    }

    pub fn locator(&self) -> &str {
        &self.locator
    }

    pub fn content_digest(&self) -> &str {
        &self.content_digest
    }
}

/// Bounded evidence admission contract for one provider invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroundingContract {
    admitted_citation_locators: Vec<String>,
    admitted_evidence: Vec<AdmittedEvidence>,
    repository_root: Option<PathBuf>,
    preservation_anchors: Vec<PreservationAnchor>,
}

impl GroundingContract {
    pub fn new(
        admitted_citation_locators: impl IntoIterator<Item = impl Into<String>>,
        preservation_anchors: impl IntoIterator<Item = PreservationAnchor>,
    ) -> Result<Self, GroundingContractError> {
        let locators = admitted_citation_locators
            .into_iter()
            .map(Into::into)
            .collect::<Vec<String>>();
        let anchors = preservation_anchors.into_iter().collect::<Vec<_>>();
        if locators.is_empty() || locators.len() > 128 || anchors.is_empty() || anchors.len() > 128
        {
            return Err(GroundingContractError { _p: () });
        }
        let mut total_bytes = 0usize;
        let mut unique_locators = BTreeSet::new();
        for locator in locators {
            if locator.trim().is_empty()
                || locator.len() > 1024
                || !is_public_dlp_safe(&locator)
                || !unique_locators.insert(locator)
            {
                return Err(GroundingContractError { _p: () });
            }
        }
        let mut unique_anchors = BTreeMap::new();
        for anchor in anchors {
            if !is_public_dlp_safe(anchor.value())
                || unique_anchors
                    .insert(anchor.id().to_owned(), anchor)
                    .is_some()
            {
                return Err(GroundingContractError { _p: () });
            }
        }
        for value in unique_locators
            .iter()
            .map(String::as_str)
            .chain(unique_anchors.values().map(PreservationAnchor::value))
        {
            total_bytes = total_bytes
                .checked_add(value.len())
                .ok_or(GroundingContractError { _p: () })?;
        }
        if total_bytes > 65_536 {
            return Err(GroundingContractError { _p: () });
        }
        Ok(Self {
            admitted_citation_locators: unique_locators.into_iter().collect(),
            admitted_evidence: Vec::new(),
            repository_root: None,
            preservation_anchors: unique_anchors.into_values().collect(),
        })
    }

    pub fn with_evidence(
        repository_root: impl AsRef<Path>,
        admitted_evidence: impl IntoIterator<Item = AdmittedEvidence>,
        preservation_anchors: impl IntoIterator<Item = PreservationAnchor>,
    ) -> Result<Self, GroundingContractError> {
        let repository_root = fs::canonicalize(repository_root.as_ref())
            .map_err(|_| GroundingContractError { _p: () })?;
        let root_metadata = fs::symlink_metadata(&repository_root)
            .map_err(|_| GroundingContractError { _p: () })?;
        if !repository_root.is_absolute()
            || !root_metadata.is_dir()
            || root_metadata.file_type().is_symlink()
        {
            return Err(GroundingContractError { _p: () });
        }

        let mut evidence = admitted_evidence.into_iter().collect::<Vec<_>>();
        if evidence.is_empty() || evidence.len() > 128 {
            return Err(GroundingContractError { _p: () });
        }
        evidence.sort_by(|left, right| left.locator.cmp(&right.locator));
        if evidence
            .windows(2)
            .any(|pair| pair[0].locator == pair[1].locator)
        {
            return Err(GroundingContractError { _p: () });
        }
        let locators = evidence
            .iter()
            .map(|item| item.locator.clone())
            .collect::<Vec<_>>();
        let mut contract = Self::new(locators, preservation_anchors)?;
        let evidence_bytes = evidence.iter().try_fold(0usize, |total, item| {
            total
                .checked_add(item.locator.len())
                .and_then(|value| value.checked_add(item.content_digest.len()))
        });
        if evidence_bytes.is_none_or(|bytes| bytes > 65_536) {
            return Err(GroundingContractError { _p: () });
        }
        contract.admitted_evidence = evidence;
        contract.repository_root = Some(repository_root);
        Ok(contract)
    }

    pub fn admitted_citation_locators(&self) -> &[String] {
        &self.admitted_citation_locators
    }

    pub fn preservation_anchors(&self) -> &[PreservationAnchor] {
        &self.preservation_anchors
    }

    pub fn admits_citation(&self, locator: &str) -> bool {
        self.admitted_citation_locators
            .binary_search_by(|candidate| candidate.as_str().cmp(locator))
            .is_ok()
    }

    pub fn admitted_evidence(&self, locator: &str) -> Option<&AdmittedEvidence> {
        self.admitted_evidence
            .binary_search_by(|candidate| candidate.locator.as_str().cmp(locator))
            .ok()
            .map(|index| &self.admitted_evidence[index])
    }

    pub fn has_verifiable_evidence(&self) -> bool {
        self.repository_root.is_some() && !self.admitted_evidence.is_empty()
    }

    /// Reopens each uniquely cited file once and verifies the packet digest and
    /// exact support bytes against current repository content.
    pub fn verifies_support<'a>(
        &self,
        claims: impl IntoIterator<Item = (&'a str, &'a str, &'a str)>,
    ) -> bool {
        let Some(root) = self.repository_root.as_deref() else {
            return false;
        };
        let mut current = BTreeMap::<String, Vec<u8>>::new();
        let mut count = 0usize;
        for (locator, claimed_digest, support) in claims {
            count += 1;
            if count > 128
                || support.trim().len() < MIN_CLAIM_SUPPORT_BYTES
                || support.len() > 65_536
            {
                return false;
            }
            let Some(admitted) = self.admitted_evidence(locator) else {
                return false;
            };
            if claimed_digest != admitted.content_digest {
                return false;
            }
            let bytes = match current.get(locator) {
                Some(bytes) => bytes,
                None => {
                    let Some(bytes) = read_current_file(root, locator) else {
                        return false;
                    };
                    if ResultId::sha256(&bytes).value() != admitted.content_digest {
                        return false;
                    }
                    current.insert(locator.to_owned(), bytes);
                    current.get(locator).expect("inserted evidence must exist")
                }
            };
            if !bytes
                .windows(support.len())
                .any(|candidate| candidate == support.as_bytes())
            {
                return false;
            }
        }
        count > 0
    }
}

fn valid_locator(locator: &str) -> bool {
    !locator.is_empty()
        && locator.len() <= 1024
        && is_public_dlp_safe(locator)
        && Path::new(locator)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn read_current_file(root: &Path, locator: &str) -> Option<Vec<u8>> {
    const MAX_FILE_BYTES: u64 = 1024 * 1024;
    if !valid_locator(locator) {
        return None;
    }
    let mut joined = root.to_path_buf();
    let components = Path::new(locator).components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            return None;
        };
        joined.push(component);
        let metadata = fs::symlink_metadata(&joined).ok()?;
        if metadata.file_type().is_symlink()
            || (index + 1 < components.len() && !metadata.file_type().is_dir())
        {
            return None;
        }
    }
    let joined_metadata = fs::symlink_metadata(&joined).ok()?;
    if !joined_metadata.file_type().is_file()
        || joined_metadata.file_type().is_symlink()
        || joined_metadata.len() > MAX_FILE_BYTES
    {
        return None;
    }
    let canonical = fs::canonicalize(&joined).ok()?;
    if !canonical.starts_with(root) || canonical == root {
        return None;
    }
    let file = File::open(&canonical).ok()?;
    let opened_metadata = file.metadata().ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if joined_metadata.dev() != opened_metadata.dev()
            || joined_metadata.ino() != opened_metadata.ino()
        {
            return None;
        }
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    let final_metadata = fs::metadata(&canonical).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened_metadata.dev() != final_metadata.dev()
            || opened_metadata.ino() != final_metadata.ino()
            || opened_metadata.size() != final_metadata.size()
            || opened_metadata.mtime() != final_metadata.mtime()
            || opened_metadata.mtime_nsec() != final_metadata.mtime_nsec()
        {
            return None;
        }
    }
    Some(bytes)
}

/// Immutable value object bundling delegation configuration.
/// Provides safe constructor and defaults for overrides, tool policy,
/// flags, max output, and depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationOptions {
    pub provider_override: Option<String>,
    pub model_override: Option<String>,
    pub reasoning_override: Option<ReasoningLevel>,
    pub tool_policy: ToolPolicy,
    pub no_session: bool,
    pub max_output_bytes: usize,
    pub hard_total_token_ceiling: Option<usize>,
    pub max_output_tokens: Option<usize>,
    pub delegation_depth: usize,
    pub grounding_contract: Option<GroundingContract>,
}

impl Default for DelegationOptions {
    fn default() -> Self {
        Self {
            provider_override: None,
            model_override: None,
            reasoning_override: None,
            tool_policy: ToolPolicy::read_only_default(),
            no_session: false,
            max_output_bytes: 65_536,
            hard_total_token_ceiling: None,
            max_output_tokens: None,
            delegation_depth: 0,
            grounding_contract: None,
        }
    }
}

impl DelegationOptions {
    /// Safe constructor using secure defaults:
    /// - no overrides
    /// - ToolPolicy::read_only_default()
    /// - flags all false
    /// - max_output_bytes: 65536 (within 1..=1048576)
    /// - delegation_depth: 0
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationRequest {
    profile: String,
    prompt: String,
    provider_override: Option<String>,
    model_override: Option<String>,
    reasoning_override: Option<ReasoningLevel>,
    tool_policy: ToolPolicy,
    no_session: bool,
    max_output_bytes: usize,
    hard_total_token_ceiling: Option<usize>,
    max_output_tokens: Option<usize>,
    delegation_depth: usize,
    grounding_contract: Option<GroundingContract>,
}

impl DelegationRequest {
    pub fn new(
        profile: impl Into<String>,
        prompt: impl Into<String>,
        options: DelegationOptions,
    ) -> Result<Self, DelegationRequestError> {
        let profile = profile.into();
        if !is_valid_identity(&profile) {
            return Err(DelegationRequestError { _p: () });
        }

        let prompt = prompt.into();
        if prompt.trim().is_empty() || prompt.len() > 65536 {
            return Err(DelegationRequestError { _p: () });
        }

        let provider_override = options.provider_override;
        if let Some(ref v) = provider_override {
            if !is_valid_identity(v) {
                return Err(DelegationRequestError { _p: () });
            }
        }

        let model_override = options.model_override;
        if let Some(ref v) = model_override {
            if !is_valid_identity(v) {
                return Err(DelegationRequestError { _p: () });
            }
        }

        let tool_policy = options.tool_policy;
        let no_session = options.no_session;
        let max_output_bytes = options.max_output_bytes;
        let hard_total_token_ceiling = options.hard_total_token_ceiling;
        let max_output_tokens = options.max_output_tokens;
        let delegation_depth = options.delegation_depth;
        let grounding_contract = options.grounding_contract;
        let reasoning_override = options.reasoning_override;

        if !(1..=1_048_576).contains(&max_output_bytes) {
            return Err(DelegationRequestError { _p: () });
        }

        if hard_total_token_ceiling == Some(0)
            || max_output_tokens == Some(0)
            || matches!(
                (hard_total_token_ceiling, max_output_tokens),
                (None, Some(_))
            )
            || matches!((hard_total_token_ceiling, max_output_tokens), (Some(ceiling), Some(output)) if output > ceiling)
        {
            return Err(DelegationRequestError { _p: () });
        }

        if delegation_depth > 8 {
            return Err(DelegationRequestError { _p: () });
        }

        Ok(Self {
            profile,
            prompt,
            provider_override,
            model_override,
            reasoning_override,
            tool_policy,
            no_session,
            max_output_bytes,
            hard_total_token_ceiling,
            max_output_tokens,
            delegation_depth,
            grounding_contract,
        })
    }

    pub fn profile(&self) -> &str {
        &self.profile
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn provider_override(&self) -> Option<&str> {
        self.provider_override.as_deref()
    }

    pub fn model_override(&self) -> Option<&str> {
        self.model_override.as_deref()
    }

    pub fn reasoning_override(&self) -> Option<ReasoningLevel> {
        self.reasoning_override
    }

    pub fn tool_policy(&self) -> &ToolPolicy {
        &self.tool_policy
    }

    pub fn no_session(&self) -> bool {
        self.no_session
    }

    pub fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }

    pub fn hard_total_token_ceiling(&self) -> Option<usize> {
        self.hard_total_token_ceiling
    }

    pub fn max_output_tokens(&self) -> Option<usize> {
        self.max_output_tokens
    }

    pub fn delegation_depth(&self) -> usize {
        self.delegation_depth
    }

    pub fn grounding_contract(&self) -> Option<&GroundingContract> {
        self.grounding_contract.as_ref()
    }
}

fn is_valid_identity(s: &str) -> bool {
    let bytes = s.as_bytes();
    let len = bytes.len();
    (1..=64).contains(&len)
        && bytes
            .iter()
            .all(|&b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}
