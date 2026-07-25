//! BRAN's dependency-free terminal surface core.

pub mod diagnostics;
pub mod domain;
pub mod render;
pub mod settings;

pub use diagnostics::{DiagnosticRecord, DiagnosticStore, Severity};
pub use domain::{autocomplete, NativeImage, TerminalCapabilities, TuiAction, TuiApp, TuiEvent};
pub use render::{render_app, render_surface, RAVEN_NARROW, RAVEN_PLAIN, RAVEN_WIDE};
pub use settings::{
    apply_settings, apply_settings_using, cancel_or_decide_later, load_settings,
    local_practice_query, onboarding_complete, quick_safe_config, readiness_receipt,
    repair_proposal, resolve_advanced, AdvancedRequest, ApprovalPolicy, CapabilityProbe,
    CompletionReceipt, DeferredOnboarding, DiagnosticPolicy, Feature, FlowChoice, Guardrail,
    GuardrailResolution, GuardrailValue, OnboardingMode, OnboardingStep, OperatingProfile, Policy,
    PracticeReceipt, ProviderTokenAttestation, ReadinessReceipt, RepairProposal,
    ResolutionProvenance, ResolutionStatus, ResolvedSettings, RetentionPolicy, RootScope,
    SettingResolution, Settings, TokenProjection, ToolPolicy, ONBOARDING_STEPS,
    PRACTICE_QUERY_LIMIT, SETTINGS_VERSION,
};

/// A terminal state boundary. Platform raw-mode adapters stay outside this crate.
pub trait TerminalPort {
    fn enter(&mut self) -> Result<(), String>;
    fn restore(&mut self);
}

/// Restores the terminal when it leaves scope, including during unwinding.
pub struct TerminalGuard<'a, Port: TerminalPort> {
    port: &'a mut Port,
}

impl<'a, Port: TerminalPort> TerminalGuard<'a, Port> {
    pub fn enter(port: &'a mut Port) -> Result<Self, String> {
        if let Err(error) = port.enter() {
            port.restore();
            return Err(error);
        }
        Ok(Self { port })
    }
}

impl<Port: TerminalPort> Drop for TerminalGuard<'_, Port> {
    fn drop(&mut self) {
        self.port.restore();
    }
}

#[cfg(test)]
struct TestPort {
    restored: usize,
}

