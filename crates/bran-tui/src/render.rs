use crate::domain::{NativeImage, TerminalCapabilities, TuiApp};

pub const RAVEN_WIDE: &str = include_str!("../../../assets/tui/raven-wide.txt");
pub const RAVEN_NARROW: &str = include_str!("../../../assets/tui/raven-narrow.txt");
pub const RAVEN_PLAIN: &str = include_str!("../../../assets/tui/raven-plain.txt");

pub fn render_surface(
    capabilities: TerminalCapabilities,
    _motion: bool,
    _native_image: NativeImage,
) -> String {
    if capabilities.columns == 0 || capabilities.columns < 36 {
        if capabilities.no_color {
            return "BRAN\nALPHAZEDE.com\n".to_string();
        } else {
            return "\x1b[1mBRAN\x1b[0m\n\x1b[2mALPHAZEDE.com\x1b[0m\n".to_string();
        }
    }
    let raven = if !capabilities.unicode {
        RAVEN_PLAIN
    } else if capabilities.columns >= 80 {
        RAVEN_WIDE
    } else {
        RAVEN_NARROW
    };
    if capabilities.no_color {
        format!("{raven}\nBRAN\nALPHAZEDE.com\n")
    } else {
        format!("{raven}\n\x1b[1mBRAN\x1b[0m\n\x1b[2mALPHAZEDE.com\x1b[0m\n")
    }
}

pub fn render_app(app: &TuiApp, caps: TerminalCapabilities) -> String {
    let columns = app.columns.unwrap_or(caps.columns);
    let raven = if !caps.unicode {
        RAVEN_PLAIN
    } else if columns >= 80 {
        RAVEN_WIDE
    } else {
        RAVEN_NARROW
    };
    let bran = if caps.no_color {
        "BRAN\n"
    } else {
        "\x1b[1mBRAN\x1b[0m\n"
    };
    let alpha = if caps.no_color {
        "ALPHAZEDE.com\n"
    } else {
        "\x1b[2mALPHAZEDE.com\x1b[0m\n"
    };
    let mut out = if columns < 36 {
        bran.to_string()
    } else {
        format!("{raven}\n{bran}")
    };
    let voice = if app.resolved.as_ref().is_some_and(|r| r.effective.voice) {
        "Ctrl+S (ready)"
    } else {
        "Ctrl+S (unavailable)"
    };
    out.push_str(&format!("help: enter=submit esc=cancel {} quit\n", voice));
    if columns >= 80 {
        out.push_str("----\n");
    }
    out.push_str(&format!("> {}\n", app.input));

    if !app.suggestions.is_empty() {
        out.push_str("suggestions:\n");
        for (i, sug) in app.suggestions.iter().enumerate() {
            let cur = if i == app.selection { ">" } else { " " };
            out.push_str(&format!("{} {}\n", cur, sug));
        }
    }

    if !app.status.is_empty() {
        out.push_str(&format!("status: {}\n", app.status));
    }
    out.push_str(&format!("flow: {:?}\n", app.flow));
    out.push_str(&format!("profile: {}\n", profile_label(app.draft.profile)));
    out.push_str(&format!(
        "requested agent/model/reasoning: {}/{}/{}\n",
        app.agent.as_deref().unwrap_or("configured"),
        app.model.as_deref().unwrap_or("configured"),
        app.reasoning.as_deref().unwrap_or("configured")
    ));
    out.push_str(&format!("requested: {}\n", features(&app.draft)));
    out.push_str(&format!(
        "requested per-task token ceiling: {}\n",
        app.draft.connected_agent_task_token_ceiling.map_or_else(
            || "configure a ceiling".to_owned(),
            |value| value.to_string()
        )
    ));
    out.push_str("guardrails: bounded root, read-only, explicit approval\n");
    out.push_str(&format!("step: {}\n", step_label(app.step)));
    if let Some(r) = &app.resolved {
        out.push_str(&format!("effective: {}\n", features(&r.effective)));
        out.push_str(&format!(
            "effective per-task token ceiling: {}\n",
            r.effective.connected_agent_task_token_ceiling.map_or_else(
                || "configure a ceiling".to_owned(),
                |value| value.to_string()
            )
        ));
        let unavailable = r
            .resolutions
            .iter()
            .filter(|resolution| {
                resolution.status == crate::settings::ResolutionStatus::Unavailable
            })
            .map(|resolution| format!("{:?}", resolution.feature))
            .collect::<Vec<_>>();
        if !unavailable.is_empty() {
            out.push_str(&format!("unavailable: {}\n", unavailable.join(",")));
        }
    }

    out.push_str(alpha);
    out
}

fn features(settings: &crate::settings::Settings) -> String {
    [
        ("sqz", settings.sqz),
        ("voice", settings.voice),
        ("history", settings.structured_history),
        ("chat", settings.saved_chat),
    ]
    .into_iter()
    .filter_map(|(name, enabled)| enabled.then_some(name))
    .collect::<Vec<_>>()
    .join(",")
}

fn profile_label(profile: crate::settings::OperatingProfile) -> &'static str {
    match profile {
        crate::settings::OperatingProfile::OfflineCore => "Offline Core",
        crate::settings::OperatingProfile::CoreSqz => "Core+SQZ",
        crate::settings::OperatingProfile::ConnectedAgent => "Connected Agent",
    }
}

fn step_label(step: crate::settings::OnboardingStep) -> &'static str {
    match step {
        crate::settings::OnboardingStep::ProjectCheck => "Project Check",
        crate::settings::OnboardingStep::FlowChoice => "Quick/Advanced",
        crate::settings::OnboardingStep::OperatingProfile => "Profile (offline/core-sqz/connected)",
        crate::settings::OnboardingStep::Guardrails => {
            "Guardrails (advanced: agent= model= reasoning= retention=none|structured|saved +/-sqz +/-voice tokens=N)"
        }
        crate::settings::OnboardingStep::CapabilityCheck => "Capability Check",
        crate::settings::OnboardingStep::ReadinessReview => "Readiness",
        crate::settings::OnboardingStep::PracticeRun => "Local Practice",
        crate::settings::OnboardingStep::Apply => "Apply (explicit)",
        crate::settings::OnboardingStep::Diagnose => "Repair",
        crate::settings::OnboardingStep::Complete => "Complete",
    }
}
