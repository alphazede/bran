//! Provider-neutral agent-runtime coordination.
//! Standard library only; adapters own authentication, provider, and SQZ I/O.

use crate::adapters::{
    DlpStatus, FidelityStatus, SqzAdapter, SqzAdapterConfig, SqzIdentity, SqzPolicy, SqzPort,
    SqzReceipt, SqzStatus,
};
use crate::packet::{
    ContextPacket, PacketReceipt, PreservationAnchor, TokenEstimateMethod,
    PACKET_RECEIPT_SCHEMA_VERSION,
};

use super::delegate::DelegationRequest;
use super::receipt::{
    DelegationReceipt, DelegationReceiptParts, EffectiveExecution, InlineResult,
    RequestedExecution, SqzStages, StoredResultRef,
};
use super::result_store::{ResultId, ResultStore};
use super::runtime::{
    AgentFailure, Attestation, AuthError, AuthStore, InvocationLifecycle, InvocationMetrics,
    InvocationOutcome, InvocationState, ProviderError, ProviderPort, ProviderRequest,
};
use super::{AgentProfile, AgentProfileRegistry, ReasoningLevel, ToolPolicy};

/// Stage at which SQZ compression is applied during agent coordination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqzStage {
    Input,
    Output,
}

/// Immutable output from an agent SQZ stage: a bounded, non-blank payload
/// together with the corresponding policy receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSqzOutput {
    payload: String,
    receipt: SqzReceipt,
}

impl AgentSqzOutput {
    pub fn new(payload: impl Into<String>, receipt: SqzReceipt) -> Result<Self, AgentSqzError> {
        let payload = payload.into();
        if payload.trim().is_empty() || payload.len() > 1_048_576 {
            return Err(AgentSqzError::new(AgentSqzFailureCode::InvalidOutput, None));
        }
        Ok(Self { payload, receipt })
    }

    pub fn payload(&self) -> &str {
        &self.payload
    }

    pub fn receipt(&self) -> &SqzReceipt {
        &self.receipt
    }
}

/// Closed codes for agent SQZ failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSqzFailureCode {
    InputFailed,
    OutputFailed,
    InvalidOutput,
}

/// SQZ failure with optional partial policy evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSqzError {
    code: AgentSqzFailureCode,
    receipt: Option<Box<SqzReceipt>>,
}

impl AgentSqzError {
    pub fn new(code: AgentSqzFailureCode, receipt: Option<SqzReceipt>) -> Self {
        Self {
            code,
            receipt: receipt.map(Box::new),
        }
    }

    pub fn code(&self) -> AgentSqzFailureCode {
        self.code
    }

    pub fn receipt(&self) -> Option<&SqzReceipt> {
        self.receipt.as_deref()
    }

    fn into_receipt(self) -> Option<SqzReceipt> {
        self.receipt.map(|receipt| *receipt)
    }
}

/// Provider-neutral port for SQZ evaluation at agent coordination seams.
pub trait AgentSqzPort {
    fn evaluate(
        &self,
        stage: SqzStage,
        payload: &str,
        max_output_bytes: usize,
    ) -> Result<AgentSqzOutput, AgentSqzError>;

    fn evaluate_with_anchors(
        &self,
        stage: SqzStage,
        payload: &str,
        max_output_bytes: usize,
        preservation_anchors: &[PreservationAnchor],
    ) -> Result<AgentSqzOutput, AgentSqzError> {
        if preservation_anchors.is_empty() {
            self.evaluate(stage, payload, max_output_bytes)
        } else {
            Err(AgentSqzError::new(AgentSqzFailureCode::InvalidOutput, None))
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentSqzAdapter<P> {
    port: P,
    policy: SqzPolicy,
    configured_max_output_bytes: usize,
}

impl<P> AgentSqzAdapter<P> {
    pub fn new(port: P, policy: SqzPolicy, configured_max_output_bytes: usize) -> Self {
        Self {
            port,
            policy,
            configured_max_output_bytes,
        }
    }
}

impl<P: SqzPort + Clone> AgentSqzPort for AgentSqzAdapter<P> {
    fn evaluate(
        &self,
        stage: SqzStage,
        payload: &str,
        max_output_bytes: usize,
    ) -> Result<AgentSqzOutput, AgentSqzError> {
        self.evaluate_with_anchors(stage, payload, max_output_bytes, &[])
    }

    fn evaluate_with_anchors(
        &self,
        stage: SqzStage,
        payload: &str,
        max_output_bytes: usize,
        preservation_anchors: &[PreservationAnchor],
    ) -> Result<AgentSqzOutput, AgentSqzError> {
        let bytes = payload.len();
        let packet = ContextPacket {
            items: vec![],
            payload: payload.to_owned(),
            receipt: PacketReceipt {
                schema_version: PACKET_RECEIPT_SCHEMA_VERSION,
                seed_ids: vec![],
                admitted_dependency_ids: vec![],
                selected_ids: vec![],
                omitted_ids: vec![],
                dependency_closure_truncated: false,
                dependency_max_depth: 0,
                dependency_max_nodes: 0,
                raw_bytes: bytes,
                estimated_tokens: bytes.div_ceil(4),
                token_estimate_method: TokenEstimateMethod::BytesDividedByFourCeiling,
                effective_max_items: 0,
                effective_max_bytes: bytes,
                runtime_token_ceiling: None,
                truncated: false,
            },
        };
        let anchors = if !preservation_anchors.is_empty() {
            preservation_anchors.to_vec()
        } else if self.policy == SqzPolicy::PublicOff {
            Vec::new()
        } else {
            let anchor = payload.chars().take(128).collect::<String>();
            vec![PreservationAnchor::new(
                match stage {
                    SqzStage::Input => "agent-input",
                    SqzStage::Output => "agent-output",
                },
                anchor,
            )
            .map_err(|_| AgentSqzError::new(AgentSqzFailureCode::InvalidOutput, None))?]
        };
        let adapter = SqzAdapter::new(
            self.port.clone(),
            SqzAdapterConfig::new(
                self.policy,
                SqzIdentity::approved(),
                self.configured_max_output_bytes,
                anchors,
            ),
        );
        match adapter.evaluate(packet, max_output_bytes) {
            Ok(output) => AgentSqzOutput::new(output.packet.payload, output.receipt),
            Err(error) => Err(AgentSqzError::new(
                match stage {
                    SqzStage::Input => AgentSqzFailureCode::InputFailed,
                    SqzStage::Output => AgentSqzFailureCode::OutputFailed,
                },
                Some(*error.receipt),
            )),
        }
    }
}

/// Runtime configuration for connected agent behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentRuntimeConfig {
    enabled: bool,
    max_delegation_depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentRuntimeConfigError {
    _p: (),
}

impl AgentRuntimeConfig {
    pub fn new(
        enabled: bool,
        max_delegation_depth: usize,
    ) -> Result<Self, AgentRuntimeConfigError> {
        if !(1..=8).contains(&max_delegation_depth) {
            return Err(AgentRuntimeConfigError { _p: () });
        }
        Ok(Self {
            enabled,
            max_delegation_depth,
        })
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn max_delegation_depth(&self) -> usize {
        self.max_delegation_depth
    }
}

impl Default for AgentRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_delegation_depth: 8,
        }
    }
}

/// Immutable authority determined by the host, never by a delegation request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentRuntimeAuthority {
    explicit_offline: bool,
    project_trusted: bool,
    mutation_authorized: bool,
}

impl AgentRuntimeAuthority {
    pub const fn new(
        explicit_offline: bool,
        project_trusted: bool,
        mutation_authorized: bool,
    ) -> Self {
        Self {
            explicit_offline,
            project_trusted,
            mutation_authorized,
        }
    }
}

/// Borrowed seam bundle for agent runtime ports and stores.
pub struct RuntimePorts<'a, Auth, Prov, SqzP, Store>
where
    Auth: AuthStore,
    Prov: ProviderPort<<Auth as AuthStore>::Credential>,
    SqzP: AgentSqzPort,
    Store: ResultStore,
{
    pub auth_store: &'a Auth,
    pub provider_port: &'a Prov,
    pub agent_sqz_port: &'a SqzP,
    pub result_store: &'a mut Store,
}

impl<'a, Auth, Prov, SqzP, Store> RuntimePorts<'a, Auth, Prov, SqzP, Store>
where
    Auth: AuthStore,
    Prov: ProviderPort<<Auth as AuthStore>::Credential>,
    SqzP: AgentSqzPort,
    Store: ResultStore,
{
    pub fn new(
        auth_store: &'a Auth,
        provider_port: &'a Prov,
        agent_sqz_port: &'a SqzP,
        result_store: &'a mut Store,
    ) -> Self {
        Self {
            auth_store,
            provider_port,
            agent_sqz_port,
            result_store,
        }
    }
}

/// The only runtime error: a receipt invariant which should be unreachable
/// after the validated request, profile, and provider contracts are honored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRuntimeInternalError {
    ReceiptInvariant,
}

/// Provider-neutral agent coordination state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentRuntime {
    config: AgentRuntimeConfig,
}

impl AgentRuntime {
    pub const fn new(config: AgentRuntimeConfig) -> Self {
        Self { config }
    }

    pub const fn config(&self) -> AgentRuntimeConfig {
        self.config
    }

