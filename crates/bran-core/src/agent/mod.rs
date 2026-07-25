//! Provider-neutral connected-agent contracts.

pub mod coordinator;
pub mod delegate;
pub mod receipt;
pub mod result_store;
pub mod runtime;
pub mod synthetic;

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReasoningLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReasoningLevelError {
    _p: (),
}

impl ReasoningLevel {
    pub fn parse(s: &str) -> Result<Self, ReasoningLevelError> {
        match s {
            "off" => Ok(Self::Off),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            _ => Err(ReasoningLevelError { _p: () }),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match *self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl std::fmt::Display for ReasoningLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPolicy {
    allow: BTreeSet<String>,
    deny: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPolicyError {
    TooMany,
    InvalidName,
    Duplicate,
    Conflict,
}

fn is_valid_tool_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let len = bytes.len();
    (1..=32).contains(&len)
        && bytes
            .iter()
            .all(|&b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

impl ToolPolicy {
    pub fn new(
        allow: impl IntoIterator<Item = impl AsRef<str>>,
        deny: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Self, ToolPolicyError> {
        let mut allow_set = BTreeSet::<String>::new();
        for item in allow {
            let name = item.as_ref();
            if !is_valid_tool_name(name) {
                return Err(ToolPolicyError::InvalidName);
            }
            if !allow_set.insert(name.to_string()) {
                return Err(ToolPolicyError::Duplicate);
            }
            if allow_set.len() > 32 {
                return Err(ToolPolicyError::TooMany);
            }
        }
        let mut deny_set = BTreeSet::<String>::new();
        for item in deny {
            let name = item.as_ref();
            if !is_valid_tool_name(name) {
                return Err(ToolPolicyError::InvalidName);
            }
            if !deny_set.insert(name.to_string()) {
                return Err(ToolPolicyError::Duplicate);
            }
            if deny_set.len() > 32 {
                return Err(ToolPolicyError::TooMany);
            }
        }
        for name in &allow_set {
            if deny_set.contains(name) {
                return Err(ToolPolicyError::Conflict);
            }
        }
        Ok(Self {
            allow: allow_set,
            deny: deny_set,
        })
    }

    pub fn read_only_default() -> Self {
        Self::new(["read", "search"], ["write", "edit", "shell", "network"]).unwrap()
    }

    pub fn allows(&self, name: &str) -> bool {
        self.allow.contains(name) && !self.deny.contains(name)
    }

    pub fn allowed(&self) -> impl ExactSizeIterator<Item = &str> {
        self.allow.iter().map(String::as_str)
    }

    pub fn denied(&self) -> impl ExactSizeIterator<Item = &str> {
        self.deny.iter().map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    name: String,
    provider: String,
    model: String,
    account_handle: String,
    default_reasoning_level: ReasoningLevel,
    tool_policy: ToolPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentProfileError {
    InvalidIdentity,
}

fn is_valid_identity(s: &str) -> bool {
    let bytes = s.as_bytes();
    let len = bytes.len();
    (1..=64).contains(&len)
        && bytes
            .iter()
            .all(|&b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

impl AgentProfile {
    pub fn new(
        name: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        account_handle: impl Into<String>,
        default_reasoning_level: ReasoningLevel,
        tool_policy: ToolPolicy,
    ) -> Result<Self, AgentProfileError> {
        let name = name.into();
        let provider = provider.into();
        let model = model.into();
        let account_handle = account_handle.into();
        if !is_valid_identity(&name)
            || !is_valid_identity(&provider)
            || !is_valid_identity(&model)
            || !is_valid_identity(&account_handle)
            || !crate::adapters::is_public_dlp_safe(&account_handle)
        {
            return Err(AgentProfileError::InvalidIdentity);
        }
        Ok(Self {
            name,
            provider,
            model,
            account_handle,
            default_reasoning_level,
            tool_policy,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn account_handle(&self) -> &str {
        &self.account_handle
    }

    pub fn default_reasoning_level(&self) -> ReasoningLevel {
        self.default_reasoning_level
    }

    pub fn tool_policy(&self) -> &ToolPolicy {
        &self.tool_policy
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryError {
    InvalidIdentity,
    DuplicateProvider,
    DuplicateModel,
    UnknownProvider,
    UnknownModel,
    DuplicateProfile,
    UnknownProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderRegistry {
    providers: BTreeSet<String>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, identity: impl Into<String>) -> Result<(), RegistryError> {
        let id = identity.into();
        if !is_valid_identity(&id) {
            return Err(RegistryError::InvalidIdentity);
        }
        if !self.providers.insert(id) {
            return Err(RegistryError::DuplicateProvider);
        }
        Ok(())
    }

    pub fn contains(&self, identity: &str) -> bool {
        self.providers.contains(identity)
    }

    pub fn require(&self, identity: &str) -> Result<(), RegistryError> {
        if self.contains(identity) {
            Ok(())
        } else {
            Err(RegistryError::UnknownProvider)
        }
    }

    pub fn names(&self) -> Vec<&str> {
        self.providers.iter().map(|s| s.as_str()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelRegistry {
    models: BTreeMap<String, BTreeSet<String>>,
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<(), RegistryError> {
        let provider = provider.into();
        let model = model.into();
        if !is_valid_identity(&provider) || !is_valid_identity(&model) {
            return Err(RegistryError::InvalidIdentity);
        }
        if !self.models.entry(provider).or_default().insert(model) {
            return Err(RegistryError::DuplicateModel);
        }
        Ok(())
    }

    pub fn contains(&self, model: &str) -> bool {
        self.models.values().any(|models| models.contains(model))
    }

    pub fn contains_for_provider(&self, provider: &str, model: &str) -> bool {
        self.models
            .get(provider)
            .is_some_and(|models| models.contains(model))
    }

    pub fn require_for_provider(&self, provider: &str, model: &str) -> Result<(), RegistryError> {
        if self.contains_for_provider(provider, model) {
            Ok(())
        } else {
            Err(RegistryError::UnknownModel)
        }
    }

    pub fn names(&self) -> Vec<&str> {
        self.models
            .values()
            .flat_map(|models| models.iter().map(String::as_str))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfileRegistry {
    provider_registry: ProviderRegistry,
    model_registry: ModelRegistry,
    profiles: BTreeMap<String, AgentProfile>,
}

impl AgentProfileRegistry {
    pub fn new(provider_registry: ProviderRegistry, model_registry: ModelRegistry) -> Self {
        Self {
            provider_registry,
            model_registry,
            profiles: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, profile: AgentProfile) -> Result<(), RegistryError> {
        self.provider_registry.require(profile.provider())?;
        self.model_registry
            .require_for_provider(profile.provider(), profile.model())?;
        if self.profiles.contains_key(profile.name()) {
            return Err(RegistryError::DuplicateProfile);
        }
        self.profiles.insert(profile.name().to_string(), profile);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<&AgentProfile, RegistryError> {
        self.profiles.get(name).ok_or(RegistryError::UnknownProfile)
    }

    pub fn profiles(&self) -> Vec<&AgentProfile> {
        self.profiles.values().collect()
    }

    pub fn provider_registry(&self) -> &ProviderRegistry {
        &self.provider_registry
    }

    pub fn model_registry(&self) -> &ModelRegistry {
        &self.model_registry
    }
}

pub fn synthetic_builtin_profiles() -> AgentProfileRegistry {
    let mut pr = ProviderRegistry::new();
    pr.register("fixture-provider").expect("");
    let mut mr = ModelRegistry::new();
    mr.register("fixture-provider", "fixture-sol").expect("");
    mr.register("fixture-provider", "fixture-luna").expect("");
    let mut apr = AgentProfileRegistry::new(pr, mr);
    let sol = AgentProfile::new(
        "sol",
        "fixture-provider",
        "fixture-sol",
        "sol-default",
        ReasoningLevel::High,
        ToolPolicy::read_only_default(),
    )
    .expect("");
    apr.register(sol).expect("");
    let luna = AgentProfile::new(
        "luna",
        "fixture-provider",
        "fixture-luna",
        "luna-default",
        ReasoningLevel::Low,
        ToolPolicy::read_only_default(),
    )
    .expect("");
    apr.register(luna).expect("");
    apr
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::runtime::ProviderPort;

    fn run_sqz_fixture() -> bool {
        if std::env::var_os("BRAN_P3_SQZ_FIXTURE").is_none() {
            return false;
        }
        use std::io::{Read, Write};
        let mut payload = Vec::new();
        std::io::stdin().read_to_end(&mut payload).unwrap();
        if payload == b"fixture-sqz-timeout" {
            std::thread::sleep(std::time::Duration::from_millis(50));
            return true;
        }
        let mut output = std::io::stdout();
        writeln!(output, "{}", payload.len()).unwrap();
        output.write_all(&payload).unwrap();
        output.write_all(b"\n").unwrap();
        output.flush().unwrap();
        true
    }

    #[cfg(test)]
    fn run_external_host_fixture() -> bool {
        if std::env::var_os("BRAN_P3_EXTERNAL_HOST_FIXTURE").is_none() {
            return false;
        }
        use std::io::{BufRead, BufReader, Read, Write};
        let mut input = BufReader::new(std::io::stdin());
        let read = |input: &mut BufReader<std::io::Stdin>| -> Vec<u8> {
            let mut line = String::new();
            input.read_line(&mut line).unwrap();
            let mut bytes = vec![0; line.trim().parse::<usize>().unwrap()];
            input.read_exact(&mut bytes).unwrap();
            let mut newline = [0];
            input.read_exact(&mut newline).unwrap();
            bytes
        };
        let write = |payload: Vec<u8>| {
            let mut output = std::io::stdout();
            writeln!(output, "{}", payload.len()).unwrap();
            output.write_all(&payload).unwrap();
            output.write_all(b"\n").unwrap();
            output.flush().unwrap();
        };
        let first = read(&mut input);
        let digest = crate::agent::result_store::ResultId::sha256(&first)
            .value()
            .to_owned();
        let field = |payload: &[u8], wanted: &str| -> String {
            let mut rest = &payload[3..];
            while !rest.is_empty() {
                let end = rest.iter().position(|b| *b == b'\n').unwrap();
                let key = std::str::from_utf8(&rest[..end]).unwrap();
                rest = &rest[end + 1..];
                let end = rest.iter().position(|b| *b == b'\n').unwrap();
                let len = std::str::from_utf8(&rest[..end])
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                rest = &rest[end + 1..];
                let value = &rest[..len];
                rest = &rest[len..];
                if key == wanted {
                    return std::str::from_utf8(value).unwrap().to_owned();
                }
            }
            panic!("missing fixture field")
        };
        let prompt = field(&first, "prompt");
        if prompt == "fixture-timeout" {
            std::thread::sleep(std::time::Duration::from_millis(50));
            return true;
        }
        let provider = field(&first, "provider");
        let model = field(&first, "model");
        let reasoning = field(&first, "reasoning");
        let policy = field(&first, "tool_policy");
        let account = field(&first, "account_ref");
        let max = field(&first, "max_output_tokens");
        let ceiling = field(&first, "hard_total_token_ceiling");
        let exact = if prompt == "fixture-over-budget" {
            (ceiling.parse::<usize>().unwrap() + 1).to_string()
        } else if max == "unavailable" {
            "unavailable".to_owned()
        } else {
            (10 + max.parse::<usize>().unwrap()).to_string()
        };
        let frame = |stage: &str, status: &str, extra: Vec<(&str, String)>| {
            let preflight_id = if stage == "result" && prompt == "fixture-binding-mismatch" {
                "wrong-pf"
            } else {
                "fixture-pf-1"
            };
            let mut fields = vec![
                ("stage", stage.to_owned()),
                ("status", status.to_owned()),
                ("provider", provider.clone()),
                ("model", model.clone()),
                ("reasoning", reasoning.clone()),
                ("tool_policy", policy.clone()),
                (
                    "account_ref",
                    if stage == "result" && prompt == "fixture-account-mismatch" {
                        "wrong-account".to_owned()
                    } else {
                        account.clone()
                    },
                ),
                ("request_digest", digest.clone()),
                ("preflight_id", preflight_id.to_owned()),
                ("input_tokens", "10".to_owned()),
                ("max_output_tokens", max.clone()),
                ("exact_complete_tokens", exact.clone()),
                ("hard_total_token_ceiling", ceiling.clone()),
            ];
            fields.extend(extra);
            let mut out = b"v1\n".to_vec();
            for (k, v) in fields {
                out.extend_from_slice(k.as_bytes());
                out.push(b'\n');
                out.extend_from_slice(v.len().to_string().as_bytes());
                out.push(b'\n');
                out.extend_from_slice(v.as_bytes());
            }
            write(out);
        };
        frame("preflight-result", "supported", vec![]);
        if prompt == "fixture-over-budget" {
            return true;
        }
        let _ = read(&mut input);
        let answer = if prompt == "fixture-output-bound" {
            "x".repeat(1025)
        } else {
            "fixture answer".to_owned()
        };
        frame(
            "result",
            "complete",
            vec![
                ("actual_input_tokens", "10".to_owned()),
                ("actual_output_tokens", "7".to_owned()),
                ("answer", answer),
                ("citation", "fixture citation".to_owned()),
                ("citation", "fixture second citation".to_owned()),
                ("provider_run_id", "fixture-run-1".to_owned()),
            ],
        );
        true
    }

    struct ExternalFixtureAuth(crate::adapters::SafeAccountReference);

    impl crate::agent::runtime::AuthStore for ExternalFixtureAuth {
        type Credential = crate::adapters::SafeAccountReference;

        fn resolve(
            &self,
            account_handle: &str,
        ) -> Result<Self::Credential, crate::agent::runtime::AuthError> {
            if account_handle == "sol-default" {
                Ok(self.0.clone())
            } else {
                Err(crate::agent::runtime::AuthError::Missing)
            }
        }
    }

    #[test]
    fn p3_agent_profile_contract() {
        if run_sqz_fixture() {
            return;
        }
        if run_external_host_fixture() {
            return;
        }
        // ReasoningLevel: all seven exact lowercase parses + canonical strings, plus invalid uppercase/unknown
        assert_eq!(ReasoningLevel::parse("off").unwrap(), ReasoningLevel::Off);
        assert_eq!(ReasoningLevel::parse("off").unwrap().as_str(), "off");
        assert_eq!(
            ReasoningLevel::parse("minimal").unwrap(),
            ReasoningLevel::Minimal
        );
        assert_eq!(
            ReasoningLevel::parse("minimal").unwrap().as_str(),
            "minimal"
        );
        assert_eq!(ReasoningLevel::parse("low").unwrap(), ReasoningLevel::Low);
        assert_eq!(ReasoningLevel::parse("low").unwrap().as_str(), "low");
        assert_eq!(
            ReasoningLevel::parse("medium").unwrap(),
            ReasoningLevel::Medium
        );
        assert_eq!(ReasoningLevel::parse("medium").unwrap().as_str(), "medium");
        assert_eq!(ReasoningLevel::parse("high").unwrap(), ReasoningLevel::High);
        assert_eq!(ReasoningLevel::parse("high").unwrap().as_str(), "high");
        assert_eq!(
            ReasoningLevel::parse("xhigh").unwrap(),
            ReasoningLevel::Xhigh
        );
        assert_eq!(ReasoningLevel::parse("xhigh").unwrap().as_str(), "xhigh");
        assert_eq!(ReasoningLevel::parse("max").unwrap(), ReasoningLevel::Max);
        assert_eq!(ReasoningLevel::parse("max").unwrap().as_str(), "max");
        assert_eq!(
            ReasoningLevel::parse("High"),
            Err(ReasoningLevelError { _p: () })
        );
        assert_eq!(
            ReasoningLevel::parse("foo"),
            Err(ReasoningLevelError { _p: () })
        );

        // ToolPolicy read-only default allows read/search, rejects write/edit/shell/network
        let ro = ToolPolicy::read_only_default();
        assert!(ro.allows("read"));
        assert!(ro.allows("search"));
        assert!(!ro.allows("write"));
        assert!(!ro.allows("edit"));
        assert!(!ro.allows("shell"));
        assert!(!ro.allows("network"));
        assert_eq!(ro.allowed().collect::<Vec<_>>(), vec!["read", "search"]);
        assert_eq!(
            ro.denied().collect::<Vec<_>>(),
            vec!["edit", "network", "shell", "write"]
        );

        let connected = synthetic::connected_receipt();
        let connected_json = connected.to_json();
        let unattested = synthetic::connected_unattested_receipt();
        assert_eq!(connected.outcome(), unattested.outcome());
        assert!(matches!(
            unattested.effective().profile(),
            runtime::Attestation::Unavailable
        ));
        assert!(connected.no_session());
        assert!(connected_json.contains("\"schema_version\":\"bran-agent-receipt-v1\""));
        assert!(connected_json.contains("\"fidelity_status\":\"passed\""));
        assert!(connected_json.contains("\"dlp_status\":\"passed\""));
        assert!(connected_json.contains("\"provider_run_id\":{\"state\":\"attested\""));
        assert!(!connected_json.contains("synthetic connected request"));
        let luna_request = delegate::DelegationRequest::new(
            "luna",
            "synthetic luna request",
            delegate::DelegationOptions {
                no_session: true,
                provider_override: Some("fixture-provider".to_string()),
                model_override: Some("fixture-sol".to_string()),
                reasoning_override: Some(ReasoningLevel::Medium),
                ..delegate::DelegationOptions::new()
            },
        )
        .unwrap();
        let luna = synthetic::connected_receipt_for(&luna_request, true);
        assert!(luna.no_session());
        assert!(matches!(
            luna.effective().profile(),
            runtime::Attestation::Attested(profile) if profile.name() == "luna"
        ));
        assert!(matches!(
            luna.effective().model(),
            runtime::Attestation::Attested(model) if model == "fixture-sol"
        ));

        // invalid name, duplicate, and conflict
        assert_eq!(
            ToolPolicy::new(vec!["Read"] as Vec<&str>, vec![] as Vec<&str>),
            Err(ToolPolicyError::InvalidName)
        );
        assert_eq!(
            ToolPolicy::new(vec!["a", "a"] as Vec<&str>, vec![] as Vec<&str>),
            Err(ToolPolicyError::Duplicate)
        );
        assert_eq!(
            ToolPolicy::new(vec!["a"] as Vec<&str>, vec!["a"] as Vec<&str>),
            Err(ToolPolicyError::Conflict)
        );

        // synthetic_builtin_profiles: deterministic names luna then sol
        let apr = synthetic_builtin_profiles();
        let ps = apr.profiles();
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].name(), "luna");
        assert_eq!(ps[1].name(), "sol");

        // exact provider/model/account/default reasoning + read/search allowed, mutation denied
        let luna_p = apr.get("luna").unwrap();
        assert_eq!(luna_p.provider(), "fixture-provider");
        assert_eq!(luna_p.model(), "fixture-luna");
        assert_eq!(luna_p.account_handle(), "luna-default");
        assert_eq!(luna_p.default_reasoning_level(), ReasoningLevel::Low);
        let lp = luna_p.tool_policy();
        assert!(lp.allows("read"));
        assert!(lp.allows("search"));
        assert!(!lp.allows("write"));
        assert!(!lp.allows("edit"));
        assert!(!lp.allows("shell"));
        assert!(!lp.allows("network"));

        let sol_p = apr.get("sol").unwrap();
        assert_eq!(sol_p.provider(), "fixture-provider");
        assert_eq!(sol_p.model(), "fixture-sol");
        assert_eq!(sol_p.account_handle(), "sol-default");
        assert_eq!(sol_p.default_reasoning_level(), ReasoningLevel::High);
        let sp = sol_p.tool_policy();
        assert!(sp.allows("read"));
        assert!(sp.allows("search"));
        assert!(!sp.allows("write"));
        assert!(!sp.allows("edit"));
        assert!(!sp.allows("shell"));
        assert!(!sp.allows("network"));

        // ProviderRegistry rejects invalid and duplicate, returns typed unknown
        let mut pr = ProviderRegistry::new();
        assert_eq!(pr.register("bad name"), Err(RegistryError::InvalidIdentity));
        pr.register("good-p").unwrap();
        assert_eq!(pr.register("good-p"), Err(RegistryError::DuplicateProvider));
        assert_eq!(pr.require("nope"), Err(RegistryError::UnknownProvider));

        // ModelRegistry rejects invalid and duplicate pairs, returns typed unknown
        let mut mr = ModelRegistry::new();
        assert_eq!(
            mr.register("good-p", "bad@name"),
            Err(RegistryError::InvalidIdentity)
        );
        mr.register("good-p", "good-m").unwrap();
        assert_eq!(
            mr.register("good-p", "good-m"),
            Err(RegistryError::DuplicateModel)
        );
        assert_eq!(
            mr.require_for_provider("good-p", "nope"),
            Err(RegistryError::UnknownModel)
        );

        // AgentProfileRegistry rejects unknown provider, unknown model, duplicate profile, unknown lookup (no silent fallback)
        let mut pr1 = ProviderRegistry::new();
        pr1.register("prov-x").unwrap();
        let mut mr1 = ModelRegistry::new();
        mr1.register("prov-x", "mod-y").unwrap();
        let mut apr1 = AgentProfileRegistry::new(pr1, mr1);
        let bad_prov_p = AgentProfile::new(
            "p1",
            "unknown-prov",
            "mod-y",
            "a",
            ReasoningLevel::Off,
            ToolPolicy::read_only_default(),
        )
        .unwrap();
        assert_eq!(
            apr1.register(bad_prov_p),
            Err(RegistryError::UnknownProvider)
        );

        let mut pr2 = ProviderRegistry::new();
        pr2.register("prov-x").unwrap();
        let mut mr2 = ModelRegistry::new();
        mr2.register("prov-x", "mod-y").unwrap();
        let mut apr2 = AgentProfileRegistry::new(pr2, mr2);
        let bad_mod_p = AgentProfile::new(
            "p2",
            "prov-x",
            "unknown-mod",
            "a",
            ReasoningLevel::Off,
            ToolPolicy::read_only_default(),
        )
        .unwrap();
        assert_eq!(apr2.register(bad_mod_p), Err(RegistryError::UnknownModel));

        let mut pr3 = ProviderRegistry::new();
        pr3.register("prov-x").unwrap();
        let mut mr3 = ModelRegistry::new();
        mr3.register("prov-x", "mod-y").unwrap();
        let mut apr3 = AgentProfileRegistry::new(pr3, mr3);
        let dup1 = AgentProfile::new(
            "dup-p",
            "prov-x",
            "mod-y",
            "a1",
            ReasoningLevel::Off,
            ToolPolicy::read_only_default(),
        )
        .unwrap();
        apr3.register(dup1).unwrap();
        let dup2 = AgentProfile::new(
            "dup-p",
            "prov-x",
            "mod-y",
            "a2",
            ReasoningLevel::High,
            ToolPolicy::read_only_default(),
        )
        .unwrap();
        assert_eq!(apr3.register(dup2), Err(RegistryError::DuplicateProfile));

        let mut pr4 = ProviderRegistry::new();
        pr4.register("prov-x").unwrap();
        let mut mr4 = ModelRegistry::new();
        mr4.register("prov-x", "mod-y").unwrap();
        let mut apr4 = AgentProfileRegistry::new(pr4, mr4);
        let okp = AgentProfile::new(
            "ok-p",
            "prov-x",
            "mod-y",
            "a",
            ReasoningLevel::Low,
            ToolPolicy::read_only_default(),
        )
        .unwrap();
        apr4.register(okp).unwrap();
        assert_eq!(apr4.get("no-such"), Err(RegistryError::UnknownProfile));

        let mut pr5 = ProviderRegistry::new();
        pr5.register("prov-x").unwrap();
        pr5.register("prov-z").unwrap();
        let mut mr5 = ModelRegistry::new();
        mr5.register("prov-z", "mod-y").unwrap();
        let mut apr5 = AgentProfileRegistry::new(pr5, mr5);
        let mismatched_pair = AgentProfile::new(
            "p5",
            "prov-x",
            "mod-y",
            "a",
            ReasoningLevel::Low,
            ToolPolicy::read_only_default(),
        )
        .unwrap();
        assert_eq!(
            apr5.register(mismatched_pair),
            Err(RegistryError::UnknownModel)
        );

        // Folded external-host assertions use this test binary as a narrowly test-only fixture.
        let host_req = crate::agent::runtime::ProviderRequest::new(
            "fixture-provider",
            "fixture-sol",
            ReasoningLevel::Medium,
            ToolPolicy::read_only_default(),
            "host v1 preflight+execute",
            1024,
            0,
        )
        .unwrap();
        assert_eq!(host_req.hard_total_token_ceiling(), None);
        assert_eq!(host_req.max_output_tokens(), None);
        let lower_budget = host_req.clone().with_token_budget(110, 100).unwrap();
        assert_eq!(lower_budget.hard_total_token_ceiling(), Some(110));
        assert_eq!(lower_budget.max_output_tokens(), Some(100));
        let higher_budget = host_req.clone().with_token_budget(12345, 100).unwrap();
        assert_eq!(higher_budget.hard_total_token_ceiling(), Some(12345));
        assert_eq!(higher_budget.max_output_tokens(), Some(100));
        assert!(host_req.clone().with_token_budget(0, 1).is_err());
        assert!(host_req.clone().with_token_budget(100, 101).is_err());
        let account =
            crate::adapters::provider::SafeAccountReference::new("acct-safe-ref").unwrap();
        let fixture = crate::adapters::ExternalHostAdapter::current_test_fixture(
            std::time::Duration::from_secs(2),
        )
        .unwrap();
        let host_out = fixture
            .invoke(&host_req, &account)
            .expect("fixture-host roundtrip");
        assert_eq!(host_out.answer(), "fixture answer");
        assert_eq!(host_out.provider_run_id(), Some("fixture-run-1"));
        assert_eq!(host_out.effective_model(), Some("fixture-sol"));
        assert_eq!(host_out.effective_reasoning(), Some("medium"));
        assert_eq!(host_out.actual_input_tokens(), Some(10));
        assert_eq!(host_out.actual_output_tokens(), Some(7));
        assert!(host_out
            .citations()
            .contains(&"fixture citation".to_string()));
        assert!(host_out
            .citations()
            .contains(&"fixture second citation".to_string()));
        assert_eq!(host_out.enforced_token_ceiling(), None);
        assert!(fixture.invoke(&higher_budget, &account).is_ok());
        let request = |prompt| {
            crate::agent::runtime::ProviderRequest::new(
                "fixture-provider",
                "fixture-sol",
                ReasoningLevel::Medium,
                ToolPolicy::read_only_default(),
                prompt,
                1024,
                0,
            )
            .and_then(|r| r.with_token_budget(12_345, 100))
            .unwrap()
        };
        assert!(
            fixture
                .invoke(&request("fixture-over-budget"), &account)
                .is_err(),
            "rejected preflight sends no execute"
        );
        assert!(fixture
            .invoke(&request("fixture-binding-mismatch"), &account)
            .is_err());
        assert!(matches!(
            fixture.invoke(&request("fixture-account-mismatch"), &account),
            Err(crate::agent::runtime::ProviderError::InvalidOutput)
        ));
        assert!(fixture
            .invoke(&request("fixture-output-bound"), &account)
            .is_err());
        let to_adapter = crate::adapters::ExternalHostAdapter::current_test_fixture(
            std::time::Duration::from_millis(5),
        )
        .unwrap();
        let to_res = to_adapter.invoke(&request("fixture-timeout"), &account);
        assert!(matches!(
            to_res,
            Err(crate::agent::runtime::ProviderError::Timeout)
        ));

        let sqz_fixture = crate::adapters::ExternalSqzPort::current_test_fixture(
            std::time::Duration::from_secs(2),
        )
        .unwrap();
        let sqz_output = crate::adapters::SqzPort::compress(
            &sqz_fixture,
            "fixture SQZ payload preserved exactly",
        )
        .unwrap();
        assert_eq!(sqz_output.payload, "fixture SQZ payload preserved exactly");
        assert_eq!(
            sqz_output.identity,
            crate::adapters::SqzIdentity::approved()
        );
        assert!(crate::adapters::ExternalSqzPort::new(std::env::current_exe().unwrap()).is_err());
        let sqz_timeout = crate::adapters::ExternalSqzPort::current_test_fixture(
            std::time::Duration::from_millis(5),
        )
        .unwrap();
        assert!(matches!(
            crate::adapters::SqzPort::compress(&sqz_timeout, "fixture-sqz-timeout"),
            Err(crate::adapters::SqzPortError {
                code: crate::adapters::SqzPortErrorCode::Timeout
            })
        ));

        let external_request = crate::agent::delegate::DelegationRequest::new(
            "sol",
            "real external SQZ and provider fixture bridge",
            crate::agent::delegate::DelegationOptions {
                no_session: true,
                ..crate::agent::delegate::DelegationOptions::new()
            },
        )
        .unwrap();
        let external_provider = crate::adapters::ExternalHostAdapter::current_test_fixture(
            std::time::Duration::from_secs(2),
        )
        .unwrap()
        .with_profile("sol")
        .unwrap();
        let external_sqz = crate::agent::coordinator::AgentSqzAdapter::new(
            crate::adapters::ExternalSqzPort::current_test_fixture(std::time::Duration::from_secs(
                2,
            ))
            .unwrap(),
            crate::adapters::SqzPolicy::PublicOn,
            external_request.max_output_bytes(),
        );
        let auth =
            ExternalFixtureAuth(crate::adapters::SafeAccountReference::new("sol-default").unwrap());
        let mut results = crate::agent::result_store::MemoryResultStore::new(
            8,
            2 * 1024 * 1024,
            1024 * 1024,
            u64::MAX,
        )
        .unwrap();
        let external_receipt = crate::agent::coordinator::AgentRuntime::new(
            crate::agent::coordinator::AgentRuntimeConfig::new(true, 8).unwrap(),
        )
        .invoke(
            &external_request,
            crate::agent::coordinator::AgentRuntimeAuthority::new(false, true, false),
            &synthetic_builtin_profiles(),
            || {
                crate::agent::coordinator::RuntimePorts::new(
                    &auth,
                    &external_provider,
                    &external_sqz,
                    &mut results,
                )
            },
            0,
        )
        .unwrap();
        assert!(matches!(
            external_receipt.outcome(),
            crate::agent::runtime::InvocationOutcome::Complete { .. }
        ));
        let external_json = external_receipt.to_json();
        assert!(external_json.contains("\"result_id\":\"sha256:"));
        assert!(external_json.contains("\"sqz_id\":{\"algorithm\":\"sha256\""));
        assert!(external_json.contains("\"policy\":\"public-on\""));
        assert!(!external_json.contains("\"status\":\"off\""));
        assert!(external_json.contains("\"profile_name\":\"sol\""));

        assert!(crate::adapters::provider::SafeAccountReference::new("secret value").is_err());
        assert!(crate::agent::runtime::ProviderOutput::new(
            "",
            Vec::<String>::new(),
            None::<String>,
            None::<String>,
            None::<String>,
            crate::agent::runtime::ProviderTokenUsage {
                actual_input_tokens: None,
                actual_output_tokens: None,
            },
            vec![],
        )
        .is_err());
    }
}