#[cfg(test)]
impl TerminalPort for TestPort {
    fn enter(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn restore(&mut self) {
        self.restored += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p3_tui_surface() {
        let wide = render_surface(TerminalCapabilities::wide(), true, NativeImage::Unavailable);
        assert!(wide.contains(RAVEN_WIDE));
        assert!(wide.contains("BRAN"));
        assert!(wide.contains("ALPHAZEDE.com"));

        let narrow = render_surface(
            TerminalCapabilities::narrow(),
            true,
            NativeImage::Unavailable,
        );
        assert!(narrow.contains(RAVEN_NARROW));

        let plain = render_surface(
            TerminalCapabilities::plain(),
            true,
            NativeImage::Unavailable,
        );
        assert!(plain.contains(RAVEN_PLAIN));

        let no_color = render_surface(
            TerminalCapabilities::wide().no_color(),
            true,
            NativeImage::Unavailable,
        );
        assert!(!no_color.contains('\x1b'));

        let still_one = render_surface(
            TerminalCapabilities::wide(),
            false,
            NativeImage::Unavailable,
        );
        let still_two = render_surface(
            TerminalCapabilities::wide(),
            false,
            NativeImage::Unavailable,
        );
        assert_eq!(still_one, still_two);

        let failed_native = render_surface(TerminalCapabilities::wide(), true, NativeImage::Failed);
        assert!(failed_native.contains(RAVEN_WIDE));

        let zero_width = render_surface(
            TerminalCapabilities {
                columns: 0,
                unicode: true,
                no_color: false,
            },
            true,
            NativeImage::Unavailable,
        );
        assert!(
            zero_width.contains("BRAN")
                && zero_width.contains("ALPHAZEDE.com")
                && !zero_width.contains(RAVEN_WIDE)
                && !zero_width.contains(RAVEN_NARROW)
                && !zero_width.contains(RAVEN_PLAIN)
        );
        let below_minimum_width = render_surface(
            TerminalCapabilities {
                columns: 35,
                unicode: false,
                no_color: true,
            },
            false,
            NativeImage::Failed,
        );
        assert!(
            below_minimum_width == "BRAN\nALPHAZEDE.com\n"
                && !below_minimum_width.contains(RAVEN_PLAIN)
        );

        let result_bound = autocomplete("a", "amber\napple\narch\nafter");
        assert_eq!(result_bound.len(), 3);
        assert_eq!(result_bound[0], "amber");
        assert_eq!(result_bound[2], "arch");
        let scan_bound = autocomplete(
            "late",
            "zero\none\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nlate",
        );
        assert!(scan_bound.is_empty());

        let mut app = TuiApp::default();
        app.handle(TuiEvent::Input('q'));
        assert_eq!(app.input, "q");
        app.input.clear();
        app.step = OnboardingStep::Complete;
        app.handle(TuiEvent::Input('q'));
        assert_eq!(app.input, "q");
        app.input.clear();
        app.step = OnboardingStep::ProjectCheck;
        app.handle(TuiEvent::CtrlS);
        assert_eq!(app.status, "Voice shortcut unavailable");

        app.handle(TuiEvent::Resize { columns: 40 });
        let live = render_app(&app, TerminalCapabilities::wide().no_color());
        assert!(live.contains(RAVEN_NARROW));
        assert!(live.ends_with("ALPHAZEDE.com\n"));

        let mut normal = TestPort { restored: 0 };
        {
            let _guard = TerminalGuard::enter(&mut normal).unwrap();
        }
        assert_eq!(normal.restored, 1);

        let mut early = TestPort { restored: 0 };
        let early_result: Result<(), ()> = {
            let _guard = TerminalGuard::enter(&mut early).unwrap();
            Err(())
        };
        assert_eq!(early_result, Err(()));
        assert_eq!(early.restored, 1);

        let mut panicked = TestPort { restored: 0 };
        let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = TerminalGuard::enter(&mut panicked).unwrap();
            panic!("test unwind");
        }));
        assert!(panic_result.is_err());
        assert_eq!(panicked.restored, 1);
    }

    #[test]
    fn p3_tui_onboarding_diagnostics() {
        assert_eq!(
            OnboardingStep::ProjectCheck.next(),
            Some(OnboardingStep::FlowChoice)
        );
        assert_eq!(
            OnboardingStep::FlowChoice.next(),
            Some(OnboardingStep::OperatingProfile)
        );
        assert_eq!(
            OnboardingStep::OperatingProfile.next(),
            Some(OnboardingStep::Guardrails)
        );
        assert_eq!(
            OnboardingStep::Guardrails.next(),
            Some(OnboardingStep::CapabilityCheck)
        );
        assert_eq!(
            OnboardingStep::CapabilityCheck.next(),
            Some(OnboardingStep::ReadinessReview)
        );
        assert_eq!(
            OnboardingStep::ReadinessReview.next(),
            Some(OnboardingStep::PracticeRun)
        );
        assert_eq!(
            OnboardingStep::PracticeRun.next(),
            Some(OnboardingStep::Apply)
        );
        assert_eq!(OnboardingStep::Apply.next(), Some(OnboardingStep::Diagnose));
        assert_eq!(
            OnboardingStep::Diagnose.next(),
            Some(OnboardingStep::Complete)
        );
        assert_eq!(OnboardingStep::Complete.next(), None);

        let quick = quick_safe_config();
        assert_eq!(quick.profile, OperatingProfile::OfflineCore);
        assert_eq!(quick.connected_agent_task_token_ceiling, None);
        assert!(quick.sqz && quick.diagnostics);
        assert!(!quick.structured_history);
        assert!(
            !quick.voice && !quick.saved_chat && !quick.network && !quick.auth && !quick.mutation
        );

        let offline = resolve_advanced(
            AdvancedRequest::new(quick.clone()),
            CapabilityProbe::default(),
            Policy::default(),
        );
        assert_eq!(
            offline.effective_profile,
            Some(OperatingProfile::OfflineCore)
        );
        assert!(offline.offline_core_usable);

        let mut core_sqz = quick.clone();
        core_sqz.profile = OperatingProfile::CoreSqz;
        let core_sqz_resolution = resolve_advanced(
            AdvancedRequest::new(core_sqz),
            CapabilityProbe {
                sqz_available: true,
                ..CapabilityProbe::default()
            },
            Policy::default(),
        );
        assert_eq!(
            core_sqz_resolution.effective_profile,
            Some(OperatingProfile::CoreSqz)
        );
        assert!(core_sqz_resolution.effective.sqz);

        let mut connected = quick.clone();
        connected.profile = OperatingProfile::ConnectedAgent;
        let connected_resolution = resolve_advanced(
            AdvancedRequest::new(connected),
            CapabilityProbe {
                connected_agent_runtime: true,
                network_available: true,
                auth_available: true,
                ..CapabilityProbe::default()
            },
            Policy::default(),
        );
        assert_eq!(
            connected_resolution.effective_profile,
            Some(OperatingProfile::ConnectedAgent)
        );
        assert!(connected_resolution.effective.network && connected_resolution.effective.auth);
        assert!(!connected_resolution.effective.mutation);

        let mut requested = quick.clone();
        requested.profile = OperatingProfile::ConnectedAgent;
        requested.voice = true;
        requested.saved_chat = true;
        requested.network = true;
        requested.auth = true;
        requested.mutation = true;
        requested.connected_agent_task_token_ceiling = Some(12_345);
        let unavailable = resolve_advanced(
            AdvancedRequest::new(requested.clone()),
            CapabilityProbe::default(),
            Policy { lock_sqz_on: true },
        );
        assert_eq!(unavailable.resolutions[0].feature, Feature::Sqz);
        assert!(unavailable.resolutions[0].requested && !unavailable.resolutions[0].effective);
        assert_eq!(
            unavailable.resolutions[0].status,
            ResolutionStatus::Unavailable
        );
        assert!(unavailable.resolutions[0].policy_locked);
        assert_eq!(
            unavailable.resolutions[2].status,
            ResolutionStatus::Unavailable
        );
        assert_eq!(
            unavailable.resolutions[4].status,
            ResolutionStatus::Unavailable
        );
        assert_eq!(
            unavailable.resolutions[8].status,
            ResolutionStatus::Unavailable
        );
        assert_eq!(
            unavailable.requested.profile,
            OperatingProfile::ConnectedAgent
        );
        assert_eq!(unavailable.effective.profile, OperatingProfile::OfflineCore);
        assert!(!unavailable.effective.network && !unavailable.effective.auth);
        assert_eq!(unavailable.effective_profile, None);

        let locked = resolve_advanced(
            AdvancedRequest::new(requested.clone()),
            CapabilityProbe {
                sqz_available: true,
                ..CapabilityProbe::default()
            },
            Policy { lock_sqz_on: true },
        );
        assert_eq!(locked.resolutions[0].status, ResolutionStatus::Locked);
        assert!(locked.resolutions[0].effective && locked.resolutions[0].policy_locked);
        assert_eq!(
            unavailable.guardrails[0].effective,
            GuardrailValue::BoundedCurrentRoot
        );
        assert_eq!(
            unavailable.guardrails[1].effective,
            GuardrailValue::ReadOnlyTools
        );
        assert_eq!(
            unavailable.guardrails[2].effective,
            GuardrailValue::ExplicitApproval
        );
        assert_eq!(
            unavailable.guardrails[3].effective,
            GuardrailValue::ZeroConversationRetention
        );
        assert_eq!(
            unavailable.guardrails[4].effective,
            GuardrailValue::BoundedDiagnostics
        );

        let readiness = readiness_receipt(&unavailable, None);
        assert_eq!(readiness.requested, requested);
        assert_eq!(readiness.effective, unavailable.effective);
        assert_eq!(
            readiness.requested.connected_agent_task_token_ceiling,
            Some(12_345)
        );
        assert_eq!(
            readiness.effective.connected_agent_task_token_ceiling,
            Some(12_345)
        );
        assert_eq!(readiness.locked.len(), 1);
        assert_eq!(readiness.locked[0], Feature::Sqz);
        assert_eq!(readiness.unavailable.len(), 7);
        assert_eq!(readiness.unavailable[0], Feature::Sqz);
        assert_eq!(readiness.unavailable[1], Feature::Voice);
        assert_eq!(readiness.unavailable[2], Feature::SavedChat);
        assert_eq!(readiness.unavailable[3], Feature::Network);
        assert_eq!(readiness.unavailable[4], Feature::Auth);
        assert_eq!(readiness.unavailable[5], Feature::Mutation);
        assert_eq!(readiness.unavailable[6], Feature::ConnectedAgent);
        assert_eq!(readiness.token_projection.label, "estimated");
        assert_eq!(readiness.token_projection.actual_provider_tokens, None);

        let practice = local_practice_query("local practice");
        assert!(practice.accepted);
        assert!(!practice.network_used && !practice.auth_used && !practice.audio_used);
        assert!(!practice.chat_storage_used && !practice.mutation_used);

        let temp_dir =
            std::env::temp_dir().join(format!("bran-tui-onboarding-{}", std::process::id()));
        let settings_path = temp_dir.join("settings.conf");
        std::fs::remove_dir_all(&temp_dir).ok();
        std::fs::create_dir(&temp_dir).unwrap();
        assert!(!onboarding_complete(&settings_path).unwrap());
        let mut persisted = quick.clone();
        persisted.connected_agent_task_token_ceiling = Some(12_345);
        let completion = apply_settings(&settings_path, &persisted).unwrap();
        assert!(completion.completed && !completion.should_nag && completion.bytes_written > 0);
        assert!(onboarding_complete(&settings_path).unwrap());
        assert_eq!(
            load_settings(&settings_path).unwrap(),
            Some(persisted.clone())
        );
        let saved = std::fs::read_to_string(&settings_path).unwrap();
        let mut migrated = persisted.clone();
        migrated.connected_agent_task_token_ceiling = None;
        std::fs::write(
            &settings_path,
            saved
                .replacen(&format!("version={SETTINGS_VERSION}"), "version=2", 1)
                .replacen(
                    "connected_agent_task_token_ceiling=12345",
                    "connected_agent_task_token_ceiling=8500",
                    1,
                ),
        )
        .unwrap();
        assert_eq!(load_settings(&settings_path).unwrap(), Some(migrated));
        std::fs::write(
            &settings_path,
            saved.replacen(&format!("version={SETTINGS_VERSION}"), "version=1", 1),
        )
        .unwrap();
        assert_eq!(load_settings(&settings_path).unwrap(), None);
        std::fs::write(
            &settings_path,
            saved.replacen(
                "connected_agent_task_token_ceiling=12345",
                "connected_agent_task_token_ceiling=0",
                1,
            ),
        )
        .unwrap();
        assert_eq!(load_settings(&settings_path).unwrap(), None);
        std::fs::write(
            &settings_path,
            saved.replacen(
                "connected_agent_task_token_ceiling=12345",
                "connected_agent_task_token_ceiling=4294967296",
                1,
            ),
        )
        .unwrap();
        assert_eq!(load_settings(&settings_path).unwrap(), None);
        std::fs::write(
            &settings_path,
            format!(
                "version={SETTINGS_VERSION}\ncompleted=true\nprofile=offline-core\nsqz=true\ndiagnostics=true\nvoice=false\nstructured_history=false\nsaved_chat=false\nnetwork=false\nauth=false\nmutation=false\nroot=bounded-current-root\ntools=read-only\napproval=explicit\nretention=zero-conversation\n"
            ),
        )
        .unwrap();
        assert!(!onboarding_complete(&settings_path).unwrap());
        std::fs::write(
            &settings_path,
            format!(
                "version={SETTINGS_VERSION}\ncompleted=true\nprofile=offline-core\nsqz=true\ndiagnostics=true\nvoice=false\nstructured_history=false\nsaved_chat=false\nnetwork=false\nauth=false\nmutation=false\nroot=bounded-current-root\ntools=read-only\napproval=explicit\nretention=zero-conversation\ndiagnostic_policy=bounded\nextra=foo\n"
            ),
        )
        .unwrap();
        assert!(!onboarding_complete(&settings_path).unwrap());
        std::fs::write(&settings_path, b"known-prior-bytes").unwrap();
        let replace_error = apply_settings_using(&settings_path, &quick, |_, _| {
            Err(std::io::Error::other("injected replace failure"))
        });
        assert!(replace_error.is_err());
        assert_eq!(std::fs::read(&settings_path).unwrap(), b"known-prior-bytes");
        assert!(!temp_dir
            .join(format!(".settings.conf.{}.0.tmp", std::process::id()))
            .exists());
        std::fs::remove_file(&settings_path).unwrap();
        std::fs::remove_dir(&temp_dir).unwrap();

        let deferred = cancel_or_decide_later();
        assert!(!deferred.completed && !deferred.should_nag);
        assert_eq!(deferred.settings, quick);
        let mut current = quick.clone();
        current.voice = true;
        current.connected_agent_task_token_ceiling = Some(12_345);
        let repair = repair_proposal(
            &current,
            CapabilityProbe {
                sqz_available: true,
                voice_available: false,
                saved_chat_available: true,
                connected_agent_runtime: true,
                ..CapabilityProbe::default()
            },
        );
        assert_eq!(repair.preserved, current);
        assert_eq!(repair.failed_capabilities.len(), 1);
        assert_eq!(repair.failed_capabilities[0], Feature::Voice);

        let mut disabled = DiagnosticStore::new(false, 2);
        disabled.record("disabled", Severity::Warning, "token=disabled-secret");
        assert_eq!(disabled.records().len(), 0);
        let mut diagnostics = DiagnosticStore::new(true, 2);
        diagnostics.record(
            "warning-code",
            Severity::Warning,
            "token=token-secret api_key=api-key-secret password=password-secret prompt=PROMPT_SENTINEL audio=AUDIO_SENTINEL chat=CHAT_SENTINEL /home/home-user",
        );
        diagnostics.record(
            "warning-code",
            Severity::Warning,
            "token=token-secret api_key=api-key-secret password=password-secret prompt=PROMPT_SENTINEL audio=AUDIO_SENTINEL chat=CHAT_SENTINEL /home/home-user",
        );
        assert_eq!(diagnostics.records().len(), 1);
        assert_eq!(diagnostics.records()[0].count, 2);
        assert!(diagnostics.records()[0].message.contains("[REDACTED]"));
        assert!(!diagnostics.records()[0].message.contains("token-secret"));
        assert!(!diagnostics.records()[0].message.contains("api-key-secret"));
        assert!(!diagnostics.records()[0].message.contains("password-secret"));
        assert!(!diagnostics.records()[0].message.contains("PROMPT_SENTINEL"));
        assert!(!diagnostics.records()[0].message.contains("AUDIO_SENTINEL"));
        assert!(!diagnostics.records()[0].message.contains("CHAT_SENTINEL"));
        assert!(!diagnostics.records()[0].message.contains("home-user"));
        let diagnostic_export = diagnostics.export_text();
        assert!(diagnostic_export.contains("unique_count=1"));
        assert!(diagnostic_export.contains("fingerprint(non-cryptographic-default-hasher)"));
        assert!(diagnostic_export.contains("code=warning-code"));
        assert!(!diagnostic_export.contains("token-secret"));
        assert!(!diagnostic_export.contains("api-key-secret"));
        assert!(!diagnostic_export.contains("password-secret"));
        assert!(!diagnostic_export.contains("home-user"));
        assert!(!diagnostic_export.contains("PROMPT_SENTINEL"));
        assert!(!diagnostic_export.contains("AUDIO_SENTINEL"));
        assert!(!diagnostic_export.contains("CHAT_SENTINEL"));
        diagnostics.record("error-one", Severity::Error, "first error");
        diagnostics.record("error-two", Severity::Error, "second error");
        assert_eq!(diagnostics.records().len(), 2);
        assert_eq!(diagnostics.records().front().unwrap().code, "error-one");
        assert_eq!(diagnostics.records().back().unwrap().code, "error-two");
        let rotated_export = diagnostics.export_text();
        assert!(rotated_export.contains("code=error-one"));
        assert!(rotated_export.contains("code=error-two"));

        let enter = |app: &mut TuiApp, input: &str| {
            app.input = input.to_owned();
            app.handle(TuiEvent::Enter);
        };
        let mut app = TuiApp::default();
        enter(&mut app, "");
        enter(&mut app, "quick");
        enter(&mut app, "offline");
        enter(&mut app, "");
        enter(&mut app, "next");
        enter(&mut app, "next");
        enter(&mut app, "local practice");
        assert_eq!(app.step, OnboardingStep::Apply);
        assert_eq!(app.status, "practice accepted=true");
        enter(&mut app, "not apply");
        assert_eq!(app.status, "choose a current-step option");
        enter(&mut app, "apply");
        assert_eq!(app.take_action(), Some(TuiAction::Apply));
        assert_eq!(app.step, OnboardingStep::Apply);
        app.handle(TuiEvent::ApplyFinished { persisted: false });
        assert_eq!(app.step, OnboardingStep::Diagnose);
        enter(&mut app, "repair");
        assert_eq!(app.step, OnboardingStep::Complete);
        assert_eq!(app.status, "repair failed=1");
        enter(&mut app, "view");
        assert_eq!(
            app.take_action(),
            Some(TuiAction::Query {
                query: "view".to_owned(),
                trust_current_root: false,
                agent: None,
                model: None,
                reasoning: None,
            })
        );
        enter(&mut app, "trust-current-root");
        assert!(app.trust_current_root);
        enter(&mut app, "view");
        assert_eq!(
            app.take_action(),
            Some(TuiAction::Query {
                query: "view".to_owned(),
                trust_current_root: true,
                agent: None,
                model: None,
                reasoning: None,
            })
        );
        app.handle(TuiEvent::Escape);
        assert_eq!(app.step, OnboardingStep::ProjectCheck);
        assert!(!app.trust_current_root);

        enter(&mut app, "");
        enter(&mut app, "advanced");
        enter(&mut app, "core-sqz");
        enter(&mut app, "-sqz +voice +history +chat tokens=12345");
        assert!(
            !app.draft.sqz
                && app.draft.voice
                && app.draft.structured_history
                && app.draft.saved_chat
        );
        assert_eq!(app.draft.connected_agent_task_token_ceiling, Some(12_345));
        assert_eq!(app.draft.profile, OperatingProfile::OfflineCore);
        let configured = app.draft.clone();
        enter(&mut app, "+voice tokens=0");
        assert_eq!(app.draft, configured);
        assert_eq!(app.status, "choose a current-step option");
        enter(&mut app, "");
        enter(&mut app, "next");
        assert!(app.resolved.as_ref().is_some_and(|r| !r.requested.sqz
            && !r.effective.sqz
            && r.requested.profile == OperatingProfile::OfflineCore
            && r.effective.profile == OperatingProfile::OfflineCore
            && !r.effective.voice));
        app.handle(TuiEvent::CtrlS);
        assert_eq!(app.status, "Voice shortcut unavailable");
        app.capability.voice_available = true;
        app.step = OnboardingStep::CapabilityCheck;
        enter(&mut app, "");
        assert!(app.resolved.as_ref().is_some_and(|r| r.effective.voice));
        assert!(
            render_app(&app, TerminalCapabilities::wide().no_color()).contains("Ctrl+S (ready)")
        );
        app.handle(TuiEvent::CtrlS);
        assert_eq!(app.status, "Voice shortcut ready");
        enter(&mut app, "next");
        enter(&mut app, "local practice");
        enter(&mut app, "apply");
        app.handle(TuiEvent::ApplyFinished { persisted: true });
        assert_eq!(app.step, OnboardingStep::Complete);

        let mut connected = TuiApp::default();
        enter(&mut connected, "");
        enter(&mut connected, "advanced");
        enter(&mut connected, "connected");
        enter(
            &mut connected,
            "agent=research model=fast reasoning=max retention=saved",
        );
        assert_eq!(connected.agent.as_deref(), Some("research"));
        assert_eq!(connected.model.as_deref(), Some("fast"));
        assert_eq!(connected.reasoning.as_deref(), Some("max"));
        assert!(connected.draft.structured_history && connected.draft.saved_chat);
        assert_eq!(connected.draft.retention, RetentionPolicy::ZeroConversation);
        enter(&mut connected, "retention=none");
        assert!(!connected.draft.structured_history && !connected.draft.saved_chat);
        enter(&mut connected, "retention=structured");
        assert!(connected.draft.structured_history && !connected.draft.saved_chat);
        enter(&mut connected, "retention=saved");
        assert!(connected.draft.structured_history && connected.draft.saved_chat);
        let requested = (
            connected.agent.clone(),
            connected.model.clone(),
            connected.reasoning.clone(),
        );
        enter(&mut connected, "agent=Invalid reasoning=ultra");
        assert_eq!(
            (
                connected.agent.clone(),
                connected.model.clone(),
                connected.reasoning.clone(),
            ),
            requested
        );
        assert_eq!(connected.status, "choose a current-step option");
        connected.step = OnboardingStep::Complete;
        enter(&mut connected, "find the contract");
        assert_eq!(
            connected.take_action(),
            Some(TuiAction::Query {
                query: "find the contract".to_owned(),
                trust_current_root: false,
                agent: Some("research".to_owned()),
                model: Some("fast".to_owned()),
                reasoning: Some("max".to_owned()),
            })
        );
    }
}
