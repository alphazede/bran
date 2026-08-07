//! Immutable versioned delegation receipt value types.
//! Standard library only. Uses existing agent/runtime/result_store and adapters::SqzReceipt.

use super::result_store::ResultId;
use super::runtime::{Attestation, InvocationOutcome, ProviderClaim};
use super::{AgentProfile, ReasoningLevel, ToolPolicy};
use crate::adapters::{DlpStatus, FidelityStatus, SqzReceipt, SqzStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiptError {
    _p: (),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedExecution {
    profile_name: String,
    provider: Attestation<String>,
    model: Attestation<String>,
    reasoning: Attestation<ReasoningLevel>,
    tool_policy: ToolPolicy,
}

impl RequestedExecution {
    pub fn new(
        profile_name: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        reasoning: ReasoningLevel,
        tool_policy: ToolPolicy,
    ) -> Result<Self, ReceiptError> {
        let profile_name = profile_name.into();
        let provider = provider.into();
        let model = model.into();
        if !is_valid_identity(&profile_name)
            || !is_valid_identity(&provider)
            || !is_valid_identity(&model)
        {
            return Err(ReceiptError { _p: () });
        }
        Ok(Self {
            profile_name,
            provider: Attestation::Attested(provider),
            model: Attestation::Attested(model),
            reasoning: Attestation::Attested(reasoning),
            tool_policy,
        })
    }

    pub fn with_attestations(
        profile_name: impl Into<String>,
        provider: Attestation<String>,
        model: Attestation<String>,
        reasoning: Attestation<ReasoningLevel>,
        tool_policy: ToolPolicy,
    ) -> Result<Self, ReceiptError> {
        let profile_name = profile_name.into();
        if !is_valid_identity(&profile_name)
            || matches!(&provider, Attestation::Attested(value) if !is_valid_identity(value))
            || matches!(&model, Attestation::Attested(value) if !is_valid_identity(value))
        {
            return Err(ReceiptError { _p: () });
        }
        Ok(Self {
            profile_name,
            provider,
            model,
            reasoning,
            tool_policy,
        })
    }

    pub fn profile_name(&self) -> &str {
        &self.profile_name
    }

    pub fn provider(&self) -> &Attestation<String> {
        &self.provider
    }

    pub fn model(&self) -> &Attestation<String> {
        &self.model
    }

    pub fn reasoning(&self) -> &Attestation<ReasoningLevel> {
        &self.reasoning
    }

    pub fn tool_policy(&self) -> &ToolPolicy {
        &self.tool_policy
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveExecution {
    profile: Attestation<AgentProfile>,
    provider: Attestation<String>,
    model: Attestation<String>,
    reasoning: Attestation<ReasoningLevel>,
    tool_policy: ToolPolicy,
}

impl EffectiveExecution {
    pub fn new(
        profile: Attestation<AgentProfile>,
        provider: Attestation<String>,
        model: Attestation<String>,
        reasoning: Attestation<ReasoningLevel>,
        tool_policy: ToolPolicy,
    ) -> Self {
        Self {
            profile,
            provider,
            model,
            reasoning,
            tool_policy,
        }
    }

    pub fn profile(&self) -> &Attestation<AgentProfile> {
        &self.profile
    }

    pub fn provider(&self) -> &Attestation<String> {
        &self.provider
    }

    pub fn model(&self) -> &Attestation<String> {
        &self.model
    }

    pub fn reasoning(&self) -> &Attestation<ReasoningLevel> {
        &self.reasoning
    }

    pub fn tool_policy(&self) -> &ToolPolicy {
        &self.tool_policy
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqzStages {
    input: Option<SqzReceipt>,
    output: Option<SqzReceipt>,
}

impl SqzStages {
    pub fn new(input: Option<SqzReceipt>, output: Option<SqzReceipt>) -> Self {
        Self { input, output }
    }

    pub fn input(&self) -> Option<&SqzReceipt> {
        self.input.as_ref()
    }

    pub fn output(&self) -> Option<&SqzReceipt> {
        self.output.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineResult {
    answer: String,
    citations: Vec<String>,
    claims: Vec<ProviderClaim>,
}

impl InlineResult {
    pub fn new(
        answer: impl Into<String>,
        citations: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ReceiptError> {
        let answer = answer.into();
        if answer.trim().is_empty() || answer.len() > 1_048_576 {
            return Err(ReceiptError { _p: () });
        }
        let citations: Vec<String> = citations.into_iter().map(Into::into).collect();
        if citations.len() > 128 {
            return Err(ReceiptError { _p: () });
        }
        for citation in &citations {
            if citation.trim().is_empty() || citation.len() > 1024 {
                return Err(ReceiptError { _p: () });
            }
        }
        Ok(Self {
            answer,
            citations,
            claims: Vec::new(),
        })
    }

    pub fn with_claims(
        answer: impl Into<String>,
        citations: impl IntoIterator<Item = impl Into<String>>,
        claims: impl IntoIterator<Item = ProviderClaim>,
    ) -> Result<Self, ReceiptError> {
        let mut result = Self::new(answer, citations)?;
        let claims = claims.into_iter().collect::<Vec<_>>();
        if claims.len() > 128 {
            return Err(ReceiptError { _p: () });
        }
        result.claims = claims;
        Ok(result)
    }

    pub fn answer(&self) -> &str {
        &self.answer
    }

    pub fn citations(&self) -> &[String] {
        &self.citations
    }

    pub fn claims(&self) -> &[ProviderClaim] {
        &self.claims
    }

    /// Stable byte representation stored under `StoredResultRef::result_id`.
    pub fn encode_canonical(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(
            self.answer.len()
                + self.citations.iter().map(String::len).sum::<usize>()
                + self
                    .claims
                    .iter()
                    .map(|claim| {
                        claim.id().len()
                            + claim.text().len()
                            + claim.locator().len()
                            + claim.content_digest().len()
                            + claim.support().len()
                            + 32
                    })
                    .sum::<usize>()
                + 48,
        );
        encoded.extend_from_slice(if self.claims.is_empty() {
            b"bran-agent-result-v1"
        } else {
            b"bran-agent-result-v2"
        });
        encode_field(&mut encoded, self.answer.as_bytes());
        encoded.extend_from_slice(&(self.citations.len() as u64).to_be_bytes());
        for citation in &self.citations {
            encode_field(&mut encoded, citation.as_bytes());
        }
        if !self.claims.is_empty() {
            encoded.extend_from_slice(&(self.claims.len() as u64).to_be_bytes());
            for claim in &self.claims {
                encode_field(&mut encoded, claim.id().as_bytes());
                encode_field(&mut encoded, claim.text().as_bytes());
                encoded.push(u8::from(claim.material()));
                encode_field(&mut encoded, claim.locator().as_bytes());
                encode_field(&mut encoded, claim.content_digest().as_bytes());
                encode_field(&mut encoded, claim.support().as_bytes());
            }
        }
        encoded
    }

    /// Decodes only the exact canonical representation; trailing or malformed
    /// bytes are rejected before the normal answer/citation bounds are applied.
    pub fn decode_canonical(encoded: &[u8]) -> Result<Self, ReceiptError> {
        let (version, mut remaining) =
            if let Some(remaining) = encoded.strip_prefix(b"bran-agent-result-v2") {
                (2, remaining)
            } else if let Some(remaining) = encoded.strip_prefix(b"bran-agent-result-v1") {
                (1, remaining)
            } else {
                return Err(ReceiptError { _p: () });
            };
        let answer = decode_field(&mut remaining)?;
        let citation_count = decode_u64(&mut remaining)?;
        let citation_count =
            usize::try_from(citation_count).map_err(|_| ReceiptError { _p: () })?;
        if citation_count > 128 {
            return Err(ReceiptError { _p: () });
        }
        let mut citations = Vec::with_capacity(citation_count);
        for _ in 0..citation_count {
            citations.push(decode_field(&mut remaining)?);
        }
        let answer = String::from_utf8(answer).map_err(|_| ReceiptError { _p: () })?;
        let citations = citations
            .into_iter()
            .map(|citation| String::from_utf8(citation).map_err(|_| ReceiptError { _p: () }))
            .collect::<Result<Vec<_>, _>>()?;
        if version == 1 {
            if !remaining.is_empty() {
                return Err(ReceiptError { _p: () });
            }
            return Self::new(answer, citations);
        }
        let claim_count = decode_u64(&mut remaining)?;
        let claim_count = usize::try_from(claim_count).map_err(|_| ReceiptError { _p: () })?;
        if claim_count == 0 || claim_count > 128 {
            return Err(ReceiptError { _p: () });
        }
        let mut claims = Vec::with_capacity(claim_count);
        for _ in 0..claim_count {
            let id = String::from_utf8(decode_field(&mut remaining)?)
                .map_err(|_| ReceiptError { _p: () })?;
            let text = String::from_utf8(decode_field(&mut remaining)?)
                .map_err(|_| ReceiptError { _p: () })?;
            let (&material, rest) = remaining.split_first().ok_or(ReceiptError { _p: () })?;
            remaining = rest;
            let material = match material {
                0 => false,
                1 => true,
                _ => return Err(ReceiptError { _p: () }),
            };
            let locator = String::from_utf8(decode_field(&mut remaining)?)
                .map_err(|_| ReceiptError { _p: () })?;
            let content_digest = String::from_utf8(decode_field(&mut remaining)?)
                .map_err(|_| ReceiptError { _p: () })?;
            let support = String::from_utf8(decode_field(&mut remaining)?)
                .map_err(|_| ReceiptError { _p: () })?;
            claims.push(
                ProviderClaim::new(id, text, material, locator, content_digest, support)
                    .map_err(|_| ReceiptError { _p: () })?,
            );
        }
        if !remaining.is_empty() {
            return Err(ReceiptError { _p: () });
        }
        Self::with_claims(answer, citations, claims)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredResultRef {
    result_id: ResultId,
    artifact_ids: Vec<ResultId>,
}

impl StoredResultRef {
    pub fn new(
        result_id: ResultId,
        artifact_ids: impl IntoIterator<Item = ResultId>,
    ) -> Result<Self, ReceiptError> {
        let artifact_ids: Vec<ResultId> = artifact_ids.into_iter().collect();
        if artifact_ids.len() > 64 {
            return Err(ReceiptError { _p: () });
        }
        Ok(Self {
            result_id,
            artifact_ids,
        })
    }

    pub fn result_id(&self) -> &ResultId {
        &self.result_id
    }

    pub fn artifact_ids(&self) -> &[ResultId] {
        &self.artifact_ids
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationReceiptParts {
    pub outcome: InvocationOutcome,
    pub requested: RequestedExecution,
    pub effective: EffectiveExecution,
    pub sqz: SqzStages,
    pub inline_result: Option<InlineResult>,
    pub stored_ref: Option<StoredResultRef>,
    pub no_session: bool,
    pub provenance: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationReceipt {
    schema_version: &'static str,
    outcome: InvocationOutcome,
    requested: RequestedExecution,
    effective: EffectiveExecution,
    sqz: SqzStages,
    inline_result: Option<InlineResult>,
    stored_ref: Option<StoredResultRef>,
    no_session: bool,
    provenance: Vec<String>,
    provider_run_id: Attestation<String>,
}

impl DelegationReceipt {
    pub fn new(parts: DelegationReceiptParts) -> Result<Self, ReceiptError> {
        if parts.provenance.len() > 128 {
            return Err(ReceiptError { _p: () });
        }
        for entry in &parts.provenance {
            if !is_valid_identity(entry) {
                return Err(ReceiptError { _p: () });
            }
        }
        for sqz_receipt in [parts.sqz.input.as_ref(), parts.sqz.output.as_ref()]
            .into_iter()
            .flatten()
        {
            if sqz_receipt.monotonic_call_latency.as_nanos() > u64::MAX as u128 {
                return Err(ReceiptError { _p: () });
            }
        }
        match &parts.outcome {
            InvocationOutcome::Complete { .. } => {
                let (Some(inline), Some(stored), Some(input), Some(output)) = (
                    parts.inline_result.as_ref(),
                    parts.stored_ref.as_ref(),
                    parts.sqz.input.as_ref(),
                    parts.sqz.output.as_ref(),
                ) else {
                    return Err(ReceiptError { _p: () });
                };
                if !accepted_sqz(input)
                    || !accepted_sqz(output)
                    || stored.result_id() != &ResultId::sha256(&inline.encode_canonical())
                    || (output.status != SqzStatus::Off
                        && output.sqz_id.as_ref().is_none_or(|id| {
                            id.algorithm != ResultId::sha256(inline.answer().as_bytes()).algorithm()
                                || id.value != ResultId::sha256(inline.answer().as_bytes()).value()
                        }))
                {
                    return Err(ReceiptError { _p: () });
                }
            }
            InvocationOutcome::Incomplete { .. } => {
                if parts.stored_ref.is_some() {
                    return Err(ReceiptError { _p: () });
                }
            }
        }

        Ok(Self {
            schema_version: "bran-agent-receipt-v1",
            outcome: parts.outcome,
            requested: parts.requested,
            effective: parts.effective,
            sqz: parts.sqz,
            inline_result: parts.inline_result,
            stored_ref: parts.stored_ref,
            no_session: parts.no_session,
            provenance: parts.provenance,
            provider_run_id: Attestation::Unavailable,
        })
    }

    pub fn schema_version(&self) -> &'static str {
        self.schema_version
    }

    pub fn outcome(&self) -> &InvocationOutcome {
        &self.outcome
    }

    pub fn requested(&self) -> &RequestedExecution {
        &self.requested
    }

    pub fn effective(&self) -> &EffectiveExecution {
        &self.effective
    }

    pub fn sqz_stages(&self) -> &SqzStages {
        &self.sqz
    }

    pub fn inline_result(&self) -> Option<&InlineResult> {
        self.inline_result.as_ref()
    }

    pub fn stored_result_ref(&self) -> Option<&StoredResultRef> {
        self.stored_ref.as_ref()
    }

    pub fn no_session(&self) -> bool {
        self.no_session
    }

    pub fn provenance(&self) -> &[String] {
        &self.provenance
    }

    pub fn provider_run_id(&self) -> &Attestation<String> {
        &self.provider_run_id
    }

    pub fn with_provider_run_id(
        mut self,
        provider_run_id: Attestation<String>,
    ) -> Result<Self, ReceiptError> {
        if matches!(&provider_run_id, Attestation::Attested(value) if !is_valid_identity(value)) {
            return Err(ReceiptError { _p: () });
        }
        self.provider_run_id = provider_run_id;
        Ok(self)
    }

    /// Canonical, versioned JSON receipt evidence. It intentionally excludes
    /// credentials and the original prompt.
    pub fn to_json(&self) -> String {
        let mut json = String::new();
        json.push('{');
        field_str(&mut json, "schema_version", self.schema_version);
        json.push(',');
        field_outcome(&mut json, "outcome", &self.outcome);
        json.push(',');
        field_requested(&mut json, "requested", &self.requested);
        json.push(',');
        field_effective(&mut json, "effective", &self.effective);
        json.push(',');
        field_attestation_string(&mut json, "provider_run_id", &self.provider_run_id);
        json.push(',');
        field_sqz(&mut json, "sqz", &self.sqz);
        json.push(',');
        field_inline(&mut json, "inline_result", self.inline_result.as_ref());
        json.push(',');
        field_stored(&mut json, "stored_result_ref", self.stored_ref.as_ref());
        json.push(',');
        field_bool(&mut json, "no_session", self.no_session);
        json.push(',');
        field_strings(&mut json, "provenance", &self.provenance);
        json.push('}');
        json
    }
}

fn key(json: &mut String, name: &str) {
    string(json, name);
    json.push(':');
}

fn string(json: &mut String, value: &str) {
    json.push('"');
    for ch in value.chars() {
        match ch {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            '\u{08}' => json.push_str("\\b"),
            '\t' => json.push_str("\\t"),
            '\n' => json.push_str("\\n"),
            '\u{0c}' => json.push_str("\\f"),
            '\r' => json.push_str("\\r"),
            ch if ch <= '\u{1f}' || ch == '\u{7f}' => {
                use std::fmt::Write as _;
                write!(json, "\\u{:04x}", ch as u32).expect("writing to String cannot fail");
            }
            ch => json.push(ch),
        }
    }
    json.push('"');
}

fn field_str(json: &mut String, name: &str, value: &str) {
    key(json, name);
    string(json, value);
}

fn field_bool(json: &mut String, name: &str, value: bool) {
    key(json, name);
    json.push_str(if value { "true" } else { "false" });
}

fn field_usize(json: &mut String, name: &str, value: usize) {
    use std::fmt::Write as _;
    key(json, name);
    write!(json, "{value}").expect("writing to String cannot fail");
}

fn field_u64(json: &mut String, name: &str, value: u64) {
    use std::fmt::Write as _;
    key(json, name);
    write!(json, "{value}").expect("writing to String cannot fail");
}

fn field_option_usize(json: &mut String, name: &str, value: Option<usize>) {
    key(json, name);
    match value {
        Some(value) => json.push_str(&value.to_string()),
        None => json.push_str("null"),
    }
}

fn field_strings(json: &mut String, name: &str, values: &[String]) {
    key(json, name);
    strings(json, values);
}

fn strings(json: &mut String, values: &[String]) {
    json.push('[');
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            json.push(',');
        }
        string(json, value);
    }
    json.push(']');
}

fn attestation<T>(
    json: &mut String,
    value: &Attestation<T>,
    write_value: impl FnOnce(&mut String, &T),
) {
    json.push('{');
    match value {
        Attestation::Attested(value) => {
            field_str(json, "state", "attested");
            json.push(',');
            key(json, "value");
            write_value(json, value);
        }
        Attestation::Unavailable => {
            field_str(json, "state", "unavailable");
            json.push(',');
            json.push_str("\"value\":null");
        }
    }
    json.push('}');
}

fn field_attestation_string(json: &mut String, name: &str, value: &Attestation<String>) {
    key(json, name);
    attestation(json, value, |json, value| string(json, value));
}

fn field_attestation_reasoning(json: &mut String, name: &str, value: &Attestation<ReasoningLevel>) {
    key(json, name);
    attestation(json, value, |json, value| string(json, value.as_str()));
}

fn policy(json: &mut String, policy: &ToolPolicy) {
    json.push('{');
    key(json, "allow");
    json.push('[');
    for (index, value) in policy.allowed().enumerate() {
        if index != 0 {
            json.push(',');
        }
        string(json, value);
    }
    json.push_str("],\"deny\":[");
    for (index, value) in policy.denied().enumerate() {
        if index != 0 {
            json.push(',');
        }
        string(json, value);
    }
    json.push_str("]}");
}

fn field_requested(json: &mut String, name: &str, requested: &RequestedExecution) {
    key(json, name);
    json.push('{');
    field_str(json, "profile_name", requested.profile_name());
    json.push(',');
    field_attestation_string(json, "provider", requested.provider());
    json.push(',');
    field_attestation_string(json, "model", requested.model());
    json.push(',');
    field_attestation_reasoning(json, "reasoning", requested.reasoning());
    json.push(',');
    key(json, "tool_policy");
    policy(json, requested.tool_policy());
    json.push('}');
}

fn profile(json: &mut String, profile: &AgentProfile) {
    json.push('{');
    field_str(json, "name", profile.name());
    json.push(',');
    field_str(json, "provider", profile.provider());
    json.push(',');
    field_str(json, "model", profile.model());
    json.push(',');
    field_str(
        json,
        "default_reasoning_level",
        profile.default_reasoning_level().as_str(),
    );
    json.push(',');
    key(json, "tool_policy");
    policy(json, profile.tool_policy());
    json.push('}');
}

fn field_effective(json: &mut String, name: &str, effective: &EffectiveExecution) {
    key(json, name);
    json.push('{');
    key(json, "profile");
    attestation(json, effective.profile(), profile);
    json.push(',');
    field_attestation_string(json, "provider", effective.provider());
    json.push(',');
    field_attestation_string(json, "model", effective.model());
    json.push(',');
    field_attestation_reasoning(json, "reasoning", effective.reasoning());
    json.push(',');
    key(json, "tool_policy");
    policy(json, effective.tool_policy());
    json.push('}');
}

fn field_outcome(json: &mut String, name: &str, outcome: &InvocationOutcome) {
    key(json, name);
    let (state, failure, metrics) = match outcome {
        InvocationOutcome::Complete { metrics } => ("complete", None, metrics),
        InvocationOutcome::Incomplete { failure, metrics } => {
            ("incomplete", Some(*failure), metrics)
        }
    };
    json.push('{');
    field_str(json, "state", state);
    json.push(',');
    key(json, "failure");
    match failure {
        Some(failure) => string(json, failure_name(failure)),
        None => json.push_str("null"),
    }
    json.push(',');
    key(json, "metrics");
    json.push('{');
    field_option_usize(json, "actual_input_tokens", metrics.actual_input_tokens());
    json.push(',');
    field_option_usize(json, "actual_output_tokens", metrics.actual_output_tokens());
    json.push(',');
    field_usize(json, "input_bytes", metrics.input_bytes());
    json.push(',');
    field_usize(json, "output_bytes", metrics.output_bytes());
    json.push(',');
    field_u64(json, "latency_ms", metrics.latency_ms());
    json.push_str("}}");
}

fn failure_name(failure: super::runtime::AgentFailure) -> &'static str {
    match failure {
        super::runtime::AgentFailure::AgentDisabled => "agent_disabled",
        super::runtime::AgentFailure::ExplicitOffline => "explicit_offline",
        super::runtime::AgentFailure::ProjectUntrusted => "project_untrusted",
        super::runtime::AgentFailure::UnknownProfile => "unknown_profile",
        super::runtime::AgentFailure::UnknownProvider => "unknown_provider",
        super::runtime::AgentFailure::UnknownModel => "unknown_model",
        super::runtime::AgentFailure::MissingAuth => "missing_auth",
        super::runtime::AgentFailure::ProviderUnavailable => "provider_unavailable",
        super::runtime::AgentFailure::ProviderFailed => "provider_failed",
        super::runtime::AgentFailure::Timeout => "timeout",
        super::runtime::AgentFailure::Cancelled => "cancelled",
        super::runtime::AgentFailure::DepthExceeded => "depth_exceeded",
        super::runtime::AgentFailure::DeniedTool => "denied_tool",
        super::runtime::AgentFailure::InvalidOutput => "invalid_output",
        super::runtime::AgentFailure::DlpRejected => "dlp_rejected",
        super::runtime::AgentFailure::GroundingFailed => "grounding_failed",
        super::runtime::AgentFailure::ClaimUnsupported => "claim_unsupported",
        super::runtime::AgentFailure::TokenBudgetUnattested => "token_budget_unattested",
        super::runtime::AgentFailure::TokenCeilingExceeded => "token_ceiling_exceeded",
        super::runtime::AgentFailure::SqzInputFailed => "sqz_input_failed",
        super::runtime::AgentFailure::SqzOutputFailed => "sqz_output_failed",
        super::runtime::AgentFailure::ResultStoreFailed => "result_store_failed",
    }
}

fn field_sqz(json: &mut String, name: &str, stages: &SqzStages) {
    key(json, name);
    json.push('{');
    key(json, "input");
    optional_sqz(json, stages.input());
    json.push(',');
    key(json, "output");
    optional_sqz(json, stages.output());
    json.push('}');
}

fn optional_sqz(json: &mut String, receipt: Option<&SqzReceipt>) {
    match receipt {
        Some(receipt) => sqz(json, receipt),
        None => json.push_str("null"),
    }
}

/// Canonical JSON for a standalone SQZ receipt.
pub fn sqz_receipt_json(receipt: &SqzReceipt) -> String {
    let mut json = String::new();
    sqz(&mut json, receipt);
    json
}

fn sqz(json: &mut String, receipt: &SqzReceipt) {
    json.push('{');
    field_str(json, "schema_version", receipt.schema_version);
    json.push(',');
    key(json, "configured_identity");
    sqz_identity(json, &receipt.configured_identity);
    json.push(',');
    key(json, "returned_identity");
    match &receipt.returned_identity {
        Some(identity) => sqz_identity(json, identity),
        None => json.push_str("null"),
    }
    json.push(',');
    field_str(json, "policy", sqz_policy(receipt.policy));
    json.push(',');
    field_str(json, "status", sqz_status(receipt.status));
    json.push(',');
    key(json, "failure_reason");
    match &receipt.failure_reason {
        Some(reason) => sqz_failure_reason(json, reason),
        None => json.push_str("null"),
    }
    json.push(',');
    key(json, "monotonic_call_latency_ns");
    let monotonic_call_latency_ns = u64::try_from(receipt.monotonic_call_latency.as_nanos())
        .expect(
            "monotonic_call_latency_ns fits in u64 because DelegationReceipt::new validates it",
        );
    {
        use std::fmt::Write as _;
        write!(json, "{monotonic_call_latency_ns}").expect("writing to String cannot fail");
    }
    json.push(',');
    field_usize(json, "raw_bytes", receipt.raw_bytes);
    json.push(',');
    field_option_usize(
        json,
        "candidate_compressed_bytes",
        receipt.candidate_compressed_bytes,
    );
    json.push(',');
    field_usize(json, "returned_bytes", receipt.returned_bytes);
    json.push(',');
    field_usize(
        json,
        "raw_token_estimate_bytes_divided_by_four_ceiling",
        receipt.raw_token_estimate_bytes_divided_by_four_ceiling,
    );
    json.push(',');
    field_option_usize(
        json,
        "candidate_token_estimate_bytes_divided_by_four_ceiling",
        receipt.candidate_token_estimate_bytes_divided_by_four_ceiling,
    );
    json.push(',');
    field_usize(
        json,
        "returned_token_estimate_bytes_divided_by_four_ceiling",
        receipt.returned_token_estimate_bytes_divided_by_four_ceiling,
    );
    json.push(',');
    field_option_usize(json, "actual_input_tokens", receipt.actual_input_tokens);
    json.push(',');
    field_option_usize(json, "actual_output_tokens", receipt.actual_output_tokens);
    json.push(',');
    field_str(
        json,
        "fidelity_status",
        fidelity_status(receipt.fidelity_status),
    );
    json.push(',');
    field_strings(
        json,
        "required_fidelity_anchor_ids",
        &receipt.required_fidelity_anchor_ids,
    );
    json.push(',');
    field_strings(
        json,
        "missing_fidelity_anchor_ids",
        &receipt.missing_fidelity_anchor_ids,
    );
    json.push(',');
    field_str(json, "dlp_status", dlp_status(receipt.dlp_status));
    json.push(',');
    field_strings(json, "dlp_findings", &receipt.dlp_findings);
    json.push(',');
    field_usize(
        json,
        "requested_max_output_bytes",
        receipt.requested_max_output_bytes,
    );
    json.push(',');
    field_usize(
        json,
        "effective_max_output_bytes",
        receipt.effective_max_output_bytes,
    );
    json.push(',');
    key(json, "sqz_id");
    match &receipt.sqz_id {
        Some(id) => {
            json.push('{');
            field_str(json, "algorithm", id.algorithm);
            json.push(',');
            field_str(json, "value", &id.value);
            json.push('}');
        }
        None => json.push_str("null"),
    }
    json.push('}');
}

fn sqz_failure_reason(json: &mut String, reason: &crate::adapters::SqzFailureReason) {
    use crate::adapters::{SqzFailureReason, SqzPortErrorCode};

    let (kind, port_error_code_opt) = match reason {
        SqzFailureReason::ConfiguredIdentityMismatch => ("configured-identity-mismatch", None),
        SqzFailureReason::ReturnedIdentityMismatch => ("returned-identity-mismatch", None),
        SqzFailureReason::PortUnavailable(code) => ("port-unavailable", Some(*code)),
        SqzFailureReason::FidelityAnchorsUnavailable => ("fidelity-anchors-unavailable", None),
        SqzFailureReason::MissingFidelityAnchors => ("missing-fidelity-anchors", None),
        SqzFailureReason::FidelityAnchorMissingFromInput => {
            ("fidelity-anchor-missing-from-input", None)
        }
        SqzFailureReason::ConflictingFidelityAnchorIds => ("conflicting-fidelity-anchor-ids", None),
        SqzFailureReason::DlpFindings => ("dlp-findings", None),
        SqzFailureReason::OutputExceedsBound => ("output-exceeds-bound", None),
    };
    json.push('{');
    field_str(json, "kind", kind);
    if let Some(port_error_code) = port_error_code_opt {
        json.push(',');
        key(json, "port_error_code");
        let port_error_code_name = match port_error_code {
            SqzPortErrorCode::Unavailable => "unavailable",
            SqzPortErrorCode::Timeout => "timeout",
            SqzPortErrorCode::ExecutionFailed => "execution-failed",
            SqzPortErrorCode::InvalidOutput => "invalid-output",
        };
        string(json, port_error_code_name);
    }
    json.push('}');
}

fn sqz_identity(json: &mut String, identity: &crate::adapters::SqzIdentity) {
    json.push('{');
    field_str(json, "source", &identity.source);
    json.push(',');
    field_str(json, "version", &identity.version);
    json.push(',');
    field_str(json, "sha256", &identity.sha256);
    json.push('}');
}

fn sqz_policy(policy: crate::adapters::SqzPolicy) -> &'static str {
    match policy {
        crate::adapters::SqzPolicy::PublicOff => "public-off",
        crate::adapters::SqzPolicy::PublicOn => "public-on",
        crate::adapters::SqzPolicy::InternalLocked => "internal-locked",
    }
}

fn sqz_status(status: SqzStatus) -> &'static str {
    match status {
        SqzStatus::Off => "off",
        SqzStatus::Applied => "applied",
        SqzStatus::NotBeneficial => "not-beneficial",
        SqzStatus::Failed => "failed",
    }
}

fn fidelity_status(status: FidelityStatus) -> &'static str {
    match status {
        FidelityStatus::NotEvaluated => "not-evaluated",
        FidelityStatus::RequiredButUnavailable => "required-but-unavailable",
        FidelityStatus::Passed => "passed",
        FidelityStatus::Missing => "missing",
    }
}

fn dlp_status(status: DlpStatus) -> &'static str {
    match status {
        DlpStatus::NotEvaluated => "not-evaluated",
        DlpStatus::InputPassed => "input-passed",
        DlpStatus::Passed => "passed",
        DlpStatus::Findings => "findings",
    }
}

fn field_inline(json: &mut String, name: &str, inline: Option<&InlineResult>) {
    key(json, name);
    match inline {
        Some(inline) => {
            json.push('{');
            field_str(json, "answer", inline.answer());
            json.push(',');
            field_strings(json, "citations", inline.citations());
            json.push(',');
            key(json, "claims");
            json.push('[');
            for (index, claim) in inline.claims().iter().enumerate() {
                if index != 0 {
                    json.push(',');
                }
                json.push('{');
                field_str(json, "id", claim.id());
                json.push(',');
                field_str(json, "text", claim.text());
                json.push(',');
                field_bool(json, "material", claim.material());
                json.push(',');
                field_str(json, "locator", claim.locator());
                json.push(',');
                field_str(json, "content_digest", claim.content_digest());
                json.push(',');
                field_str(json, "support", claim.support());
                json.push('}');
            }
            json.push(']');
            json.push('}');
        }
        None => json.push_str("null"),
    }
}

fn field_stored(json: &mut String, name: &str, stored: Option<&StoredResultRef>) {
    key(json, name);
    match stored {
        Some(stored) => {
            json.push('{');
            field_str(json, "result_id", &stored.result_id().to_string());
            json.push(',');
            key(json, "artifact_ids");
            json.push('[');
            for (index, id) in stored.artifact_ids().iter().enumerate() {
                if index != 0 {
                    json.push(',');
                }
                string(json, &id.to_string());
            }
            json.push_str("]}");
        }
        None => json.push_str("null"),
    }
}

fn accepted_sqz(receipt: &SqzReceipt) -> bool {
    if receipt.failure_reason.is_some()
        || receipt.fidelity_status != FidelityStatus::Passed
        || receipt.dlp_status != DlpStatus::Passed
        || !receipt.dlp_findings.is_empty()
    {
        return false;
    }
    match (receipt.policy, receipt.status) {
        (crate::adapters::SqzPolicy::PublicOff, SqzStatus::Off) => {
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
            crate::adapters::SqzPolicy::PublicOn | crate::adapters::SqzPolicy::InternalLocked,
            SqzStatus::Applied | SqzStatus::NotBeneficial,
        ) => receipt.sqz_id.as_ref().is_some_and(|id| {
            id.algorithm == "sha256"
                && id.value.len() == 64
                && id
                    .value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        }),
        _ => false,
    }
}

fn encode_field(encoded: &mut Vec<u8>, field: &[u8]) {
    encoded.extend_from_slice(&(field.len() as u64).to_be_bytes());
    encoded.extend_from_slice(field);
}

fn decode_u64(encoded: &mut &[u8]) -> Result<u64, ReceiptError> {
    let raw = encoded.get(..8).ok_or(ReceiptError { _p: () })?;
    *encoded = &encoded[8..];
    Ok(u64::from_be_bytes(
        raw.try_into().map_err(|_| ReceiptError { _p: () })?,
    ))
}

fn decode_field(encoded: &mut &[u8]) -> Result<Vec<u8>, ReceiptError> {
    let length = decode_u64(encoded)?;
    let length = usize::try_from(length).map_err(|_| ReceiptError { _p: () })?;
    let field = encoded.get(..length).ok_or(ReceiptError { _p: () })?;
    *encoded = &encoded[length..];
    Ok(field.to_vec())
}

fn is_valid_identity(s: &str) -> bool {
    let bytes = s.as_bytes();
    let len = bytes.len();
    (1..=64).contains(&len)
        && bytes
            .iter()
            .all(|&b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}
