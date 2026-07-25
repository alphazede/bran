//! Deterministic, offline-first onboarding settings.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub const SETTINGS_VERSION: u32 = 3;
pub const PRACTICE_QUERY_LIMIT: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OnboardingStep {
    ProjectCheck,
    FlowChoice,
    OperatingProfile,
    Guardrails,
    CapabilityCheck,
    ReadinessReview,
    PracticeRun,
    Apply,
    Diagnose,
    Complete,
}

pub const ONBOARDING_STEPS: [OnboardingStep; 10] = [
    OnboardingStep::ProjectCheck,
    OnboardingStep::FlowChoice,
    OnboardingStep::OperatingProfile,
    OnboardingStep::Guardrails,
    OnboardingStep::CapabilityCheck,
    OnboardingStep::ReadinessReview,
    OnboardingStep::PracticeRun,
    OnboardingStep::Apply,
    OnboardingStep::Diagnose,
    OnboardingStep::Complete,
];

impl OnboardingStep {
    pub const fn next(self) -> Option<Self> {
        let index = self as usize;
        if index + 1 == ONBOARDING_STEPS.len() {
            None
        } else {
            Some(ONBOARDING_STEPS[index + 1])
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlowChoice {
    Quick,
    Advanced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatingProfile {
    OfflineCore,
    CoreSqz,
    ConnectedAgent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OnboardingMode {
    FirstRun,
    Repair,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Feature {
    Sqz,
    Diagnostics,
    Voice,
    StructuredHistory,
    SavedChat,
    Network,
    Auth,
    Mutation,
    CoreSqz,
    ConnectedAgent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootScope {
    BoundedCurrentRoot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolPolicy {
    ReadOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalPolicy {
    Explicit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetentionPolicy {
    ZeroConversation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticPolicy {
    Bounded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Guardrail {
    Root,
    Tools,
    Approval,
    Retention,
    Diagnostics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardrailValue {
    BoundedCurrentRoot,
    ReadOnlyTools,
    ExplicitApproval,
    ZeroConversationRetention,
    BoundedDiagnostics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolutionStatus {
    Effective,
    Disabled,
    Locked,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolutionProvenance {
    QuickSafe,
    Request,
    InternalPolicy,
    CapabilityProbe,
    OfflineCore,
    Repair,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingResolution {
    pub feature: Feature,
    pub requested: bool,
    pub effective: bool,
    pub provenance: ResolutionProvenance,
    pub status: ResolutionStatus,
    pub policy_locked: bool,
    pub capability_available: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuardrailResolution {
    pub guardrail: Guardrail,
    pub requested: GuardrailValue,
    pub effective: GuardrailValue,
    pub provenance: ResolutionProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Settings {
    pub profile: OperatingProfile,
    pub sqz: bool,
    pub diagnostics: bool,
    pub voice: bool,
    pub structured_history: bool,
    pub saved_chat: bool,
    pub network: bool,
    pub auth: bool,
    pub mutation: bool,
    /// Per Connected Agent task; not an Arena total or provider telemetry.
    pub connected_agent_task_token_ceiling: Option<u32>,
    pub root: RootScope,
    pub tools: ToolPolicy,
    pub approval: ApprovalPolicy,
    pub retention: RetentionPolicy,
    pub diagnostic_policy: DiagnosticPolicy,
}

impl Default for Settings {
    fn default() -> Self {
        quick_safe_config()
    }
}

pub fn quick_safe_config() -> Settings {
    Settings {
        profile: OperatingProfile::OfflineCore,
        sqz: true,
        diagnostics: true,
        voice: false,
        structured_history: false,
        saved_chat: false,
        network: false,
        auth: false,
        mutation: false,
        connected_agent_task_token_ceiling: None,
        root: RootScope::BoundedCurrentRoot,
        tools: ToolPolicy::ReadOnly,
        approval: ApprovalPolicy::Explicit,
        retention: RetentionPolicy::ZeroConversation,
        diagnostic_policy: DiagnosticPolicy::Bounded,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdvancedRequest {
    pub settings: Settings,
}

impl AdvancedRequest {
    pub fn new(settings: Settings) -> Self {
        Self { settings }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CapabilityProbe {
    pub sqz_available: bool,
    pub voice_available: bool,
    pub saved_chat_available: bool,
    pub connected_agent_runtime: bool,
    pub network_available: bool,
    pub auth_available: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Policy {
    pub lock_sqz_on: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedSettings {
    pub requested: Settings,
    pub effective: Settings,
    /// `None` keeps an unavailable ConnectedAgent request visible instead of
    /// substituting another profile. The offline core still remains usable.
    pub effective_profile: Option<OperatingProfile>,
    pub offline_core_usable: bool,
    pub resolutions: Vec<SettingResolution>,
    pub guardrails: Vec<GuardrailResolution>,
}

pub fn resolve_advanced(
    request: AdvancedRequest,
    probe: CapabilityProbe,
    policy: Policy,
) -> ResolvedSettings {
    let requested = request.settings;
    let mut effective = requested.clone();
    let mut resolutions = Vec::with_capacity(10);
    let sqz_requested = requested.sqz || requested.profile == OperatingProfile::CoreSqz;

    let (sqz, sqz_provenance, sqz_status, sqz_locked) =
        if !probe.sqz_available && (sqz_requested || policy.lock_sqz_on) {
            (
                false,
                ResolutionProvenance::CapabilityProbe,
                ResolutionStatus::Unavailable,
                policy.lock_sqz_on,
            )
        } else if policy.lock_sqz_on {
            (
                true,
                ResolutionProvenance::InternalPolicy,
                ResolutionStatus::Locked,
                true,
            )
        } else {
            (
                sqz_requested,
                ResolutionProvenance::Request,
                if sqz_requested {
                    ResolutionStatus::Effective
                } else {
                    ResolutionStatus::Disabled
                },
                false,
            )
        };
    effective.sqz = sqz;
    resolutions.push(SettingResolution {
        feature: Feature::Sqz,
        requested: sqz_requested,
        effective: sqz,
        provenance: sqz_provenance,
        status: sqz_status,
        policy_locked: sqz_locked,
        capability_available: Some(probe.sqz_available),
    });

    resolve_requested(
        &mut resolutions,
        Feature::Diagnostics,
        requested.diagnostics,
        &mut effective.diagnostics,
    );
    resolve_capability(
        &mut resolutions,
        Feature::Voice,
        requested.voice,
        probe.voice_available,
        &mut effective.voice,
    );
    resolve_requested(
        &mut resolutions,
        Feature::StructuredHistory,
        requested.structured_history,
        &mut effective.structured_history,
    );
    resolve_capability(
        &mut resolutions,
        Feature::SavedChat,
        requested.saved_chat,
        probe.saved_chat_available,
        &mut effective.saved_chat,
    );

    let connected_requested = requested.profile == OperatingProfile::ConnectedAgent;
    resolve_capability(
        &mut resolutions,
        Feature::Network,
        requested.network || connected_requested,
        probe.network_available && probe.connected_agent_runtime,
        &mut effective.network,
    );
    resolve_capability(
        &mut resolutions,
        Feature::Auth,
        requested.auth || connected_requested,
        probe.auth_available && probe.connected_agent_runtime,
        &mut effective.auth,
    );
    effective.mutation = false;
    resolutions.push(SettingResolution {
        feature: Feature::Mutation,
        requested: requested.mutation,
        effective: false,
        provenance: ResolutionProvenance::OfflineCore,
        status: if requested.mutation {
            ResolutionStatus::Unavailable
        } else {
            ResolutionStatus::Disabled
        },
        policy_locked: false,
        capability_available: None,
    });

    let effective_profile = match requested.profile {
        OperatingProfile::CoreSqz if !probe.sqz_available => None,
        OperatingProfile::ConnectedAgent
            if !probe.connected_agent_runtime
                || !probe.network_available
                || !probe.auth_available =>
        {
            None
        }
        profile => Some(profile),
    };
    resolutions.push(SettingResolution {
        feature: Feature::ConnectedAgent,
        requested: requested.profile == OperatingProfile::ConnectedAgent,
        effective: effective_profile == Some(OperatingProfile::ConnectedAgent),
        provenance: if requested.profile == OperatingProfile::ConnectedAgent {
            ResolutionProvenance::CapabilityProbe
        } else {
            ResolutionProvenance::Request
        },
        status: if requested.profile != OperatingProfile::ConnectedAgent {
            ResolutionStatus::Disabled
        } else if effective_profile.is_some() {
            ResolutionStatus::Effective
        } else {
            ResolutionStatus::Unavailable
        },
        policy_locked: false,
        capability_available: Some(probe.connected_agent_runtime),
    });
    resolutions.push(SettingResolution {
        feature: Feature::CoreSqz,
        requested: requested.profile == OperatingProfile::CoreSqz,
        effective: effective_profile == Some(OperatingProfile::CoreSqz),
        provenance: if requested.profile == OperatingProfile::CoreSqz {
            ResolutionProvenance::CapabilityProbe
        } else {
            ResolutionProvenance::Request
        },
        status: if requested.profile != OperatingProfile::CoreSqz {
            ResolutionStatus::Disabled
        } else if probe.sqz_available {
            ResolutionStatus::Effective
        } else {
            ResolutionStatus::Unavailable
        },
        policy_locked: false,
        capability_available: Some(probe.sqz_available),
    });

    if effective_profile.is_none() {
        effective.profile = OperatingProfile::OfflineCore;
    }

    let guardrails = guardrail_resolutions(&requested);
    ResolvedSettings {
        requested,
        effective,
        effective_profile,
        offline_core_usable: true,
        resolutions,
        guardrails,
    }
}

fn resolve_requested(
    resolutions: &mut Vec<SettingResolution>,
    feature: Feature,
    requested: bool,
    effective: &mut bool,
) {
    *effective = requested;
    resolutions.push(SettingResolution {
        feature,
        requested,
        effective: requested,
        provenance: ResolutionProvenance::Request,
        status: if requested {
            ResolutionStatus::Effective
        } else {
            ResolutionStatus::Disabled
        },
        policy_locked: false,
        capability_available: None,
    });
}

fn guardrail_resolutions(settings: &Settings) -> Vec<GuardrailResolution> {
    vec![
        GuardrailResolution {
            guardrail: Guardrail::Root,
            requested: match settings.root {
                RootScope::BoundedCurrentRoot => GuardrailValue::BoundedCurrentRoot,
            },
            effective: match settings.root {
                RootScope::BoundedCurrentRoot => GuardrailValue::BoundedCurrentRoot,
            },
            provenance: ResolutionProvenance::Request,
        },
        GuardrailResolution {
            guardrail: Guardrail::Tools,
            requested: match settings.tools {
                ToolPolicy::ReadOnly => GuardrailValue::ReadOnlyTools,
            },
            effective: match settings.tools {
                ToolPolicy::ReadOnly => GuardrailValue::ReadOnlyTools,
            },
            provenance: ResolutionProvenance::Request,
        },
        GuardrailResolution {
            guardrail: Guardrail::Approval,
            requested: match settings.approval {
                ApprovalPolicy::Explicit => GuardrailValue::ExplicitApproval,
            },
            effective: match settings.approval {
                ApprovalPolicy::Explicit => GuardrailValue::ExplicitApproval,
            },
            provenance: ResolutionProvenance::Request,
        },
        GuardrailResolution {
            guardrail: Guardrail::Retention,
            requested: match settings.retention {
                RetentionPolicy::ZeroConversation => GuardrailValue::ZeroConversationRetention,
            },
            effective: match settings.retention {
                RetentionPolicy::ZeroConversation => GuardrailValue::ZeroConversationRetention,
            },
            provenance: ResolutionProvenance::Request,
        },
        GuardrailResolution {
            guardrail: Guardrail::Diagnostics,
            requested: match settings.diagnostic_policy {
                DiagnosticPolicy::Bounded => GuardrailValue::BoundedDiagnostics,
            },
            effective: match settings.diagnostic_policy {
                DiagnosticPolicy::Bounded => GuardrailValue::BoundedDiagnostics,
            },
            provenance: ResolutionProvenance::Request,
        },
    ]
}

fn resolve_capability(
    resolutions: &mut Vec<SettingResolution>,
    feature: Feature,
    requested: bool,
    available: bool,
    effective: &mut bool,
) {
    *effective = requested && available;
    resolutions.push(SettingResolution {
        feature,
        requested,
        effective: *effective,
        provenance: ResolutionProvenance::CapabilityProbe,
        status: if !requested {
            ResolutionStatus::Disabled
        } else if available {
            ResolutionStatus::Effective
        } else {
            ResolutionStatus::Unavailable
        },
        policy_locked: false,
        capability_available: Some(available),
    });
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderTokenAttestation {
    pub actual_tokens: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TokenProjection {
    pub estimated_tokens: u64,
    pub label: &'static str,
    pub actual_provider_tokens: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadinessReceipt {
    pub requested: Settings,
    pub effective: Settings,
    pub effective_profile: Option<OperatingProfile>,
    pub facts: Vec<SettingResolution>,
    pub guardrails: Vec<GuardrailResolution>,
    pub locked: Vec<Feature>,
    pub unavailable: Vec<Feature>,
    pub retention: &'static str,
    pub data_flow: &'static str,
    pub token_projection: TokenProjection,
}

pub fn readiness_receipt(
    resolved: &ResolvedSettings,
    attestation: Option<ProviderTokenAttestation>,
) -> ReadinessReceipt {
    let locked = resolved
        .resolutions
        .iter()
        .filter(|item| item.status == ResolutionStatus::Locked || item.policy_locked)
        .map(|item| item.feature)
        .collect();
    let unavailable = resolved
        .resolutions
        .iter()
        .filter(|item| item.status == ResolutionStatus::Unavailable)
        .map(|item| item.feature)
        .collect();
    ReadinessReceipt {
        requested: resolved.requested.clone(),
        effective: resolved.effective.clone(),
        effective_profile: resolved.effective_profile,
        facts: resolved.resolutions.clone(),
        guardrails: resolved.guardrails.clone(),
        locked,
        unavailable,
        retention: "No credentials or chat content retained.",
        data_flow: "Local deterministic settings and bounded practice metadata only.",
        token_projection: TokenProjection {
            estimated_tokens: 0,
            label: "estimated",
            actual_provider_tokens: attestation.map(|value| value.actual_tokens),
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PracticeReceipt {
    pub accepted: bool,
    pub query_bytes: usize,
    pub limit: usize,
    pub network_used: bool,
    pub auth_used: bool,
    pub audio_used: bool,
    pub chat_storage_used: bool,
    pub mutation_used: bool,
}

pub fn local_practice_query(query: &str) -> PracticeReceipt {
    PracticeReceipt {
        accepted: !query.is_empty() && query.len() <= PRACTICE_QUERY_LIMIT,
        query_bytes: query.len().min(PRACTICE_QUERY_LIMIT),
        limit: PRACTICE_QUERY_LIMIT,
        network_used: false,
        auth_used: false,
        audio_used: false,
        chat_storage_used: false,
        mutation_used: false,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeferredOnboarding {
    pub settings: Settings,
    pub completed: bool,
    pub should_nag: bool,
}

pub fn cancel_or_decide_later() -> DeferredOnboarding {
    DeferredOnboarding {
        settings: quick_safe_config(),
        completed: false,
        should_nag: false,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepairProposal {
    pub preserved: Settings,
    pub failed_capabilities: Vec<Feature>,
}

pub fn repair_proposal(current: &Settings, probe: CapabilityProbe) -> RepairProposal {
    let mut failed_capabilities = Vec::new();
    if (current.sqz || current.profile == OperatingProfile::CoreSqz) && !probe.sqz_available {
        failed_capabilities.push(Feature::Sqz);
    }
    if current.voice && !probe.voice_available {
        failed_capabilities.push(Feature::Voice);
    }
    if current.saved_chat && !probe.saved_chat_available {
        failed_capabilities.push(Feature::SavedChat);
    }
    if current.profile == OperatingProfile::ConnectedAgent && !probe.connected_agent_runtime {
        failed_capabilities.push(Feature::ConnectedAgent);
    }
    RepairProposal {
        preserved: current.clone(),
        failed_capabilities,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionReceipt {
    pub version: u32,
    pub completed: bool,
    pub should_nag: bool,
    pub bytes_written: usize,
}

pub fn apply_settings(path: &Path, settings: &Settings) -> io::Result<CompletionReceipt> {
    apply_settings_using(path, settings, |from, to| fs::rename(from, to))
}

pub fn apply_settings_using<F>(
    path: &Path,
    settings: &Settings,
    replace: F,
) -> io::Result<CompletionReceipt>
where
    F: Fn(&Path, &Path) -> io::Result<()>,
{
    let bytes = encode(settings);
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("settings");
    let mut wrote = false;
    for suffix in 0..64 {
        let candidate = directory.join(format!(".{name}.{}.{}.tmp", std::process::id(), suffix));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                let outcome = (|| {
                    file.write_all(&bytes)?;
                    file.sync_all()?;
                    replace(&candidate, path)
                })();
                if outcome.is_err() {
                    let _ = fs::remove_file(&candidate);
                }
                outcome?;
                wrote = true;
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    if !wrote {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "settings temporary name exhausted",
        ));
    }
    Ok(CompletionReceipt {
        version: SETTINGS_VERSION,
        completed: true,
        should_nag: false,
        bytes_written: bytes.len(),
    })
}

pub fn onboarding_complete(path: &Path) -> io::Result<bool> {
    Ok(load_settings(path)?.is_some())
}

pub fn load_settings(path: &Path) -> io::Result<Option<Settings>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(decode(&bytes))
}

fn decode(bytes: &[u8]) -> Option<Settings> {
    let contents = match std::str::from_utf8(bytes) {
        Ok(contents) => contents,
        _ => return None,
    };
    if bytes.contains(&b'\r') || !contents.ends_with('\n') {
        return None;
    }
    let lines: Vec<&str> = contents.lines().collect();
    if lines.len() != 17 {
        return None;
    }
    let legacy_settings = lines[0] == "version=2";
    if (!legacy_settings && lines[0] != format!("version={SETTINGS_VERSION}"))
        || lines[1] != "completed=true"
    {
        return None;
    }
    let profile = match lines[2] {
        "profile=offline-core" => OperatingProfile::OfflineCore,
        "profile=core-sqz" => OperatingProfile::CoreSqz,
        "profile=connected-agent" => OperatingProfile::ConnectedAgent,
        _ => return None,
    };
    let boolean_field_prefixes = [
        "sqz=",
        "diagnostics=",
        "voice=",
        "structured_history=",
        "saved_chat=",
        "network=",
        "auth=",
        "mutation=",
    ];
    let mut values = [false; 8];
    for (index, prefix) in boolean_field_prefixes.iter().enumerate() {
        let field = lines[3 + index];
        if !field.starts_with(prefix) {
            return None;
        }
        let value = &field[prefix.len()..];
        values[index] = match value {
            "true" => true,
            "false" => false,
            _ => return None,
        };
    }
    let connected_agent_task_token_ceiling =
        match lines[11].strip_prefix("connected_agent_task_token_ceiling=")? {
            "unset" if !legacy_settings => None,
            // Version 2 persisted the former default indistinguishably from a
            // user choice, so retire that value instead of silently applying it.
            "8500" if legacy_settings => None,
            value => Some(value.parse::<u32>().ok().filter(|value| *value > 0)?),
        };
    if lines[12] != "root=bounded-current-root"
        || lines[13] != "tools=read-only"
        || lines[14] != "approval=explicit"
        || lines[15] != "retention=zero-conversation"
        || lines[16] != "diagnostic_policy=bounded"
    {
        return None;
    }
    Some(Settings {
        profile,
        sqz: values[0],
        diagnostics: values[1],
        voice: values[2],
        structured_history: values[3],
        saved_chat: values[4],
        network: values[5],
        auth: values[6],
        mutation: values[7],
        connected_agent_task_token_ceiling,
        root: RootScope::BoundedCurrentRoot,
        tools: ToolPolicy::ReadOnly,
        approval: ApprovalPolicy::Explicit,
        retention: RetentionPolicy::ZeroConversation,
        diagnostic_policy: DiagnosticPolicy::Bounded,
    })
}

fn encode(settings: &Settings) -> Vec<u8> {
    format!(
        "version={SETTINGS_VERSION}\ncompleted=true\nprofile={}\nsqz={}\ndiagnostics={}\nvoice={}\nstructured_history={}\nsaved_chat={}\nnetwork={}\nauth={}\nmutation={}\nconnected_agent_task_token_ceiling={}\nroot=bounded-current-root\ntools=read-only\napproval=explicit\nretention=zero-conversation\ndiagnostic_policy=bounded\n",
        profile_name(settings.profile), settings.sqz, settings.diagnostics, settings.voice,
        settings.structured_history, settings.saved_chat, settings.network, settings.auth, settings.mutation,
        settings.connected_agent_task_token_ceiling.map_or_else(|| "unset".to_owned(), |value| value.to_string()),
    ).into_bytes()
}

fn profile_name(profile: OperatingProfile) -> &'static str {
    match profile {
        OperatingProfile::OfflineCore => "offline-core",
        OperatingProfile::CoreSqz => "core-sqz",
        OperatingProfile::ConnectedAgent => "connected-agent",
    }
}