    /// Coordinates one request. Operational failures are always incomplete,
    /// content-free receipts; only impossible receipt construction fails here.
    pub fn invoke<'a, Auth, Prov, SqzP, Store, Factory>(
        &self,
        request: &DelegationRequest,
        authority: AgentRuntimeAuthority,
        agent_profile_registry: &AgentProfileRegistry,
        ports: Factory,
        now_tick: u64,
    ) -> Result<DelegationReceipt, AgentRuntimeInternalError>
    where
        Auth: AuthStore + 'a,
        Prov: ProviderPort<Auth::Credential> + 'a,
        SqzP: AgentSqzPort + 'a,
        Store: ResultStore + 'a,
        Factory: FnOnce() -> RuntimePorts<'a, Auth, Prov, SqzP, Store>,
    {
        let mut lifecycle = InvocationLifecycle::configured();
        let provisional = requested(request, None, None, None)?;
        let unavailable = unavailable_effective(request.tool_policy().clone());
        if !self.config.enabled() {
            return incomplete(
                provisional,
                unavailable,
                AgentFailure::AgentDisabled,
                None,
                None,
                0,
                0,
                request.no_session(),
            );
        }
        if authority.explicit_offline {
            return incomplete(
                provisional,
                unavailable,
                AgentFailure::ExplicitOffline,
                None,
                None,
                0,
                0,
                request.no_session(),
            );
        }
        if !authority.project_trusted {
            return incomplete(
                provisional,
                unavailable,
                AgentFailure::ProjectUntrusted,
                None,
                None,
                0,
                0,
                request.no_session(),
            );
        }
        if request.delegation_depth() > self.config.max_delegation_depth() {
            return incomplete(
                provisional,
                unavailable,
                AgentFailure::DepthExceeded,
                None,
                None,
                0,
                0,
                request.no_session(),
            );
        }

        let profile = match agent_profile_registry.get(request.profile()) {
            Ok(profile) => profile,
            Err(_) => {
                return incomplete(
                    provisional,
                    unavailable,
                    AgentFailure::UnknownProfile,
                    None,
                    None,
                    0,
                    0,
                    request.no_session(),
                )
            }
        };
        if denied_tool(request.tool_policy(), profile.tool_policy()) {
            return incomplete(
                requested(request, Some(profile), None, None)?,
                unavailable_effective(request.tool_policy().clone()),
                AgentFailure::DeniedTool,
                None,
                None,
                0,
                0,
                request.no_session(),
            );
        }
        if !authority.mutation_authorized && mutation_requested(request.tool_policy()) {
            return incomplete(
                requested(request, Some(profile), None, None)?,
                unavailable_effective(request.tool_policy().clone()),
                AgentFailure::DeniedTool,
                None,
                None,
                0,
                0,
                request.no_session(),
            );
        }

        let provider = request.provider_override().unwrap_or(profile.provider());
        let model = request.model_override().unwrap_or(profile.model());
        let reasoning = request
            .reasoning_override()
            .unwrap_or(profile.default_reasoning_level());
        let requested = requested(request, Some(profile), Some(provider), Some(model))?;
        let selected = unavailable_effective(request.tool_policy().clone());
        if agent_profile_registry
            .provider_registry()
            .require(provider)
            .is_err()
        {
            return incomplete(
                requested,
                selected,
                AgentFailure::UnknownProvider,
                None,
                None,
                0,
                0,
                request.no_session(),
            );
        }
        if agent_profile_registry
            .model_registry()
            .require_for_provider(provider, model)
            .is_err()
        {
            return incomplete(
                requested,
                selected,
                AgentFailure::UnknownModel,
                None,
                None,
                0,
                0,
                request.no_session(),
            );
        }

        let ports = ports();
        let credential = match ports.auth_store.resolve(profile.account_handle()) {
            Ok(credential) => credential,
            Err(AuthError::Missing | AuthError::Unavailable | AuthError::Locked) => {
                return incomplete(
                    requested,
                    selected,
                    AgentFailure::MissingAuth,
                    None,
                    None,
                    0,
                    0,
                    request.no_session(),
                )
            }
        };
        let grounding_anchors = request
            .grounding_contract()
            .map(|contract| contract.preservation_anchors())
            .unwrap_or(&[]);
        let input = match ports.agent_sqz_port.evaluate_with_anchors(
            SqzStage::Input,
            request.prompt(),
            request.max_output_bytes(),
            grounding_anchors,
        ) {
            Ok(output) => output,
            Err(error) => {
                let failure = sqz_failure(error.code(), SqzStage::Input);
                return incomplete(
                    requested,
                    selected,
                    failure,
                    error.into_receipt(),
                    None,
                    0,
                    0,
                    request.no_session(),
                );
            }
        };
        let input_bytes = input.payload().len();
        if !grounding_input_accepted(request, &input) {
            return incomplete(
                requested,
                selected,
                AgentFailure::GroundingFailed,
                Some(input.receipt().clone()),
                None,
                input_bytes,
                0,
                request.no_session(),
            );
        }
        if !accepted_sqz(input.receipt(), input.payload()) {
            return incomplete(
                requested,
                selected,
                AgentFailure::SqzInputFailed,
                Some(input.receipt().clone()),
                None,
                0,
                0,
                request.no_session(),
            );
        }
        let provider_request = match ProviderRequest::new(
            provider,
            model,
            reasoning,
            request.tool_policy().clone(),
            input.payload(),
            request.max_output_bytes(),
            request.delegation_depth(),
        )
        .map(|provider_request| provider_request.with_no_session(request.no_session()))
        .and_then(|provider_request| {
            match (
                request.hard_total_token_ceiling(),
                request.max_output_tokens(),
            ) {
                (Some(ceiling), Some(output)) => {
                    provider_request.with_token_budget(ceiling, output)
                }
                (Some(ceiling), None) => provider_request.with_total_token_ceiling(ceiling),
                (None, None) => Ok(provider_request),
                (None, Some(_)) => Err(super::runtime::ProviderRequestError::InvalidTokenBudget),
            }
        }) {
            Ok(provider_request) => provider_request,
            Err(_) => {
                return incomplete(
                    requested,
                    selected,
                    AgentFailure::SqzInputFailed,
                    Some(input.receipt().clone()),
                    None,
                    input_bytes,
                    0,
                    request.no_session(),
                )
            }
        };
        lifecycle
            .advance(InvocationState::PacketReady)
            .and_then(|_| lifecycle.advance(InvocationState::Running))
            .map_err(|_| AgentRuntimeInternalError::ReceiptInvariant)?;
        let provider_output = match ports.provider_port.invoke(&provider_request, &credential) {
            Ok(output) => output,
            Err(error) => {
                return incomplete(
                    requested,
                    selected,
                    provider_failure(error),
                    Some(input.receipt().clone()),
                    None,
                    input_bytes,
                    0,
                    request.no_session(),
                )
            }
        };
        lifecycle
            .advance(InvocationState::Validating)
            .map_err(|_| AgentRuntimeInternalError::ReceiptInvariant)?;
        if !safe_provider_surfaces(&provider_output) {
            return incomplete(
                requested,
                selected,
                AgentFailure::DlpRejected,
                Some(input.receipt().clone()),
                None,
                input_bytes,
                0,
                request.no_session(),
            );
        }
        if !grounded_citations_accepted(request, provider_output.citations()) {
            return incomplete(
                requested,
                selected,
                AgentFailure::GroundingFailed,
                Some(input.receipt().clone()),
                None,
                input_bytes,
                0,
                request.no_session(),
            );
        }
        let effective = provider_effective(
            profile,
            provider,
            &provider_output,
            request.tool_policy().clone(),
            agent_profile_registry,
        );
        if request.grounding_contract().is_some()
            && !grounding_execution_attested(&requested, &effective)
        {
            return incomplete(
                requested,
                effective,
                AgentFailure::InvalidOutput,
                Some(input.receipt().clone()),
                None,
                input_bytes,
                0,
                request.no_session(),
            );
        }
        if !grounded_claims_accepted(request, &provider_output) {
            return incomplete(
                requested,
                effective,
                AgentFailure::ClaimUnsupported,
                Some(input.receipt().clone()),
                None,
                input_bytes,
                0,
                request.no_session(),
            );
        }
        if let Err(failure) = enforced_token_budget(request, &provider_output) {
            return incomplete(
                requested,
                effective,
                failure,
                Some(input.receipt().clone()),
                None,
                input_bytes,
                0,
                request.no_session(),
            );
        }
        let output = match ports.agent_sqz_port.evaluate(
            SqzStage::Output,
            provider_output.answer(),
            request.max_output_bytes(),
        ) {
            Ok(output) => output,
            Err(error) => {
                let failure = sqz_failure(error.code(), SqzStage::Output);
                return incomplete(
                    requested,
                    effective,
                    failure,
                    Some(input.receipt().clone()),
                    error.into_receipt(),
                    input_bytes,
                    0,
                    request.no_session(),
                );
            }
        };

        if !accepted_sqz(output.receipt(), output.payload()) {
            return incomplete(
                requested,
                effective,
                AgentFailure::SqzOutputFailed,
                Some(input.receipt().clone()),
                Some(output.receipt().clone()),
                input_bytes,
                0,
                request.no_session(),
            );
        }
        if request.grounding_contract().is_some() && output.payload() != provider_output.answer() {
            return incomplete(
                requested,
                effective,
                AgentFailure::ClaimUnsupported,
                Some(input.receipt().clone()),
                Some(output.receipt().clone()),
                input_bytes,
                output.payload().len(),
                request.no_session(),
            );
        }

        let inline = InlineResult::with_claims(
            output.payload(),
            provider_output.citations().iter().cloned(),
            provider_output.claims().iter().cloned(),
        )
        .map_err(|_| AgentRuntimeInternalError::ReceiptInvariant)?;
        let stored_ref = match store_output(
            ports.result_store,
            &inline,
            provider_output.artifacts(),
            now_tick,
        ) {
            Some(stored_ref) => stored_ref,
            None => {
                return incomplete(
                    requested,
                    effective,
                    AgentFailure::ResultStoreFailed,
                    Some(input.receipt().clone()),
                    Some(output.receipt().clone()),
                    input_bytes,
                    output.payload().len(),
                    request.no_session(),
                )
            }
        };
        complete(
            &mut lifecycle,
            requested,
            effective,
            input.receipt().clone(),
            output.receipt().clone(),
            inline,
            stored_ref,
            provider_output.actual_input_tokens(),
            provider_output.actual_output_tokens(),
            input_bytes,
            output.payload().len(),
            request.no_session(),
            request.grounding_contract().is_some(),
            provider_run_id(&provider_output),
        )
    }
}

