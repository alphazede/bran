//! Immutable delegation request contract.

use std::collections::{BTreeMap, BTreeSet};

use crate::adapters::is_public_dlp_safe;
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
pub struct GroundingContract {
    admitted_citation_locators: Vec<String>,
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
            preservation_anchors: unique_anchors.into_values().collect(),
        })
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
