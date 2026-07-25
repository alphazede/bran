/// Explicit terminal facts used to choose a character-art surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalCapabilities {
    pub columns: u16,
    pub unicode: bool,
    pub no_color: bool,
}

impl TerminalCapabilities {
    pub const fn wide() -> Self {
        Self {
            columns: 80,
            unicode: true,
            no_color: false,
        }
    }

    pub const fn narrow() -> Self {
        Self {
            columns: 40,
            unicode: true,
            no_color: false,
        }
    }

    pub const fn plain() -> Self {
        Self {
            columns: 80,
            unicode: false,
            no_color: false,
        }
    }

    pub const fn no_color(mut self) -> Self {
        self.no_color = true;
        self
    }
}

/// Native image paths are deliberately not used by this core surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeImage {
    Unavailable,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TuiEvent {
    Input(char),
    Backspace,
    Enter,
    Escape,
    Up,
    Down,
    CtrlS,
    Quit,
    Resize { columns: u16 },
    ApplyFinished { persisted: bool },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TuiAction {
    Query {
        query: String,
        trust_current_root: bool,
        agent: Option<String>,
        model: Option<String>,
        reasoning: Option<String>,
    },
    Apply,
    Quit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TuiApp {
    pub input: String,
    pub status: String,
    pub motion: bool,
    pub native_image: NativeImage,
    pub flow: crate::settings::FlowChoice,
    pub draft: crate::settings::Settings,
    pub resolved: Option<crate::settings::ResolvedSettings>,
    pub suggestions: Vec<String>,
    pub selection: usize,
    pub pending_action: Option<TuiAction>,
    pub step: crate::settings::OnboardingStep,
    pub columns: Option<u16>,
    pub capability: crate::settings::CapabilityProbe,
    pub policy: crate::settings::Policy,
    /// Volatile, per-process authority. This is deliberately never persisted
    /// in repository settings.
    pub trust_current_root: bool,
    /// Volatile connected-agent choices; never written to project settings.
    pub agent: Option<String>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
}

impl Default for TuiApp {
    fn default() -> Self {
        Self {
            input: String::new(),
            status: String::new(),
            motion: false,
            native_image: NativeImage::Unavailable,
            flow: crate::settings::FlowChoice::Quick,
            draft: crate::settings::quick_safe_config(),
            resolved: None,
            suggestions: Vec::new(),
            selection: 0,
            pending_action: None,
            step: crate::settings::OnboardingStep::ProjectCheck,
            columns: None,
            capability: crate::settings::CapabilityProbe::default(),
            policy: crate::settings::Policy::default(),
            trust_current_root: false,
            agent: None,
            model: None,
            reasoning: None,
        }
    }
}

impl TuiApp {
    pub fn handle(&mut self, event: TuiEvent) {
        match event {
            TuiEvent::Input(character) => self.input.push(character),
            TuiEvent::Backspace => {
                self.input.pop();
            }
            TuiEvent::Enter => self.submit(),
            TuiEvent::Escape => {
                let d = crate::settings::cancel_or_decide_later();
                self.draft = d.settings;
                self.input.clear();
                self.suggestions.clear();
                self.selection = 0;
                self.status = "cancelled (decide later; no nag)".to_string();
                self.resolved = None;
                self.pending_action = None;
                self.step = crate::settings::OnboardingStep::ProjectCheck;
                self.flow = crate::settings::FlowChoice::Quick;
                self.trust_current_root = false;
                self.agent = None;
                self.model = None;
                self.reasoning = None;
            }
            TuiEvent::Up => {
                if !self.suggestions.is_empty() {
                    self.selection = if self.selection == 0 {
                        self.suggestions.len() - 1
                    } else {
                        self.selection - 1
                    };
                }
            }
            TuiEvent::Down => {
                if !self.suggestions.is_empty() {
                    self.selection = (self.selection + 1) % self.suggestions.len();
                }
            }
            TuiEvent::CtrlS => {
                self.status = if self.resolved.as_ref().is_some_and(|r| r.effective.voice) {
                    "Voice shortcut ready".to_owned()
                } else {
                    "Voice shortcut unavailable".to_owned()
                };
            }
            TuiEvent::Quit => {
                self.pending_action = Some(TuiAction::Quit);
                self.status = "quit".to_string();
            }
            TuiEvent::Resize { columns } => {
                self.columns = Some(columns);
            }
            TuiEvent::ApplyFinished { persisted } => {
                if self.step == crate::settings::OnboardingStep::Apply {
                    self.step = if persisted {
                        crate::settings::OnboardingStep::Complete
                    } else {
                        crate::settings::OnboardingStep::Diagnose
                    };
                    self.status = if persisted {
                        "applied".to_owned()
                    } else {
                        "apply failed".to_owned()
                    };
                }
            }
        }
    }

    fn submit(&mut self) {
        if let Some(choice) = self.suggestions.get(self.selection).cloned() {
            self.input = choice;
        }
        self.suggestions.clear();
        self.selection = 0;
        let input = self.input.trim().to_owned();
        if input.eq_ignore_ascii_case("back") {
            if let Some(index) = crate::settings::ONBOARDING_STEPS
                .iter()
                .position(|&s| s == self.step)
            {
                self.step = crate::settings::ONBOARDING_STEPS[index.saturating_sub(1)];
            }
            self.input.clear();
            return;
        }
        match self.step {
            crate::settings::OnboardingStep::ProjectCheck => {
                if input.eq_ignore_ascii_case("trust-current-root") {
                    self.trust_current_root = true;
                } else if !input.is_empty() && !input.eq_ignore_ascii_case("next") {
                    return self.invalid();
                }
            }
            crate::settings::OnboardingStep::FlowChoice => {
                match input.to_ascii_lowercase().as_str() {
                    "quick" | "advanced" => {
                        self.flow = if input.eq_ignore_ascii_case("quick") {
                            crate::settings::FlowChoice::Quick
                        } else {
                            crate::settings::FlowChoice::Advanced
                        };
                        self.draft = crate::settings::quick_safe_config();
                    }
                    _ => return self.invalid(),
                }
            }
            crate::settings::OnboardingStep::OperatingProfile => {
                self.draft.profile = match input.to_ascii_lowercase().as_str() {
                    "offline" | "offline-core" => crate::settings::OperatingProfile::OfflineCore,
                    "core-sqz" | "sqz" => crate::settings::OperatingProfile::CoreSqz,
                    "connected" | "connected-agent" => {
                        crate::settings::OperatingProfile::ConnectedAgent
                    }
                    _ => return self.invalid(),
                };
                let connected =
                    self.draft.profile == crate::settings::OperatingProfile::ConnectedAgent;
                self.draft.network = connected;
                self.draft.auth = connected;
            }
            crate::settings::OnboardingStep::Guardrails => {
                if input.is_empty() || input.eq_ignore_ascii_case("next") {
                    self.advance_step();
                    self.input.clear();
                    return;
                }
                if self.flow == crate::settings::FlowChoice::Advanced && self.set_features(&input) {
                    self.status = "features requested; enter to continue".to_owned();
                    self.input.clear();
                    return;
                }
                return self.invalid();
            }
            crate::settings::OnboardingStep::CapabilityCheck => self.refresh_resolved(),
            crate::settings::OnboardingStep::ReadinessReview => {
                if self.resolved.is_none() {
                    self.refresh_resolved();
                }
                if let Some(resolved) = &self.resolved {
                    let _ = crate::settings::readiness_receipt(resolved, None);
                }
            }
            crate::settings::OnboardingStep::PracticeRun if !input.is_empty() => {
                let practice = crate::settings::local_practice_query(&input);
                self.status = format!("practice accepted={}", practice.accepted);
                self.input.clear();
                self.advance_step();
                return;
            }
            crate::settings::OnboardingStep::PracticeRun => return self.invalid(),
            crate::settings::OnboardingStep::Apply if input.eq_ignore_ascii_case("apply") => {
                return self.request_apply()
            }
            crate::settings::OnboardingStep::Apply => return self.invalid(),
            crate::settings::OnboardingStep::Diagnose
                if input.is_empty()
                    || input.eq_ignore_ascii_case("next")
                    || input.eq_ignore_ascii_case("repair") =>
            {
                let repair = crate::settings::repair_proposal(&self.draft, self.capability);
                self.status = format!("repair failed={}", repair.failed_capabilities.len());
                self.input.clear();
                self.advance_step();
                return;
            }
            crate::settings::OnboardingStep::Diagnose => return self.invalid(),
            crate::settings::OnboardingStep::Complete
                if input.eq_ignore_ascii_case("trust-current-root") =>
            {
                self.trust_current_root = true;
                self.input.clear();
                self.status = "current root trusted for this run".to_owned();
                return;
            }
            crate::settings::OnboardingStep::Complete if !input.is_empty() => {
                return self.query(input)
            }
            crate::settings::OnboardingStep::Complete => return,
        }
        self.input.clear();
        self.advance_step();
        self.status = format!("step: {:?}", self.step);
    }

    pub fn take_action(&mut self) -> Option<TuiAction> {
        self.pending_action.take()
    }

    fn advance_step(&mut self) {
        if let Some(n) = self.step.next() {
            self.step = n;
        }
    }

    fn refresh_resolved(&mut self) {
        let req = crate::settings::AdvancedRequest::new(self.draft.clone());
        let res = crate::settings::resolve_advanced(req, self.capability, self.policy);
        self.resolved = Some(res);
    }

    fn set_features(&mut self, input: &str) -> bool {
        let mut settings = self.draft.clone();
        let mut token_ceiling = None;
        let mut agent = self.agent.clone();
        let mut model = self.model.clone();
        let mut reasoning = self.reasoning.clone();
        for feature in input.split_ascii_whitespace() {
            match feature {
                "+sqz" => {
                    settings.sqz = true;
                    if settings.profile == crate::settings::OperatingProfile::OfflineCore {
                        settings.profile = crate::settings::OperatingProfile::CoreSqz;
                    }
                }
                "-sqz" => {
                    settings.sqz = false;
                    if settings.profile == crate::settings::OperatingProfile::CoreSqz {
                        settings.profile = crate::settings::OperatingProfile::OfflineCore;
                    }
                }
                "+voice" => settings.voice = true,
                "-voice" => settings.voice = false,
                "+history" => settings.structured_history = true,
                "-history" => settings.structured_history = false,
                "+chat" => settings.saved_chat = true,
                "-chat" => settings.saved_chat = false,
                "retention=none" => {
                    settings.structured_history = false;
                    settings.saved_chat = false;
                }
                "retention=structured" => {
                    settings.structured_history = true;
                    settings.saved_chat = false;
                }
                "retention=saved" => {
                    settings.structured_history = true;
                    settings.saved_chat = true;
                }
                value if value.starts_with("agent=") => {
                    let value = &value["agent=".len()..];
                    if !valid_identifier(value) {
                        return false;
                    }
                    agent = Some(value.to_owned());
                }
                value if value.starts_with("model=") => {
                    let value = &value["model=".len()..];
                    if !valid_identifier(value) {
                        return false;
                    }
                    model = Some(value.to_owned());
                }
                value if value.starts_with("reasoning=") => {
                    let value = &value["reasoning=".len()..];
                    if !matches!(
                        value,
                        "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
                    ) {
                        return false;
                    }
                    reasoning = Some(value.to_owned());
                }
                value if value.starts_with("tokens=") => {
                    match (token_ceiling, value["tokens=".len()..].parse::<u32>().ok()) {
                        (None, Some(value)) if value > 0 => token_ceiling = Some(value),
                        _ => return false,
                    }
                }
                _ => return false,
            }
        }
        if let Some(value) = token_ceiling {
            settings.connected_agent_task_token_ceiling = Some(value);
        }
        self.draft = settings;
        self.agent = agent;
        self.model = model;
        self.reasoning = reasoning;
        true
    }

    fn invalid(&mut self) {
        self.input.clear();
        self.status = "choose a current-step option".to_owned();
    }

    fn query(&mut self, query: String) {
        self.pending_action = Some(TuiAction::Query {
            query: query.clone(),
            trust_current_root: self.trust_current_root,
            agent: self.agent.clone(),
            model: self.model.clone(),
            reasoning: self.reasoning.clone(),
        });
        self.status = format!("query: {query}");
    }

    pub fn update_suggestions(&mut self, candidates: &str) {
        self.suggestions = autocomplete(&self.input, candidates);
        if !self.suggestions.is_empty() && self.selection >= self.suggestions.len() {
            self.selection = 0;
        }
    }

    fn request_apply(&mut self) {
        self.refresh_resolved();
        self.pending_action = Some(TuiAction::Apply);
        self.status = "apply requested".to_owned();
    }
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

pub const AUTOCOMPLETE_SCAN_BUDGET: usize = 10;
pub const AUTOCOMPLETE_RESULT_BUDGET: usize = 3;

/// Returns prefix matches from at most the first ten newline-separated candidates.
pub fn autocomplete(query: &str, candidates: &str) -> Vec<String> {
    let query = query.to_ascii_lowercase();
    let mut results = Vec::new();
    for candidate in candidates.lines().take(AUTOCOMPLETE_SCAN_BUDGET) {
        if candidate.to_ascii_lowercase().starts_with(&query) {
            results.push(candidate.to_owned());
            if results.len() == AUTOCOMPLETE_RESULT_BUDGET {
                break;
            }
        }
    }
    results
}