fn requested(
    request: &DelegationRequest,
    profile: Option<&AgentProfile>,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<RequestedExecution, AgentRuntimeInternalError> {
    RequestedExecution::with_attestations(
        request.profile(),
        provider
            .or(request.provider_override())
            .or(profile.map(AgentProfile::provider))
            .map(|value| Attestation::Attested(value.to_string()))
            .unwrap_or(Attestation::Unavailable),
        model
            .or(request.model_override())
            .or(profile.map(AgentProfile::model))
            .map(|value| Attestation::Attested(value.to_string()))
            .unwrap_or(Attestation::Unavailable),
        request
            .reasoning_override()
            .or(profile.map(AgentProfile::default_reasoning_level))
            .map(Attestation::Attested)
            .unwrap_or(Attestation::Unavailable),
        request.tool_policy().clone(),
    )
    .map_err(|_| AgentRuntimeInternalError::ReceiptInvariant)
}

fn unavailable_effective(tool_policy: ToolPolicy) -> EffectiveExecution {
    EffectiveExecution::new(
        Attestation::Unavailable,
        Attestation::Unavailable,
        Attestation::Unavailable,
        Attestation::Unavailable,
        tool_policy,
    )
}

fn provider_effective(
    profile: &AgentProfile,
    provider: &str,
    output: &super::runtime::ProviderOutput,
    tool_policy: ToolPolicy,
    registry: &AgentProfileRegistry,
) -> EffectiveExecution {
    let model = match output.effective_model() {
        Some(model) => {
            if registry
                .model_registry()
                .require_for_provider(provider, model)
                .is_err()
            {
                Attestation::Unavailable
            } else {
                Attestation::Attested(model.to_string())
            }
        }
        None => Attestation::Unavailable,
    };
    let reasoning = match output.effective_reasoning() {
        Some(reasoning) => ReasoningLevel::parse(reasoning)
            .map(Attestation::Attested)
            .unwrap_or(Attestation::Unavailable),
        None => Attestation::Unavailable,
    };
    EffectiveExecution::new(
        match output.effective_profile() {
            Some(value) if value == profile.name() => Attestation::Attested(profile.clone()),
            _ => Attestation::Unavailable,
        },
        match output.effective_provider() {
            Some(value) if value == provider => Attestation::Attested(provider.to_string()),
            _ => Attestation::Unavailable,
        },
        model,
        reasoning,
        tool_policy,
    )
}

fn denied_tool(request: &ToolPolicy, profile: &ToolPolicy) -> bool {
    request.allowed().any(|tool| !profile.allows(tool))
}

fn mutation_requested(policy: &ToolPolicy) -> bool {
    ["write", "edit", "shell", "network"]
        .into_iter()
        .any(|tool| policy.allows(tool))
}

fn sqz_failure(code: AgentSqzFailureCode, stage: SqzStage) -> AgentFailure {
    match (code, stage) {
        (AgentSqzFailureCode::InvalidOutput, SqzStage::Output) => AgentFailure::InvalidOutput,
        (_, SqzStage::Input) => AgentFailure::SqzInputFailed,
        _ => AgentFailure::SqzOutputFailed,
    }
}

fn provider_failure(error: ProviderError) -> AgentFailure {
    match error {
        ProviderError::Timeout => AgentFailure::Timeout,
        ProviderError::Cancelled => AgentFailure::Cancelled,
        ProviderError::InvalidOutput => AgentFailure::InvalidOutput,
        ProviderError::Unavailable => AgentFailure::ProviderUnavailable,
        ProviderError::Failed => AgentFailure::ProviderFailed,
    }
}

fn enforced_token_budget(
    request: &DelegationRequest,
    output: &super::runtime::ProviderOutput,
) -> Result<(), AgentFailure> {
    let Some(ceiling) = request.hard_total_token_ceiling() else {
        return Ok(());
    };
    if output.enforced_token_ceiling() != Some(ceiling) {
        return Err(AgentFailure::TokenBudgetUnattested);
    }
    let known_total = output
        .actual_input_tokens()
        .unwrap_or(0)
        .checked_add(output.actual_output_tokens().unwrap_or(0))
        .ok_or(AgentFailure::TokenCeilingExceeded)?;
    if known_total > ceiling {
        Err(AgentFailure::TokenCeilingExceeded)
    } else {
        Ok(())
    }
}

fn safe_provider_surfaces(output: &super::runtime::ProviderOutput) -> bool {
    let strings_are_safe = output
        .citations()
        .iter()
        .map(String::as_bytes)
        .chain(output.provider_run_id().map(str::as_bytes))
        .chain(output.effective_profile().map(str::as_bytes))
        .chain(output.effective_provider().map(str::as_bytes))
        .chain(output.effective_model().map(str::as_bytes))
        .chain(output.effective_reasoning().map(str::as_bytes))
        .chain(output.claims().iter().flat_map(|claim| {
            [
                claim.id().as_bytes(),
                claim.text().as_bytes(),
                claim.locator().as_bytes(),
                claim.content_digest().as_bytes(),
                claim.support().as_bytes(),
            ]
        }))
        .all(|bytes| crate::adapters::sqz::public_dlp_findings(bytes).is_empty());
    strings_are_safe
        && output.artifacts().iter().all(|artifact| {
            artifact.id() == &ResultId::sha256(artifact.bytes())
                && crate::adapters::sqz::public_dlp_findings(artifact.bytes()).is_empty()
        })
}

fn grounding_input_accepted(request: &DelegationRequest, output: &AgentSqzOutput) -> bool {
    let Some(contract) = request.grounding_contract() else {
        return true;
    };
    contract.preservation_anchors().iter().all(|anchor| {
        output.payload().contains(anchor.value())
            && output
                .receipt()
                .required_fidelity_anchor_ids
                .iter()
                .any(|id| id == anchor.id())
            && !output
                .receipt()
                .missing_fidelity_anchor_ids
                .iter()
                .any(|id| id == anchor.id())
    })
}

fn grounded_citations_accepted(request: &DelegationRequest, citations: &[String]) -> bool {
    let Some(contract) = request.grounding_contract() else {
        return true;
    };
    !citations.is_empty()
        && citations.iter().all(|citation| {
            crate::adapters::is_public_dlp_safe(citation) && contract.admits_citation(citation)
        })
}

fn grounding_execution_attested(
    requested: &RequestedExecution,
    effective: &EffectiveExecution,
) -> bool {
    matches!(effective.profile(), Attestation::Attested(profile) if profile.name() == requested.profile_name())
        && matches!((requested.provider(), effective.provider()),
            (Attestation::Attested(requested), Attestation::Attested(effective)) if requested == effective)
        && matches!((requested.model(), effective.model()),
            (Attestation::Attested(requested), Attestation::Attested(effective)) if requested == effective)
        && matches!((requested.reasoning(), effective.reasoning()),
            (Attestation::Attested(requested), Attestation::Attested(effective)) if requested == effective)
}

fn grounded_claims_accepted(
    request: &DelegationRequest,
    output: &super::runtime::ProviderOutput,
) -> bool {
    let Some(contract) = request.grounding_contract() else {
        return true;
    };
    let claims = output.claims();
    let grounded_answer = claims
        .iter()
        .map(|claim| claim.text())
        .collect::<Vec<_>>()
        .join("\n");
    contract.has_verifiable_evidence()
        && !claims.is_empty()
        && output.answer() == grounded_answer
        && claims.iter().all(|claim| {
            claim.material()
                && claim.text() == claim.support()
                && output
                    .citations()
                    .iter()
                    .any(|citation| citation == claim.locator())
        })
        && contract.verifies_support(
            claims
                .iter()
                .map(|claim| (claim.locator(), claim.content_digest(), claim.support())),
        )
}

fn accepted_sqz(receipt: &SqzReceipt, payload: &str) -> bool {
    if receipt.failure_reason.is_some()
        || receipt.fidelity_status != FidelityStatus::Passed
        || receipt.dlp_status != DlpStatus::Passed
        || !receipt.dlp_findings.is_empty()
        || receipt.returned_bytes != payload.len()
    {
        return false;
    }
    match (receipt.policy, receipt.status) {
        (SqzPolicy::PublicOff, SqzStatus::Off) => {
            receipt.raw_bytes == receipt.returned_bytes
                && receipt.raw_token_estimate_bytes_divided_by_four_ceiling
                    == receipt.returned_token_estimate_bytes_divided_by_four_ceiling
                && receipt.raw_token_estimate_bytes_divided_by_four_ceiling
                    == receipt.raw_bytes.div_ceil(4)
                && receipt.candidate_compressed_bytes.is_none()
                && receipt
                    .candidate_token_estimate_bytes_divided_by_four_ceiling
                    .is_none()
                && receipt.actual_input_tokens.is_none()
                && receipt.actual_output_tokens.is_none()
                && receipt.returned_identity.is_none()
                && receipt.sqz_id.is_none()
        }
        (
            SqzPolicy::PublicOn | SqzPolicy::InternalLocked,
            SqzStatus::Applied | SqzStatus::NotBeneficial,
        ) => receipt.sqz_id.as_ref().is_some_and(|id| {
            id.algorithm == ResultId::sha256(payload.as_bytes()).algorithm()
                && id.value == ResultId::sha256(payload.as_bytes()).value()
        }),
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn incomplete(
    requested: RequestedExecution,
    effective: EffectiveExecution,
    failure: AgentFailure,
    input: Option<SqzReceipt>,
    output: Option<SqzReceipt>,
    input_bytes: usize,
    output_bytes: usize,
    no_session: bool,
) -> Result<DelegationReceipt, AgentRuntimeInternalError> {
    let mut lifecycle = InvocationLifecycle::configured();
    lifecycle
        .incomplete()
        .map_err(|_| AgentRuntimeInternalError::ReceiptInvariant)?;
    receipt(DelegationReceiptParts {
        outcome: InvocationOutcome::new(
            InvocationState::Incomplete,
            Some(failure),
            InvocationMetrics::new(None, None, input_bytes, output_bytes, 0),
        )
        .map_err(|_| AgentRuntimeInternalError::ReceiptInvariant)?,
        requested,
        effective,
        sqz: SqzStages::new(input, output),
        inline_result: None,
        stored_ref: None,
        no_session,
        provenance: vec!["bran-agent-runtime".to_string()],
    })
}

#[allow(clippy::too_many_arguments)]
fn complete(
    lifecycle: &mut InvocationLifecycle,
    requested: RequestedExecution,
    effective: EffectiveExecution,
    input: SqzReceipt,
    output: SqzReceipt,
    inline: InlineResult,
    stored_ref: StoredResultRef,
    actual_input_tokens: Option<usize>,
    actual_output_tokens: Option<usize>,
    input_bytes: usize,
    output_bytes: usize,
    no_session: bool,
    grounding_validated: bool,
    provider_run_id: Attestation<String>,
) -> Result<DelegationReceipt, AgentRuntimeInternalError> {
    lifecycle
        .complete()
        .map_err(|_| AgentRuntimeInternalError::ReceiptInvariant)?;
    let mut provenance = vec!["bran-agent-runtime".to_string()];
    if grounding_validated {
        provenance.push("bran-grounding-validated".to_string());
    }
    receipt(DelegationReceiptParts {
        outcome: InvocationOutcome::new(
            InvocationState::Complete,
            None,
            InvocationMetrics::new(
                actual_input_tokens,
                actual_output_tokens,
                input_bytes,
                output_bytes,
                0,
            ),
        )
        .map_err(|_| AgentRuntimeInternalError::ReceiptInvariant)?,
        requested,
        effective,
        sqz: SqzStages::new(Some(input), Some(output)),
        inline_result: Some(inline),
        stored_ref: Some(stored_ref),
        no_session,
        provenance,
    })?
    .with_provider_run_id(provider_run_id)
    .map_err(|_| AgentRuntimeInternalError::ReceiptInvariant)
}

fn provider_run_id(output: &super::runtime::ProviderOutput) -> Attestation<String> {
    output
        .provider_run_id()
        .map(|value| Attestation::Attested(value.to_string()))
        .unwrap_or(Attestation::Unavailable)
}

fn receipt(parts: DelegationReceiptParts) -> Result<DelegationReceipt, AgentRuntimeInternalError> {
    DelegationReceipt::new(parts).map_err(|_| AgentRuntimeInternalError::ReceiptInvariant)
}

fn store_output<Store: ResultStore>(
    store: &mut Store,
    inline: &InlineResult,
    artifacts: &[super::runtime::LosslessArtifact],
    now_tick: u64,
) -> Option<StoredResultRef> {
    let result_encoded = inline.encode_canonical();
    let mut batch: Vec<&[u8]> = vec![result_encoded.as_slice()];
    for artifact in artifacts {
        batch.push(artifact.bytes());
    }
    let limits = store.limits();
    let total_bytes = batch
        .iter()
        .try_fold(0usize, |total, payload| total.checked_add(payload.len()))?;
    if limits.max_entries == 0
        || limits.max_total_bytes == 0
        || limits.max_item_bytes == 0
        || limits.max_age_ticks == 0
        || limits.max_item_bytes > limits.max_total_bytes
        || batch.len() > limits.max_entries
        || total_bytes > limits.max_total_bytes
        || batch
            .iter()
            .any(|payload| payload.len() > limits.max_item_bytes)
    {
        return None;
    }
    let stored_ids = store.put_batch(&batch, now_tick).ok()?;
    if stored_ids.len() != batch.len()
        || stored_ids.first() != Some(&ResultId::sha256(&result_encoded))
    {
        return None;
    }
    for (stored_id, payload) in stored_ids.iter().zip(batch.iter()) {
        if store.get(stored_id, now_tick).ok().as_deref() != Some(*payload) {
            return None;
        }
    }
    let result_id = stored_ids[0].clone();
    let artifact_ids: Vec<_> = stored_ids.into_iter().skip(1).collect();
    for (stored_id, artifact) in artifact_ids.iter().zip(artifacts.iter()) {
        if *stored_id != *artifact.id() {
            return None;
        }
    }
    StoredResultRef::new(result_id, artifact_ids).ok()
}

#[cfg(test)]
mod tests {
    use super::super::delegate::{
        AdmittedEvidence, DelegationOptions, DelegationRequest, GroundingContract,
    };
    use super::super::result_store::{MemoryResultStore, ResultStoreError};
    use super::super::runtime::{
        AgentFailure, ArtifactKind, Attestation, AuthError, AuthStore, InvocationLifecycle,
        InvocationOutcome, InvocationState, LosslessArtifact, ProviderClaim, ProviderError,
        ProviderExecutionEvidence, ProviderOutput, ProviderPort, ProviderRequest,
        ProviderTokenUsage,
    };
    use super::*;
    use crate::adapters::{
        DlpStatus, FidelityStatus, SqzFailureReason, SqzId, SqzIdentity, SqzPolicy, SqzPort,
        SqzPortError, SqzPortOutput, SqzReceipt, SqzStatus,
    };
    use crate::agent::{
        AgentProfile, AgentProfileRegistry, ModelRegistry, ProviderRegistry, ReasoningLevel,
        ToolPolicy,
    };
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::Duration;

    #[derive(Clone)]
    struct OffWitnessSqzPort {
        calls: Rc<Cell<usize>>,
    }

    impl SqzPort for OffWitnessSqzPort {
        fn compress(&self, input: &str) -> Result<SqzPortOutput, SqzPortError> {
            self.calls.set(self.calls.get() + 1);
            Ok(SqzPortOutput::new(input, SqzIdentity::approved()))
        }
    }

    fn make_sqz_receipt(payload: &str) -> SqzReceipt {
        let id = SqzIdentity::approved();
        let raw_len = payload.len();
        let tok = raw_len / 4 + usize::from(!raw_len.is_multiple_of(4));
        let content_id = ResultId::sha256(payload.as_bytes());
        SqzReceipt {
            schema_version: "1.0.0",
            configured_identity: id.clone(),
            returned_identity: Some(id),
            policy: SqzPolicy::PublicOn,
            status: SqzStatus::Applied,
            failure_reason: None,
            monotonic_call_latency: Duration::ZERO,
            raw_bytes: raw_len,
            candidate_compressed_bytes: None,
            returned_bytes: raw_len,
            raw_token_estimate_bytes_divided_by_four_ceiling: tok,
            candidate_token_estimate_bytes_divided_by_four_ceiling: None,
            returned_token_estimate_bytes_divided_by_four_ceiling: tok,
            actual_input_tokens: None,
            actual_output_tokens: None,
            fidelity_status: FidelityStatus::Passed,
            required_fidelity_anchor_ids: vec![],
            missing_fidelity_anchor_ids: vec![],
            dlp_status: DlpStatus::Passed,
            dlp_findings: vec![],
            requested_max_output_bytes: 65_536,
            effective_max_output_bytes: 65_536,
            sqz_id: Some(SqzId {
                algorithm: content_id.algorithm(),
                value: content_id.value().to_owned(),
            }),
        }
    }

    fn build_registry() -> AgentProfileRegistry {
        let mut pr = ProviderRegistry::new();
        pr.register("fixture-provider").unwrap();
        pr.register("other-provider").unwrap();
        let mut mr = ModelRegistry::new();
        mr.register("fixture-provider", "fixture-sol").unwrap();
        mr.register("fixture-provider", "fixture-luna").unwrap();
        mr.register("other-provider", "other-model").unwrap();
        let mut apr = AgentProfileRegistry::new(pr, mr);
        let sol_policy =
            ToolPolicy::new(["read", "search", "write"], ["edit", "shell", "network"]).unwrap();
        let sol = AgentProfile::new(
            "sol",
            "fixture-provider",
            "fixture-sol",
            "sol-default",
            ReasoningLevel::High,
            sol_policy,
        )
        .unwrap();
        apr.register(sol).unwrap();
        // Restricted profile: denies "search" so default request tp triggers denied_tool
        let restricted =
            ToolPolicy::new(["read"], ["search", "write", "edit", "shell", "network"]).unwrap();
        let luna = AgentProfile::new(
            "luna",
            "fixture-provider",
            "fixture-luna",
            "luna-default",
            ReasoningLevel::Low,
            restricted,
        )
        .unwrap();
        apr.register(luna).unwrap();
        apr
    }

    fn make_request(profile: &str, prompt: &str, opts: DelegationOptions) -> DelegationRequest {
        DelegationRequest::new(profile, prompt, opts).unwrap()
    }

    fn make_trusted_opts() -> DelegationOptions {
        DelegationOptions::new()
    }

    struct FakeAuthStore {
        calls: Cell<usize>,
        fail_handle: Option<String>,
    }

    impl FakeAuthStore {
        fn always_ok() -> Self {
            Self {
                calls: Cell::new(0),
                fail_handle: None,
            }
        }
        fn missing_for(handle: &str) -> Self {
            Self {
                calls: Cell::new(0),
                fail_handle: Some(handle.to_string()),
            }
        }
    }

    impl AuthStore for FakeAuthStore {
        type Credential = String;
        fn resolve(&self, account_handle: &str) -> Result<Self::Credential, AuthError> {
            self.calls.set(self.calls.get() + 1);
            if self.fail_handle.as_deref() == Some(account_handle) {
                Err(AuthError::Missing)
            } else {
                Ok("test-cred".to_string())
            }
        }
    }

    #[derive(Clone)]
    enum ProvOutcome {
        Ok(Box<ProviderOutput>),
        Err(ProviderError),
    }

    struct FakeProviderPort {
        calls: Cell<usize>,
        token_budgets: RefCell<Vec<(Option<usize>, Option<usize>)>>,
        no_sessions: RefCell<Vec<bool>>,
        outcome: ProvOutcome,
        attest_budget: bool,
    }

    impl FakeProviderPort {
        fn success(out: ProviderOutput) -> Self {
            Self {
                calls: Cell::new(0),
                token_budgets: RefCell::new(Vec::new()),
                no_sessions: RefCell::new(Vec::new()),
                outcome: ProvOutcome::Ok(Box::new(out)),
                attest_budget: true,
            }
        }
        fn unattested(out: ProviderOutput) -> Self {
            Self {
                calls: Cell::new(0),
                token_budgets: RefCell::new(Vec::new()),
                no_sessions: RefCell::new(Vec::new()),
                outcome: ProvOutcome::Ok(Box::new(out)),
                attest_budget: false,
            }
        }
        fn fail(err: ProviderError) -> Self {
            Self {
                calls: Cell::new(0),
                token_budgets: RefCell::new(Vec::new()),
                no_sessions: RefCell::new(Vec::new()),
                outcome: ProvOutcome::Err(err),
                attest_budget: false,
            }
        }
    }

    impl ProviderPort<String> for FakeProviderPort {
        fn invoke(
            &self,
            request: &ProviderRequest,
            _credential: &String,
        ) -> Result<ProviderOutput, ProviderError> {
            self.calls.set(self.calls.get() + 1);
            self.token_budgets.borrow_mut().push((
                request.hard_total_token_ceiling(),
                request.max_output_tokens(),
            ));
            self.no_sessions.borrow_mut().push(request.no_session());
            match &self.outcome {
                ProvOutcome::Ok(o) if self.attest_budget => {
                    match request.hard_total_token_ceiling() {
                        Some(ceiling) => (**o)
                            .clone()
                            .with_token_budget_attestation(ceiling)
                            .map_err(|_| ProviderError::InvalidOutput),
                        None => Ok((**o).clone()),
                    }
                }
                ProvOutcome::Ok(o) => Ok((**o).clone()),
                ProvOutcome::Err(e) => Err(*e),
            }
        }
    }

    #[derive(Clone, Copy)]
    enum SqzBehavior {
        Passthrough,
        FailInput,
        FailOutput,
        RewriteOutput,
        DlpOutput,
    }

    struct FakeAgentSqzPort {
        calls: Cell<usize>,
        behavior: SqzBehavior,
    }

    impl FakeAgentSqzPort {
        fn passthrough() -> Self {
            Self {
                calls: Cell::new(0),
                behavior: SqzBehavior::Passthrough,
            }
        }
        fn fail_input() -> Self {
            Self {
                calls: Cell::new(0),
                behavior: SqzBehavior::FailInput,
            }
        }
        fn fail_output() -> Self {
            Self {
                calls: Cell::new(0),
                behavior: SqzBehavior::FailOutput,
            }
        }
        fn rewrite_output() -> Self {
            Self {
                calls: Cell::new(0),
                behavior: SqzBehavior::RewriteOutput,
            }
        }
        fn dlp_output() -> Self {
            Self {
                calls: Cell::new(0),
                behavior: SqzBehavior::DlpOutput,
            }
        }
    }

    impl AgentSqzPort for FakeAgentSqzPort {
        fn evaluate(
            &self,
            stage: SqzStage,
            payload: &str,
            _max_output_bytes: usize,
        ) -> Result<AgentSqzOutput, AgentSqzError> {
            let n = self.calls.get() + 1;
            self.calls.set(n);
            match self.behavior {
                SqzBehavior::FailInput => {
                    return Err(AgentSqzError::new(AgentSqzFailureCode::InputFailed, None));
                }
                SqzBehavior::FailOutput => {
                    if n >= 2 {
                        return Err(AgentSqzError::new(AgentSqzFailureCode::OutputFailed, None));
                    }
                }
                SqzBehavior::Passthrough => {}
                SqzBehavior::RewriteOutput if stage == SqzStage::Output => {
                    let rewritten = "SQZ answer";
                    return AgentSqzOutput::new(rewritten, make_sqz_receipt(rewritten));
                }
                SqzBehavior::DlpOutput if stage == SqzStage::Output => {
                    let canary = "api_key=canary";
                    let mut receipt = make_sqz_receipt(canary);
                    receipt.status = SqzStatus::Failed;
                    receipt.failure_reason = Some(SqzFailureReason::DlpFindings);
                    receipt.dlp_status = DlpStatus::Findings;
                    receipt.dlp_findings = vec!["credential_assignment".to_string()];
                    return AgentSqzOutput::new(canary, receipt);
                }
                SqzBehavior::RewriteOutput | SqzBehavior::DlpOutput => {}
            }
            let receipt = make_sqz_receipt(payload);
            AgentSqzOutput::new(payload.to_owned(), receipt)
        }

        fn evaluate_with_anchors(
            &self,
            stage: SqzStage,
            payload: &str,
            max_output_bytes: usize,
            preservation_anchors: &[PreservationAnchor],
        ) -> Result<AgentSqzOutput, AgentSqzError> {
            if preservation_anchors
                .iter()
                .any(|anchor| !payload.contains(anchor.value()))
            {
                return Err(AgentSqzError::new(AgentSqzFailureCode::InvalidOutput, None));
            }
            let mut output = self.evaluate(stage, payload, max_output_bytes)?;
            output.receipt.required_fidelity_anchor_ids = preservation_anchors
                .iter()
                .map(|anchor| anchor.id().to_owned())
                .collect();
            output.receipt.missing_fidelity_anchor_ids.clear();
            Ok(output)
        }
    }

    fn make_success_provider_output() -> ProviderOutput {
        let answer = "The answer is 42 with citations.";
        let citations = vec!["src1".to_string(), "doc2".to_string()];
        let artifact = LosslessArtifact::new(
            ArtifactKind::Json,
            "application/json",
            b"{\"patch\":\"diff\"}".to_vec(),
        )
        .unwrap();
        let tokens = ProviderTokenUsage {
            actual_input_tokens: Some(123),
            actual_output_tokens: Some(9),
        };
        ProviderOutput::with_effective_execution(
            answer,
            citations,
            Some("prov-run-xyz"),
            ProviderExecutionEvidence::new(
                Some("sol"),
                Some("fixture-provider"),
                Some("fixture-sol"),
                Some("high"),
            )
            .unwrap(),
            tokens,
            vec![artifact],
        )
        .unwrap()
    }

    fn grounding_fixture() -> (PathBuf, String, String) {
        let root = std::env::temp_dir().join(format!(
            "bran-claim-grounding-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let src1 = "pub fn supported_symbol() -> u32 { 42 }\n";
        let doc2 = "The architecture contract preserves grounded evidence.\n";
        fs::write(root.join("src1"), src1).unwrap();
        fs::write(root.join("doc2"), doc2).unwrap();
        (
            root,
            ResultId::sha256(src1.as_bytes()).value().to_owned(),
            ResultId::sha256(doc2.as_bytes()).value().to_owned(),
        )
    }

    fn grounded_provider_output(src1_digest: &str, doc2_digest: &str) -> ProviderOutput {
        let claims = [
            ProviderClaim::new(
                "claim-src1",
                "supported_symbol",
                true,
                "src1",
                src1_digest,
                "supported_symbol",
            )
            .unwrap(),
            ProviderClaim::new(
                "claim-doc2",
                "architecture contract preserves grounded evidence",
                true,
                "doc2",
                doc2_digest,
                "architecture contract preserves grounded evidence",
            )
            .unwrap(),
        ];
        ProviderOutput::with_effective_execution(
            claims
                .iter()
                .map(ProviderClaim::text)
                .collect::<Vec<_>>()
                .join("\n"),
            ["src1", "doc2"],
            Some("prov-run-xyz"),
            ProviderExecutionEvidence::new(
                Some("sol"),
                Some("fixture-provider"),
                Some("fixture-sol"),
                Some("high"),
            )
            .unwrap(),
            ProviderTokenUsage {
                actual_input_tokens: Some(123),
                actual_output_tokens: Some(9),
            },
            Vec::<LosslessArtifact>::new(),
        )
        .unwrap()
        .with_claims(claims)
        .unwrap()
    }

    fn custom_grounded_provider_output(
        citations: &[&str],
        claim: ProviderClaim,
        effective_model: Option<&str>,
    ) -> ProviderOutput {
        let answer = claim.text().to_owned();
        ProviderOutput::with_effective_execution(
            answer,
            citations.iter().copied(),
            Some("prov-run-grounded"),
            ProviderExecutionEvidence::new(
                Some("sol"),
                Some("fixture-provider"),
                effective_model,
                Some("high"),
            )
            .unwrap(),
            ProviderTokenUsage {
                actual_input_tokens: Some(123),
                actual_output_tokens: Some(9),
            },
            Vec::<LosslessArtifact>::new(),
        )
        .unwrap()
        .with_claims([claim])
        .unwrap()
    }

    fn single_citation_provider_output(claim: ProviderClaim) -> ProviderOutput {
        custom_grounded_provider_output(&["src1"], claim, Some("fixture-sol"))
    }

    fn canonical_result(answer: &str, citations: &[String]) -> Vec<u8> {
        InlineResult::new(answer, citations.iter().cloned())
            .unwrap()
            .encode_canonical()
    }

    #[test]
    fn p3_agent_runtime() {
        let registry = build_registry();
        let success_out = make_success_provider_output();
        let success_answer = success_out.answer().to_string();
        let success_citations = success_out.citations().to_vec();
        let rt = AgentRuntime::new(AgentRuntimeConfig::new(true, 2).unwrap());
        assert!(!AgentRuntimeConfig::default().enabled());
        assert_eq!(AgentRuntimeConfig::default().max_delegation_depth(), 8);
        assert!(AgentProfile::new(
            "canary",
            "fixture-provider",
            "fixture-sol",
            "ghp_canary",
            ReasoningLevel::Low,
            ToolPolicy::read_only_default(),
        )
        .is_err());
        let default_request = make_request("sol", "default budget", DelegationOptions::new());
        assert_eq!(default_request.hard_total_token_ceiling(), None);
        assert_eq!(default_request.max_output_tokens(), None);
        let mut lower_options = DelegationOptions::new();
        lower_options.hard_total_token_ceiling = Some(100);
        lower_options.max_output_tokens = Some(99);
        let lower_request = make_request("sol", "lower budget", lower_options);
        assert_eq!(lower_request.hard_total_token_ceiling(), Some(100));
        assert_eq!(lower_request.max_output_tokens(), Some(99));
        let mut higher_options = DelegationOptions::new();
        higher_options.hard_total_token_ceiling = Some(12_345);
        higher_options.max_output_tokens = Some(12_345);
        let higher_request = make_request("sol", "higher budget", higher_options);
        assert_eq!(higher_request.hard_total_token_ceiling(), Some(12_345));
        assert_eq!(higher_request.max_output_tokens(), Some(12_345));
        let mut invalid_options = DelegationOptions::new();
        invalid_options.hard_total_token_ceiling = Some(100);
        invalid_options.max_output_tokens = Some(101);
        assert!(DelegationRequest::new("sol", "invalid budget", invalid_options).is_err());
        let mut zero_total_options = DelegationOptions::new();
        zero_total_options.hard_total_token_ceiling = Some(0);
        assert!(DelegationRequest::new("sol", "zero total", zero_total_options).is_err());
        let mut zero_output_options = DelegationOptions::new();
        zero_output_options.hard_total_token_ceiling = Some(1);
        zero_output_options.max_output_tokens = Some(0);
        assert!(DelegationRequest::new("sol", "zero output", zero_output_options).is_err());

        let off_calls = Rc::new(Cell::new(0));
        let off_adapter = AgentSqzAdapter::new(
            OffWitnessSqzPort {
                calls: Rc::clone(&off_calls),
            },
            SqzPolicy::PublicOff,
            1024,
        );
        let off_input = off_adapter
            .evaluate(SqzStage::Input, "unchanged input", 1024)
            .unwrap();
        let off_output = off_adapter
            .evaluate(SqzStage::Output, "unchanged output", 1024)
            .unwrap();
        assert_eq!(off_input.payload(), "unchanged input");
        assert_eq!(off_output.payload(), "unchanged output");
        assert_eq!(off_input.receipt().policy, SqzPolicy::PublicOff);
        assert_eq!(off_output.receipt().policy, SqzPolicy::PublicOff);
        assert_eq!(off_input.receipt().status, SqzStatus::Off);
        assert_eq!(off_output.receipt().status, SqzStatus::Off);
        assert_eq!(off_input.receipt().fidelity_status, FidelityStatus::Passed);
        assert_eq!(off_output.receipt().fidelity_status, FidelityStatus::Passed);
        assert_eq!(off_input.receipt().dlp_status, DlpStatus::Passed);
        assert_eq!(off_output.receipt().dlp_status, DlpStatus::Passed);
        assert!(off_input.receipt().sqz_id.is_none());
        assert!(off_output.receipt().sqz_id.is_none());
        let unsafe_off_input = off_adapter
            .evaluate(SqzStage::Input, "api_key=canary", 1024)
            .unwrap();
        let unsafe_off_output = off_adapter
            .evaluate(SqzStage::Output, "token=canary", 1024)
            .unwrap();
        assert_eq!(unsafe_off_input.receipt().status, SqzStatus::Failed);
        assert_eq!(unsafe_off_output.receipt().status, SqzStatus::Failed);
        assert_eq!(unsafe_off_input.receipt().dlp_status, DlpStatus::Findings);
        assert_eq!(unsafe_off_output.receipt().dlp_status, DlpStatus::Findings);
        assert!(unsafe_off_input.receipt().sqz_id.is_none());
        assert!(unsafe_off_output.receipt().sqz_id.is_none());
        assert_eq!(off_calls.get(), 0);
        let off_auth = FakeAuthStore::always_ok();
        let off_provider = FakeProviderPort::success(success_out.clone());
        let mut off_store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
        let off_ports = RuntimePorts::new(&off_auth, &off_provider, &off_adapter, &mut off_store);
        let off_receipt = rt
            .invoke(
                &make_request("sol", "lossless off", make_trusted_opts()),
                AgentRuntimeAuthority::new(false, true, false),
                &registry,
                || off_ports,
                999,
            )
            .unwrap();
        assert!(matches!(
            off_receipt.outcome(),
            InvocationOutcome::Complete { .. }
        ));
        assert_eq!(
            off_receipt.sqz_stages().input().unwrap().status,
            SqzStatus::Off
        );
        assert_eq!(
            off_receipt.sqz_stages().output().unwrap().status,
            SqzStatus::Off
        );
        assert!(off_receipt.sqz_stages().input().unwrap().sqz_id.is_none());
        assert!(off_receipt.sqz_stages().output().unwrap().sqz_id.is_none());
        assert!(!off_receipt
            .provenance()
            .contains(&"bran-grounding-validated".to_string()));
        assert_eq!(off_calls.get(), 0);

        let grounded_prompt = "Ground this request in architecture-contract-alpha. The middle deliberately extends beyond the legacy prefix so the final evidence cannot be skipped by a short anchor. Preserve late-evidence-anchor-omega exactly.";
        let grounding_anchors = vec![
            PreservationAnchor::new("architecture", "architecture-contract-alpha").unwrap(),
            PreservationAnchor::new("late-evidence", "late-evidence-anchor-omega").unwrap(),
        ];
        let (grounding_root, src1_digest, doc2_digest) = grounding_fixture();
        let grounding = GroundingContract::with_evidence(
            &grounding_root,
            [
                AdmittedEvidence::new("src1", &src1_digest).unwrap(),
                AdmittedEvidence::new("doc2", &doc2_digest).unwrap(),
            ],
            grounding_anchors.clone(),
        )
        .unwrap();
        let mut grounded_options = make_trusted_opts();
        grounded_options.grounding_contract = Some(grounding);
        let grounded_auth = FakeAuthStore::always_ok();
        let grounded_provider =
            FakeProviderPort::success(grounded_provider_output(&src1_digest, &doc2_digest));
        let mut grounded_store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
        let grounded_ports = RuntimePorts::new(
            &grounded_auth,
            &grounded_provider,
            &off_adapter,
            &mut grounded_store,
        );
        let grounded_receipt = rt
            .invoke(
                &make_request("sol", grounded_prompt, grounded_options),
                AgentRuntimeAuthority::new(false, true, false),
                &registry,
                || grounded_ports,
                1000,
            )
            .unwrap();
        assert!(matches!(
            grounded_receipt.outcome(),
            InvocationOutcome::Complete { .. }
        ));
        let grounded_input = grounded_receipt.sqz_stages().input().unwrap();
        assert_eq!(
            grounded_input.required_fidelity_anchor_ids,
            vec!["architecture".to_string(), "late-evidence".to_string()]
        );
        assert!(grounded_input.missing_fidelity_anchor_ids.is_empty());
        assert!(grounded_receipt
            .provenance()
            .contains(&"bran-grounding-validated".to_string()));
        let grounded_inline = grounded_receipt.inline_result().unwrap();
        assert_eq!(grounded_inline.claims().len(), 2);
        assert!(grounded_inline
            .encode_canonical()
            .starts_with(b"bran-agent-result-v2"));
        assert_eq!(
            InlineResult::decode_canonical(&grounded_inline.encode_canonical()).unwrap(),
            grounded_inline.clone()
        );
        assert_eq!(off_calls.get(), 0);

        let rewritten_grounding = GroundingContract::with_evidence(
            &grounding_root,
            [
                AdmittedEvidence::new("src1", &src1_digest).unwrap(),
                AdmittedEvidence::new("doc2", &doc2_digest).unwrap(),
            ],
            grounding_anchors.clone(),
        )
        .unwrap();
        let mut rewritten_options = make_trusted_opts();
        rewritten_options.grounding_contract = Some(rewritten_grounding);
        let rewritten_auth = FakeAuthStore::always_ok();
        let rewritten_provider =
            FakeProviderPort::success(grounded_provider_output(&src1_digest, &doc2_digest));
        let rewritten_sqz = FakeAgentSqzPort::rewrite_output();
        let mut rewritten_store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
        let rewritten_receipt = rt
            .invoke(
                &make_request("sol", grounded_prompt, rewritten_options),
                AgentRuntimeAuthority::new(false, true, false),
                &registry,
                || {
                    RuntimePorts::new(
                        &rewritten_auth,
                        &rewritten_provider,
                        &rewritten_sqz,
                        &mut rewritten_store,
                    )
                },
                1001,
            )
            .unwrap();
        assert!(
            matches!(
                rewritten_receipt.outcome(),
                InvocationOutcome::Incomplete {
                    failure: AgentFailure::ClaimUnsupported,
                    ..
                }
            ),
            "unexpected outcome: {:?}",
            rewritten_receipt.outcome()
        );
        assert!(rewritten_receipt.inline_result().is_none());
        assert!(rewritten_receipt.stored_result_ref().is_none());

        let missing_claims_grounding = GroundingContract::with_evidence(
            &grounding_root,
            [
                AdmittedEvidence::new("src1", &src1_digest).unwrap(),
                AdmittedEvidence::new("doc2", &doc2_digest).unwrap(),
            ],
            grounding_anchors.clone(),
        )
        .unwrap();
        let mut missing_claims_options = make_trusted_opts();
        missing_claims_options.grounding_contract = Some(missing_claims_grounding);
        let missing_claims_auth = FakeAuthStore::always_ok();
        let missing_claims_provider = FakeProviderPort::success(success_out.clone());
        let mut missing_claims_store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
        let missing_claims_receipt = rt
            .invoke(
                &make_request("sol", grounded_prompt, missing_claims_options),
                AgentRuntimeAuthority::new(false, true, false),
                &registry,
                || {
                    RuntimePorts::new(
                        &missing_claims_auth,
                        &missing_claims_provider,
                        &off_adapter,
                        &mut missing_claims_store,
                    )
                },
                1001,
            )
            .unwrap();
        assert!(matches!(
            missing_claims_receipt.outcome(),
            InvocationOutcome::Incomplete {
                failure: AgentFailure::ClaimUnsupported,
                ..
            }
        ));
        assert!(missing_claims_receipt.inline_result().is_none());

        let rejected_grounding = GroundingContract::with_evidence(
            &grounding_root,
            [AdmittedEvidence::new("src1", &src1_digest).unwrap()],
            grounding_anchors.clone(),
        )
        .unwrap();
        let mut rejected_options = make_trusted_opts();
        rejected_options.grounding_contract = Some(rejected_grounding);
        let rejected_auth = FakeAuthStore::always_ok();
        let rejected_provider =
            FakeProviderPort::success(grounded_provider_output(&src1_digest, &doc2_digest));
        let mut rejected_store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
        let rejected_ports = RuntimePorts::new(
            &rejected_auth,
            &rejected_provider,
            &off_adapter,
            &mut rejected_store,
        );
        let rejected_receipt = rt
            .invoke(
                &make_request("sol", grounded_prompt, rejected_options),
                AgentRuntimeAuthority::new(false, true, false),
                &registry,
                || rejected_ports,
                1002,
            )
            .unwrap();
        assert!(matches!(
            rejected_receipt.outcome(),
            InvocationOutcome::Incomplete {
                failure: AgentFailure::GroundingFailed,
                ..
            }
        ));
        assert!(rejected_receipt.inline_result().is_none());
        assert!(rejected_receipt.stored_result_ref().is_none());

        let invented_grounding = GroundingContract::with_evidence(
            &grounding_root,
            [AdmittedEvidence::new("src1", &src1_digest).unwrap()],
            grounding_anchors.clone(),
        )
        .unwrap();
        let invented_output = single_citation_provider_output(
            ProviderClaim::new(
                "claim-invented",
                "nonexistent_symbol",
                true,
                "src1",
                &src1_digest,
                "nonexistent_symbol",
            )
            .unwrap(),
        );
        let mut invented_options = make_trusted_opts();
        invented_options.grounding_contract = Some(invented_grounding);
        let invented_auth = FakeAuthStore::always_ok();
        let invented_provider = FakeProviderPort::success(invented_output);
        let mut invented_store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
        let invented_receipt = rt
            .invoke(
                &make_request("sol", grounded_prompt, invented_options),
                AgentRuntimeAuthority::new(false, true, false),
                &registry,
                || {
                    RuntimePorts::new(
                        &invented_auth,
                        &invented_provider,
                        &off_adapter,
                        &mut invented_store,
                    )
                },
                1003,
            )
            .unwrap();
        assert!(matches!(
            invented_receipt.outcome(),
            InvocationOutcome::Incomplete {
                failure: AgentFailure::ClaimUnsupported,
                ..
            }
        ));
        assert!(invented_receipt.inline_result().is_none());
        assert!(invented_receipt.stored_result_ref().is_none());

        let mismatch_grounding = GroundingContract::with_evidence(
            &grounding_root,
            [AdmittedEvidence::new("src1", &src1_digest).unwrap()],
            grounding_anchors.clone(),
        )
        .unwrap();
        let mismatch_output = single_citation_provider_output(
            ProviderClaim::new(
                "claim-mismatch",
                "unsupported summary",
                true,
                "src1",
                &src1_digest,
                "supported_symbol",
            )
            .unwrap(),
        );
        let mut mismatch_options = make_trusted_opts();
        mismatch_options.grounding_contract = Some(mismatch_grounding);
        let mismatch_auth = FakeAuthStore::always_ok();
        let mismatch_provider = FakeProviderPort::success(mismatch_output);
        let mut mismatch_store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
        let mismatch_receipt = rt
            .invoke(
                &make_request("sol", grounded_prompt, mismatch_options),
                AgentRuntimeAuthority::new(false, true, false),
                &registry,
                || {
                    RuntimePorts::new(
                        &mismatch_auth,
                        &mismatch_provider,
                        &off_adapter,
                        &mut mismatch_store,
                    )
                },
                1004,
            )
            .unwrap();
        assert!(matches!(
            mismatch_receipt.outcome(),
            InvocationOutcome::Incomplete {
                failure: AgentFailure::ClaimUnsupported,
                ..
            }
        ));

        let wrong_digest_grounding = GroundingContract::with_evidence(
            &grounding_root,
            [AdmittedEvidence::new("src1", &src1_digest).unwrap()],
            grounding_anchors.clone(),
        )
        .unwrap();
        let wrong_digest_output = single_citation_provider_output(
            ProviderClaim::new(
                "claim-wrong-digest",
                "supported_symbol",
                true,
                "src1",
                "b".repeat(64),
                "supported_symbol",
            )
            .unwrap(),
        );
        let mut wrong_digest_options = make_trusted_opts();
        wrong_digest_options.grounding_contract = Some(wrong_digest_grounding);
        let wrong_digest_auth = FakeAuthStore::always_ok();
        let wrong_digest_provider = FakeProviderPort::success(wrong_digest_output);
        let mut wrong_digest_store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
        let wrong_digest_receipt = rt
            .invoke(
                &make_request("sol", grounded_prompt, wrong_digest_options),
                AgentRuntimeAuthority::new(false, true, false),
                &registry,
                || {
                    RuntimePorts::new(
                        &wrong_digest_auth,
                        &wrong_digest_provider,
                        &off_adapter,
                        &mut wrong_digest_store,
                    )
                },
                1005,
            )
            .unwrap();
        assert!(matches!(
            wrong_digest_receipt.outcome(),
            InvocationOutcome::Incomplete {
                failure: AgentFailure::ClaimUnsupported,
                ..
            }
        ));

        let uncited_grounding = GroundingContract::with_evidence(
            &grounding_root,
            [
                AdmittedEvidence::new("src1", &src1_digest).unwrap(),
                AdmittedEvidence::new("doc2", &doc2_digest).unwrap(),
            ],
            grounding_anchors.clone(),
        )
        .unwrap();
        let uncited_output = custom_grounded_provider_output(
            &["doc2"],
            ProviderClaim::new(
                "claim-uncited",
                "supported_symbol",
                true,
                "src1",
                &src1_digest,
                "supported_symbol",
            )
            .unwrap(),
            Some("fixture-sol"),
        );
        let mut uncited_options = make_trusted_opts();
        uncited_options.grounding_contract = Some(uncited_grounding);
        let uncited_auth = FakeAuthStore::always_ok();
        let uncited_provider = FakeProviderPort::success(uncited_output);
        let mut uncited_store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
        let uncited_receipt = rt
            .invoke(
                &make_request("sol", grounded_prompt, uncited_options),
                AgentRuntimeAuthority::new(false, true, false),
                &registry,
                || {
                    RuntimePorts::new(
                        &uncited_auth,
                        &uncited_provider,
                        &off_adapter,
                        &mut uncited_store,
                    )
                },
                1006,
            )
            .unwrap();
        assert!(matches!(
            uncited_receipt.outcome(),
            InvocationOutcome::Incomplete {
                failure: AgentFailure::ClaimUnsupported,
                ..
            }
        ));

        let nonmaterial_grounding = GroundingContract::with_evidence(
            &grounding_root,
            [AdmittedEvidence::new("src1", &src1_digest).unwrap()],
            grounding_anchors.clone(),
        )
        .unwrap();
        let nonmaterial_output = single_citation_provider_output(
            ProviderClaim::new(
                "claim-nonmaterial",
                "supported_symbol",
                false,
                "src1",
                &src1_digest,
                "supported_symbol",
            )
            .unwrap(),
        );
        let mut nonmaterial_options = make_trusted_opts();
        nonmaterial_options.grounding_contract = Some(nonmaterial_grounding);
        let nonmaterial_auth = FakeAuthStore::always_ok();
        let nonmaterial_provider = FakeProviderPort::success(nonmaterial_output);
        let mut nonmaterial_store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
        let nonmaterial_receipt = rt
            .invoke(
                &make_request("sol", grounded_prompt, nonmaterial_options),
                AgentRuntimeAuthority::new(false, true, false),
                &registry,
                || {
                    RuntimePorts::new(
                        &nonmaterial_auth,
                        &nonmaterial_provider,
                        &off_adapter,
                        &mut nonmaterial_store,
                    )
                },
                1007,
            )
            .unwrap();
        assert!(matches!(
            nonmaterial_receipt.outcome(),
            InvocationOutcome::Incomplete {
                failure: AgentFailure::ClaimUnsupported,
                ..
            }
        ));

        let unattested_grounding = GroundingContract::with_evidence(
            &grounding_root,
            [AdmittedEvidence::new("src1", &src1_digest).unwrap()],
            grounding_anchors.clone(),
        )
        .unwrap();
        let unattested_output = custom_grounded_provider_output(
            &["src1"],
            ProviderClaim::new(
                "claim-unattested",
                "supported_symbol",
                true,
                "src1",
                &src1_digest,
                "supported_symbol",
            )
            .unwrap(),
            None,
        );
        let mut unattested_options = make_trusted_opts();
        unattested_options.grounding_contract = Some(unattested_grounding);
        let unattested_auth = FakeAuthStore::always_ok();
        let unattested_provider = FakeProviderPort::success(unattested_output);
        let mut unattested_store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
        let unattested_receipt = rt
            .invoke(
                &make_request("sol", grounded_prompt, unattested_options),
                AgentRuntimeAuthority::new(false, true, false),
                &registry,
                || {
                    RuntimePorts::new(
                        &unattested_auth,
                        &unattested_provider,
                        &off_adapter,
                        &mut unattested_store,
                    )
                },
                1008,
            )
            .unwrap();
        assert!(matches!(
            unattested_receipt.outcome(),
            InvocationOutcome::Incomplete {
                failure: AgentFailure::InvalidOutput,
                ..
            }
        ));
        assert!(!unattested_receipt
            .provenance()
            .contains(&"bran-grounding-validated".to_string()));

        let stale_grounding = GroundingContract::with_evidence(
            &grounding_root,
            [AdmittedEvidence::new("src1", &src1_digest).unwrap()],
            grounding_anchors.clone(),
        )
        .unwrap();
        fs::write(
            grounding_root.join("src1"),
            "pub fn replacement_symbol() -> u32 { 7 }\n",
        )
        .unwrap();
        let stale_output = single_citation_provider_output(
            ProviderClaim::new(
                "claim-stale",
                "supported_symbol",
                true,
                "src1",
                &src1_digest,
                "supported_symbol",
            )
            .unwrap(),
        );
        let mut stale_options = make_trusted_opts();
        stale_options.grounding_contract = Some(stale_grounding);
        let stale_auth = FakeAuthStore::always_ok();
        let stale_provider = FakeProviderPort::success(stale_output);
        let mut stale_store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
        let stale_receipt = rt
            .invoke(
                &make_request("sol", grounded_prompt, stale_options),
                AgentRuntimeAuthority::new(false, true, false),
                &registry,
                || RuntimePorts::new(&stale_auth, &stale_provider, &off_adapter, &mut stale_store),
                1009,
            )
            .unwrap();
        assert!(matches!(
            stale_receipt.outcome(),
            InvocationOutcome::Incomplete {
                failure: AgentFailure::ClaimUnsupported,
                ..
            }
        ));
        assert!(stale_receipt.inline_result().is_none());
        assert!(stale_receipt.stored_result_ref().is_none());
        assert!(AdmittedEvidence::new("../src1", &src1_digest).is_err());
        assert!(AdmittedEvidence::new("/src1", &src1_digest).is_err());
        // Degenerate support: "u32" genuinely occurs in the fixture, so
        // substring existence alone would accept it and prove nothing. The
        // parse-time invariant and the boundary check must each reject it on
        // their own. Uses a dedicated file because src1 was just rewritten to
        // make the stale case stale.
        let floor_body = "pub fn supported_symbol() -> u32 { 42 }\n";
        let floor_digest = ResultId::sha256(floor_body.as_bytes()).value().to_owned();
        fs::write(grounding_root.join("floor-src"), floor_body).unwrap();
        let floor_contract = GroundingContract::with_evidence(
            &grounding_root,
            [AdmittedEvidence::new("floor-src", &floor_digest).unwrap()],
            grounding_anchors.clone(),
        )
        .unwrap();
        assert!(ProviderClaim::new(
            "claim-degenerate",
            "u32",
            true,
            "floor-src",
            &floor_digest,
            "u32",
        )
        .is_err());
        assert!(!floor_contract.verifies_support([("floor-src", floor_digest.as_str(), "u32")]));
        assert!(!floor_contract.verifies_support([(
            "floor-src",
            floor_digest.as_str(),
            "   u32   "
        )]));
        // A present span exactly at the floor still verifies, so the floor
        // rejects degenerate spans without rejecting legitimate short symbols.
        assert!(floor_contract.verifies_support([(
            "floor-src",
            floor_digest.as_str(),
            "supported_sy"
        )]));
        assert!(ProviderClaim::new(
            "claim-at-floor",
            "supported_sy",
            true,
            "floor-src",
            &floor_digest,
            "supported_sy",
        )
        .is_ok());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("src1", grounding_root.join("linked-src1")).unwrap();
            let replacement = "pub fn replacement_symbol() -> u32 { 7 }\n";
            let replacement_digest = ResultId::sha256(replacement.as_bytes()).value().to_owned();
            let linked_contract = GroundingContract::with_evidence(
                &grounding_root,
                [AdmittedEvidence::new("linked-src1", &replacement_digest).unwrap()],
                grounding_anchors.clone(),
            )
            .unwrap();
            assert!(!linked_contract.verifies_support([(
                "linked-src1",
                replacement_digest.as_str(),
                "replacement_symbol",
            )]));
        }
        fs::remove_dir_all(&grounding_root).unwrap();
        let empty_grounding = GroundingContract::new(["src1"], grounding_anchors.clone()).unwrap();
        let mut empty_options = make_trusted_opts();
        empty_options.grounding_contract = Some(empty_grounding);
        let empty_request = make_request("sol", grounded_prompt, empty_options);
        assert!(!super::grounded_citations_accepted(&empty_request, &[]));
        assert!(super::grounded_citations_accepted(&default_request, &[]));
        assert!(GroundingContract::new(["token=canary"], grounding_anchors).is_err());
        assert!(!crate::adapters::is_public_dlp_safe("token=canary"));

        // --- disabled (runtime disabled): zero calls, incomplete, no success fields, flag passthrough
        {
            let disabled_rt = AgentRuntime::new(AgentRuntimeConfig::new(false, 2).unwrap());
            let auth = FakeAuthStore::always_ok();
            let prov = FakeProviderPort::success(success_out.clone());
            let sqz = FakeAgentSqzPort::passthrough();
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let factory_calls = Cell::new(0);
            let mut opts = DelegationOptions::new();
            opts.no_session = true;
            let req = make_request("sol", "disabled test", opts);
            let rec = disabled_rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || {
                        factory_calls.set(factory_calls.get() + 1);
                        RuntimePorts::new(&auth, &prov, &sqz, &mut store)
                    },
                    1000,
                )
                .unwrap();
            assert_eq!(factory_calls.get(), 0);
            assert_eq!(auth.calls.get(), 0);
            assert_eq!(prov.calls.get(), 0);
            assert_eq!(sqz.calls.get(), 0);
            assert_eq!(store.receipt().entry_count, 0);
            assert_eq!(store.receipt().total_bytes, 0);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::AgentDisabled);
                }
                _ => panic!("expected incomplete disabled"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
            assert!(rec.no_session());
            assert!(rec.sqz_stages().input().is_none());
            assert!(rec.sqz_stages().output().is_none());
            assert_eq!(rec.requested().provider(), &Attestation::Unavailable);
            assert_eq!(rec.requested().model(), &Attestation::Unavailable);
            assert_eq!(rec.requested().reasoning(), &Attestation::Unavailable);
            assert_eq!(rec.effective().profile(), &Attestation::Unavailable);
            assert_eq!(rec.provider_run_id(), &Attestation::Unavailable);
        }

        // --- offline: treated as disabled, zero calls before any store/port/auth/sqz
        {
            let auth = FakeAuthStore::always_ok();
            let prov = FakeProviderPort::success(success_out.clone());
            let sqz = FakeAgentSqzPort::passthrough();
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let factory_calls = Cell::new(0);
            let opts = DelegationOptions::new();
            let req = make_request("sol", "offline test", opts);
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(true, true, false),
                    &registry,
                    || {
                        factory_calls.set(factory_calls.get() + 1);
                        RuntimePorts::new(&auth, &prov, &sqz, &mut store)
                    },
                    1001,
                )
                .unwrap();
            assert_eq!(factory_calls.get(), 0);
            assert_eq!(auth.calls.get(), 0);
            assert_eq!(prov.calls.get(), 0);
            assert_eq!(sqz.calls.get(), 0);
            assert_eq!(store.receipt().entry_count, 0);
            assert_eq!(store.receipt().total_bytes, 0);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::ExplicitOffline);
                }
                _ => panic!("expected offline disabled"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        // --- !project_trusted: also disabled early
        {
            let auth = FakeAuthStore::always_ok();
            let prov = FakeProviderPort::success(success_out.clone());
            let sqz = FakeAgentSqzPort::passthrough();
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let factory_calls = Cell::new(0);
            let opts = DelegationOptions::new();
            let req = make_request("sol", "untrusted", opts);
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, false, false),
                    &registry,
                    || {
                        factory_calls.set(factory_calls.get() + 1);
                        RuntimePorts::new(&auth, &prov, &sqz, &mut store)
                    },
                    1002,
                )
                .unwrap();
            assert_eq!(factory_calls.get(), 0);
            assert_eq!(auth.calls.get(), 0);
            assert_eq!(prov.calls.get(), 0);
            assert_eq!(sqz.calls.get(), 0);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::ProjectUntrusted);
                }
                _ => panic!("expected untrusted disabled"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        // --- depth exceeded: early, zero calls
        {
            let depth_rt = AgentRuntime::new(AgentRuntimeConfig::new(true, 1).unwrap());
            let auth = FakeAuthStore::always_ok();
            let prov = FakeProviderPort::success(success_out.clone());
            let sqz = FakeAgentSqzPort::passthrough();
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let mut opts = make_trusted_opts();
            opts.delegation_depth = 5;
            let req = make_request("sol", "deep prompt", opts);
            let rec = depth_rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1003,
                )
                .unwrap();
            assert_eq!(auth.calls.get(), 0);
            assert_eq!(prov.calls.get(), 0);
            assert_eq!(sqz.calls.get(), 0);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::DepthExceeded);
                }
                _ => panic!("expected depth"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        // --- unknown profile: early, typed, no success result
        {
            let auth = FakeAuthStore::always_ok();
            let prov = FakeProviderPort::success(success_out.clone());
            let sqz = FakeAgentSqzPort::passthrough();
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let req = make_request("no-such-prof", "prompt", make_trusted_opts());
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1004,
                )
                .unwrap();
            assert_eq!(auth.calls.get(), 0);
            assert_eq!(prov.calls.get(), 0);
            assert_eq!(sqz.calls.get(), 0);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::UnknownProfile);
                }
                _ => panic!("expected unknown profile"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        // --- unknown provider (via override): after profile, before auth/sqz
        {
            let auth = FakeAuthStore::always_ok();
            let prov = FakeProviderPort::success(success_out.clone());
            let sqz = FakeAgentSqzPort::passthrough();
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let mut opts = make_trusted_opts();
            opts.provider_override = Some("no-such-prov".to_string());
            let req = make_request("sol", "prov-override", opts);
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1005,
                )
                .unwrap();
            assert_eq!(auth.calls.get(), 0);
            assert_eq!(prov.calls.get(), 0);
            assert_eq!(sqz.calls.get(), 0);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::UnknownProvider);
                }
                _ => panic!("expected unknown provider"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        // --- unknown model (via override)
        {
            let auth = FakeAuthStore::always_ok();
            let prov = FakeProviderPort::success(success_out.clone());
            let sqz = FakeAgentSqzPort::passthrough();
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let mut opts = make_trusted_opts();
            opts.model_override = Some("no-such-model".to_string());
            let req = make_request("sol", "model-override", opts);
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1006,
                )
                .unwrap();
            assert_eq!(auth.calls.get(), 0);
            assert_eq!(prov.calls.get(), 0);
            assert_eq!(sqz.calls.get(), 0);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::UnknownModel);
                }
                _ => panic!("expected unknown model"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        // --- model registered only under another provider: rejected before auth/SQZ/provider
        {
            let auth = FakeAuthStore::always_ok();
            let prov = FakeProviderPort::success(success_out.clone());
            let sqz = FakeAgentSqzPort::passthrough();
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let mut opts = make_trusted_opts();
            opts.model_override = Some("other-model".to_string());
            let req = make_request("sol", "mismatched-model-override", opts);
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1006,
                )
                .unwrap();
            assert_eq!(auth.calls.get(), 0);
            assert_eq!(prov.calls.get(), 0);
            assert_eq!(sqz.calls.get(), 0);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::UnknownModel);
                }
                _ => panic!("expected mismatched model"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        // --- denied tool: custom valid name outside the six proves request.allowed subset of profile.allowed
        {
            let auth = FakeAuthStore::always_ok();
            let prov = FakeProviderPort::success(success_out.clone());
            let sqz = FakeAgentSqzPort::passthrough();
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let mut opts = make_trusted_opts();
            opts.tool_policy = ToolPolicy::new(
                ["read", "search", "custom_tool"],
                ["edit", "shell", "network"],
            )
            .unwrap();
            let request = make_request("sol", "custom x", opts);
            let receipt = rt
                .invoke(
                    &request,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1007,
                )
                .unwrap();
            match receipt.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::DeniedTool);
                }
                _ => panic!("expected denied tool"),
            }
            assert_eq!(auth.calls.get(), 0);
            assert_eq!(prov.calls.get(), 0);
            assert_eq!(sqz.calls.get(), 0);
            assert!(receipt.inline_result().is_none());
            assert!(receipt.stored_result_ref().is_none());
        }

        // --- host policy denies requested mutation before ports exist
        {
            let auth = FakeAuthStore::always_ok();
            let prov = FakeProviderPort::success(success_out.clone());
            let sqz = FakeAgentSqzPort::passthrough();
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let factory_calls = Cell::new(0);
            let mut opts = make_trusted_opts();
            opts.tool_policy =
                ToolPolicy::new(["read", "write"], ["edit", "shell", "network"]).unwrap();
            let req = make_request("sol", "write denied", opts);
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || {
                        factory_calls.set(factory_calls.get() + 1);
                        RuntimePorts::new(&auth, &prov, &sqz, &mut store)
                    },
                    1008,
                )
                .unwrap();
            assert_eq!(factory_calls.get(), 0);
            assert_eq!(auth.calls.get(), 0);
            assert_eq!(prov.calls.get(), 0);
            assert_eq!(sqz.calls.get(), 0);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::DeniedTool);
                }
                _ => panic!("expected host mutation denial"),
            }
        }

        // --- missing auth: auth called, sqz/provider not, typed failure, no success result
        {
            let auth = FakeAuthStore::missing_for("sol-default");
            let prov = FakeProviderPort::success(success_out.clone());
            let sqz = FakeAgentSqzPort::passthrough();
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let req = make_request("sol", "needs auth", make_trusted_opts());
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1008,
                )
                .unwrap();
            assert_eq!(auth.calls.get(), 1);
            assert_eq!(prov.calls.get(), 0);
            assert_eq!(sqz.calls.get(), 0);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::MissingAuth);
                }
                _ => panic!("expected missing auth"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        // --- sqz input failed: auth yes, sqz once (input), no provider, typed
        {
            let auth = FakeAuthStore::always_ok();
            let prov = FakeProviderPort::success(success_out.clone());
            let sqz = FakeAgentSqzPort::fail_input();
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let req = make_request("sol", "sqz will fail in", make_trusted_opts());
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1009,
                )
                .unwrap();
            assert_eq!(auth.calls.get(), 1);
            assert_eq!(sqz.calls.get(), 1);
            assert_eq!(prov.calls.get(), 0);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::SqzInputFailed);
                }
                _ => panic!("expected sqz input failed"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        // --- provider timeout: sqz input once, provider once, no output sqz, no success result
        {
            let auth = FakeAuthStore::always_ok();
            let sqz = FakeAgentSqzPort::passthrough();
            let prov = FakeProviderPort::fail(ProviderError::Timeout);
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let req = make_request("sol", "timeout", make_trusted_opts());
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1010,
                )
                .unwrap();
            assert_eq!(auth.calls.get(), 1);
            assert_eq!(sqz.calls.get(), 1);
            assert_eq!(prov.calls.get(), 1);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::Timeout);
                }
                _ => panic!("expected timeout"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
            // output sqz not reached
            // (input sqz receipt present)
            assert!(rec.sqz_stages().input().is_some());
            assert!(rec.sqz_stages().output().is_none());
        }

        // --- provider cancelled
        {
            let auth = FakeAuthStore::always_ok();
            let sqz = FakeAgentSqzPort::passthrough();
            let prov = FakeProviderPort::fail(ProviderError::Cancelled);
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let req = make_request("sol", "cancel", make_trusted_opts());
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1011,
                )
                .unwrap();
            assert_eq!(auth.calls.get(), 1);
            assert_eq!(sqz.calls.get(), 1);
            assert_eq!(prov.calls.get(), 1);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::Cancelled);
                }
                _ => panic!("expected cancelled"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        // --- provider failure is distinct from an unavailable provider
        {
            let auth = FakeAuthStore::always_ok();
            let sqz = FakeAgentSqzPort::passthrough();
            let prov = FakeProviderPort::fail(ProviderError::Failed);
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let rec = rt
                .invoke(
                    &make_request("sol", "failed", make_trusted_opts()),
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1011,
                )
                .unwrap();
            assert!(matches!(
                rec.outcome(),
                InvocationOutcome::Incomplete {
                    failure: AgentFailure::ProviderFailed,
                    ..
                }
            ));
        }

        // --- completion requires an exact provider enforcement attestation
        {
            let auth = FakeAuthStore::always_ok();
            let sqz = FakeAgentSqzPort::passthrough();
            let prov = FakeProviderPort::unattested(success_out.clone());
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let mut options = make_trusted_opts();
            options.hard_total_token_ceiling = Some(12_345);
            let rec = rt
                .invoke(
                    &make_request("sol", "unattested budget", options),
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1011,
                )
                .unwrap();
            assert!(matches!(
                rec.outcome(),
                InvocationOutcome::Incomplete {
                    failure: AgentFailure::TokenBudgetUnattested,
                    ..
                }
            ));
            assert_eq!(sqz.calls.get(), 1);
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        // --- attested actual usage cannot exceed the configured hard total
        {
            let over_output = ProviderOutput::with_effective_execution(
                "bounded answer",
                vec!["safe-citation"],
                Some("bounded-run"),
                ProviderExecutionEvidence::new(
                    Some("sol"),
                    Some("fixture-provider"),
                    Some("fixture-sol"),
                    Some("high"),
                )
                .unwrap(),
                ProviderTokenUsage {
                    actual_input_tokens: Some(8),
                    actual_output_tokens: Some(7),
                },
                Vec::<LosslessArtifact>::new(),
            )
            .unwrap();
            let auth = FakeAuthStore::always_ok();
            let sqz = FakeAgentSqzPort::passthrough();
            let prov = FakeProviderPort::success(over_output);
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let mut options = make_trusted_opts();
            options.hard_total_token_ceiling = Some(10);
            options.max_output_tokens = Some(10);
            let rec = rt
                .invoke(
                    &make_request("sol", "over budget", options),
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1011,
                )
                .unwrap();
            assert!(matches!(
                rec.outcome(),
                InvocationOutcome::Incomplete {
                    failure: AgentFailure::TokenCeilingExceeded,
                    ..
                }
            ));
            assert_eq!(sqz.calls.get(), 1);
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        // --- every provider surface is DLP-checked before output or storage
        {
            let unsafe_citation = ProviderOutput::with_effective_execution(
                "safe answer",
                vec!["token=canary"],
                Some("safe-run"),
                ProviderExecutionEvidence::new(
                    Some("sol"),
                    Some("fixture-provider"),
                    Some("fixture-sol"),
                    Some("high"),
                )
                .unwrap(),
                ProviderTokenUsage {
                    actual_input_tokens: None,
                    actual_output_tokens: None,
                },
                Vec::<LosslessArtifact>::new(),
            )
            .unwrap();
            let auth = FakeAuthStore::always_ok();
            let sqz = FakeAgentSqzPort::passthrough();
            let prov = FakeProviderPort::success(unsafe_citation);
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let rec = rt
                .invoke(
                    &make_request("sol", "citation dlp", make_trusted_opts()),
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1011,
                )
                .unwrap();
            assert!(matches!(
                rec.outcome(),
                InvocationOutcome::Incomplete {
                    failure: AgentFailure::DlpRejected,
                    ..
                }
            ));
            assert_eq!(sqz.calls.get(), 1);
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        {
            let unsafe_artifact = LosslessArtifact::new(
                ArtifactKind::Opaque,
                "application/octet-stream",
                b"password=canary".to_vec(),
            )
            .unwrap();
            let unsafe_output = ProviderOutput::with_effective_execution(
                "safe answer",
                vec!["safe-citation"],
                Some("safe-run"),
                ProviderExecutionEvidence::new(
                    Some("sol"),
                    Some("fixture-provider"),
                    Some("fixture-sol"),
                    Some("high"),
                )
                .unwrap(),
                ProviderTokenUsage {
                    actual_input_tokens: None,
                    actual_output_tokens: None,
                },
                vec![unsafe_artifact],
            )
            .unwrap();
            let auth = FakeAuthStore::always_ok();
            let sqz = FakeAgentSqzPort::passthrough();
            let prov = FakeProviderPort::success(unsafe_output);
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let rec = rt
                .invoke(
                    &make_request("sol", "artifact dlp", make_trusted_opts()),
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1011,
                )
                .unwrap();
            assert!(matches!(
                rec.outcome(),
                InvocationOutcome::Incomplete {
                    failure: AgentFailure::DlpRejected,
                    ..
                }
            ));
            assert_eq!(sqz.calls.get(), 1);
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        // --- sqz output failed (after input + provider): has input sqz, failure typed, no inline/stored
        {
            let auth = FakeAuthStore::always_ok();
            let sqz = FakeAgentSqzPort::fail_output();
            let prov = FakeProviderPort::success(success_out.clone());
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let req = make_request("sol", "sqz out fail", make_trusted_opts());
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1012,
                )
                .unwrap();
            assert_eq!(auth.calls.get(), 1);
            assert_eq!(sqz.calls.get(), 2);
            assert_eq!(prov.calls.get(), 1);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::SqzOutputFailed);
                }
                _ => panic!("expected sqz output failed"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
        }

        // --- result store failed: sqz x2 + prov, but store rejects, no inline/stored success
        {
            // small max_item_bytes so encode of result exceeds
            let auth = FakeAuthStore::always_ok();
            let sqz = FakeAgentSqzPort::passthrough();
            let prov = FakeProviderPort::success(success_out.clone());
            let mut store = MemoryResultStore::new(8, 10000, 25, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let req = make_request("sol", "store will fail", make_trusted_opts());
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    1013,
                )
                .unwrap();
            assert_eq!(auth.calls.get(), 1);
            assert_eq!(sqz.calls.get(), 2);
            assert_eq!(prov.calls.get(), 1);
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::ResultStoreFailed);
                }
                _ => panic!("expected result store failed"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
            // sqz stages present (input and the output one)
            assert!(rec.sqz_stages().input().is_some());
            assert!(rec.sqz_stages().output().is_some());
        }

        // --- success path: sqz x2, tokens, inline exact, sha refs, byte-exact readback, requested/effective
        {
            let auth = FakeAuthStore::always_ok();
            let sqz = FakeAgentSqzPort::passthrough();
            let prov = FakeProviderPort::success(success_out.clone());
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let mut options = make_trusted_opts();
            options.hard_total_token_ceiling = Some(12_345);
            options.max_output_tokens = Some(100);
            let req = make_request("sol", "success prompt", options);
            let now = 5000u64;
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    now,
                )
                .unwrap();
            assert_eq!(auth.calls.get(), 1);
            assert_eq!(sqz.calls.get(), 2);
            assert_eq!(prov.calls.get(), 1);
            assert_eq!(
                prov.token_budgets.borrow().as_slice(),
                &[(Some(12_345), Some(100))]
            );
            match rec.outcome() {
                InvocationOutcome::Complete { metrics } => {
                    assert_eq!(metrics.actual_input_tokens(), Some(123));
                    assert_eq!(metrics.actual_output_tokens(), Some(9));
                }
                _ => panic!("expected complete"),
            }
            assert!(rec.sqz_stages().input().is_some());
            assert!(rec.sqz_stages().output().is_some());
            let input_sqz = rec.sqz_stages().input().unwrap();
            assert_eq!(input_sqz.sqz_id.as_ref().unwrap().algorithm, "sha256");
            assert_eq!(
                input_sqz.sqz_id.as_ref().unwrap().value,
                ResultId::sha256(b"success prompt").value()
            );
            let output_sqz = rec.sqz_stages().output().unwrap();
            assert_eq!(output_sqz.sqz_id.as_ref().unwrap().algorithm, "sha256");
            assert_eq!(
                output_sqz.sqz_id.as_ref().unwrap().value,
                ResultId::sha256(success_answer.as_bytes()).value()
            );
            let receipt_json = rec.to_json();
            assert!(receipt_json.contains("\"monotonic_call_latency_ns\":0"));
            assert!(receipt_json.contains("\"policy\":\"public-on\""));
            assert!(receipt_json.contains("\"status\":\"applied\""));
            assert!(receipt_json.contains("\"fidelity_status\":\"passed\""));
            assert!(receipt_json.contains("\"dlp_status\":\"passed\""));
            let input_sha256 = input_sqz.sqz_id.as_ref().unwrap().value.as_str();
            assert_eq!(input_sha256.len(), 64);
            assert!(input_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
            assert!(receipt_json.contains(&format!("\"value\":\"{}\"", input_sha256)));
            let inline = rec.inline_result().expect("must have inline");
            assert_eq!(inline.answer(), "The answer is 42 with citations.");
            assert_eq!(
                inline.citations(),
                &["src1".to_string(), "doc2".to_string()]
            );
            let sref = rec.stored_result_ref().expect("must have stored ref");
            let expected_result = canonical_result(&success_answer, &success_citations);
            assert_eq!(
                sref.result_id(),
                &super::super::result_store::ResultId::sha256(&expected_result)
            );
            assert_eq!(sref.artifact_ids().len(), 1);
            assert_eq!(&sref.artifact_ids()[0], success_out.artifacts()[0].id());
            // byte-exact store readback
            let result_back = store.get(sref.result_id(), now).expect("result readback");
            assert_eq!(result_back, expected_result);
            let decoded = InlineResult::decode_canonical(&result_back).unwrap();
            assert_eq!(decoded, *inline);
            assert_eq!(
                ResultId::parse(&sref.result_id().to_string()).unwrap(),
                *sref.result_id()
            );
            assert!(ResultId::parse(sref.result_id().value()).is_err());
            let mut trailing = result_back.clone();
            trailing.push(0);
            assert!(InlineResult::decode_canonical(&trailing).is_err());
            let art_back = store
                .get(&sref.artifact_ids()[0], now)
                .expect("artifact readback");
            assert_eq!(art_back, b"{\"patch\":\"diff\"}");
            // requested/effective evidence
            assert_eq!(rec.requested().profile_name(), "sol");
            assert_eq!(
                rec.requested().provider(),
                &Attestation::Attested("fixture-provider".to_string())
            );
            assert_eq!(
                rec.requested().model(),
                &Attestation::Attested("fixture-sol".to_string())
            );
            match rec.effective().profile() {
                Attestation::Attested(p) => assert_eq!(p.name(), "sol"),
                _ => panic!("expected attested profile"),
            }
            assert_eq!(
                rec.provider_run_id(),
                &Attestation::Attested("prov-run-xyz".to_string())
            );
            assert_eq!(
                rec.effective().provider(),
                &Attestation::Attested("fixture-provider".to_string())
            );
            assert_eq!(
                rec.effective().model(),
                &Attestation::Attested("fixture-sol".to_string())
            );
            assert_eq!(
                rec.effective().reasoning(),
                &Attestation::Attested(ReasoningLevel::High)
            );
            assert!(!rec.no_session());
        }

        // multi-artifact constrained-capacity: exact readback of every returned ID
        {
            let first_artifact = LosslessArtifact::new(
                ArtifactKind::Json,
                "application/json",
                b"{\"1\":true}".to_vec(),
            )
            .unwrap();
            let second_artifact = LosslessArtifact::new(
                ArtifactKind::Json,
                "application/json",
                b"{\"2\":true}".to_vec(),
            )
            .unwrap();
            let output = ProviderOutput::with_effective_execution(
                "ans",
                vec!["c"],
                Some("r1"),
                ProviderExecutionEvidence::new(
                    Some("sol"),
                    Some("fixture-provider"),
                    Some("fixture-sol"),
                    Some("high"),
                )
                .unwrap(),
                ProviderTokenUsage {
                    actual_input_tokens: None,
                    actual_output_tokens: None,
                },
                vec![first_artifact.clone(), second_artifact.clone()],
            )
            .unwrap();
            let provider_port = FakeProviderPort::success(output.clone());
            let auth_store = FakeAuthStore::always_ok();
            let sqz_port = FakeAgentSqzPort::passthrough();
            let mut store = MemoryResultStore::new(3, 1000, 500, 10000).unwrap();
            let ports = RuntimePorts::new(&auth_store, &provider_port, &sqz_port, &mut store);
            let request = make_request("sol", "ma", make_trusted_opts());
            let receipt = rt
                .invoke(
                    &request,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    8000,
                )
                .unwrap();
            let stored_ref = receipt.stored_result_ref().unwrap();
            assert_eq!(stored_ref.artifact_ids().len(), 2);
            let expected_result = canonical_result("ans", &["c".to_string()]);
            let result_id = stored_ref.result_id().clone();
            let first_artifact_id = stored_ref.artifact_ids()[0].clone();
            let second_artifact_id = stored_ref.artifact_ids()[1].clone();
            assert_eq!(store.get(&result_id, 8000).unwrap(), expected_result);
            assert_eq!(
                store.get(&first_artifact_id, 8000).unwrap(),
                b"{\"1\":true}"
            );
            assert_eq!(
                store.get(&second_artifact_id, 8000).unwrap(),
                b"{\"2\":true}"
            );
        }

        // --- output SQZ governs inline content and result storage
        {
            let auth = FakeAuthStore::always_ok();
            let sqz = FakeAgentSqzPort::rewrite_output();
            let prov = FakeProviderPort::success(success_out.clone());
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let req = make_request("sol", "rewrite prompt", make_trusted_opts());
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    5001,
                )
                .unwrap();
            let inline = rec.inline_result().unwrap();
            assert_eq!(inline.answer(), "SQZ answer");
            let stored = rec.stored_result_ref().unwrap();
            let expected = canonical_result("SQZ answer", &success_citations);
            assert_eq!(stored.result_id(), &ResultId::sha256(&expected));
            assert_eq!(store.get(stored.result_id(), 5001).unwrap(), expected);
            assert_eq!(stored.artifact_ids()[0], *success_out.artifacts()[0].id());
        }

        // --- DLP-rejected output never reaches inline content or the result store
        {
            let auth = FakeAuthStore::always_ok();
            let sqz = FakeAgentSqzPort::dlp_output();
            let prov = FakeProviderPort::success(success_out.clone());
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let req = make_request("sol", "dlp prompt", make_trusted_opts());
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    5002,
                )
                .unwrap();
            match rec.outcome() {
                InvocationOutcome::Incomplete { failure, .. } => {
                    assert_eq!(*failure, AgentFailure::SqzOutputFailed);
                }
                _ => panic!("expected SQZ output failure"),
            }
            assert!(rec.inline_result().is_none());
            assert!(rec.stored_result_ref().is_none());
            assert_eq!(store.receipt().entry_count, 0);
        }

        let mut lifecycle = InvocationLifecycle::configured();
        assert!(lifecycle.complete().is_err());
        assert!(lifecycle.advance(InvocationState::PacketReady).is_ok());
        assert!(lifecycle.advance(InvocationState::Running).is_ok());
        assert!(lifecycle.advance(InvocationState::Validating).is_ok());
        assert!(lifecycle.complete().is_ok());
        assert!(lifecycle.incomplete().is_err());

        // --- success with no_session flag: flag set on complete receipt (connected invocation)
        {
            let auth = FakeAuthStore::always_ok();
            let sqz = FakeAgentSqzPort::passthrough();
            let prov = FakeProviderPort::success(success_out.clone());
            let mut store = MemoryResultStore::new(8, 10000, 2000, 10000).unwrap();
            let ports = RuntimePorts::new(&auth, &prov, &sqz, &mut store);
            let mut opts = make_trusted_opts();
            opts.no_session = true;
            let req = make_request("sol", "no session success", opts);
            let rec = rt
                .invoke(
                    &req,
                    AgentRuntimeAuthority::new(false, true, false),
                    &registry,
                    || ports,
                    6000,
                )
                .unwrap();
            assert!(rec.no_session());
            assert_eq!(prov.no_sessions.borrow().as_slice(), &[true]);
            match rec.outcome() {
                InvocationOutcome::Complete { .. } => {}
                _ => panic!("no-session must allow complete success"),
            }
            assert_eq!(sqz.calls.get(), 2);
            assert!(rec.stored_result_ref().is_some());
        }

        // --- ResultStore invariants (straightforward public API): bounded, oldest-first, dedup, ttl, byte exact
        {
            let mut rs = MemoryResultStore::new(2, 1024, 512, 100).unwrap();
            assert_eq!(
                rs.put_batch(std::iter::empty::<&[u8]>(), 10),
                Err(ResultStoreError::EmptyInput)
            );
            let p1 = rs.put(b"first-bytes", 10).unwrap();
            let _p2 = rs.put(b"second-bytes", 11).unwrap();
            assert_eq!(rs.receipt().entry_count, 2);
            // deduplicating: identical bytes -> same id, no growth
            let p1b = rs.put(b"first-bytes", 12).unwrap();
            assert_eq!(p1.id, p1b.id);
            assert_eq!(rs.receipt().entry_count, 2);
            // deterministic oldest-first eviction on capacity
            let _p3 = rs.put(b"third-bytes", 20).unwrap();
            assert_eq!(rs.receipt().entry_count, 2);
            assert!(rs.get(&p1.id, 20).is_err());
            // ttl-aware eviction on get
            let mut rs2 = MemoryResultStore::new(5, 1024, 512, 5).unwrap();
            let old = rs2.put(b"ttl-old", 0).unwrap().id;
            assert!(rs2.get(&old, 0).is_ok());
            assert!(rs2.get(&old, 10).is_err());
            // byte exact already asserted in success path above
        }
    }
}
