//! Controlled in-memory adapters for deterministic integration tests.
//! This harness is inert until explicitly invoked by a caller.

use std::time::Duration;

use crate::adapters::{
    DlpStatus, FidelityStatus, SqzId, SqzIdentity, SqzPolicy, SqzReceipt, SqzStatus,
};

use super::coordinator::{
    AgentRuntime, AgentRuntimeAuthority, AgentRuntimeConfig, AgentSqzError, AgentSqzOutput,
    AgentSqzPort, RuntimePorts, SqzStage,
};
use super::delegate::{DelegationOptions, DelegationRequest};
use super::receipt::DelegationReceipt;
use super::result_store::{MemoryResultStore, ResultId};
use super::runtime::{
    AuthError, AuthStore, ProviderError, ProviderExecutionEvidence, ProviderOutput, ProviderPort,
    ProviderRequest, ProviderTokenUsage,
};
use super::synthetic_builtin_profiles;

/// Invokes the real runtime with deterministic, in-memory fixture ports.
pub fn connected_receipt() -> DelegationReceipt {
    connected_receipt_for(&fixture_request(), true)
}

/// As [`connected_receipt`], but the provider omits effective-execution evidence.
pub fn connected_unattested_receipt() -> DelegationReceipt {
    connected_receipt_for(&fixture_request(), false)
}

/// Runs a caller-supplied request through the real runtime and injected fixture ports.
pub fn connected_receipt_for(request: &DelegationRequest, attested: bool) -> DelegationReceipt {
    let runtime = AgentRuntime::new(AgentRuntimeConfig::new(true, 1).expect("fixture config"));
    let auth = FixtureAuthStore;
    let provider = FixtureProvider {
        attested,
        profile: request.profile().to_owned(),
    };
    let sqz = FixtureSqz;
    let mut results = MemoryResultStore::new(8, 65_536, 65_536, 8).expect("fixture store");
    runtime
        .invoke(
            request,
            AgentRuntimeAuthority::new(false, true, false),
            &synthetic_builtin_profiles(),
            || RuntimePorts::new(&auth, &provider, &sqz, &mut results),
            0,
        )
        .expect("fixture receipt invariant")
}

/// Small runtime context for the CLI headless `-p` prompt path.
/// Production path uses disabled (default) or explicit-offline authority to obtain
/// the real typed incomplete receipt via the executor (never wires providers, auth,
/// network or SDKs). Tests may bypass via connected_receipt_for to exercise full
/// success and unattested-effective paths with canonical to_json.
pub fn headless_incomplete_receipt_for(
    request: &DelegationRequest,
    offline: bool,
) -> DelegationReceipt {
    let runtime = if offline {
        AgentRuntime::new(AgentRuntimeConfig::new(true, 1).expect("headless config"))
    } else {
        AgentRuntime::new(AgentRuntimeConfig::default())
    };
    let authority = AgentRuntimeAuthority::new(offline, true, false);
    let apr = synthetic_builtin_profiles();
    let auth = FixtureAuthStore;
    let prov = FixtureProvider {
        attested: false,
        profile: request.profile().to_owned(),
    };
    let sqz = FixtureSqz;
    let mut results = MemoryResultStore::new(8, 65_536, 65_536, 8).expect("headless store");
    runtime
        .invoke(
            request,
            authority,
            &apr,
            || RuntimePorts::new(&auth, &prov, &sqz, &mut results),
            0,
        )
        .expect("headless receipt invariant")
}

fn fixture_request() -> DelegationRequest {
    DelegationRequest::new(
        "sol",
        "synthetic connected request",
        DelegationOptions {
            no_session: true,
            ..DelegationOptions::new()
        },
    )
    .expect("fixture request")
}

struct FixtureAuthStore;

impl AuthStore for FixtureAuthStore {
    type Credential = ();

    fn resolve(&self, _account_handle: &str) -> Result<Self::Credential, AuthError> {
        Ok(())
    }
}

struct FixtureProvider {
    attested: bool,
    profile: String,
}

impl ProviderPort<()> for FixtureProvider {
    fn invoke(
        &self,
        request: &ProviderRequest,
        _credential: &(),
    ) -> Result<ProviderOutput, ProviderError> {
        let tokens = ProviderTokenUsage {
            actual_input_tokens: Some(6),
            actual_output_tokens: Some(4),
        };
        let output = if self.attested {
            ProviderOutput::with_effective_execution(
                "synthetic connected answer",
                ["synthetic-citation"],
                Some("synthetic-run"),
                ProviderExecutionEvidence::new(
                    Some(self.profile.as_str()),
                    Some(request.provider()),
                    Some(request.model()),
                    Some(request.requested().as_str()),
                )
                .expect("fixture evidence"),
                tokens,
                [],
            )
            .expect("fixture output")
        } else {
            ProviderOutput::new(
                "synthetic connected answer",
                ["synthetic-citation"],
                Some("synthetic-run"),
                None::<String>,
                None::<String>,
                tokens,
                [],
            )
            .expect("fixture output")
        };
        match request.hard_total_token_ceiling() {
            Some(ceiling) => output
                .with_token_budget_attestation(ceiling)
                .map_err(|_| ProviderError::InvalidOutput),
            None => Ok(output),
        }
    }
}

struct FixtureSqz;

impl AgentSqzPort for FixtureSqz {
    fn evaluate(
        &self,
        _stage: SqzStage,
        payload: &str,
        _max_output_bytes: usize,
    ) -> Result<AgentSqzOutput, AgentSqzError> {
        AgentSqzOutput::new(payload, fixture_sqz_receipt(payload))
    }
}

fn fixture_sqz_receipt(payload: &str) -> SqzReceipt {
    let payload_id = ResultId::sha256(payload.as_bytes());
    let bytes = payload.len();
    SqzReceipt {
        schema_version: "1.0.0",
        configured_identity: SqzIdentity::approved(),
        returned_identity: Some(SqzIdentity::approved()),
        policy: SqzPolicy::PublicOn,
        status: SqzStatus::Applied,
        failure_reason: None,
        monotonic_call_latency: Duration::ZERO,
        raw_bytes: bytes,
        candidate_compressed_bytes: None,
        returned_bytes: bytes,
        raw_token_estimate_bytes_divided_by_four_ceiling: bytes.div_ceil(4),
        candidate_token_estimate_bytes_divided_by_four_ceiling: None,
        returned_token_estimate_bytes_divided_by_four_ceiling: bytes.div_ceil(4),
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
            algorithm: payload_id.algorithm(),
            value: payload_id.value().to_owned(),
        }),
    }
}
