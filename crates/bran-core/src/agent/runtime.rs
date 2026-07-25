//! Provider-neutral connected-agent runtime boundaries.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    Missing,
    Unavailable,
    Locked,
}

pub trait AuthStore {
    type Credential;

    fn resolve(&self, account_handle: &str) -> Result<Self::Credential, AuthError>;
}

use super::result_store::ResultId;
use super::{ReasoningLevel, ToolPolicy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderRequestError {
    InvalidProvider,
    InvalidModel,
    BlankPrompt,
    PromptTooLarge,
    InvalidMaxOutput,
    InvalidTokenBudget,
    InvalidDelegationDepth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRequest {
    provider: String,
    model: String,
    requested: ReasoningLevel,
    tool_policy: ToolPolicy,
    prompt: String,
    max_output_bytes: usize,
    no_session: bool,
    hard_total_token_ceiling: Option<usize>,
    max_output_tokens: Option<usize>,
    delegation_depth: usize,
}

impl ProviderRequest {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        requested: ReasoningLevel,
        tool_policy: ToolPolicy,
        prompt: impl Into<String>,
        max_output_bytes: usize,
        delegation_depth: usize,
    ) -> Result<Self, ProviderRequestError> {
        let provider = provider.into();
        let model = model.into();
        let prompt = prompt.into();

        if !is_valid_name(&provider) {
            return Err(ProviderRequestError::InvalidProvider);
        }
        if !is_valid_name(&model) {
            return Err(ProviderRequestError::InvalidModel);
        }
        if prompt.trim().is_empty() {
            return Err(ProviderRequestError::BlankPrompt);
        }
        if prompt.len() > 65536 {
            return Err(ProviderRequestError::PromptTooLarge);
        }
        if !(1..=1048576).contains(&max_output_bytes) {
            return Err(ProviderRequestError::InvalidMaxOutput);
        }
        if delegation_depth > 8 {
            return Err(ProviderRequestError::InvalidDelegationDepth);
        }

        Ok(Self {
            provider,
            model,
            requested,
            tool_policy,
            prompt,
            max_output_bytes,
            no_session: false,
            hard_total_token_ceiling: None,
            max_output_tokens: None,
            delegation_depth,
        })
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn requested(&self) -> ReasoningLevel {
        self.requested
    }

    pub fn tool_policy(&self) -> &ToolPolicy {
        &self.tool_policy
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }

    pub fn with_no_session(mut self, no_session: bool) -> Self {
        self.no_session = no_session;
        self
    }

    pub fn no_session(&self) -> bool {
        self.no_session
    }

    /// Sets an optional, user-configured token allocation for hosts that can
    /// attest exact token usage.
    pub fn with_token_budget(
        mut self,
        hard_total_token_ceiling: usize,
        max_output_tokens: usize,
    ) -> Result<Self, ProviderRequestError> {
        if hard_total_token_ceiling == 0
            || max_output_tokens == 0
            || max_output_tokens > hard_total_token_ceiling
        {
            return Err(ProviderRequestError::InvalidTokenBudget);
        }
        self.hard_total_token_ceiling = Some(hard_total_token_ceiling);
        self.max_output_tokens = Some(max_output_tokens);
        Ok(self)
    }

    pub fn with_total_token_ceiling(
        mut self,
        hard_total_token_ceiling: usize,
    ) -> Result<Self, ProviderRequestError> {
        if hard_total_token_ceiling == 0 {
            return Err(ProviderRequestError::InvalidTokenBudget);
        }
        self.hard_total_token_ceiling = Some(hard_total_token_ceiling);
        Ok(self)
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
}

fn is_valid_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    let len = bytes.len();
    (1..=64).contains(&len)
        && bytes
            .iter()
            .all(|&b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderError {
    Unavailable,
    Timeout,
    Cancelled,
    Failed,
    InvalidOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderOutputError {
    _p: (),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Patch,
    Json,
    ValidationReceipt,
    Opaque,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LosslessArtifactError {
    _p: (),
}

/// Immutable lossless artifact that preserves exact bytes and is independently
/// content-addressed via SHA-256 ResultId. Used to keep patch JSON and
/// validation receipts byte-exact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LosslessArtifact {
    kind: ArtifactKind,
    media_type: String,
    bytes: Vec<u8>,
    id: ResultId,
}

impl LosslessArtifact {
    /// Constructs a LosslessArtifact.
    ///
    /// Rejects:
    /// - empty bytes or > 1048576 bytes
    /// - blank or >128 byte media_type that is not lowercase ASCII
    ///   (a-z, 0-9, / - + . _ allowed; must be lowercase)
    pub fn new(
        kind: ArtifactKind,
        media_type: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<Self, LosslessArtifactError> {
        let media_type = media_type.into();
        let bytes = bytes.into();

        if bytes.is_empty() || bytes.len() > 1_048_576 {
            return Err(LosslessArtifactError { _p: () });
        }
        if media_type.trim().is_empty() || media_type.len() > 128 {
            return Err(LosslessArtifactError { _p: () });
        }
        if !media_type
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'+' | b'.' | b'_'))
        {
            return Err(LosslessArtifactError { _p: () });
        }

        let id = ResultId::sha256(&bytes);
        Ok(Self {
            kind,
            media_type,
            bytes,
            id,
        })
    }

    pub fn kind(&self) -> ArtifactKind {
        self.kind
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn id(&self) -> &ResultId {
        &self.id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderTokenUsage {
    pub actual_input_tokens: Option<usize>,
    pub actual_output_tokens: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOutput {
    answer: String,
    citations: Vec<String>,
    provider_run_id: Option<String>,
    effective_model: Option<String>,
    effective_reasoning: Option<String>,
    effective_profile: Option<String>,
    effective_provider: Option<String>,
    actual_input_tokens: Option<usize>,
    actual_output_tokens: Option<usize>,
    enforced_token_ceiling: Option<usize>,
    artifacts: Vec<LosslessArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderExecutionEvidence {
    effective_profile: Option<String>,
    effective_provider: Option<String>,
    effective_model: Option<String>,
    effective_reasoning: Option<String>,
}

impl ProviderExecutionEvidence {
    pub fn new(
        effective_profile: Option<impl Into<String>>,
        effective_provider: Option<impl Into<String>>,
        effective_model: Option<impl Into<String>>,
        effective_reasoning: Option<impl Into<String>>,
    ) -> Result<Self, ProviderOutputError> {
        let evidence = Self {
            effective_profile: effective_profile.map(Into::into),
            effective_provider: effective_provider.map(Into::into),
            effective_model: effective_model.map(Into::into),
            effective_reasoning: effective_reasoning.map(Into::into),
        };
        if matches!(&evidence.effective_profile, Some(value) if !is_valid_name(value))
            || matches!(&evidence.effective_provider, Some(value) if !is_valid_name(value))
            || matches!(&evidence.effective_model, Some(value) if !is_valid_name(value))
            || matches!(&evidence.effective_reasoning, Some(value) if !is_valid_name(value))
        {
            return Err(ProviderOutputError { _p: () });
        }
        Ok(evidence)
    }
}

impl ProviderOutput {
    pub fn new(
        answer: impl Into<String>,
        citations: impl IntoIterator<Item = impl Into<String>>,
        provider_run_id: Option<impl Into<String>>,
        effective_model: Option<impl Into<String>>,
        effective_reasoning: Option<impl Into<String>>,
        token_usage: ProviderTokenUsage,
        artifacts: impl IntoIterator<Item = LosslessArtifact>,
    ) -> Result<Self, ProviderOutputError> {
        let answer = answer.into();
        if answer.trim().is_empty() || answer.len() > 1048576 {
            return Err(ProviderOutputError { _p: () });
        }

        let citations: Vec<String> = citations.into_iter().map(Into::into).collect();
        if citations.len() > 128 {
            return Err(ProviderOutputError { _p: () });
        }
        for c in &citations {
            if c.trim().is_empty() || c.len() > 1024 {
                return Err(ProviderOutputError { _p: () });
            }
        }

        let provider_run_id = provider_run_id.map(Into::into);
        let effective_model = effective_model.map(Into::into);
        let effective_reasoning = effective_reasoning.map(Into::into);
        if matches!(&provider_run_id, Some(v) if !is_valid_name(v))
            || matches!(&effective_model, Some(v) if !is_valid_name(v))
            || matches!(&effective_reasoning, Some(v) if !is_valid_name(v))
        {
            return Err(ProviderOutputError { _p: () });
        }

        let artifacts: Vec<LosslessArtifact> = artifacts.into_iter().collect();
        if artifacts.len() > 64 {
            return Err(ProviderOutputError { _p: () });
        }

        Ok(Self {
            answer,
            citations,
            provider_run_id,
            effective_model,
            effective_reasoning,
            effective_profile: None,
            effective_provider: None,
            actual_input_tokens: token_usage.actual_input_tokens,
            actual_output_tokens: token_usage.actual_output_tokens,
            enforced_token_ceiling: None,
            artifacts,
        })
    }

    pub fn with_effective_execution(
        answer: impl Into<String>,
        citations: impl IntoIterator<Item = impl Into<String>>,
        provider_run_id: Option<impl Into<String>>,
        evidence: ProviderExecutionEvidence,
        token_usage: ProviderTokenUsage,
        artifacts: impl IntoIterator<Item = LosslessArtifact>,
    ) -> Result<Self, ProviderOutputError> {
        let ProviderExecutionEvidence {
            effective_profile,
            effective_provider,
            effective_model,
            effective_reasoning,
        } = evidence;
        let mut output = Self::new(
            answer,
            citations,
            provider_run_id,
            effective_model,
            effective_reasoning,
            token_usage,
            artifacts,
        )?;
        output.effective_profile = effective_profile;
        output.effective_provider = effective_provider;
        Ok(output)
    }

    pub fn answer(&self) -> &str {
        &self.answer
    }

    pub fn citations(&self) -> &[String] {
        &self.citations
    }

    pub fn provider_run_id(&self) -> Option<&str> {
        self.provider_run_id.as_deref()
    }

    pub fn effective_model(&self) -> Option<&str> {
        self.effective_model.as_deref()
    }

    pub fn effective_reasoning(&self) -> Option<&str> {
        self.effective_reasoning.as_deref()
    }

    pub fn effective_profile(&self) -> Option<&str> {
        self.effective_profile.as_deref()
    }

    pub fn effective_provider(&self) -> Option<&str> {
        self.effective_provider.as_deref()
    }

    pub fn actual_input_tokens(&self) -> Option<usize> {
        self.actual_input_tokens
    }

    pub fn actual_output_tokens(&self) -> Option<usize> {
        self.actual_output_tokens
    }

    /// Attests that the provider adapter enforced this exact aggregate ceiling.
    /// Adapters must set this only after their preflight and result checks bind
    /// the provider run to the request's hard total token ceiling.
    pub fn with_token_budget_attestation(
        mut self,
        hard_total_token_ceiling: usize,
    ) -> Result<Self, ProviderOutputError> {
        if hard_total_token_ceiling == 0 {
            return Err(ProviderOutputError { _p: () });
        }
        self.enforced_token_ceiling = Some(hard_total_token_ceiling);
        Ok(self)
    }

    pub fn enforced_token_ceiling(&self) -> Option<usize> {
        self.enforced_token_ceiling
    }

    pub fn artifacts(&self) -> &[LosslessArtifact] {
        &self.artifacts
    }
}

pub trait ProviderPort<C> {
    fn invoke(
        &self,
        request: &ProviderRequest,
        credential: &C,
    ) -> Result<ProviderOutput, ProviderError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationState {
    Configured,
    PacketReady,
    Running,
    Validating,
    Complete,
    Incomplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InvocationLifecycle {
    state: InvocationState,
}

impl InvocationLifecycle {
    pub(crate) const fn configured() -> Self {
        Self {
            state: InvocationState::Configured,
        }
    }

    pub(crate) fn advance(&mut self, next: InvocationState) -> Result<(), AgentFailure> {
        if matches!(
            (self.state, next),
            (InvocationState::Configured, InvocationState::PacketReady)
                | (InvocationState::PacketReady, InvocationState::Running)
                | (InvocationState::Running, InvocationState::Validating)
        ) {
            self.state = next;
            Ok(())
        } else {
            Err(AgentFailure::InvalidOutput)
        }
    }

    pub(crate) fn complete(&mut self) -> Result<(), AgentFailure> {
        if self.state == InvocationState::Validating {
            self.state = InvocationState::Complete;
            Ok(())
        } else {
            Err(AgentFailure::InvalidOutput)
        }
    }

    pub(crate) fn incomplete(&mut self) -> Result<(), AgentFailure> {
        if matches!(
            self.state,
            InvocationState::Complete | InvocationState::Incomplete
        ) {
            Err(AgentFailure::InvalidOutput)
        } else {
            self.state = InvocationState::Incomplete;
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentFailure {
    AgentDisabled,
    ExplicitOffline,
    ProjectUntrusted,
    UnknownProfile,
    UnknownProvider,
    UnknownModel,
    MissingAuth,
    ProviderUnavailable,
    ProviderFailed,
    Timeout,
    Cancelled,
    DepthExceeded,
    DeniedTool,
    InvalidOutput,
    DlpRejected,
    GroundingFailed,
    TokenBudgetUnattested,
    TokenCeilingExceeded,
    SqzInputFailed,
    SqzOutputFailed,
    ResultStoreFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attestation<T> {
    Attested(T),
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvocationMetrics {
    actual_input_tokens: Option<usize>,
    actual_output_tokens: Option<usize>,
    input_bytes: usize,
    output_bytes: usize,
    latency_ms: u64,
}

impl InvocationMetrics {
    pub fn new(
        actual_input_tokens: Option<usize>,
        actual_output_tokens: Option<usize>,
        input_bytes: usize,
        output_bytes: usize,
        latency_ms: u64,
    ) -> Self {
        Self {
            actual_input_tokens,
            actual_output_tokens,
            input_bytes,
            output_bytes,
            latency_ms,
        }
    }

    pub fn actual_input_tokens(&self) -> Option<usize> {
        self.actual_input_tokens
    }

    pub fn actual_output_tokens(&self) -> Option<usize> {
        self.actual_output_tokens
    }

    pub fn input_bytes(&self) -> usize {
        self.input_bytes
    }

    pub fn output_bytes(&self) -> usize {
        self.output_bytes
    }

    pub fn latency_ms(&self) -> u64 {
        self.latency_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationOutcome {
    Complete {
        metrics: InvocationMetrics,
    },
    Incomplete {
        failure: AgentFailure,
        metrics: InvocationMetrics,
    },
}

impl InvocationOutcome {
    /// Compact constructor returning closed AgentFailure error.
    /// Complete must not carry failure; Incomplete must carry one.
    pub fn new(
        state: InvocationState,
        failure: Option<AgentFailure>,
        metrics: InvocationMetrics,
    ) -> Result<Self, AgentFailure> {
        match (state, failure) {
            (InvocationState::Complete, None) => Ok(InvocationOutcome::Complete { metrics }),
            (InvocationState::Complete, Some(f)) => Err(f),
            (InvocationState::Incomplete, Some(f)) => Ok(InvocationOutcome::Incomplete {
                failure: f,
                metrics,
            }),
            (InvocationState::Incomplete, None) => Err(AgentFailure::InvalidOutput),
            _ => Err(AgentFailure::InvalidOutput),
        }
    }
}
