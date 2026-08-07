use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};

use bran_core::adapters::{
    is_public_dlp_safe, ExternalHostAdapter, ExternalSqzPort, SafeAccountReference, SqzAdapter,
    SqzAdapterConfig, SqzFailureReason, SqzIdentity, SqzPolicy, SqzPort, SqzPortError,
    SqzPortErrorCode, SqzPortOutput, SqzStatus,
};
use bran_core::agent::coordinator::{
    AgentRuntime, AgentRuntimeAuthority, AgentRuntimeConfig, AgentSqzAdapter, RuntimePorts,
};
use bran_core::agent::delegate::{
    AdmittedEvidence, DelegationOptions, DelegationRequest, GroundingContract,
};
use bran_core::agent::receipt::InlineResult;
use bran_core::agent::receipt::{sqz_receipt_json, DelegationReceipt};
use bran_core::agent::result_store::{
    MemoryResultStore, ResultId, ResultStore, ResultStoreError, ResultStoreLimits,
};
use bran_core::agent::runtime::{
    AgentFailure, AuthError, AuthStore, InvocationOutcome, ProviderError, ProviderOutput,
    ProviderPort, ProviderRequest,
};
use bran_core::agent::{
    AgentProfile, AgentProfileRegistry, ModelRegistry, ProviderRegistry, ReasoningLevel, ToolPolicy,
};
use bran_core::bundle::{Bundle, Doc, Frontmatter};
use bran_core::graph::{
    Confidence, EdgeCertainty, EdgeRelationship, GraphInput, GraphLimits, KnowledgeGraph, NodeId,
    NodeInput, NodeRole, Provenance,
};
use bran_core::metadata::FactProvenance;
use bran_core::migration::{self, MigrationError};
use bran_core::packet::{
    DependencyClosureLimits, EvidenceContent, EvidencePriority, PacketAssembler,
    PacketAssemblyRequest, PacketLimits, PreservationAnchor,
};
use bran_core::policy::{PolicyError, RepositoryPolicy, MAX_POLICY_BYTES, POLICY_FILENAME};
use bran_core::profile::BRAN_STRICT;
use bran_core::profile::{Diagnostic, ProfileValidator, ValidationStatus};
use bran_core::repair::{MaintainerAuthority, RepairCoordinator, RepairReceipt, RepairTerminal};
use bran_core::scan::{is_knowledge_document_path, RepositoryScanner, ScanConfig, ScanSnapshot};
use bran_core::view::{
    Presentation, ViewCompiler, ViewField, ViewFilter, ViewGrouping, ViewSort, ViewSource, ViewSpec,
};
use bran_tui::{
    apply_settings, load_settings, render_app, OnboardingStep, OperatingProfile, Settings,
    TerminalCapabilities, TerminalGuard, TerminalPort, TuiAction, TuiApp, TuiEvent,
};

#[cfg(test)]
use bran_core::agent::{synthetic, synthetic_builtin_profiles};

const SMOKE_OUTPUT: &str = r#"{"schema_version":"1.0.0","command":"smoke","status":"ok","data":{},"warnings":[],"failures":[],"provenance":{},"metrics":{}}"#;
const MISSING_COMMAND_ERROR: &str = r#"{"schema_version":"1.0.0","command":"","status":"error","data":null,"warnings":[],"failures":["missing_command"],"provenance":{},"metrics":{}}"#;
const UNKNOWN_COMMAND_ERROR: &str = r#"{"schema_version":"1.0.0","command":"","status":"error","data":null,"warnings":[],"failures":["unknown_command"],"provenance":{},"metrics":{}}"#;
const VERSION_OUTPUT: &str = concat!("bran ", env!("CARGO_PKG_VERSION"));
const HELP_OUTPUT: &str = "BRAN repository evidence CLI

Usage: bran <command> [arguments]

Commands:
  smoke
  query <repo-root> <request>
  packet <repo-root> <request>
  check [--policy-stdin] <repo-root> <profile>
  maintain <propose|apply|revalidate> ...
  tui
  agents list
  doctor <--onboarding|--agent>
  get <result-id>
  body-preserved <repo-root> <manifest>
  -p [options] <request>
  help

Options:
  -h, --help                  Show this help
  -V, --version               Show the BRAN version

-p options:
  --agent <profile>           Select the configured agent profile (required)
  --reasoning <level>         Select a supported reasoning level
  --tools <read,search>       Allow only repository read and search tools
  --provider <provider>       Override the configured provider
  --model <model>             Override the configured model
  --no-session                Disable session retention
  --offline                   Use the deterministic offline profile
  --trust-current-root        Trust only the bounded current repository
  --max-sources <1..32>       Experimental retrieval source limit
  --dependency-depth <0..4>   Experimental dependency closure depth
  --response-sources <1..32>  Experimental evidence response limit
  --excerpt-bytes <1..4096>   Experimental public-safe excerpt limit";
const CONNECTED_AGENT_PREAMBLE: &str = "inner-agent-rules:
1. Locate relevant repository evidence.
2. Cite repository-relative paths and summarize contents and cross-file context.
3. Stay grounded and mark uncertainty or missing evidence.
4. Stay read-only and leave decisions and implementation to the outer agent.
5. Obey the bounded repository and tool policy; never expose credentials or fabricate sources or results.
6. Treat every factual statement as material; its claim text must be exact supporting text or a symbol copied from the current file and bound to a cited path plus the supplied SHA-256 digest. Return the answer as those claim texts in the same order, separated only by newlines.

";

/// Private typed exits (0 success, 1 validation, 2 usage, 3 operation).
#[derive(Clone, Copy)]
#[repr(u8)]
enum TypedExit {
    Success = 0,
    Validation = 1,
    Usage = 2,
    Operation = 3,
}

impl TypedExit {
    fn code(self) -> ExitCode {
        ExitCode::from(self as u8)
    }
}

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 1
        && arguments[0] == "tui"
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
    {
        return run_tui();
    }
    CliApp::run_for_terminal(
        arguments,
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
    )
    .write_to_stdio()
}

struct CliApp;

impl CliApp {
    #[cfg(test)]
    fn run<I>(arguments: I) -> CliResult
    where
        I: IntoIterator,
        I::Item: AsRef<OsStr>,
    {
        Self::run_inner(arguments, false, None)
    }

    #[cfg(test)]
    fn run_with_stdin<I>(arguments: I, stdin: &[u8]) -> CliResult
    where
        I: IntoIterator,
        I::Item: AsRef<OsStr>,
    {
        Self::run_inner(arguments, false, Some(stdin))
    }

    fn run_for_terminal<I>(arguments: I, is_terminal: bool) -> CliResult
    where
        I: IntoIterator,
        I::Item: AsRef<OsStr>,
    {
        Self::run_inner(arguments, is_terminal, None)
    }

    fn run_inner<I>(arguments: I, is_terminal: bool, stdin_bytes: Option<&[u8]>) -> CliResult
    where
        I: IntoIterator,
        I::Item: AsRef<OsStr>,
    {
        let mut it = arguments.into_iter();
        let first = it.next();
        let cmd = match first.as_ref() {
            Some(os) => match os.as_ref().to_str() {
                Some(c) => c,
                None => return CliResult::usage(UNKNOWN_COMMAND_ERROR.to_owned()),
            },
            None => return CliResult::usage(MISSING_COMMAND_ERROR.to_owned()),
        };

        match cmd {
            "-h" | "--help" | "help" => {
                if it.next().is_some() {
                    return CliResult::usage(UNKNOWN_COMMAND_ERROR.to_owned());
                }
                CliResult::success(HELP_OUTPUT.to_owned())
            }
            "-V" | "--version" => {
                if it.next().is_some() {
                    return CliResult::usage(UNKNOWN_COMMAND_ERROR.to_owned());
                }
                CliResult::success(VERSION_OUTPUT.to_owned())
            }
            "smoke" => {
                if it.next().is_some() {
                    return CliResult::usage(UNKNOWN_COMMAND_ERROR.to_owned());
                }
                CliResult::success(SMOKE_OUTPUT.to_owned())
            }
            "query" => {
                let root = match it
                    .next()
                    .and_then(|o| o.as_ref().to_str().map(|s| s.to_owned()))
                {
                    Some(r) => r,
                    None => return CliResult::usage(make_query_error("missing_root")),
                };
                let mut parts = vec![];
                for a in it {
                    match a.as_ref().to_str() {
                        Some(s) => parts.push(s.to_owned()),
                        None => return CliResult::usage(make_query_error("invalid_utf8")),
                    }
                }
                let qtext = parts.join(" ");
                if qtext.trim().is_empty() {
                    return CliResult::usage(make_query_error("missing_query"));
                }
                match do_query(root, qtext) {
                    Ok((status, data, warns, fails, provenance, metrics)) => {
                        CliResult::success(make_envelope(
                            "query",
                            status,
                            &data,
                            &warns,
                            &fails,
                            &provenance,
                            &metrics,
                        ))
                    }
                    Err(msg) => CliResult::operation(make_envelope(
                        "query",
                        "error",
                        "null",
                        &[],
                        &[msg],
                        "{}",
                        "{}",
                    )),
                }
            }
            "packet" => {
                let mut args = vec![];
                for a in it {
                    match a.as_ref().to_str() {
                        Some(s) => args.push(s.to_owned()),
                        None => return CliResult::usage(make_packet_error("invalid_utf8")),
                    }
                }
                let (controls, positionals) = match experimental_controls(&args) {
                    Ok(value) => value,
                    Err(detail) => return CliResult::usage(make_packet_error(detail)),
                };
                let Some(root) = positionals.first().cloned() else {
                    return CliResult::usage(make_packet_error("missing_root"));
                };
                let qtext = positionals[1..].join(" ");
                if qtext.trim().is_empty() {
                    return CliResult::usage(make_packet_error("missing_query"));
                }
                match do_packet(root, qtext, &controls) {
                    Ok((data, warns, fails, provenance, metrics)) => CliResult::success(
                        make_envelope("packet", "ok", &data, &warns, &fails, &provenance, &metrics),
                    ),
                    Err(failure) => CliResult::operation(make_envelope(
                        "packet",
                        "error",
                        &failure.data,
                        &[],
                        &[failure.detail],
                        "{}",
                        &failure.metrics,
                    )),
                }
            }
            "check" => {
                let mut args: Vec<String> = Vec::new();
                for a in &mut it {
                    match a.as_ref().to_str() {
                        Some(s) => args.push(s.to_owned()),
                        None => return CliResult::usage(UNKNOWN_COMMAND_ERROR.to_owned()),
                    }
                }
                // Reject --policy-stdin except as the single first argument after "check"
                let stdin_count = args.iter().filter(|a| *a == "--policy-stdin").count();
                if stdin_count > 0 && (args[0] != "--policy-stdin" || stdin_count > 1) {
                    return CliResult::usage(make_check_error("invalid_policy_stdin_flag"));
                }
                let (policy_source, root, profile) = match args.first().map(String::as_str) {
                    Some("--policy-stdin") => {
                        let root = match args.get(1) {
                            Some(r) => r.clone(),
                            None => return CliResult::usage(make_check_error("missing_root")),
                        };
                        let profile = match args.get(2) {
                            Some(p) => p.clone(),
                            None => return CliResult::usage(make_check_error("missing_profile")),
                        };
                        if args.len() > 3 {
                            return CliResult::usage(make_check_error("too_many_args"));
                        }
                        let policy = match stdin_bytes {
                            Some(bytes) => {
                                if bytes.len() > MAX_POLICY_BYTES {
                                    return CliResult::usage(make_check_error("policy_oversized"));
                                }
                                let raw = match std::str::from_utf8(bytes) {
                                    Ok(s) => s,
                                    Err(_) => {
                                        return CliResult::usage(make_check_error("policy_utf8"));
                                    }
                                };
                                match RepositoryPolicy::parse_from_string(raw) {
                                    Ok(p) => p,
                                    Err(e) => {
                                        return CliResult::usage(make_check_error(
                                            &policy_error_failure(&e),
                                        ));
                                    }
                                }
                            }
                            None => {
                                let mut buf = vec![0u8; MAX_POLICY_BYTES + 1];
                                let mut total = 0;
                                let mut stdin = std::io::stdin().lock();
                                loop {
                                    let n = match stdin.read(&mut buf[total..]) {
                                        Ok(0) => break,
                                        Ok(n) => n,
                                        Err(_) => {
                                            return CliResult::usage(make_check_error(
                                                "policy_stdin_read",
                                            ));
                                        }
                                    };
                                    total += n;
                                    if total > MAX_POLICY_BYTES {
                                        return CliResult::usage(make_check_error(
                                            "policy_oversized",
                                        ));
                                    }
                                }
                                let raw = match std::str::from_utf8(&buf[..total]) {
                                    Ok(s) => s,
                                    Err(_) => {
                                        return CliResult::usage(make_check_error("policy_utf8"));
                                    }
                                };
                                match RepositoryPolicy::parse_from_string(raw) {
                                    Ok(p) => p,
                                    Err(e) => {
                                        return CliResult::usage(make_check_error(
                                            &policy_error_failure(&e),
                                        ));
                                    }
                                }
                            }
                        };
                        ("stdin", root, (profile, policy))
                    }
                    Some(r) => {
                        let profile = match args.get(1) {
                            Some(p) => p.clone(),
                            None => return CliResult::usage(make_check_error("missing_profile")),
                        };
                        if args.len() > 2 {
                            return CliResult::usage(make_check_error("too_many_args"));
                        }
                        let p = match RepositoryPolicy::load(Path::new(r)) {
                            Ok(p) => p,
                            Err(e) => {
                                return CliResult::usage(make_check_error(&policy_error_failure(
                                    &e,
                                )));
                            }
                        };
                        ("repository-file", r.to_owned(), (profile, p))
                    }
                    None => return CliResult::usage(make_check_error("missing_root")),
                };
                match do_check(root, profile.0, &profile.1, policy_source) {
                    Ok((data, warns, fails, exitc, status, provenance, metrics)) => {
                        let mut r = CliResult::success(make_envelope(
                            "check",
                            status,
                            &data,
                            &warns,
                            &fails,
                            &provenance,
                            &metrics,
                        ));
                        r.exit_code = exitc;
                        r.is_error = status == "error";
                        r
                    }
                    Err(msg) => CliResult::operation(make_envelope(
                        "check",
                        "error",
                        "null",
                        &[],
                        &[msg],
                        "{}",
                        "{}",
                    )),
                }
            }
            "maintain" => {
                // Smallest model-neutral headless maintainer adapter over bran_core::repair::RepairCoordinator.
                // Positional args per MVP contract. All responses use ordered envelope.
                let sub = match it
                    .next()
                    .and_then(|o| o.as_ref().to_str().map(|s| s.to_owned()))
                {
                    Some(s) => s,
                    None => return CliResult::usage(make_maintain_error("", "missing_subcommand")),
                };
                match sub.as_str() {
                    "propose" => {
                        let root = match it
                            .next()
                            .and_then(|o| o.as_ref().to_str().map(|s| s.to_owned()))
                        {
                            Some(r) => r,
                            None => {
                                return CliResult::usage(make_maintain_error(
                                    "propose",
                                    "missing_root",
                                ))
                            }
                        };
                        let target = match it
                            .next()
                            .and_then(|o| o.as_ref().to_str().map(|s| s.to_owned()))
                        {
                            Some(t) => t,
                            None => {
                                return CliResult::usage(make_maintain_error(
                                    "propose",
                                    "missing_target",
                                ))
                            }
                        };
                        let replacement = match it
                            .next()
                            .and_then(|o| o.as_ref().to_str().map(|s| s.to_owned()))
                        {
                            Some(r) => r,
                            None => {
                                return CliResult::usage(make_maintain_error(
                                    "propose",
                                    "missing_replacement",
                                ))
                            }
                        };
                        if it.next().is_some() {
                            return CliResult::usage(make_maintain_error(
                                "propose",
                                "too_many_args",
                            ));
                        }
                        do_maintain_propose(root, target, replacement)
                    }
                    "apply" => {
                        let root = match it
                            .next()
                            .and_then(|o| o.as_ref().to_str().map(|s| s.to_owned()))
                        {
                            Some(r) => r,
                            None => {
                                return CliResult::usage(make_maintain_error(
                                    "apply",
                                    "missing_root",
                                ))
                            }
                        };
                        let target = match it
                            .next()
                            .and_then(|o| o.as_ref().to_str().map(|s| s.to_owned()))
                        {
                            Some(t) => t,
                            None => {
                                return CliResult::usage(make_maintain_error(
                                    "apply",
                                    "missing_target",
                                ))
                            }
                        };
                        let replacement = match it
                            .next()
                            .and_then(|o| o.as_ref().to_str().map(|s| s.to_owned()))
                        {
                            Some(r) => r,
                            None => {
                                return CliResult::usage(make_maintain_error(
                                    "apply",
                                    "missing_replacement",
                                ))
                            }
                        };
                        let digest = match it
                            .next()
                            .and_then(|o| o.as_ref().to_str().map(|s| s.to_owned()))
                        {
                            Some(d) => d,
                            None => {
                                return CliResult::usage(make_maintain_error(
                                    "apply",
                                    "missing_digest",
                                ))
                            }
                        };
                        let authority = match it
                            .next()
                            .and_then(|o| o.as_ref().to_str().map(|s| s.to_owned()))
                        {
                            Some(a) => a,
                            None => {
                                return CliResult::usage(make_maintain_error(
                                    "apply",
                                    "missing_authority",
                                ))
                            }
                        };
                        if it.next().is_some() {
                            return CliResult::usage(make_maintain_error("apply", "too_many_args"));
                        }
                        do_maintain_apply(root, target, replacement, digest, authority)
                    }
                    "revalidate" => {
                        let root = match it
                            .next()
                            .and_then(|o| o.as_ref().to_str().map(|s| s.to_owned()))
                        {
                            Some(r) => r,
                            None => {
                                return CliResult::usage(make_maintain_error(
                                    "revalidate",
                                    "missing_root",
                                ))
                            }
                        };
                        if it.next().is_some() {
                            return CliResult::usage(make_maintain_error(
                                "revalidate",
                                "too_many_args",
                            ));
                        }
                        do_maintain_revalidate(root)
                    }
                    _ => CliResult::usage(make_maintain_error(&sub, "unknown_subcommand")),
                }
            }
            "tui" => {
                if it.next().is_some() {
                    return CliResult::usage(UNKNOWN_COMMAND_ERROR.to_owned());
                }
                if !is_terminal {
                    return CliResult::operation(make_envelope(
                        "tui",
                        "error",
                        "null",
                        &[],
                        &["tui_unavailable_non_tty".to_owned()],
                        "{}",
                        "{}",
                    ));
                }
                CliResult {
                    output: String::new(),
                    exit_code: TypedExit::Success.code(),
                    is_error: false,
                    is_interactive: true,
                }
            }
            "agents" => {
                // Slice 3.4 Packet D1: agents list only (headless, no auth/provider/network, no -p)
                let sub = match it
                    .next()
                    .and_then(|o| o.as_ref().to_str().map(|s| s.to_owned()))
                {
                    Some(s) => s,
                    None => return CliResult::usage(make_agents_error("", "missing_subcommand")),
                };
                match sub.as_str() {
                    "list" => {
                        if it.next().is_some() {
                            return CliResult::usage(make_agents_error("list", "too_many_args"));
                        }
                        do_agents_list()
                    }
                    _ => CliResult::usage(make_agents_error(&sub, "unknown_subcommand")),
                }
            }
            "doctor" => {
                let mode = match it
                    .next()
                    .and_then(|o| o.as_ref().to_str().map(str::to_owned))
                {
                    Some(mode) => mode,
                    None => return CliResult::usage(make_doctor_error("missing_mode")),
                };
                if it.next().is_some() {
                    return CliResult::usage(make_doctor_error("too_many_args"));
                }
                match mode.as_str() {
                    "--onboarding" | "--agent" => do_doctor(&mode),
                    _ => CliResult::usage(make_doctor_error("unknown_mode")),
                }
            }
            "get" => {
                let result_id = match it
                    .next()
                    .and_then(|value| value.as_ref().to_str().map(str::to_owned))
                {
                    Some(value) => value,
                    None => return CliResult::usage(make_get_error("missing_result_id")),
                };
                if it.next().is_some() {
                    return CliResult::usage(make_get_error("too_many_args"));
                }
                do_get(Path::new("."), &result_id)
            }
            "body-preserved" => {
                let root = match it
                    .next()
                    .and_then(|o| o.as_ref().to_str().map(|s| s.to_owned()))
                {
                    Some(r) => r,
                    None => return CliResult::usage(make_body_preserved_error("missing_root")),
                };
                let manifest = match it
                    .next()
                    .and_then(|o| o.as_ref().to_str().map(|s| s.to_owned()))
                {
                    Some(m) => m,
                    None => return CliResult::usage(make_body_preserved_error("missing_manifest")),
                };
                if it.next().is_some() {
                    return CliResult::usage(make_body_preserved_error("too_many_args"));
                }
                do_body_preserved(root, manifest)
            }
            "-p" => {
                // Headless prompt surface: configured transport, never literal secrets.
                let mut rest: Vec<String> = vec![];
                for a in it {
                    match a.as_ref().to_str() {
                        Some(s) => rest.push(s.to_owned()),
                        None => return CliResult::usage(make_p_error("invalid_utf8")),
                    }
                }
                do_headless_p(rest)
            }
            _ => CliResult::usage(UNKNOWN_COMMAND_ERROR.to_owned()),
        }
    }
}

struct CrosstermPort;

impl TerminalPort for CrosstermPort {
    fn enter(&mut self) -> Result<(), String> {
        terminal::enable_raw_mode().map_err(|error| error.to_string())?;
        execute!(std::io::stdout(), EnterAlternateScreen, Hide).map_err(|error| error.to_string())
    }

    fn restore(&mut self) {
        let _ = execute!(std::io::stdout(), Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

#[derive(Clone)]
struct ConnectedCancellation {
    flag: Arc<AtomicBool>,
    published: Arc<AtomicBool>,
    publication: Arc<Mutex<()>>,
}

impl ConnectedCancellation {
    fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            published: Arc::new(AtomicBool::new(false)),
            publication: Arc::new(Mutex::new(())),
        }
    }

    fn cancel(&self) {
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !self.published.load(Ordering::Acquire) {
            self.flag.store(true, Ordering::Release);
        }
    }

    fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

struct ConnectedTuiTask {
    result: mpsc::Receiver<CliResult>,
    cancellation: ConnectedCancellation,
    worker: Option<JoinHandle<()>>,
}

impl ConnectedTuiTask {
    fn start(
        query: String,
        settings: Settings,
        trust_current_root: bool,
        agent: Option<String>,
        model: Option<String>,
        reasoning: Option<String>,
    ) -> Self {
        Self::start_with(move |cancellation| {
            let selection = (agent, model, reasoning);
            do_tui_connected_query(
                Path::new("."),
                query,
                &settings,
                trust_current_root,
                selection,
                Some(cancellation),
            )
        })
    }

    fn start_with(work: impl FnOnce(ConnectedCancellation) -> CliResult + Send + 'static) -> Self {
        let cancellation = ConnectedCancellation::new();
        let worker_cancellation = cancellation.clone();
        let (sender, result) = mpsc::channel();
        let worker = thread::spawn(move || {
            let output = work(worker_cancellation);
            let _ = sender.send(output);
        });
        Self {
            result,
            cancellation,
            worker: Some(worker),
        }
    }

    fn cancel(&self) {
        self.cancellation.cancel();
    }

    fn finished(&mut self) -> Option<CliResult> {
        match self.result.try_recv() {
            Ok(result) => {
                if let Some(worker) = self.worker.take() {
                    let _ = worker.join();
                }
                Some(result)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                if let Some(worker) = self.worker.take() {
                    let _ = worker.join();
                }
                Some(CliResult::operation(make_p_error(
                    "connected_worker_failed",
                )))
            }
        }
    }
}

impl Drop for ConnectedTuiTask {
    fn drop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_tui() -> ExitCode {
    let mut port = CrosstermPort;
    let result = (|| -> Result<(), String> {
        let _guard =
            TerminalGuard::enter(&mut port).map_err(|e| format!("tui terminal error: {e}"))?;
        let mut app = TuiApp::default();
        let mut capability_cache = CapabilityCache::default();
        app.capability = capability_cache.resolve(&app.draft);
        match load_settings(Path::new(".bran/settings.conf")) {
            Ok(Some(settings)) => {
                app.draft = settings;
                app.capability = capability_cache.resolve(&app.draft);
                app.step = OnboardingStep::Complete;
                app.status = "resumed settings".to_owned();
                let req = bran_tui::AdvancedRequest::new(app.draft.clone());
                app.resolved = Some(bran_tui::resolve_advanced(req, app.capability, app.policy));
            }
            Ok(None) => {}
            Err(_) => app.status = "settings unavailable; onboarding".to_owned(),
        }
        let mut connected_task: Option<ConnectedTuiTask> = None;
        let mut quit_after_cancel = false;
        loop {
            if let Some(task) = connected_task.as_mut() {
                if let Some(result) = task.finished() {
                    app.status = result.output;
                    connected_task = None;
                    if quit_after_cancel {
                        return Ok(());
                    }
                }
            }
            if let Err(error) = draw_tui(&app) {
                return Err(format!("tui render error: {error}"));
            }
            if !event::poll(Duration::from_millis(50))
                .map_err(|error| format!("tui input error: {error}"))?
            {
                continue;
            }
            let event = match event::read() {
                Ok(event) => event,
                Err(error) => return Err(format!("tui input error: {error}")),
            };
            match event {
                Event::Resize(columns, _) => app.handle(TuiEvent::Resize { columns }),
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    if let Some(task) = connected_task.as_ref() {
                        if key.code == KeyCode::Esc {
                            task.cancel();
                            app.status = "cancelling connected request".to_owned();
                        } else if key.code == KeyCode::Char('c')
                            && key.modifiers == KeyModifiers::CONTROL
                        {
                            task.cancel();
                            quit_after_cancel = true;
                            app.status = "cancelling before quit".to_owned();
                        }
                        continue;
                    }
                    if let Some(event) = map_key(key) {
                        app.handle(event);
                    } else if key.code == KeyCode::Tab {
                        app.update_suggestions(tui_candidates(&app));
                    }
                    app.capability = capability_cache.resolve(&app.draft);
                }
                _ => continue,
            }
            if let Some(action) = app.take_action() {
                match action {
                    TuiAction::Query {
                        query,
                        trust_current_root,
                        agent,
                        model,
                        reasoning,
                    } if app.draft.profile == OperatingProfile::ConnectedAgent => {
                        let resolved = bran_tui::resolve_advanced(
                            bran_tui::AdvancedRequest::new(app.draft.clone()),
                            app.capability,
                            app.policy,
                        );
                        if resolved.effective_profile != Some(OperatingProfile::ConnectedAgent) {
                            app.status = make_p_error("connected_agent_unavailable");
                        } else if !trust_current_root {
                            app.status = make_p_error("project_trust_required");
                        } else {
                            connected_task = Some(ConnectedTuiTask::start(
                                query,
                                resolved.effective.clone(),
                                true,
                                agent,
                                model,
                                reasoning,
                            ));
                            app.status = "connected request running; Escape cancels".to_owned();
                        }
                        app.resolved = Some(resolved);
                    }
                    TuiAction::Query { query, .. } => match do_query(".".to_owned(), query) {
                        Ok((status, data, warnings, failures, provenance, metrics)) => {
                            app.status = make_envelope(
                                "query",
                                status,
                                &data,
                                &warnings,
                                &failures,
                                &provenance,
                                &metrics,
                            )
                        }
                        Err(error) => {
                            app.status =
                                make_envelope("query", "error", "null", &[], &[error], "{}", "{}")
                        }
                    },
                    TuiAction::Apply => {
                        let path = Path::new(".bran/settings.conf");
                        let resolved = bran_tui::resolve_advanced(
                            bran_tui::AdvancedRequest::new(app.draft.clone()),
                            app.capability,
                            app.policy,
                        );
                        let effective = resolved.effective.clone();
                        let persisted = std::fs::create_dir_all(".bran")
                            .and_then(|_| apply_settings(path, &effective))
                            .is_ok();
                        app.resolved = Some(resolved);
                        if persisted {
                            app.draft = effective;
                        }
                        app.handle(TuiEvent::ApplyFinished { persisted });
                    }
                    TuiAction::Quit => return Ok(()),
                }
            }
        }
    })();
    if let Err(error) = result {
        eprintln!("{}", error);
        return TypedExit::Operation.code();
    }
    ExitCode::SUCCESS
}

fn draw_tui(app: &TuiApp) -> std::io::Result<()> {
    let columns = terminal::size().map(|(columns, _)| columns).unwrap_or(80);
    let mut output = std::io::stdout();
    queue!(
        output,
        Clear(ClearType::All),
        crossterm::cursor::MoveTo(0, 0)
    )?;
    let no_color = std::env::var_os("NO_COLOR").is_some();
    write!(
        output,
        "{}",
        render_app(
            app,
            TerminalCapabilities {
                columns,
                unicode: true,
                no_color
            }
        )
    )?;
    output.flush()
}

fn map_key(key: KeyEvent) -> Option<TuiEvent> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(TuiEvent::Quit),
        (KeyCode::Char('s'), KeyModifiers::CONTROL) => Some(TuiEvent::CtrlS),
        (KeyCode::Char(character), modifiers)
            if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT =>
        {
            Some(TuiEvent::Input(character))
        }
        (KeyCode::Backspace, _) => Some(TuiEvent::Backspace),
        (KeyCode::Enter, _) => Some(TuiEvent::Enter),
        (KeyCode::Esc, _) => Some(TuiEvent::Escape),
        (KeyCode::Up, _) => Some(TuiEvent::Up),
        (KeyCode::Down, _) => Some(TuiEvent::Down),
        _ => None,
    }
}

fn tui_candidates(app: &TuiApp) -> &'static str {
    use bran_tui::OnboardingStep::*;
    match app.step {
        ProjectCheck => "next\ntrust-current-root",
        FlowChoice => "quick\nadvanced",
        OperatingProfile => "offline\ncore-sqz\nconnected",
        Guardrails => "next\nagent=\nmodel=\nreasoning=\ntokens=\nretention=none\nretention=structured\nretention=saved\n+sqz\n-sqz\n+voice\n-voice\n+history\n-history\n+chat\n-chat",
        Diagnose => "next\nrepair",
        Apply => "apply",
        Complete => "trust-current-root\nquery\npacket",
        _ => "back",
    }
}

fn make_envelope(
    command: &str,
    status: &str,
    data: &str,
    warnings: &[String],
    failures: &[String],
    provenance: &str,
    metrics: &str,
) -> String {
    let w = warnings
        .iter()
        .map(|s| format!("\"{}\"", json_escape(s)))
        .collect::<Vec<_>>()
        .join(",");
    let f = failures
        .iter()
        .map(|s| format!("\"{}\"", json_escape(s)))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"schema_version\":\"1.0.0\",\"command\":\"{}\",\"status\":\"{}\",\"data\":{},\"warnings\":[{}],\"failures\":[{}],\"provenance\":{},\"metrics\":{}}}",
        json_escape(command),
        json_escape(status),
        data,
        w,
        f,
        provenance,
        metrics
    )
}

fn make_query_error(detail: &str) -> String {
    make_envelope(
        "query",
        "error",
        "null",
        &[],
        &[detail.to_owned()],
        "{}",
        "{}",
    )
}

fn make_packet_error(detail: &str) -> String {
    make_envelope(
        "packet",
        "error",
        "null",
        &[],
        &[detail.to_owned()],
        "{}",
        "{}",
    )
}

fn make_check_error(detail: &str) -> String {
    make_envelope(
        "check",
        "error",
        "null",
        &[],
        &[detail.to_owned()],
        "{}",
        "{}",
    )
}

fn policy_error_code(error: &PolicyError) -> &'static str {
    match error {
        PolicyError::UnsafePath { .. } => "policy_unsafe_path",
        PolicyError::Io { .. } => "policy_io",
        PolicyError::MalformedYaml { .. } => "policy_malformed_yaml",
        PolicyError::MissingSchemaVersion => "policy_missing_schema_version",
        PolicyError::UnsupportedVersion { .. } => "policy_unsupported_version",
        PolicyError::UnknownField { .. } => "policy_unknown_field",
        PolicyError::UnknownNestedField { .. } => "policy_unknown_nested_field",
        PolicyError::InvalidFieldShape { .. } => "policy_schema_invalid",
        PolicyError::Oversized { .. } => "policy_oversized",
        PolicyError::InvalidMigrationState => "policy_invalid_migration_state",
        PolicyError::InvalidCoverageClass => "policy_invalid_coverage_class",
        PolicyError::EmptyExclusionReason => "policy_empty_exclusion_reason",
        PolicyError::UnsafeDocumentPath => "policy_unsafe_document_path",
        PolicyError::DuplicateKey { .. } => "policy_duplicate_key",
        PolicyError::DuplicateDocumentPath { .. } => "policy_duplicate_document_path",
        PolicyError::OverlappingClassification { .. } => "policy_overlapping_classification",
    }
}

fn policy_error_failure(error: &PolicyError) -> String {
    match error {
        PolicyError::InvalidFieldShape { field, expected } => format!(
            "policy:{}:{}:expected_{}",
            policy_error_code(error),
            field,
            expected
        ),
        _ => format!("policy:{}", policy_error_code(error)),
    }
}

fn make_maintain_error(sub: &str, detail: &str) -> String {
    let command = if sub.is_empty() {
        "maintain".to_owned()
    } else {
        format!("maintain.{}", sub)
    };
    make_envelope(
        &command,
        "error",
        "null",
        &[],
        &[detail.to_owned()],
        "{}",
        "{}",
    )
}

fn make_agents_error(sub: &str, detail: &str) -> String {
    let command = if sub.is_empty() {
        "agents".to_owned()
    } else {
        format!("agents.{}", sub)
    };
    make_envelope(
        &command,
        "error",
        "null",
        &[],
        &[detail.to_owned()],
        "{}",
        "{}",
    )
}

fn make_p_error(detail: &str) -> String {
    make_envelope("p", "error", "null", &[], &[detail.to_owned()], "{}", "{}")
}

fn make_get_error(detail: &str) -> String {
    make_envelope(
        "get",
        "error",
        "null",
        &[],
        &[detail.to_owned()],
        "{}",
        "{}",
    )
}

fn make_doctor_error(detail: &str) -> String {
    make_envelope(
        "doctor",
        "error",
        "null",
        &[],
        &[detail.to_owned()],
        "{}",
        "{}",
    )
}

fn make_body_preserved_error(detail: &str) -> String {
    make_envelope(
        "body-preserved",
        "error",
        "null",
        &[],
        &[detail.to_owned()],
        "{\"sources\":[\"bran-core\",\"body-preservation\"]}",
        "{}",
    )
}

fn body_preserved_report_json(report: &migration::BodyPreservedReport) -> String {
    let files_json: Vec<String> = report
        .files
        .iter()
        .map(|f| {
            format!(
                "{{\"original_path\":\"{}\",\"migrated_path\":\"{}\",\"original_body_sha256\":\"{}\",\"migrated_body_sha256\":\"{}\",\"body_preserved\":{}}}",
                json_escape(&f.original_path),
                json_escape(&f.migrated_path),
                json_escape(&f.original_body_sha256),
                json_escape(&f.migrated_body_sha256),
                f.body_preserved
            )
        })
        .collect();
    let findings_json: Vec<String> = report
        .findings
        .iter()
        .map(|f| {
            format!(
                "{{\"severity\":\"{}\",\"code\":\"{}\",\"path\":\"{}\",\"message\":\"{}\"}}",
                json_escape(&f.severity),
                json_escape(&f.code),
                json_escape(&f.path),
                json_escape(&f.message)
            )
        })
        .collect();
    let files_checked = report.files.len();
    format!(
        "{{\"schema_version\":{},\"manifest_path\":\"{}\",\"files\":[{}],\"findings\":[{}],\"passed\":{},\"metrics\":{{\"files_checked\":{},\"findings_count\":{}}}}}",
        report.schema_version,
        json_escape(&report.manifest_path),
        files_json.join(","),
        findings_json.join(","),
        report.passed,
        files_checked,
        findings_json.len()
    )
}

fn do_body_preserved(root: String, manifest: String) -> CliResult {
    let root_path = Path::new(&root);
    let report = match migration::validate_body_preservation_from_path(root_path, &manifest) {
        Ok(report) => report,
        Err(error) => {
            let code = match error {
                MigrationError::ReadError => {
                    return CliResult::operation(make_envelope(
                        "body-preserved",
                        "error",
                        "null",
                        &[],
                        &["body_preserved_io_unavailable".to_owned()],
                        "{\"sources\":[\"bran-core\",\"body-preservation\"]}",
                        "{}",
                    ));
                }
                _ => "body_preserved_validation_failed",
            };
            return CliResult {
                output: make_envelope(
                    "body-preserved",
                    "error",
                    "null",
                    &[],
                    &[code.to_owned()],
                    "{\"sources\":[\"bran-core\",\"body-preservation\"]}",
                    "{}",
                ),
                exit_code: TypedExit::Validation.code(),
                is_error: true,
                is_interactive: false,
            };
        }
    };
    let data = body_preserved_report_json(&report);
    let files_checked = report.files.len();
    let findings_count = report.findings.len();
    let status = if report.passed { "ok" } else { "failed" };
    let exit_code = if report.passed {
        TypedExit::Success.code()
    } else {
        TypedExit::Validation.code()
    };
    CliResult {
        output: make_envelope(
            "body-preserved",
            status,
            &data,
            &[],
            &[],
            "{\"sources\":[\"bran-core\",\"body-preservation\"]}",
            &format!(
                "{{\"files_checked\":{},\"findings_count\":{}}}",
                files_checked, findings_count
            ),
        ),
        exit_code,
        is_error: !report.passed,
        is_interactive: false,
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            _ => out.push(c),
        }
    }
    out
}

/// Small type aliases to keep return signatures readable (addresses clippy::type-complexity).
type QueryPacketResult = Result<
    (
        &'static str,
        String,
        Vec<String>,
        Vec<String>,
        String,
        String,
    ),
    String,
>;
type PacketResult = Result<(String, Vec<String>, Vec<String>, String, String), CliPacketFailure>;

struct CliPacketFailure {
    detail: String,
    data: String,
    metrics: String,
}

impl From<String> for CliPacketFailure {
    fn from(detail: String) -> Self {
        Self {
            detail,
            data: "null".to_owned(),
            metrics: "{}".to_owned(),
        }
    }
}
type CheckResult = Result<
    (
        String,
        Vec<String>,
        Vec<String>,
        ExitCode,
        &'static str,
        String,
        String,
    ),
    String,
>;

const QUERY_RESULT_LIMIT: usize = 32;
const DEPENDENCY_DEPTH_LIMIT: usize = 4;
const EXCERPT_BYTE_LIMIT: usize = 4_096;

#[derive(Clone, Copy, Default)]
struct ExperimentalControls {
    max_sources: Option<usize>,
    effective_max_sources: Option<usize>,
    dependency_depth: Option<usize>,
    response_sources: Option<usize>,
    excerpt_bytes: Option<usize>,
}

impl ExperimentalControls {
    fn max_sources(self) -> usize {
        self.effective_max_sources
            .or(self.max_sources)
            .unwrap_or(QUERY_RESULT_LIMIT)
    }

    fn dependency_depth(self) -> usize {
        self.dependency_depth.unwrap_or(DEPENDENCY_DEPTH_LIMIT)
    }

    fn controls_json(self) -> String {
        format!(
            "{{\"requested\":{{\"max_sources\":{},\"dependency_depth\":{},\"response_sources\":{},\"excerpt_bytes\":{}}},\"effective\":{{\"max_sources\":{},\"dependency_depth\":{},\"response_sources\":{},\"excerpt_bytes\":{}}}}}",
            option_json(self.max_sources), option_json(self.dependency_depth),
            option_json(self.response_sources), option_json(self.excerpt_bytes),
            self.max_sources(), self.dependency_depth(),
            option_json(self.response_sources),
            option_json(self.excerpt_bytes),
        )
    }
}

fn option_json(value: Option<usize>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}

fn experimental_controls(
    args: &[String],
) -> Result<(ExperimentalControls, Vec<String>), &'static str> {
    let mut controls = ExperimentalControls::default();
    let mut positionals = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let slot = match arg.as_str() {
            "--max-sources" => Some((&mut controls.max_sources, QUERY_RESULT_LIMIT)),
            "--dependency-depth" => Some((&mut controls.dependency_depth, DEPENDENCY_DEPTH_LIMIT)),
            "--response-sources" => Some((&mut controls.response_sources, QUERY_RESULT_LIMIT)),
            "--excerpt-bytes" => Some((&mut controls.excerpt_bytes, EXCERPT_BYTE_LIMIT)),
            _ => None,
        };
        if let Some((slot, maximum)) = slot {
            if slot.is_some() {
                return Err("duplicate_option");
            }
            i += 1;
            let Some(value) = args.get(i) else {
                return Err("missing_value");
            };
            let Ok(value) = value.parse::<usize>() else {
                return Err("invalid_control");
            };
            if (arg != "--dependency-depth" && value == 0) || value > maximum {
                return Err("invalid_control");
            }
            *slot = Some(value);
        } else if arg.starts_with("--") {
            return Err("unknown_option");
        } else {
            positionals.push(arg.clone());
        }
        i += 1;
    }
    Ok((controls, positionals))
}

#[derive(Clone, Debug)]
struct SourceRanking {
    id: NodeId,
    locator: String,
    rank: usize,
    exact_matches: usize,
    partial_matches: usize,
    match_reason: String,
    active: u8,
    canonical: u8,
    public_safe: u8,
    confidence: u8,
    freshness: String,
}

const RANKED_FACT_KEYS: &[&str] = &[
    "title",
    "tags",
    "tag",
    "alias",
    "aliases",
    "purpose",
    "responsibility",
    "symbol",
    "implements",
    "validates",
    "test",
    "tests",
    "keyword",
    "keywords",
];
const PARENT_QUERY_FACT_KEYS: &[&str] = &["okf_status", "status", "public_boundary"];

/// Common grammatical words that must not drive selection (issue #18).
const QUERY_STOP_WORDS: &[&str] = &["about", "and", "for", "from", "not", "the", "this", "with"];

/// Plain query words and high-specificity identifier units.
///
/// A hyphenated, underscored, or dotted identifier ("example-entity-unit") is
/// one high-specificity unit for match purposes: its sub-tokens are never
/// extracted as independent terms, so a sub-token match cannot count as the
/// entity matching (issue #18).
fn query_terms_and_entities(query_text: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut terms = BTreeSet::new();
    let mut entities = BTreeSet::new();
    let mut run = String::new();
    for character in query_text.chars() {
        if character.is_alphanumeric() || matches!(character, '-' | '_' | '.') {
            run.push(character.to_ascii_lowercase());
        } else {
            classify_query_run(&run, &mut terms, &mut entities);
            run.clear();
        }
    }
    classify_query_run(&run, &mut terms, &mut entities);
    (terms, entities)
}

/// One maximal identifier-character run becomes either a high-specificity
/// entity unit or a plain word. Short separator fragments ("e.g.", "v0.2")
/// are dropped entirely rather than treated as words.
fn classify_query_run(run: &str, terms: &mut BTreeSet<String>, entities: &mut BTreeSet<String>) {
    if run.is_empty() {
        return;
    }
    let trimmed = run.trim_matches(['-', '_', '.']);
    if trimmed.contains(['-', '_', '.']) {
        if trimmed.len() >= 6 {
            entities.insert(trimmed.to_owned());
        }
        return;
    }
    if trimmed.len() >= 3 && !QUERY_STOP_WORDS.contains(&trimmed) {
        terms.insert(trimmed.to_owned());
    }
}

/// The body content of one scanned entry: source bytes after the frontmatter
/// (or commented YAML) header, or the whole source when no header is present.
fn document_body(snapshot: &ScanSnapshot, locator: &str) -> Option<String> {
    let entry = snapshot.entries.get(locator)?;
    let source = std::str::from_utf8(entry.source.as_ref()).ok()?;
    let start = entry
        .metadata
        .facts
        .iter()
        .find_map(|fact| match &fact.provenance {
            FactProvenance::MarkdownFrontmatter => markdown_header_end(source),
            FactProvenance::CommentedYaml => commented_header_end(source),
            _ => None,
        })
        .unwrap_or(0);
    Some(source[start..].to_owned())
}

/// One warning naming every query term or entity unit that matched no
/// document, when any. Unmatched entity units lead the list and are named
/// whole, never as their sub-tokens.
fn unmatched_query_warnings(query_text: &str, matched_terms: &BTreeSet<String>) -> Vec<String> {
    let (terms, entities) = query_terms_and_entities(query_text);
    let unmatched = entities
        .iter()
        .filter(|term| !matched_terms.contains(*term))
        .cloned()
        .chain(
            terms
                .iter()
                .filter(|term| !matched_terms.contains(*term))
                .cloned(),
        )
        .collect::<Vec<_>>();
    if unmatched.is_empty() {
        return vec![];
    }
    let listed = unmatched.len().min(8);
    let mut message = format!("unmatched_query_terms: {}", unmatched[..listed].join(","));
    if unmatched.len() > listed {
        message.push_str(&format!(",+{} more", unmatched.len() - listed));
    }
    vec![message]
}

fn source_rankings(
    graph_input: &GraphInput,
    snapshot: &ScanSnapshot,
    query_text: &str,
    max_sources: usize,
) -> (Vec<SourceRanking>, BTreeSet<String>) {
    let (terms, entities) = query_terms_and_entities(query_text);
    let mut matched_terms = BTreeSet::new();
    let mut matches = graph_input
        .nodes()
        .iter()
        .filter(|node| node.role() == NodeRole::Document)
        .filter_map(|node| {
            let locator_original = node.provenance().locator();
            let locator = locator_original.to_ascii_lowercase();
            let body =
                document_body(snapshot, locator_original).map(|body| body.to_ascii_lowercase());
            let mut exact_matches = 0;
            let mut partial_matches = 0;
            let mut exact_fields = BTreeSet::new();
            let mut partial_fields = BTreeSet::new();
            // A high-specificity identifier unit matches only as a whole; a
            // sub-token match never counts as the entity matching.
            for entity in &entities {
                let mut entity_fields = BTreeSet::new();
                if locator.contains(entity.as_str()) {
                    entity_fields.insert("path");
                }
                for (key, value) in RANKED_FACT_KEYS.iter().flat_map(|key| {
                    node.facts()
                        .values(key)
                        .into_iter()
                        .flatten()
                        .map(move |value| (*key, value.as_str()))
                }) {
                    if value.to_ascii_lowercase().contains(entity.as_str()) {
                        entity_fields.insert(key);
                    }
                }
                for (key, value) in PARENT_QUERY_FACT_KEYS.iter().flat_map(|key| {
                    node.facts()
                        .values(key)
                        .into_iter()
                        .flatten()
                        .map(move |value| (*key, value.as_str()))
                }) {
                    if value.to_ascii_lowercase().contains(entity.as_str()) {
                        entity_fields.insert(key);
                    }
                }
                if let Some(body) = &body {
                    if body.contains(entity.as_str()) {
                        entity_fields.insert("body");
                    }
                }
                if !entity_fields.is_empty() {
                    exact_matches += 1;
                    exact_fields.extend(entity_fields);
                    matched_terms.insert(entity.clone());
                }
            }
            for (key, value) in std::iter::once(("path", locator.as_str())).chain(
                RANKED_FACT_KEYS.iter().flat_map(|key| {
                    node.facts()
                        .values(key)
                        .into_iter()
                        .flatten()
                        .map(move |value| (*key, value.as_str()))
                }),
            ) {
                let value = value.to_ascii_lowercase();
                for term in &terms {
                    if value == *term {
                        exact_matches += 1;
                        exact_fields.insert(key);
                        matched_terms.insert(term.clone());
                    } else if key == "path"
                        && value
                            .split(|character: char| !character.is_alphanumeric())
                            .any(|part| part == term)
                    {
                        // A path-segment equality is a containment signal, not
                        // a whole-value equality: a generic query term that
                        // happens to appear in an unrelated path must not
                        // outrank real content matches.
                        partial_matches += 1;
                        partial_fields.insert("path");
                        matched_terms.insert(term.clone());
                    } else if value.contains(term.as_str()) {
                        partial_matches += 1;
                        partial_fields.insert(key);
                        matched_terms.insert(term.clone());
                    }
                }
            }
            for (key, value) in PARENT_QUERY_FACT_KEYS.iter().flat_map(|key| {
                node.facts()
                    .values(key)
                    .into_iter()
                    .flatten()
                    .map(move |value| (*key, value.as_str()))
            }) {
                let value = value.to_ascii_lowercase();
                for term in &terms {
                    if value.contains(term.as_str()) {
                        partial_matches += 1;
                        partial_fields.insert(key);
                        matched_terms.insert(term.clone());
                    }
                }
            }
            let mut body_matches = 0;
            if let Some(body) = &body {
                for term in &terms {
                    if body.contains(term.as_str()) {
                        body_matches += 1;
                        matched_terms.insert(term.clone());
                    }
                }
            }
            if body_matches > 0 {
                partial_matches += body_matches;
                partial_fields.insert("body");
            }
            let status_rank = match node
                .facts()
                .values("okf_status")
                .or_else(|| node.facts().values("status"))
                .and_then(|values| values.first())
                .map(String::as_str)
            {
                Some("active") => 3,
                Some("draft") => 2,
                Some("deprecated") => 1,
                _ => 0,
            };
            let public_safe =
                node.facts()
                    .values("public_boundary")
                    .into_iter()
                    .flatten()
                    .any(|value| matches!(value.as_str(), "public" | "safe")) as u8;
            let canonical = (node.facts().contains_value("canonical", "true")
                || node.facts().contains_value("canonical", "yes")
                || ((node.facts().values("authority").is_some()
                    || node.facts().values("resource").is_some())
                    && status_rank > 0)) as u8;
            let freshness = node
                .facts()
                .values("timestamp")
                .or_else(|| node.facts().values("freshness"))
                .and_then(|values| values.first())
                .cloned()
                .unwrap_or_default();
            (exact_matches + partial_matches > 0).then(|| SourceRanking {
                id: node.id().clone(),
                locator: node.provenance().locator().to_owned(),
                rank: 0,
                exact_matches,
                partial_matches,
                match_reason: if exact_fields.is_empty() {
                    format!(
                        "partial:{}",
                        partial_fields.into_iter().collect::<Vec<_>>().join("+")
                    )
                } else {
                    format!(
                        "exact:{}",
                        exact_fields.into_iter().collect::<Vec<_>>().join("+")
                    )
                },
                active: status_rank,
                canonical,
                public_safe,
                confidence: node.confidence().value(),
                freshness,
            })
        })
        .collect::<Vec<_>>();
    // A high-specificity entity unit that matched no document means the query
    // names something this repository does not contain. When nothing else in
    // the query matched at identity level (exact fact or path equality), the
    // remaining generic body matches are not evidence for the entity: return
    // no rankings so a caller cannot mistake command success for evidence
    // coverage (issue #18). Exact content matches keep the rankings, with the
    // unmatched unit still surfaced by name in the warnings.
    if entities
        .iter()
        .any(|entity| !matched_terms.contains(entity))
        && !matches.iter().any(|ranking| ranking.exact_matches > 0)
    {
        return (Vec::new(), matched_terms);
    }
    matches.sort_by(|left, right| {
        right
            .exact_matches
            .cmp(&left.exact_matches)
            .then_with(|| right.partial_matches.cmp(&left.partial_matches))
            .then_with(|| right.active.cmp(&left.active))
            .then_with(|| right.canonical.cmp(&left.canonical))
            .then_with(|| right.public_safe.cmp(&left.public_safe))
            .then_with(|| right.confidence.cmp(&left.confidence))
            .then_with(|| right.freshness.cmp(&left.freshness))
            .then_with(|| left.id.cmp(&right.id))
    });
    matches.truncate(max_sources);
    for (index, ranking) in matches.iter_mut().enumerate() {
        ranking.rank = index + 1;
    }
    (matches, matched_terms)
}

fn query_view_spec(rankings: &[SourceRanking], max_sources: usize) -> ViewSpec {
    let ids = rankings.iter().map(|ranking| ranking.id.clone()).collect();
    ViewSpec {
        source: ViewSource::NodeIds(ids),
        filter: ViewFilter::All,
        sort: ViewSort::NodeId,
        grouping: ViewGrouping::None,
        fields: vec![ViewField::ProvenanceLocator, ViewField::NodeId],
        presentation: Presentation::Json,
        max_items: max_sources,
        max_bytes: 1_048_576,
    }
}

fn source_rankings_json(rankings: &[SourceRanking], selected_ids: &BTreeSet<NodeId>) -> String {
    rankings
        .iter()
        .filter(|ranking| selected_ids.contains(&ranking.id))
        .map(|ranking| format!(
            "{{\"locator\":\"{}\",\"rank\":{},\"score\":{{\"exact\":{},\"partial\":{},\"active\":{},\"canonical\":{},\"public_safe\":{},\"confidence\":{},\"freshness\":\"{}\"}},\"match_reason\":\"{}\"}}",
            json_escape(&ranking.locator), ranking.rank, ranking.exact_matches,
            ranking.partial_matches, ranking.active, ranking.canonical, ranking.public_safe,
            ranking.confidence, json_escape(&ranking.freshness), json_escape(&ranking.match_reason)
        ))
        .collect::<Vec<_>>()
        .join(",")
}

fn ranking_freshness(ranking: &SourceRanking) -> u64 {
    ranking
        .freshness
        .bytes()
        .take(8)
        .fold(0u64, |value, byte| (value << 8) | u64::from(byte))
}

fn locator_evidence_content(
    node: &NodeInput,
    ranking: Option<&SourceRanking>,
    graph: &KnowledgeGraph,
    content_digest: Option<&str>,
    excerpt: Option<&str>,
) -> String {
    let metadata = RANKED_FACT_KEYS
        .iter()
        .chain(PARENT_QUERY_FACT_KEYS.iter())
        .filter_map(|key| node.facts().values(key).map(|values| (*key, values)))
        .flat_map(|(key, values)| {
            values
                .iter()
                .take(2)
                .map(move |value| format!("{key}={value}"))
        })
        .take(12)
        .collect::<Vec<_>>()
        .join(";");
    let relationships = graph
        .forward_edges(node.id())
        .iter()
        .filter_map(|edge_id| graph.edge(edge_id))
        .filter(|edge| edge.certainty() == EdgeCertainty::Known)
        .filter_map(|edge| {
            relationship_reason(edge.relationship()).and_then(|reason| {
                graph
                    .node(edge.target())
                    .map(|target| format!("{reason}={}", target.provenance().locator()))
            })
        })
        .take(4)
        .collect::<Vec<_>>()
        .join(";");
    let digest = content_digest.map_or_else(String::new, |digest| {
        format!("content-digest-sha256: {digest}\n")
    });
    let excerpt = excerpt.map_or_else(String::new, |excerpt| format!("excerpt: {excerpt}\n"));
    match ranking {
        Some(ranking) => format!(
            "path: {}\nrank: {}\nscore: exact={} partial={} active={} canonical={} public_safe={} confidence={} freshness={}\nmatch_reason: {}\nmetadata: {}\nrelationships: {}\n{}{}",
            ranking.locator, ranking.rank, ranking.exact_matches, ranking.partial_matches,
            ranking.active, ranking.canonical, ranking.public_safe, ranking.confidence,
            ranking.freshness, ranking.match_reason, metadata, relationships, digest, excerpt
        ),
        None => format!(
            "path: {}\nrank: dependency\nmetadata: {}\nrelationships: {}\n{}{}",
            node.provenance().locator(), metadata, relationships, digest, excerpt
        ),
    }
}

fn public_safe_excerpt(
    node: &NodeInput,
    snapshot: &ScanSnapshot,
    limit: Option<usize>,
) -> Option<String> {
    let limit = limit?;
    let boundaries = node.facts().values("public_boundary")?;
    if boundaries.is_empty()
        || !boundaries
            .iter()
            .all(|value| matches!(value.as_str(), "public" | "safe"))
    {
        return None;
    }
    let entry = snapshot.entries.get(node.provenance().locator())?;
    let source = std::str::from_utf8(&entry.source).ok()?;
    let start = entry
        .metadata
        .facts
        .iter()
        .find_map(|fact| match &fact.provenance {
            FactProvenance::MarkdownFrontmatter => markdown_header_end(source),
            FactProvenance::CommentedYaml => commented_header_end(source),
            _ => None,
        })
        .unwrap_or(0);
    let source = &source[start..];
    let mut end = limit.min(source.len());
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    Some(source[..end].to_owned())
}

fn markdown_header_end(source: &str) -> Option<usize> {
    let mut lines = source.split_inclusive('\n');
    let first = lines.next()?;
    if first.trim_start_matches('\u{feff}').trim() != "---" {
        return None;
    }
    let mut offset = first.len();
    for line in lines {
        offset += line.len();
        if line.trim() == "---" {
            return Some(offset);
        }
    }
    None
}

fn line_comment_header_end(source: &str) -> Option<usize> {
    let mut lines = source.split_inclusive('\n');
    let first = lines.next()?;
    let prefix = ["//", "#"].into_iter().find(|prefix| {
        line_comment_content(first, prefix).is_some_and(|line| line.trim() == "---")
    })?;
    let mut offset = first.len();
    for line in lines {
        offset += line.len();
        if line_comment_content(line, prefix)?.trim() == "---" {
            return Some(offset);
        }
    }
    None
}

fn commented_header_end(source: &str) -> Option<usize> {
    line_comment_header_end(source).or_else(|| block_comment_header_end(source))
}

fn block_comment_header_end(source: &str) -> Option<usize> {
    let mut lines = source.split_inclusive('\n');
    let first = lines.next()?;
    if first.trim_start().strip_prefix("/*")?.trim() != "---" {
        return None;
    }
    let mut offset = first.len();
    while let Some(line) = lines.next() {
        offset += line.len();
        let line = line.trim_start().strip_prefix('*')?;
        if line.trim() == "---" {
            let close = lines.next()?;
            offset += close.len();
            return (close.trim() == "*/").then_some(offset);
        }
    }
    None
}

fn line_comment_content<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let line = line.trim_start().strip_prefix(prefix)?;
    let line = line.strip_prefix(['/', '!']).unwrap_or(line);
    let line = line.strip_prefix(' ').unwrap_or(line);
    Some(line.strip_prefix(' ').unwrap_or(line))
}

fn relationship_reason(relationship: EdgeRelationship) -> Option<&'static str> {
    match relationship {
        EdgeRelationship::Dependency => Some("declared_dependency"),
        EdgeRelationship::Implementation => Some("declared_implementation"),
        EdgeRelationship::Replacement => Some("declared_replacement"),
        EdgeRelationship::Supersedes => Some("declared_supersedes"),
        EdgeRelationship::Validation => Some("declared_validation"),
        EdgeRelationship::Reachability => Some("declared_reachability"),
        _ => None,
    }
}

fn selected_sources(
    selected_ids: &BTreeSet<NodeId>,
    seed_ids: &BTreeSet<NodeId>,
    graph: &KnowledgeGraph,
    snapshot: &ScanSnapshot,
) -> Vec<(String, &'static str)> {
    let mut selected = BTreeMap::new();
    for id in selected_ids {
        let Some(node) = graph.node(id) else {
            continue;
        };
        let locator = node.provenance().locator();
        if !snapshot.entries.contains_key(locator) {
            continue;
        }
        let reason = if seed_ids.contains(id) {
            "metadata_seed"
        } else {
            selected_ids
                .iter()
                .flat_map(|source| graph.forward_edges(source))
                .filter_map(|edge_id| graph.edge(edge_id))
                .find_map(|edge| {
                    (edge.target() == id && edge.certainty() == EdgeCertainty::Known)
                        .then(|| relationship_reason(edge.relationship()))
                        .flatten()
                })
                .unwrap_or("declared_relationship")
        };
        selected.insert(locator.to_owned(), reason);
    }
    selected.into_iter().collect()
}

fn selected_sources_json(selected: &[(String, &'static str)]) -> (String, String) {
    let locators = selected
        .iter()
        .map(|(locator, _)| format!("\"{}\"", json_escape(locator)))
        .collect::<Vec<_>>()
        .join(",");
    let reasons = selected
        .iter()
        .map(|(locator, reason)| {
            format!(
                "{{\"locator\":\"{}\",\"reason\":\"{}\"}}",
                json_escape(locator),
                reason
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    (locators, reasons)
}

fn do_query(root: String, query_text: String) -> QueryPacketResult {
    let root_path: &Path = Path::new(&root);
    let scanner = RepositoryScanner::new(root_path, ScanConfig::default())
        .map_err(|e| format!("scan_error: {:?}", e))?;
    if !root_path.join(POLICY_FILENAME).is_file() {
        let data = format!(
            "{{\"root\":\"{}\",\"query\":\"{}\",\"bran_status\":\"unavailable\",\"selected_locators\":[],\"why_selected\":[],\"source_rankings\":[],\"candidate_source_bytes\":0,\"selected_source_bytes\":0,\"context_bytes_avoided\":0,\"estimated_tokens\":0,\"token_estimate_method\":\"bytes-divided-by-four-ceiling\",\"actual_model_input_tokens\":\"unavailable\"}}",
            json_escape(&root),
            json_escape(&query_text)
        );
        return Ok((
            "unavailable",
            data,
            vec!["native_policy_unavailable".to_owned()],
            vec![],
            "{\"sources\":[\"repository-policy-check\"]}".to_owned(),
            "{\"candidate_source_bytes\":0,\"selected_source_bytes\":0,\"context_bytes_avoided\":0,\"estimated_tokens\":0,\"token_estimate_method\":\"bytes-divided-by-four-ceiling\"}".to_owned(),
        ));
    }
    let snapshot = scanner.scan().map_err(|e| format!("scan_error: {:?}", e))?;
    let graph_input = snapshot
        .graph_input()
        .map_err(|e| format!("graph_input_error: {:?}", e))?;
    let node_count = graph_input.nodes().len().max(1);
    let edge_count = graph_input.edges().len().max(1);
    let limits =
        GraphLimits::new(node_count, edge_count).map_err(|e| format!("limits_error: {:?}", e))?;
    let (rankings, matched_terms) =
        source_rankings(&graph_input, &snapshot, &query_text, QUERY_RESULT_LIMIT);
    let spec = query_view_spec(&rankings, QUERY_RESULT_LIMIT);
    let graph =
        KnowledgeGraph::build(graph_input, limits).map_err(|e| format!("graph_error: {:?}", e))?;

    let compiled = ViewCompiler::new()
        .compile(&spec, &graph)
        .map_err(|e| format!("view_error: {:?}", e))?;
    let dependency_limits = DependencyClosureLimits::new(DEPENDENCY_DEPTH_LIMIT, 256)
        .map_err(|e| format!("dep_limits: {:?}", e))?;
    let selected_ids = PacketAssembler::evidence_ids(&compiled, &graph, dependency_limits)
        .map_err(|e| format!("packet_error: {:?}", e))?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let seed_ids = compiled
        .items()
        .iter()
        .map(|item| item.id.clone())
        .collect::<BTreeSet<_>>();
    let selected = selected_sources(&selected_ids, &seed_ids, &graph, &snapshot);
    let selected_bytes = selected
        .iter()
        .filter_map(|(locator, _)| snapshot.entries.get(locator))
        .map(|entry| entry.source.len())
        .sum::<usize>();
    let candidate_bytes = snapshot.total_bytes;
    let estimated = selected_bytes / 4 + usize::from(!selected_bytes.is_multiple_of(4));
    let context_bytes_avoided = candidate_bytes.saturating_sub(selected_bytes);

    let mut warns: Vec<String> = snapshot
        .diagnostics
        .iter()
        .map(|d| format!("{:?}", d))
        .collect();
    warns.extend(unmatched_query_warnings(&query_text, &matched_terms));

    let (locs_json, why_selected_json) = selected_sources_json(&selected);
    let source_rankings_json = source_rankings_json(&rankings, &selected_ids);
    let data = format!(
        "{{\"root\":\"{}\",\"query\":\"{}\",\"selected_locators\":[{}],\"why_selected\":[{}],\"source_rankings\":[{}],\"candidate_source_bytes\":{},\"selected_source_bytes\":{},\"context_bytes_avoided\":{},\"estimated_tokens\":{},\"token_estimate_method\":\"bytes-divided-by-four-ceiling\",\"actual_model_input_tokens\":\"unavailable\"}}",
        json_escape(&root),
        json_escape(&query_text),
        locs_json,
        why_selected_json,
        source_rankings_json,
        candidate_bytes,
        selected_bytes,
        context_bytes_avoided,
        estimated
    );
    let provenance = if locs_json.is_empty() {
        "{\"sources\":[\"repository-scanner\",\"bran-core\"]}".to_owned()
    } else {
        format!(
            "{{\"sources\":[\"repository-scanner\",\"bran-core\"],\"selected_locators\":[{}],\"why_selected\":[{}],\"source_rankings\":[{}]}}",
            locs_json, why_selected_json, source_rankings_json
        )
    };
    let metrics = format!(
        "{{\"candidate_source_bytes\":{},\"selected_source_bytes\":{},\"context_bytes_avoided\":{},\"estimated_tokens\":{},\"token_estimate_method\":\"bytes-divided-by-four-ceiling\"}}",
        candidate_bytes, selected_bytes, context_bytes_avoided, estimated
    );
    Ok(("ok", data, warns, vec![], provenance, metrics))
}

fn do_packet(root: String, query_text: String, controls: &ExperimentalControls) -> PacketResult {
    let root_path: &Path = Path::new(&root);
    let settings = load_settings(&root_path.join(".bran/settings.conf"))
        .map_err(|e| format!("settings_error: {e}"))?;
    let token_ceiling = settings.as_ref().and_then(|settings| {
        settings
            .connected_agent_task_token_ceiling
            .map(|value| value as usize)
    });
    let sqz_enabled = settings.as_ref().is_some_and(|settings| settings.sqz);
    let scanner = RepositoryScanner::new(root_path, ScanConfig::default())
        .map_err(|e| format!("scan_error: {:?}", e))?;
    let snapshot = scanner.scan().map_err(|e| format!("scan_error: {:?}", e))?;
    let graph_input = snapshot
        .graph_input()
        .map_err(|e| format!("graph_input_error: {:?}", e))?;
    let node_count = graph_input.nodes().len().max(1);
    let edge_count = graph_input.edges().len().max(1);
    let limits =
        GraphLimits::new(node_count, edge_count).map_err(|e| format!("limits_error: {:?}", e))?;
    let (rankings, matched_terms) =
        source_rankings(&graph_input, &snapshot, &query_text, controls.max_sources());
    let spec = query_view_spec(&rankings, controls.max_sources());
    let graph =
        KnowledgeGraph::build(graph_input, limits).map_err(|e| format!("graph_error: {:?}", e))?;

    let compiled = ViewCompiler::new()
        .compile(&spec, &graph)
        .map_err(|e| format!("view_error: {:?}", e))?;

    let candidate_bytes = snapshot.total_bytes;

    let pkt_limits = PacketLimits::new(256, 4 * 1024 * 1024, token_ceiling);
    let dep_limits = DependencyClosureLimits::new(controls.dependency_depth(), 256)
        .map_err(|e| format!("dep_limits: {:?}", e))?;
    let evidence_ids = PacketAssembler::evidence_ids(&compiled, &graph, dep_limits)
        .map_err(|e| format!("packet_error: {:?}", e))?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let ranking_by_id = rankings
        .iter()
        .map(|ranking| (ranking.id.clone(), ranking))
        .collect::<BTreeMap<_, _>>();
    let evidence = evidence_ids
        .iter()
        .filter_map(|id| graph.node(id))
        .enumerate()
        .map(|(index, node)| {
            let ranking = ranking_by_id.get(node.id()).copied();
            let anchors = ranking
                .filter(|ranking| ranking.rank == 1)
                .and_then(|_| {
                    PreservationAnchor::new(
                        format!("ranked-source-{index}"),
                        node.provenance().locator(),
                    )
                    .ok()
                })
                .into_iter()
                .collect::<Vec<_>>();
            EvidenceContent::new(
                node.id().clone(),
                locator_evidence_content(
                    node,
                    ranking,
                    &graph,
                    None,
                    public_safe_excerpt(node, &snapshot, controls.excerpt_bytes).as_deref(),
                ),
                if !anchors.is_empty() {
                    EvidencePriority::Required
                } else if ranking.is_some() {
                    EvidencePriority::Recommended
                } else {
                    EvidencePriority::Related
                },
                ranking.map_or(0, |ranking| u64::MAX - ranking.rank as u64),
                ranking.map_or(0, ranking_freshness),
                anchors,
            )
        })
        .collect::<Vec<_>>();
    let req = PacketAssemblyRequest {
        view: &compiled,
        graph: &graph,
        evidence: &evidence,
        limits: pkt_limits,
        dependency_limits: dep_limits,
    };
    let pkt = PacketAssembler::new()
        .assemble(&req)
        .map_err(|e| format!("packet_error: {:?}", e))?;
    let excerpt_bytes = pkt
        .items
        .iter()
        .filter_map(|item| graph.node(&item.id))
        .filter_map(|node| public_safe_excerpt(node, &snapshot, controls.excerpt_bytes))
        .map(|excerpt| excerpt.len())
        .sum::<usize>();
    let selected_id_set = pkt
        .receipt
        .selected_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let seed_id_set = pkt
        .receipt
        .seed_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let selected = selected_sources(&selected_id_set, &seed_id_set, &graph, &snapshot);
    let selected_source_bytes = selected
        .iter()
        .filter_map(|(locator, _)| snapshot.entries.get(locator))
        .map(|entry| entry.source.len())
        .sum::<usize>();
    let context_bytes_avoided = candidate_bytes.saturating_sub(selected_source_bytes);

    let (selected_locators_json, why_selected_json) = selected_sources_json(&selected);
    let source_rankings_json = source_rankings_json(&rankings, &selected_id_set);
    let seed_ids_json = pkt
        .receipt
        .seed_ids
        .iter()
        .map(|id| format!("\"{}\"", json_escape(id.as_str())))
        .collect::<Vec<_>>()
        .join(",");
    let admitted_dependency_ids_json = pkt
        .receipt
        .admitted_dependency_ids
        .iter()
        .map(|id| format!("\"{}\"", json_escape(id.as_str())))
        .collect::<Vec<_>>()
        .join(",");
    let mut selected_ids = pkt.receipt.selected_ids.clone();
    selected_ids.sort();
    let selected_ids_json = selected_ids
        .iter()
        .map(|id| format!("\"{}\"", json_escape(id.as_str())))
        .collect::<Vec<_>>()
        .join(",");
    let (sqz_port, sqz_policy) = configured_packet_sqz(sqz_enabled);
    let evaluation = SqzAdapter::new(
        sqz_port,
        SqzAdapterConfig::new(
            sqz_policy,
            SqzIdentity::approved(),
            pkt_limits.max_bytes,
            vec![],
        ),
    )
    .evaluate(pkt, pkt_limits.max_bytes)
    .map_err(|error| CliPacketFailure {
        detail: "sqz_failed".to_owned(),
        data: format!("{{\"sqz\":{}}}", sqz_receipt_json(&error.receipt)),
        metrics: format!("{{\"sqz\":{}}}", sqz_receipt_json(&error.receipt)),
    })?;
    let sqz_json = sqz_receipt_json(&evaluation.receipt);
    if evaluation.receipt.status == SqzStatus::Failed {
        return Err(CliPacketFailure {
            detail: "sqz_failed".to_owned(),
            data: format!("{{\"sqz\":{sqz_json}}}"),
            metrics: format!("{{\"sqz\":{sqz_json}}}"),
        });
    }
    let pkt = evaluation.packet;
    let encoded_packet_bytes = pkt.payload.len();
    let est = encoded_packet_bytes.div_ceil(4);
    let tr = pkt.receipt.truncated;

    let mut warns: Vec<String> = snapshot
        .diagnostics
        .iter()
        .map(|d| format!("{:?}", d))
        .collect();
    warns.extend(unmatched_query_warnings(&query_text, &matched_terms));

    let data = format!(
        "{{\"root\":\"{}\",\"query\":\"{}\",\"controls\":{},\"payload\":\"{}\",\"selected_locators\":[{}],\"why_selected\":[{}],\"source_rankings\":[{}],\"seed_ids\":[{}],\"admitted_dependency_ids\":[{}],\"selected_ids\":[{}],\"candidate_source_bytes\":{},\"selected_source_bytes\":{},\"context_bytes_avoided\":{},\"excerpt_bytes\":{},\"raw_bytes\":{},\"encoded_packet_bytes\":{},\"estimated_tokens\":{},\"token_estimate_method\":\"bytes-divided-by-four-ceiling\",\"actual_model_input_tokens\":\"unavailable\",\"runtime_token_ceiling\":{},\"truncated\":{},\"sqz\":{}}}",
        json_escape(&root),
        json_escape(&query_text),
        controls.controls_json(),
        json_escape(&pkt.payload),
        selected_locators_json,
        why_selected_json,
        source_rankings_json,
        seed_ids_json,
        admitted_dependency_ids_json,
        selected_ids_json,
        candidate_bytes,
        selected_source_bytes,
        context_bytes_avoided,
        excerpt_bytes,
        evaluation.receipt.raw_bytes,
        encoded_packet_bytes,
        est,
        token_ceiling.map_or_else(|| "null".to_owned(), |value| value.to_string()),
        tr,
        sqz_json
    );
    let provenance = if selected_locators_json.is_empty() {
        "{\"sources\":[\"repository-scanner\",\"bran-core\"]}".to_owned()
    } else {
        format!(
            "{{\"sources\":[\"repository-scanner\",\"bran-core\"],\"selected_locators\":[{}],\"why_selected\":[{}],\"source_rankings\":[{}]}}",
            selected_locators_json, why_selected_json, source_rankings_json
        )
    };
    let metrics = format!(
        "{{\"controls\":{},\"candidate_source_bytes\":{},\"selected_source_bytes\":{},\"context_bytes_avoided\":{},\"excerpt_bytes\":{},\"encoded_packet_bytes\":{},\"estimated_tokens\":{},\"token_estimate_method\":\"bytes-divided-by-four-ceiling\",\"actual_model_input_tokens\":\"unavailable\",\"sqz\":{}}}",
        controls.controls_json(), candidate_bytes, selected_source_bytes, context_bytes_avoided,
        excerpt_bytes, encoded_packet_bytes, est, sqz_json
    );
    Ok((data, warns, vec![], provenance, metrics))
}

fn do_check(
    root: String,
    selected_profile: String,
    policy: &RepositoryPolicy,
    policy_source: &str,
) -> CheckResult {
    let root_path: &Path = Path::new(&root);
    let scanner = RepositoryScanner::new(root_path, ScanConfig::knowledge_documents())
        .map_err(|e| format!("scan_error: {:?}", e))?;
    let snapshot = scanner.scan().map_err(|e| format!("scan_error: {:?}", e))?;
    let bundle = derive_bundle_from_snapshot(&snapshot)?;

    let mut vres = ProfileValidator::validate_with_policy(&bundle, &selected_profile, Some(policy));
    let boundary_diagnostics = bran_core::boundary::validate_public_boundary(root_path, policy);
    if !boundary_diagnostics.is_empty() {
        vres.bran_strict.diagnostics.extend(boundary_diagnostics);
        vres.bran_strict.diagnostics.sort();
        vres.bran_strict.diagnostics.dedup();
        vres.bran_strict.status = ValidationStatus::Fail;
    }

    let okf = &vres.okf_compatibility;
    let v0_2 = &vres.okf_v0_2;
    let strict = &vres.bran_strict;
    let okf_diags = format_diagnostics(&okf.diagnostics);
    let v0_2_diags = format_diagnostics(&v0_2.diagnostics);
    let strict_diags = format_diagnostics(&strict.diagnostics);
    let sel_err_json = match &vres.selected_profile_error {
        Some(d) => format!(
            "{{\"path\":\"{}\",\"code\":\"{}\",\"message\":\"{}\"}}",
            json_escape(&d.path),
            json_escape(&d.code),
            json_escape(&d.message)
        ),
        None => "null".to_owned(),
    };

    let status = if vres.selected_profile_error.is_some() {
        "error"
    } else if vres.selected_passed() {
        "ok"
    } else {
        "failed"
    };
    let exitc = if status == "ok" {
        TypedExit::Success.code()
    } else if status == "failed" {
        TypedExit::Validation.code()
    } else {
        TypedExit::Usage.code()
    };

    let data = format!(
        "{{\"root\":\"{}\",\"selected_profile\":\"{}\",\"okf_compatibility\":{{\"profile\":\"{}\",\"status\":\"{}\",\"diagnostics\":[{}]}},\"okf_v0_2\":{{\"profile\":\"{}\",\"status\":\"{}\",\"diagnostics\":[{}]}},\"bran_strict\":{{\"profile\":\"{}\",\"status\":\"{}\",\"diagnostics\":[{}]}},\"selected_profile_error\":{},\"selected_passed\":{},\"exit_code\":{}}}",
        json_escape(&root),
        json_escape(&selected_profile),
        json_escape(&okf.profile),
        status_str(&okf.status),
        okf_diags,
        json_escape(&v0_2.profile),
        status_str(&v0_2.status),
        v0_2_diags,
        json_escape(&strict.profile),
        status_str(&strict.status),
        strict_diags,
        sel_err_json,
        vres.selected_passed(),
        vres.exit_code()
    );

    let warns: Vec<String> = snapshot
        .diagnostics
        .iter()
        .map(|d| format!("{:?}", d))
        .collect();
    let mut fails: Vec<String> = vec![];
    if vres.selected_profile_error.is_some() {
        fails.push(format!("unknown-profile:{}", selected_profile));
    }

    let provenance = format!(
        "{{\"sources\":[\"repository-scanner\",\"bran-core\"],\"policy_source\":\"{}\"}}",
        policy_source
    );
    let metrics = format!(
        "{{\"selected_passed\":{},\"exit_code\":{}}}",
        vres.selected_passed(),
        vres.exit_code()
    );
    Ok((data, warns, fails, exitc, status, provenance, metrics))
}

fn derive_bundle_from_snapshot(snapshot: &ScanSnapshot) -> Result<Bundle, String> {
    let mut docs = vec![];
    for (path, entry) in &snapshot.entries {
        if !is_knowledge_document_path(path) {
            continue;
        }
        let source = match std::str::from_utf8(entry.source.as_ref()) {
            Ok(s) => s.to_string(),
            Err(_) => continue,
        };
        let (raw, body) = split_frontmatter(&source);
        // The scanner's flat fact parser cannot represent nested v0.2
        // families (sources, generated, verified, ...), so the raw frontmatter
        // is re-parsed structurally. A successful structural parse wins even
        // when the scanner warned; the scanner's reason is kept only when the
        // structural parse also fails.
        let fm = if raw.is_empty() {
            Frontmatter::empty()
        } else {
            match bran_core::frontmatter::parse_frontmatter(&raw) {
                Ok(fields) => Frontmatter::from_parsed(raw, fields),
                Err(_) => {
                    let reason = entry
                        .metadata
                        .warnings
                        .iter()
                        .find_map(|warning| warning.strip_prefix("malformed-metadata: "));
                    match reason {
                        Some(reason) => Frontmatter::malformed(raw, reason),
                        None => Frontmatter::malformed(raw, "invalid frontmatter"),
                    }
                }
            }
        };
        docs.push(Doc::new(path.clone(), source, body, fm));
    }
    Bundle::from_documents(docs).map_err(|e| format!("duplicate_path: {}", e.path))
}

fn split_frontmatter(source: &str) -> (String, String) {
    let lines: Vec<&str> = source.lines().collect();
    if lines.is_empty() {
        return (String::new(), source.to_string());
    }
    let first = lines[0].trim_start_matches('\u{feff}').trim();
    if first != "---" {
        return (String::new(), source.to_string());
    }
    let mut fm_lines = vec![lines[0].to_string()];
    let mut i = 1usize;
    let mut found = false;
    while i < lines.len() {
        let l = lines[i];
        fm_lines.push(l.to_string());
        if l.trim() == "---" {
            found = true;
            i += 1;
            break;
        }
        i += 1;
    }
    let body = if found && i <= lines.len() {
        lines[i..].join("\n")
    } else {
        return (source.to_string(), String::new());
    };
    let raw = fm_lines.join("\n") + "\n";
    (raw, body)
}

fn status_str(s: &ValidationStatus) -> &'static str {
    match s {
        ValidationStatus::Pass => "pass",
        ValidationStatus::Fail => "fail",
    }
}

fn format_diagnostics(diags: &[Diagnostic]) -> String {
    diags
        .iter()
        .map(|d| {
            format!(
                "{{\"path\":\"{}\",\"code\":\"{}\",\"message\":\"{}\"}}",
                json_escape(&d.path),
                json_escape(&d.code),
                json_escape(&d.message)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

// --- Slice 3.1 Packet B: headless maintainer adapter (ponytail-minimal) ---

const FIXTURE_MARKER_NAME: &str = ".bran-fixture-authority";
const FIXTURE_MARKER_BYTES: &[u8] = b"bran-cli-fixture-v1\n";

fn has_exact_fixture_marker(root: &Path) -> bool {
    std::fs::read(root.join(FIXTURE_MARKER_NAME))
        .map(|b| b == FIXTURE_MARKER_BYTES)
        .unwrap_or(false)
}

fn cli_maintainer_validator(root: &Path) -> Result<(), String> {
    if !has_exact_fixture_marker(root) {
        return Err("missing_fixture_authority_marker".to_owned());
    }
    let scanner = RepositoryScanner::new(root, ScanConfig::default())
        .map_err(|e| format!("scan_error: {:?}", e))?;
    let snapshot = scanner.scan().map_err(|e| format!("scan_error: {:?}", e))?;
    let bundle = derive_bundle_from_snapshot(&snapshot)?;
    let vres = ProfileValidator::validate(&bundle, BRAN_STRICT);
    if vres.selected_passed() {
        Ok(())
    } else {
        Err("bran_strict_failed".to_owned())
    }
}

fn receipt_data(r: &RepairReceipt) -> String {
    format!(
        "{{\"schema_version\":\"{}\",\"proposal_digest\":\"{}\",\"applied_target\":\"{}\",\"authority_tag\":\"{}\",\"revalidation\":\"{}\"}}",
        json_escape(r.schema_version),
        json_escape(&r.proposal_digest),
        json_escape(&r.applied_target),
        json_escape(&r.authority_tag),
        json_escape(&r.revalidation)
    )
}

fn do_maintain_propose(root: String, target: String, replacement: String) -> CliResult {
    let root_path: &Path = Path::new(&root);
    // Propose is read-only; pass the strict validator (not invoked until apply)
    let coord = match RepairCoordinator::new(root_path, cli_maintainer_validator) {
        Ok(c) => c,
        Err(RepairTerminal::IoPartialWrite { reason, .. }) => {
            return CliResult::operation(make_envelope(
                "maintain.propose",
                "error",
                "null",
                &[],
                &[format!("io_error:{}", reason)],
                "{}",
                "{}",
            ));
        }
        Err(RepairTerminal::UnsafePath { path }) => {
            return CliResult::operation(make_envelope(
                "maintain.propose",
                "error",
                "null",
                &[],
                &[format!("unsafe_path:{}", path)],
                "{}",
                "{}",
            ));
        }
        Err(_) => {
            return CliResult::operation(make_envelope(
                "maintain.propose",
                "error",
                "null",
                &[],
                &["coord_error".to_owned()],
                "{}",
                "{}",
            ))
        }
    };
    let repl = replacement.into_bytes();
    match coord.propose(target, repl) {
        RepairTerminal::Proposed(p) => {
            let data = format!(
                "{{\"digest\":\"{}\",\"target\":\"{}\",\"original_present\":{}}}",
                json_escape(p.digest()),
                json_escape(p.target()),
                if p.original_bytes().is_some() {
                    "true"
                } else {
                    "false"
                }
            );
            CliResult::success(make_envelope(
                "maintain.propose",
                "ok",
                &data,
                &[],
                &[],
                "{}",
                "{}",
            ))
        }
        RepairTerminal::UnsafePath { path } => CliResult::operation(make_envelope(
            "maintain.propose",
            "error",
            "null",
            &[],
            &[format!("unsafe_path:{}", path)],
            "{}",
            "{}",
        )),
        RepairTerminal::IoPartialWrite { reason, .. } => CliResult::operation(make_envelope(
            "maintain.propose",
            "error",
            "null",
            &[],
            &[format!("io_error:{}", reason)],
            "{}",
            "{}",
        )),
        _ => CliResult::operation(make_envelope(
            "maintain.propose",
            "error",
            "null",
            &[],
            &["unexpected".to_owned()],
            "{}",
            "{}",
        )),
    }
}

fn do_maintain_apply(
    root: String,
    target: String,
    replacement: String,
    digest: String,
    authority: String,
) -> CliResult {
    let root_path: &Path = Path::new(&root);
    if !has_exact_fixture_marker(root_path) {
        return CliResult::usage(make_envelope(
            "maintain.apply",
            "error",
            "null",
            &[],
            &["missing_fixture_authority_marker".to_owned()],
            "{}",
            "{}",
        ));
    }
    if digest.trim().is_empty() {
        return CliResult::usage(make_envelope(
            "maintain.apply",
            "error",
            "null",
            &[],
            &["missing_digest".to_owned()],
            "{}",
            "{}",
        ));
    }
    if authority.trim().is_empty() {
        return CliResult::usage(make_envelope(
            "maintain.apply",
            "error",
            "null",
            &[],
            &["blank_authority".to_owned()],
            "{}",
            "{}",
        ));
    }
    let coord = match RepairCoordinator::new(root_path, cli_maintainer_validator) {
        Ok(c) => c,
        Err(RepairTerminal::IoPartialWrite { reason, .. }) => {
            return CliResult::operation(make_envelope(
                "maintain.apply",
                "error",
                "null",
                &[],
                &[format!("io_error:{}", reason)],
                "{}",
                "{}",
            ));
        }
        Err(RepairTerminal::UnsafePath { path }) => {
            return CliResult::operation(make_envelope(
                "maintain.apply",
                "error",
                "null",
                &[],
                &[format!("unsafe_path:{}", path)],
                "{}",
                "{}",
            ));
        }
        Err(_) => {
            return CliResult::operation(make_envelope(
                "maintain.apply",
                "error",
                "null",
                &[],
                &["coord_error".to_owned()],
                "{}",
                "{}",
            ))
        }
    };
    // Reconstruct proposal via coordinator (per contract)
    let repl = replacement.into_bytes();
    let prop = match coord.propose(target.clone(), repl) {
        RepairTerminal::Proposed(p) => p,
        RepairTerminal::UnsafePath { path } => {
            return CliResult::operation(make_envelope(
                "maintain.apply",
                "error",
                "null",
                &[],
                &[format!("unsafe_path:{}", path)],
                "{}",
                "{}",
            ));
        }
        RepairTerminal::IoPartialWrite { reason, .. } => {
            return CliResult::operation(make_envelope(
                "maintain.apply",
                "error",
                "null",
                &[],
                &[format!("io_error:{}", reason)],
                "{}",
                "{}",
            ));
        }
        _ => {
            return CliResult::operation(make_envelope(
                "maintain.apply",
                "error",
                "null",
                &[],
                &["unexpected".to_owned()],
                "{}",
                "{}",
            ))
        }
    };
    let auth = MaintainerAuthority::new(authority);
    match coord.apply(Some(auth), prop, &digest) {
        RepairTerminal::ValidationPassed(receipt) => {
            let data = receipt_data(&receipt);
            let prov = "{\"sources\":[\"repair-coordinator\",\"bran-core\"]}".to_owned();
            CliResult::success(make_envelope(
                "maintain.apply",
                "ok",
                &data,
                &[],
                &[],
                &prov,
                "{}",
            ))
        }
        RepairTerminal::ValidationFailed { reason, .. } => CliResult {
            output: make_envelope(
                "maintain.apply",
                "error",
                "null",
                &[],
                &[reason],
                "{}",
                "{}",
            ),
            exit_code: TypedExit::Validation.code(),
            is_error: true,
            is_interactive: false,
        },
        RepairTerminal::AuthorizationFailure => CliResult::usage(make_envelope(
            "maintain.apply",
            "error",
            "null",
            &[],
            &["authorization_failure".to_owned()],
            "{}",
            "{}",
        )),
        RepairTerminal::DigestMismatch { expected, actual } => {
            let canonical_source = |source: &str| {
                if source == "missing" {
                    return true;
                }
                let Some((byte_len, lanes)) = source
                    .strip_prefix("present:")
                    .and_then(|source| source.split_once(':'))
                else {
                    return false;
                };
                byte_len
                    .parse::<usize>()
                    .is_ok_and(|parsed_len| parsed_len.to_string() == byte_len)
                    && lanes.split('-').count() == 3
                    && lanes.split('-').all(|lane| {
                        lane.len() == 16
                            && lane
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
                    })
            };
            let actual_digest_parts = actual.rsplit_once("|s:");
            let expected_digest_parts = expected.rsplit_once("|s:");
            let stale_source = match (actual_digest_parts, expected_digest_parts) {
                (Some((actual_plan, actual_source)), Some((expected_plan, expected_source))) => {
                    actual_plan
                        .strip_prefix("p:")
                        .is_some_and(|plan| !plan.is_empty())
                        && expected_plan
                            .strip_prefix("p:")
                            .is_some_and(|plan| !plan.is_empty())
                        && canonical_source(actual_source)
                        && canonical_source(expected_source)
                        && actual_plan == expected_plan
                        && actual_source != expected_source
                }
                _ => false,
            };
            if stale_source {
                CliResult::operation(make_envelope(
                    "maintain.apply",
                    "error",
                    "null",
                    &[],
                    &["stale_source".to_owned()],
                    "{}",
                    "{}",
                ))
            } else {
                CliResult::usage(make_envelope(
                    "maintain.apply",
                    "error",
                    "null",
                    &[],
                    &[format!(
                        "digest_mismatch:expected={},actual={}",
                        json_escape(&expected),
                        json_escape(&actual)
                    )],
                    "{}",
                    "{}",
                ))
            }
        }
        RepairTerminal::StaleSource => CliResult::operation(make_envelope(
            "maintain.apply",
            "error",
            "null",
            &[],
            &["stale_source".to_owned()],
            "{}",
            "{}",
        )),
        RepairTerminal::UnsafePath { path } => CliResult::operation(make_envelope(
            "maintain.apply",
            "error",
            "null",
            &[],
            &[format!("unsafe_path:{}", path)],
            "{}",
            "{}",
        )),
        RepairTerminal::IoPartialWrite { reason, .. } => CliResult::operation(make_envelope(
            "maintain.apply",
            "error",
            "null",
            &[],
            &[format!("io_partial:{}", reason)],
            "{}",
            "{}",
        )),
        _ => CliResult::operation(make_envelope(
            "maintain.apply",
            "error",
            "null",
            &[],
            &["unexpected".to_owned()],
            "{}",
            "{}",
        )),
    }
}

fn do_maintain_revalidate(root: String) -> CliResult {
    let root_path: &Path = Path::new(&root);
    match cli_maintainer_validator(root_path) {
        Ok(()) => CliResult::success(make_envelope(
            "maintain.revalidate",
            "ok",
            "{}",
            &[],
            &[],
            "{}",
            "{}",
        )),
        Err(reason) => CliResult {
            output: make_envelope(
                "maintain.revalidate",
                "error",
                "null",
                &[],
                &[reason],
                "{}",
                "{}",
            ),
            exit_code: TypedExit::Validation.code(),
            is_error: true,
            is_interactive: false,
        },
    }
}

fn do_agents_list() -> CliResult {
    if !agent_descriptor_configured() {
        return unconfigured_agents_list();
    }
    let descriptor = match ConfiguredAgentDescriptor::from_environment() {
        Ok(descriptor) => descriptor,
        Err(failure) => {
            return CliResult::operation(make_envelope(
                "agents.list",
                "error",
                "null",
                &[],
                &[failure.as_str().to_owned()],
                "{}",
                "{}",
            ))
        }
    };
    let apr = match descriptor.registry() {
        Ok(registry) => registry,
        Err(failure) => return CliResult::operation(make_agents_error("list", failure.as_str())),
    };
    let mut agent_strs = vec![];
    for p in apr.profiles() {
        let name = json_escape(p.name());
        let provider = json_escape(p.provider());
        let model = json_escape(p.model());
        let account = json_escape(p.account_handle());
        let reasoning = p.default_reasoning_level().as_str();
        let tp = p.tool_policy();
        let allow = tp
            .allowed()
            .map(|t| format!("\"{}\"", json_escape(t)))
            .collect::<Vec<_>>()
            .join(",");
        let deny = tp
            .denied()
            .map(|t| format!("\"{}\"", json_escape(t)))
            .collect::<Vec<_>>()
            .join(",");
        agent_strs.push(format!(
            "{{\"name\":\"{}\",\"provider\":\"{}\",\"model\":\"{}\",\"account_handle\":\"{}\",\"default_reasoning\":\"{}\",\"tool_policy\":{{\"allow\":[{}],\"deny\":[{}]}}}}",
            name, provider, model, account, reasoning, allow, deny
        ));
    }
    let data = format!("{{\"agents\":[{}]}}", agent_strs.join(","));
    CliResult::success(make_envelope(
        "agents.list",
        "ok",
        &data,
        &[],
        &[],
        "{}",
        "{}",
    ))
}

fn agent_descriptor_configured() -> bool {
    [
        "BRAN_AGENT_PROFILE",
        "BRAN_AGENT_PROVIDER",
        "BRAN_AGENT_MODEL",
        "BRAN_AGENT_REASONING",
        "BRAN_AGENT_ACCOUNT_REF",
    ]
    .iter()
    .filter_map(std::env::var_os)
    .any(|value| !value.is_empty())
}

fn unconfigured_agents_list() -> CliResult {
    CliResult::success(make_envelope(
        "agents.list",
        "ok",
        "{\"agents\":[]}",
        &["provider_descriptor_not_configured: set BRAN_AGENT_PROFILE, BRAN_AGENT_PROVIDER, BRAN_AGENT_MODEL, BRAN_AGENT_REASONING, and BRAN_AGENT_ACCOUNT_REF".to_owned()],
        &[],
        "{}",
        "{}",
    ))
}

/// Read-only setup diagnostics. Capability probes report `unavailable` instead
/// of initializing auth, network, or a provider to discover them.
fn do_doctor(mode: &str) -> CliResult {
    do_doctor_at(mode, Path::new(".bran/settings.conf"), None)
}

fn do_doctor_at(mode: &str, settings_path: &Path, skill_path: Option<&Path>) -> CliResult {
    let settings = load_settings(settings_path);
    let configured = settings.as_ref().ok().and_then(Option::as_ref);
    let settings_status = match &settings {
        Ok(Some(_)) => "complete",
        Ok(None) => "not_configured",
        Err(_) => "unavailable",
    };
    let token_ceiling = configured.and_then(|value| value.connected_agent_task_token_ceiling);

    let onboarding_ready = configured.is_some();
    let agent_mode = mode == "--agent";
    let skill_available = !agent_mode || agent_skill_available(skill_path);
    let packet_available = !agent_mode || packet_round_trip_available();
    let local_setup_ready = onboarding_ready && skill_available && packet_available;
    let local_capability = if agent_mode {
        connected_project_settings(configured).map(local_capability_probe)
    } else {
        None
    };
    let connected_agent_runtime =
        local_capability.is_some_and(|probe| probe.connected_agent_runtime);
    let sqz_available = local_capability.is_some_and(|probe| probe.sqz_available);
    let ready_to_attempt = local_setup_ready && connected_agent_runtime;
    let connected_execution_ready = false;
    let ready = if agent_mode {
        connected_execution_ready
    } else {
        local_setup_ready
    };

    let mut warnings = Vec::new();
    if !onboarding_ready {
        warnings.push("onboarding_settings_unavailable".to_owned());
    }
    if agent_mode && !skill_available {
        warnings.push("agent_skill_unavailable".to_owned());
    }
    if agent_mode && !packet_available {
        warnings.push("packet_round_trip_unavailable".to_owned());
    }
    if agent_mode && !connected_agent_runtime {
        warnings.push("connected_agent_runtime_unavailable".to_owned());
    }
    if agent_mode {
        warnings.push("host_attestation_unavailable".to_owned());
    }
    if agent_mode && configured.is_some_and(|settings| settings.sqz) && !sqz_available {
        warnings.push("sqz_capability_unavailable".to_owned());
    }

    let capability = |available: bool| {
        if available {
            "available"
        } else {
            "unavailable"
        }
    };
    let data = format!(
        "{{\"mode\":\"{}\",\"ready\":{},\"local_setup_ready\":{},\"connected_execution_ready\":{},\"connected_execution_status\":\"{}\",\"settings_status\":\"{}\",\"connected_task_total_token_ceiling\":{},\"capabilities\":{{\"cli_discovery\":\"available\",\"skill_discovery\":\"{}\",\"workspace_policy\":\"{}\",\"sqz\":\"{}\",\"packet_round_trip\":\"{}\",\"connected_agent_runtime\":\"{}\",\"host_attestation\":\"unavailable\"}},\"offline_return\":{{\"status\":\"available\",\"network_initialized\":false,\"auth_initialized\":false,\"provider_initialized\":false,\"conversation_retained\":false}}}}",
        if agent_mode { "agent" } else { "onboarding" },
        ready,
        local_setup_ready,
        connected_execution_ready,
        if ready_to_attempt { "ready_to_attempt" } else { "unavailable" },
        settings_status,
        token_ceiling.map_or_else(|| "null".to_owned(), |value| value.to_string()),
        if agent_mode { capability(skill_available) } else { "unavailable" },
        capability(onboarding_ready),
        capability(sqz_available),
        if agent_mode { capability(packet_available) } else { "unavailable" },
        capability(connected_agent_runtime),
    );
    let status = if ready { "ok" } else { "warning" };
    let output = make_envelope(
        "doctor",
        status,
        &data,
        &warnings,
        &[],
        "{\"sources\":[\"local-settings\",\"local-skill\",\"bran-core\"]}",
        "{\"provider_calls\":0,\"auth_calls\":0,\"network_calls\":0}",
    );
    CliResult {
        output,
        exit_code: if ready {
            TypedExit::Success.code()
        } else {
            TypedExit::Validation.code()
        },
        is_error: false,
        is_interactive: false,
    }
}

fn agent_skill_available(explicit: Option<&Path>) -> bool {
    explicit.is_some_and(Path::is_file)
        || std::env::var_os("BRAN_SKILL_PATH").is_some_and(|path| Path::new(&path).is_file())
        || [
            ".agents/skills/use-bran/SKILL.md",
            ".codex/skills/use-bran/SKILL.md",
            "skill/use-bran/SKILL.md",
            "bran/skill/use-bran/SKILL.md",
        ]
        .iter()
        .any(|path| Path::new(path).is_file())
}

fn packet_round_trip_available() -> bool {
    (|| -> Option<()> {
        let id = NodeId::parse("doctor.packet").ok()?;
        let node = NodeInput::new(
            id.clone(),
            NodeRole::Document,
            Provenance::new("bran-doctor", "builtin://packet").ok()?,
            Confidence::new(100).ok()?,
        );
        let graph = KnowledgeGraph::build(
            GraphInput::new(vec![node], vec![]),
            GraphLimits::new(1, 1).ok()?,
        )
        .ok()?;
        let view = ViewCompiler::new()
            .compile(
                &ViewSpec {
                    source: ViewSource::Roles(vec![NodeRole::Document]),
                    filter: ViewFilter::All,
                    sort: ViewSort::NodeId,
                    grouping: ViewGrouping::None,
                    fields: vec![ViewField::ProvenanceLocator, ViewField::NodeId],
                    presentation: Presentation::Json,
                    max_items: 1,
                    max_bytes: 4_096,
                },
                &graph,
            )
            .ok()?;
        let evidence = [EvidenceContent::new(
            id,
            "doctor packet round trip",
            EvidencePriority::Recommended,
            100,
            1,
            vec![],
        )];
        let request = PacketAssemblyRequest {
            view: &view,
            graph: &graph,
            evidence: &evidence,
            limits: PacketLimits::new(1, 4_096, None),
            dependency_limits: DependencyClosureLimits::new(1, 1).ok()?,
        };
        let packet = PacketAssembler::new().assemble(&request).ok()?;
        (packet.receipt.selected_ids.len() == 1).then_some(())
    })()
    .is_some()
}

fn parse_tools(s: &str) -> Result<ToolPolicy, String> {
    let parts: Vec<&str> = s.split(',').map(|x| x.trim()).collect();
    if parts.iter().any(|p| p.is_empty()) {
        return Err("empty_tools".to_owned());
    }
    let mut members: Vec<String> = vec![];
    for p in parts {
        members.push(p.to_owned());
    }
    if members.is_empty() {
        return Err("empty_tools".to_owned());
    }
    if members.len() > 32 {
        return Err("too_many_tools".to_owned());
    }
    let mut seen = BTreeSet::<String>::new();
    for m in &members {
        if !seen.insert(m.clone()) {
            return Err("duplicate_tool".to_owned());
        }
        if m != "read" && m != "search" {
            return Err("denied_tool".to_owned());
        }
    }
    let deny = vec![
        "write".to_owned(),
        "edit".to_owned(),
        "shell".to_owned(),
        "network".to_owned(),
    ];
    ToolPolicy::new(members, deny).map_err(|_| "invalid_tool".to_owned())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectedSetupFailure {
    MissingConnectedSettings,
    MissingProviderDescriptor,
    InvalidProviderDescriptor,
    MissingProviderHost,
    InvalidProviderHost,
    MissingAuthReference,
    MissingSqz,
    InvalidSqz,
    ProjectTrustRequired,
    GroundingFailed,
    RuntimeInvariant,
    ResultStoreFailed,
}

impl ConnectedSetupFailure {
    const fn as_str(self) -> &'static str {
        match self {
            Self::MissingConnectedSettings => "missing_connected_settings",
            Self::MissingProviderDescriptor => "missing_provider_descriptor",
            Self::InvalidProviderDescriptor => "invalid_provider_descriptor",
            Self::MissingProviderHost => "missing_provider_host",
            Self::InvalidProviderHost => "invalid_provider_host",
            Self::MissingAuthReference => "missing_auth_reference",
            Self::MissingSqz => "missing_sqz_configuration",
            Self::InvalidSqz => "invalid_sqz_configuration",
            Self::ProjectTrustRequired => "project_trust_required",
            Self::GroundingFailed => "repository_grounding_failed",
            Self::RuntimeInvariant => "runtime_receipt_invariant",
            Self::ResultStoreFailed => "result_store_failed",
        }
    }
}

#[derive(Clone, Debug)]
struct ConfiguredAgentDescriptor {
    profile: AgentProfile,
}

impl ConfiguredAgentDescriptor {
    fn from_environment() -> Result<Self, ConnectedSetupFailure> {
        let profile = env_identity("BRAN_AGENT_PROFILE")?;
        let provider = env_identity("BRAN_AGENT_PROVIDER")?;
        let model = env_identity("BRAN_AGENT_MODEL")?;
        let account = match std::env::var("BRAN_AGENT_ACCOUNT_REF") {
            Ok(value) if !value.trim().is_empty() && is_public_dlp_safe(&value) => {
                opaque_account_handle(&value)
            }
            _ => return Err(ConnectedSetupFailure::MissingAuthReference),
        };
        let reasoning = match std::env::var("BRAN_AGENT_REASONING") {
            Ok(value) if is_public_dlp_safe(&value) => ReasoningLevel::parse(&value)
                .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?,
            _ => return Err(ConnectedSetupFailure::MissingProviderDescriptor),
        };
        let profile = AgentProfile::new(
            profile,
            provider,
            model,
            account,
            reasoning,
            ToolPolicy::read_only_default(),
        )
        .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?;
        Ok(Self { profile })
    }

    fn registry(&self) -> Result<AgentProfileRegistry, ConnectedSetupFailure> {
        let mut providers = ProviderRegistry::new();
        providers
            .register(self.profile.provider())
            .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?;
        let mut models = ModelRegistry::new();
        models
            .register(self.profile.provider(), self.profile.model())
            .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?;
        let mut profiles = AgentProfileRegistry::new(providers, models);
        profiles
            .register(self.profile.clone())
            .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?;
        Ok(profiles)
    }

    fn registry_for_tui_request(
        &self,
        request: &DelegationRequest,
    ) -> Result<AgentProfileRegistry, ConnectedSetupFailure> {
        let mut providers = ProviderRegistry::new();
        providers
            .register(self.profile.provider())
            .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?;
        let mut models = ModelRegistry::new();
        models
            .register(self.profile.provider(), self.profile.model())
            .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?;
        if let Some(model) = request.model_override() {
            if model != self.profile.model() {
                models
                    .register(self.profile.provider(), model)
                    .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?;
            }
        }
        let profile = AgentProfile::new(
            request.profile(),
            self.profile.provider(),
            self.profile.model(),
            self.profile.account_handle(),
            self.profile.default_reasoning_level(),
            self.profile.tool_policy().clone(),
        )
        .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?;
        let mut profiles = AgentProfileRegistry::new(providers, models);
        profiles
            .register(profile)
            .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?;
        Ok(profiles)
    }
}

fn env_identity(name: &str) -> Result<String, ConnectedSetupFailure> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() && is_public_dlp_safe(&value) => Ok(value),
        Ok(_) => Err(ConnectedSetupFailure::InvalidProviderDescriptor),
        Err(_) => Err(ConnectedSetupFailure::MissingProviderDescriptor),
    }
}

fn opaque_account_handle(value: &str) -> String {
    let digest = ResultId::sha256(value.as_bytes());
    format!("account-{}", &digest.value()[..32])
}

fn offline_registry(profile_name: &str) -> Result<AgentProfileRegistry, ConnectedSetupFailure> {
    let mut providers = ProviderRegistry::new();
    providers
        .register("offline")
        .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?;
    let mut models = ModelRegistry::new();
    models
        .register("offline", "offline")
        .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?;
    let profile = AgentProfile::new(
        profile_name,
        "offline",
        "offline",
        "offline",
        ReasoningLevel::Off,
        ToolPolicy::read_only_default(),
    )
    .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?;
    let mut profiles = AgentProfileRegistry::new(providers, models);
    profiles
        .register(profile)
        .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?;
    Ok(profiles)
}

struct OfflineProvider;

impl ProviderPort<SafeAccountReference> for OfflineProvider {
    fn invoke(
        &self,
        _request: &ProviderRequest,
        _credential: &SafeAccountReference,
    ) -> Result<ProviderOutput, ProviderError> {
        Err(ProviderError::Unavailable)
    }
}

struct CancelledProvider;

impl ProviderPort<SafeAccountReference> for CancelledProvider {
    fn invoke(
        &self,
        _request: &ProviderRequest,
        _credential: &SafeAccountReference,
    ) -> Result<ProviderOutput, ProviderError> {
        Err(ProviderError::Cancelled)
    }
}

struct ConnectedConfig {
    account: ConfiguredAccount,
    provider: ExternalHostAdapter,
    sqz: ConnectedSqzPort,
    sqz_policy: SqzPolicy,
}

impl ConnectedConfig {
    fn from_environment(
        profile: &bran_core::agent::AgentProfile,
        sqz_enabled: bool,
    ) -> Result<Self, ConnectedSetupFailure> {
        let account_ref = profile.account_handle().to_owned();
        let credential = SafeAccountReference::new(&account_ref)
            .map_err(|_| ConnectedSetupFailure::MissingAuthReference)?;
        let provider = configured_host(profile)?;
        let (sqz, sqz_policy) = configured_sqz(sqz_enabled)?;
        Ok(Self {
            account: ConfiguredAccount {
                account_ref,
                credential,
            },
            provider,
            sqz,
            sqz_policy,
        })
    }
}

fn configured_host(profile: &AgentProfile) -> Result<ExternalHostAdapter, ConnectedSetupFailure> {
    let executable = std::env::var_os("BRAN_EXTERNAL_HOST_EXECUTABLE")
        .map(PathBuf::from)
        .ok_or(ConnectedSetupFailure::MissingProviderHost)?;
    let digest = std::env::var("BRAN_EXTERNAL_HOST_SHA256")
        .map_err(|_| ConnectedSetupFailure::MissingProviderHost)?;
    if !executable.is_absolute() {
        return Err(ConnectedSetupFailure::InvalidProviderHost);
    }
    let timeout = match std::env::var("BRAN_EXTERNAL_HOST_TIMEOUT_SECONDS") {
        Ok(value) => Some(
            value
                .parse::<u64>()
                .map_err(|_| ConnectedSetupFailure::InvalidProviderHost)?,
        ),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(ConnectedSetupFailure::InvalidProviderHost)
        }
    };
    let mut adapter = ExternalHostAdapter::new(executable, digest, std::iter::empty::<String>())
        .map_err(|_| ConnectedSetupFailure::InvalidProviderHost)?;
    if let Some(seconds) = timeout {
        adapter = adapter
            .with_timeout_secs(seconds)
            .map_err(|_| ConnectedSetupFailure::InvalidProviderHost)?;
    }
    adapter
        .with_profile(profile.name())
        .map_err(|_| ConnectedSetupFailure::InvalidProviderHost)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CapabilityIdentity {
    sqz_requested: bool,
    environment: [Option<OsString>; 10],
}

impl CapabilityIdentity {
    fn current(settings: &Settings) -> Self {
        Self {
            sqz_requested: settings.sqz,
            environment: [
                "BRAN_AGENT_PROFILE",
                "BRAN_AGENT_PROVIDER",
                "BRAN_AGENT_MODEL",
                "BRAN_AGENT_REASONING",
                "BRAN_AGENT_ACCOUNT_REF",
                "BRAN_EXTERNAL_HOST_EXECUTABLE",
                "BRAN_EXTERNAL_HOST_SHA256",
                "BRAN_EXTERNAL_HOST_TIMEOUT_SECONDS",
                "BRAN_SQZ_EXECUTABLE",
                "BRAN_SQZ_SHA256",
            ]
            .map(std::env::var_os),
        }
    }
}

#[derive(Default)]
struct CapabilityCache {
    identity: Option<CapabilityIdentity>,
    capability: bran_tui::CapabilityProbe,
}

impl CapabilityCache {
    fn resolve(&mut self, settings: &Settings) -> bran_tui::CapabilityProbe {
        let identity = CapabilityIdentity::current(settings);
        self.resolve_with(identity, || local_capability_probe(settings))
    }

    fn resolve_with(
        &mut self,
        identity: CapabilityIdentity,
        probe: impl FnOnce() -> bran_tui::CapabilityProbe,
    ) -> bran_tui::CapabilityProbe {
        if self.identity.as_ref() != Some(&identity) {
            self.capability = probe();
            self.identity = Some(identity);
        }
        self.capability
    }
}

fn local_capability_probe(settings: &Settings) -> bran_tui::CapabilityProbe {
    let sqz_available = configured_sqz(true).is_ok();
    let descriptor = ConfiguredAgentDescriptor::from_environment();
    let auth_available = descriptor.as_ref().is_ok_and(|descriptor| {
        SafeAccountReference::new(descriptor.profile.account_handle()).is_ok()
    });
    let network_available = descriptor
        .as_ref()
        .is_ok_and(|descriptor| configured_host(&descriptor.profile).is_ok());
    bran_tui::CapabilityProbe {
        sqz_available,
        connected_agent_runtime: auth_available
            && network_available
            && (!settings.sqz || sqz_available),
        network_available,
        auth_available,
        ..bran_tui::CapabilityProbe::default()
    }
}

fn configured_sqz(enabled: bool) -> Result<(ConnectedSqzPort, SqzPolicy), ConnectedSetupFailure> {
    if !enabled {
        return Ok((ConnectedSqzPort::Off, SqzPolicy::PublicOff));
    }
    let executable = std::env::var_os("BRAN_SQZ_EXECUTABLE")
        .map(PathBuf::from)
        .ok_or(ConnectedSetupFailure::MissingSqz)?;
    if !executable.is_absolute() {
        return Err(ConnectedSetupFailure::InvalidSqz);
    }
    Ok((
        ConnectedSqzPort::External(
            ExternalSqzPort::new(executable).map_err(|_| ConnectedSetupFailure::InvalidSqz)?,
        ),
        SqzPolicy::PublicOn,
    ))
}

fn configured_packet_sqz(enabled: bool) -> (ConnectedSqzPort, SqzPolicy) {
    if !enabled {
        return (ConnectedSqzPort::Off, SqzPolicy::PublicOff);
    }
    #[cfg(test)]
    if std::env::var_os("BRAN_P3_PACKET_SQZ_FIXTURE").is_some() {
        return (ConnectedSqzPort::Fixture, SqzPolicy::PublicOn);
    }
    let port = std::env::var_os("BRAN_SQZ_EXECUTABLE")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .and_then(|path| ExternalSqzPort::new(path).ok())
        .map(ConnectedSqzPort::External)
        .unwrap_or(ConnectedSqzPort::Off);
    (port, SqzPolicy::PublicOn)
}

#[derive(Clone, Debug)]
enum ConnectedSqzPort {
    Off,
    External(ExternalSqzPort),
    GroundedExternal {
        port: ExternalSqzPort,
        prefix: String,
    },
    #[cfg(test)]
    Fixture,
    #[cfg(test)]
    GroundedFixture(String),
    #[cfg(test)]
    FixtureDropsTask,
}

impl ConnectedSqzPort {
    fn with_grounded_task(self, task: &str, response_sources: Option<usize>) -> Self {
        let prefix = grounded_prompt_prefix(task, response_sources);
        match self {
            Self::External(port) => Self::GroundedExternal { port, prefix },
            #[cfg(test)]
            Self::Fixture => Self::GroundedFixture(prefix),
            port => port,
        }
    }
}

impl SqzPort for ConnectedSqzPort {
    fn compress(&self, input: &str) -> Result<SqzPortOutput, SqzPortError> {
        match self {
            Self::Off => Err(SqzPortError::new(SqzPortErrorCode::Unavailable)),
            Self::External(port) => port.compress(input),
            Self::GroundedExternal { port, prefix } => {
                compress_connected_evidence(input, prefix, |evidence| port.compress(evidence))
            }
            #[cfg(test)]
            Self::Fixture => Ok(SqzPortOutput::new(input, SqzIdentity::approved())),
            #[cfg(test)]
            Self::GroundedFixture(prefix) => {
                compress_connected_evidence(input, prefix, |evidence| {
                    assert!(!evidence.starts_with("task:\n"));
                    Ok(SqzPortOutput::new(
                        evidence
                            .lines()
                            .filter(|line| {
                                line.starts_with("locator=")
                                    || line.starts_with("content-digest-sha256:")
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                        SqzIdentity::approved(),
                    ))
                })
            }
            #[cfg(test)]
            Self::FixtureDropsTask => Ok(SqzPortOutput::new(
                "repository-evidence-packet:\n",
                SqzIdentity::approved(),
            )),
        }
    }
}

fn compress_connected_evidence(
    input: &str,
    prefix: &str,
    compress: impl FnOnce(&str) -> Result<SqzPortOutput, SqzPortError>,
) -> Result<SqzPortOutput, SqzPortError> {
    let Some(evidence) = input.strip_prefix(prefix) else {
        if input.starts_with("task:\n") || input.starts_with(CONNECTED_AGENT_PREAMBLE) {
            return Err(SqzPortError::new(SqzPortErrorCode::InvalidOutput));
        }
        return compress(input);
    };
    let mut output = compress(evidence)?;
    output.payload.insert_str(0, "sqz-applied-v1\n");
    output.payload.insert_str(0, prefix);
    Ok(output)
}

struct ConfiguredAccount {
    account_ref: String,
    credential: SafeAccountReference,
}

impl AuthStore for ConfiguredAccount {
    type Credential = SafeAccountReference;

    fn resolve(&self, account_handle: &str) -> Result<Self::Credential, AuthError> {
        if account_handle == self.account_ref {
            Ok(self.credential.clone())
        } else {
            Err(AuthError::Missing)
        }
    }
}

fn connected_project_settings(settings: Option<&Settings>) -> Option<&Settings> {
    settings.filter(|settings| settings.profile == OperatingProfile::ConnectedAgent)
}

#[cfg(test)]
fn initialize_connected<T>(
    settings: Option<&Settings>,
    initialize: impl FnOnce(&Settings) -> Option<T>,
) -> Option<T> {
    initialize(connected_project_settings(settings)?)
}

#[cfg(test)]
fn runtime_receipt_or_incomplete(
    request: &DelegationRequest,
    result: Result<DelegationReceipt, bran_core::agent::coordinator::AgentRuntimeInternalError>,
) -> DelegationReceipt {
    result.unwrap_or_else(|_| synthetic::headless_incomplete_receipt_for(request, false))
}

#[cfg(test)]
fn grounded_request(
    root: &Path,
    request: &DelegationRequest,
) -> Result<DelegationRequest, ConnectedSetupFailure> {
    grounded_request_with(root, request, &ExperimentalControls::default())
}

fn grounded_request_with(
    root: &Path,
    request: &DelegationRequest,
    controls: &ExperimentalControls,
) -> Result<DelegationRequest, ConnectedSetupFailure> {
    let scanner = RepositoryScanner::new(root, ScanConfig::default())
        .map_err(|_| ConnectedSetupFailure::GroundingFailed)?;
    let snapshot = scanner
        .scan()
        .map_err(|_| ConnectedSetupFailure::GroundingFailed)?;
    let graph_input = snapshot
        .graph_input()
        .map_err(|_| ConnectedSetupFailure::GroundingFailed)?;
    let node_count = graph_input.nodes().len().max(1);
    let edge_count = graph_input.edges().len().max(1);
    let (rankings, _matched_terms) = source_rankings(
        &graph_input,
        &snapshot,
        request.prompt(),
        controls.max_sources(),
    );
    let spec = query_view_spec(&rankings, controls.max_sources());
    let graph = KnowledgeGraph::build(
        graph_input,
        GraphLimits::new(node_count, edge_count)
            .map_err(|_| ConnectedSetupFailure::GroundingFailed)?,
    )
    .map_err(|_| ConnectedSetupFailure::GroundingFailed)?;
    let view = ViewCompiler::new()
        .compile(&spec, &graph)
        .map_err(|_| ConnectedSetupFailure::GroundingFailed)?;
    if view.items().is_empty() {
        return Err(ConnectedSetupFailure::GroundingFailed);
    }
    let dependency_limits = DependencyClosureLimits::new(
        controls.dependency_depth(),
        controls.max_sources() * DEPENDENCY_DEPTH_LIMIT,
    )
    .map_err(|_| ConnectedSetupFailure::GroundingFailed)?;
    let ids = PacketAssembler::evidence_ids(&view, &graph, dependency_limits)
        .map_err(|_| ConnectedSetupFailure::GroundingFailed)?;
    let ranking_by_id = rankings
        .iter()
        .map(|ranking| (ranking.id.clone(), ranking))
        .collect::<BTreeMap<_, _>>();
    let evidence = ids
        .iter()
        .filter_map(|id| graph.node(id))
        .enumerate()
        .map(|(index, node)| {
            let ranking = ranking_by_id.get(node.id()).copied();
            let digest = snapshot
                .entries
                .get(node.provenance().locator())
                .map(|entry| ResultId::sha256(entry.source.as_ref()).value().to_owned());
            let anchors = ranking
                .filter(|ranking| ranking.rank == 1)
                .and_then(|_| {
                    PreservationAnchor::new(
                        format!("ranked-source-{index}"),
                        node.provenance().locator(),
                    )
                    .ok()
                })
                .into_iter()
                .collect::<Vec<_>>();
            EvidenceContent::new(
                node.id().clone(),
                locator_evidence_content(
                    node,
                    ranking,
                    &graph,
                    digest.as_deref(),
                    public_safe_excerpt(node, &snapshot, controls.excerpt_bytes).as_deref(),
                ),
                if !anchors.is_empty() {
                    EvidencePriority::Required
                } else if ranking.is_some() {
                    EvidencePriority::Recommended
                } else {
                    EvidencePriority::Related
                },
                ranking.map_or(0, |ranking| u64::MAX - ranking.rank as u64),
                ranking.map_or(0, ranking_freshness),
                anchors,
            )
        })
        .collect::<Vec<_>>();
    let prompt_prefix = grounded_prompt_prefix(request.prompt(), controls.response_sources);
    let reserved_tokens = prompt_prefix
        .len()
        .div_ceil(4)
        .checked_add(request.max_output_tokens().unwrap_or(0))
        .and_then(|value| value.checked_add(32))
        .ok_or(ConnectedSetupFailure::GroundingFailed)?;
    let packet_token_ceiling = request
        .hard_total_token_ceiling()
        .map(|ceiling| {
            ceiling
                .checked_sub(reserved_tokens)
                .filter(|value| *value > 0)
                .ok_or(ConnectedSetupFailure::GroundingFailed)
        })
        .transpose()?;
    let packet_byte_ceiling = 65_536usize
        .checked_sub(prompt_prefix.len())
        .filter(|value| *value > 0)
        .ok_or(ConnectedSetupFailure::GroundingFailed)?;
    let packet = PacketAssembler::new()
        .assemble(&PacketAssemblyRequest {
            view: &view,
            graph: &graph,
            evidence: &evidence,
            limits: PacketLimits::new(
                QUERY_RESULT_LIMIT,
                packet_byte_ceiling,
                packet_token_ceiling,
            ),
            dependency_limits,
        })
        .map_err(|_| ConnectedSetupFailure::GroundingFailed)?;
    if packet.payload.trim().is_empty() {
        return Err(ConnectedSetupFailure::GroundingFailed);
    }
    let locators = packet
        .items
        .iter()
        .map(|item| item.provenance.locator().to_owned())
        .collect::<Vec<_>>();
    let admitted_evidence = locators
        .iter()
        .map(|locator| {
            let entry = snapshot
                .entries
                .get(locator)
                .ok_or(ConnectedSetupFailure::GroundingFailed)?;
            AdmittedEvidence::new(locator, ResultId::sha256(entry.source.as_ref()).value())
                .map_err(|_| ConnectedSetupFailure::GroundingFailed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut anchors = Vec::new();
    let mut start = 0;
    while start < request.prompt().len() {
        let mut end = (start + 512).min(request.prompt().len());
        while !request.prompt().is_char_boundary(end) {
            end -= 1;
        }
        let id = if start == 0 && end == request.prompt().len() {
            "task".to_owned()
        } else {
            format!("task-{:03}", anchors.len())
        };
        anchors.push(
            PreservationAnchor::new(id, &request.prompt()[start..end])
                .map_err(|_| ConnectedSetupFailure::GroundingFailed)?,
        );
        start = end;
    }
    anchors.extend(
        locators
            .iter()
            .enumerate()
            .map(|(index, locator)| PreservationAnchor::new(format!("source-{index}"), locator))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ConnectedSetupFailure::GroundingFailed)?,
    );
    anchors.extend(
        admitted_evidence
            .iter()
            .enumerate()
            .map(|(index, evidence)| {
                PreservationAnchor::new(format!("source-digest-{index}"), evidence.content_digest())
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ConnectedSetupFailure::GroundingFailed)?,
    );
    let grounding_contract = GroundingContract::with_evidence(root, admitted_evidence, anchors)
        .map_err(|_| ConnectedSetupFailure::GroundingFailed)?;
    let prompt = prompt_prefix + &packet.payload;
    if prompt.len() > 65_536 {
        return Err(ConnectedSetupFailure::GroundingFailed);
    }
    let mut options = DelegationOptions::new();
    options.provider_override = request.provider_override().map(str::to_owned);
    options.model_override = request.model_override().map(str::to_owned);
    options.reasoning_override = request.reasoning_override();
    options.tool_policy = request.tool_policy().clone();
    options.no_session = request.no_session();
    options.max_output_bytes = request.max_output_bytes();
    options.hard_total_token_ceiling = request.hard_total_token_ceiling();
    options.max_output_tokens = request.max_output_tokens();
    options.delegation_depth = request.delegation_depth();
    options.grounding_contract = Some(grounding_contract);
    DelegationRequest::new(request.profile(), prompt, options)
        .map_err(|_| ConnectedSetupFailure::GroundingFailed)
}

fn grounded_prompt_prefix(task: &str, response_sources: Option<usize>) -> String {
    let preamble = response_sources.map_or_else(
        || CONNECTED_AGENT_PREAMBLE.to_owned(),
        |limit| {
            CONNECTED_AGENT_PREAMBLE.replacen(
                "2. Cite repository-relative paths and summarize contents and cross-file context.",
                &format!(
                    "2. Cite repository-relative paths and summarize contents and cross-file context; return at most {limit} recommended repository-relative paths, one short relevance reason per path, confidence and missing evidence, and no implementation analysis."
                ),
                1,
            )
        },
    );
    format!("{preamble}task:\n{task}\n\nrepository-evidence-packet:\n")
}

const RESULT_STORE_LIMITS: ResultStoreLimits = ResultStoreLimits {
    max_entries: 16,
    max_total_bytes: 4 * 1024 * 1024,
    max_item_bytes: 1024 * 1024,
    max_age_ticks: 86_400,
};

struct FilesystemResultStore {
    directory: PathBuf,
}

struct CancellableResultStore<'a> {
    inner: &'a mut FilesystemResultStore,
    cancellation: Option<&'a ConnectedCancellation>,
}

impl ResultStore for CancellableResultStore<'_> {
    fn limits(&self) -> ResultStoreLimits {
        self.inner.limits()
    }

    fn put_batch(
        &mut self,
        payloads: &[&[u8]],
        now_tick: u64,
    ) -> Result<Vec<ResultId>, ResultStoreError> {
        let _publication = self
            .cancellation
            .map(|cancellation| cancellation.publication.lock())
            .transpose()
            .unwrap_or_else(|poisoned| Some(poisoned.into_inner()));
        if self
            .cancellation
            .is_some_and(ConnectedCancellation::is_cancelled)
        {
            return Err(ResultStoreError::Unavailable);
        }
        let result = self.inner.put_batch(payloads, now_tick);
        if result.is_ok() {
            if let Some(cancellation) = self.cancellation {
                cancellation.published.store(true, Ordering::Release);
            }
        }
        result
    }

    fn get(&mut self, id: &ResultId, now_tick: u64) -> Result<Vec<u8>, ResultStoreError> {
        self.inner.get(id, now_tick)
    }
}

impl FilesystemResultStore {
    fn open(root: &Path) -> Result<Self, ResultStoreError> {
        let root = fs::canonicalize(root).map_err(|_| ResultStoreError::Unavailable)?;
        let state = root.join(".bran");
        checked_directory(&state)?;
        let directory = state.join("results");
        checked_directory(&directory)?;
        Ok(Self { directory })
    }

    fn operation_lock(&self) -> Result<fs::File, ResultStoreError> {
        let path = self.directory.join(".txn-lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|_| ResultStoreError::Unavailable)?;
        let before = fs::symlink_metadata(path).map_err(|_| ResultStoreError::Unavailable)?;
        let after = file.metadata().map_err(|_| ResultStoreError::Unavailable)?;
        if !before.file_type().is_file() || before.file_type().is_symlink() || !after.is_file() {
            return Err(ResultStoreError::Corrupt);
        }
        file.lock().map_err(|_| ResultStoreError::Unavailable)?;
        Ok(file)
    }

    #[cfg(test)]
    fn batches(&self, now: u64) -> Result<Vec<(u64, PathBuf, usize, u64)>, ResultStoreError> {
        let _lock = self.operation_lock()?;
        self.batches_unlocked(now)
    }

    fn batches_unlocked(
        &self,
        now: u64,
    ) -> Result<Vec<(u64, PathBuf, usize, u64)>, ResultStoreError> {
        let scan_limit = RESULT_STORE_LIMITS
            .max_entries
            .checked_mul(4)
            .and_then(|value| value.checked_add(64))
            .ok_or(ResultStoreError::ArithmeticOverflow)?;
        let mut batches = Vec::new();
        let mut scanned = 0usize;
        let enumeration_limit = scan_limit
            .checked_add(1)
            .ok_or(ResultStoreError::ArithmeticOverflow)?;
        for entry in fs::read_dir(&self.directory)
            .map_err(|_| ResultStoreError::Unavailable)?
            .take(enumeration_limit)
        {
            scanned = scanned
                .checked_add(1)
                .ok_or(ResultStoreError::ArithmeticOverflow)?;
            if scanned > scan_limit {
                return Err(ResultStoreError::BatchCapacityExceeded);
            }
            let entry = entry.map_err(|_| ResultStoreError::Unavailable)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| ResultStoreError::Corrupt)?;
            if name == ".txn-lock" {
                continue;
            }
            if name.starts_with(".txn-") {
                let metadata = fs::symlink_metadata(entry.path())
                    .map_err(|_| ResultStoreError::Unavailable)?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(ResultStoreError::Corrupt);
                }
                fs::remove_dir_all(entry.path()).map_err(|_| ResultStoreError::Unavailable)?;
                continue;
            }
            let Some(tick) = batch_tick(&name) else {
                return Err(ResultStoreError::Corrupt);
            };
            let metadata =
                fs::symlink_metadata(entry.path()).map_err(|_| ResultStoreError::Unavailable)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ResultStoreError::Corrupt);
            }
            if now.saturating_sub(tick) >= RESULT_STORE_LIMITS.max_age_ticks {
                fs::remove_dir_all(entry.path()).map_err(|_| ResultStoreError::Unavailable)?;
                continue;
            }
            let (count, bytes) = batch_size(&entry.path(), RESULT_STORE_LIMITS)?;
            batches.push((tick, entry.path(), count, bytes));
        }
        batches.sort_by_key(|(tick, path, _, _)| (*tick, path.clone()));
        Ok(batches)
    }
}

impl ResultStore for FilesystemResultStore {
    fn limits(&self) -> ResultStoreLimits {
        RESULT_STORE_LIMITS
    }

    fn put_batch(
        &mut self,
        payloads: &[&[u8]],
        now: u64,
    ) -> Result<Vec<ResultId>, ResultStoreError> {
        if payloads.is_empty() {
            return Err(ResultStoreError::EmptyInput);
        }
        let mut bytes = 0u64;
        let mut ids = Vec::with_capacity(payloads.len());
        for payload in payloads {
            if payload.is_empty() || payload.len() > RESULT_STORE_LIMITS.max_item_bytes {
                return Err(ResultStoreError::ItemTooLarge);
            }
            bytes = bytes
                .checked_add(payload.len() as u64)
                .ok_or(ResultStoreError::ArithmeticOverflow)?;
            ids.push(ResultId::sha256(payload));
        }
        if ids.len() > RESULT_STORE_LIMITS.max_entries
            || bytes > RESULT_STORE_LIMITS.max_total_bytes as u64
        {
            return Err(ResultStoreError::BatchCapacityExceeded);
        }
        let batch_id = ResultId::sha256(
            ids.iter()
                .flat_map(|id| id.value().bytes())
                .collect::<Vec<_>>()
                .as_slice(),
        );
        let target = self
            .directory
            .join(format!("batch-{now:020}-{}", batch_id.value()));
        let _lock = self.operation_lock()?;
        match fs::symlink_metadata(&target) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(ResultStoreError::Corrupt);
                }
                for (id, payload) in ids.iter().zip(payloads.iter()) {
                    if read_regular_bounded(
                        &target.join(id.value()),
                        RESULT_STORE_LIMITS.max_item_bytes,
                    )? != *payload
                    {
                        return Err(ResultStoreError::Corrupt);
                    }
                }
                return Ok(ids);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ResultStoreError::Unavailable),
        }
        let mut batches = self.batches_unlocked(now)?;
        let mut count = batches.iter().try_fold(0usize, |sum, batch| {
            sum.checked_add(batch.2)
                .ok_or(ResultStoreError::ArithmeticOverflow)
        })?;
        let mut total = batches.iter().try_fold(0u64, |sum, batch| {
            sum.checked_add(batch.3)
                .ok_or(ResultStoreError::ArithmeticOverflow)
        })?;
        let mut evictions = Vec::new();
        while count
            .checked_add(ids.len())
            .ok_or(ResultStoreError::ArithmeticOverflow)?
            > RESULT_STORE_LIMITS.max_entries
            || total
                .checked_add(bytes)
                .ok_or(ResultStoreError::ArithmeticOverflow)?
                > RESULT_STORE_LIMITS.max_total_bytes as u64
        {
            let (_, path, removed_count, removed_bytes) = batches
                .first()
                .cloned()
                .ok_or(ResultStoreError::BatchCapacityExceeded)?;
            evictions.push(path);
            batches.remove(0);
            count = count
                .checked_sub(removed_count)
                .ok_or(ResultStoreError::ArithmeticOverflow)?;
            total = total
                .checked_sub(removed_bytes)
                .ok_or(ResultStoreError::ArithmeticOverflow)?;
        }
        let staging =
            self.directory
                .join(format!(".txn-{}-{}", std::process::id(), monotonic_nonce()));
        fs::create_dir(&staging).map_err(|_| ResultStoreError::Unavailable)?;
        let publish = (|| {
            for (id, payload) in ids.iter().zip(payloads.iter()) {
                write_private_file(&staging.join(id.value()), payload)?;
            }
            fs::rename(&staging, &target).map_err(|_| ResultStoreError::Unavailable)
        })();
        if publish.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        publish?;
        evict_after_publish(&self.directory, &target, &evictions)?;
        Ok(ids)
    }

    fn get(&mut self, id: &ResultId, now: u64) -> Result<Vec<u8>, ResultStoreError> {
        let _lock = self.operation_lock()?;
        for (_, path, _, _) in self.batches_unlocked(now)?.into_iter().rev() {
            let candidate = path.join(id.value());
            match read_regular_bounded(&candidate, RESULT_STORE_LIMITS.max_item_bytes) {
                Ok(bytes) if ResultId::sha256(&bytes) == *id => return Ok(bytes),
                Ok(_) => return Err(ResultStoreError::Corrupt),
                Err(ResultStoreError::NotFound) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(ResultStoreError::NotFound)
    }
}

fn evict_after_publish(
    directory: &Path,
    published: &Path,
    evictions: &[PathBuf],
) -> Result<(), ResultStoreError> {
    if evictions.is_empty() {
        return Ok(());
    }
    let quarantine = directory.join(format!(
        ".txn-evict-{}-{}",
        std::process::id(),
        monotonic_nonce()
    ));
    if fs::create_dir(&quarantine).is_err() {
        let _ = fs::remove_dir_all(published);
        return Err(ResultStoreError::Unavailable);
    }
    let mut moved = Vec::new();
    for source in evictions {
        let Some(name) = source.file_name() else {
            rollback_eviction(published, &quarantine, &moved);
            return Err(ResultStoreError::Corrupt);
        };
        let destination = quarantine.join(name);
        if fs::rename(source, &destination).is_err() {
            rollback_eviction(published, &quarantine, &moved);
            return Err(ResultStoreError::Unavailable);
        }
        moved.push((source.clone(), destination));
    }
    let _ = fs::remove_dir_all(quarantine);
    Ok(())
}

fn rollback_eviction(published: &Path, quarantine: &Path, moved: &[(PathBuf, PathBuf)]) {
    for (source, destination) in moved.iter().rev() {
        let _ = fs::rename(destination, source);
    }
    let _ = fs::remove_dir_all(quarantine);
    let _ = fs::remove_dir_all(published);
}

#[cfg(test)]
fn failed_publication_preserves_prior(root: &Path, now: u64) -> bool {
    let mut store = match FilesystemResultStore::open(root) {
        Ok(store) => store,
        Err(_) => return false,
    };
    let payloads = (0..RESULT_STORE_LIMITS.max_entries)
        .map(|index| format!("prior-{index}").into_bytes())
        .collect::<Vec<_>>();
    let references = payloads.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let Ok(ids) = ResultStore::put_batch(&mut store, &references, now) else {
        return false;
    };
    let rejected = b"rejected-publication";
    let rejected_id = ResultId::sha256(rejected);
    let rejected_batch_id = ResultId::sha256(rejected_id.value().as_bytes());
    let target = store.directory.join(format!(
        "batch-{:020}-{}",
        now + 1,
        rejected_batch_id.value()
    ));
    if fs::create_dir(&target).is_err()
        || ResultStore::put_batch(&mut store, &[rejected.as_slice()], now + 1).is_ok()
        || fs::remove_dir(target).is_err()
    {
        return false;
    }
    ResultStore::get(&mut store, &ids[0], now + 1).is_ok_and(|bytes| bytes == payloads[0])
}

#[cfg(test)]
fn concurrent_recovery_preserves_live_transaction(root: &Path, now: u64) -> bool {
    let store = match FilesystemResultStore::open(root) {
        Ok(store) => store,
        Err(_) => return false,
    };
    let owner = match store.operation_lock() {
        Ok(owner) => owner,
        Err(_) => return false,
    };
    let staging = store.directory.join(".txn-live-fixture");
    if fs::create_dir(&staging).is_err() {
        return false;
    }
    let observed = staging.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _ = started_tx.send(());
        let cleaned = store
            .batches(now)
            .is_ok_and(|batches| batches.is_empty() && !observed.exists());
        let _ = finished_tx.send(cleaned);
    });
    if started_rx.recv_timeout(Duration::from_secs(1)).is_err() {
        return false;
    }
    let preserved = finished_rx
        .recv_timeout(Duration::from_millis(100))
        .is_err()
        && staging.is_dir();
    drop(owner);
    let cleaned = finished_rx.recv_timeout(Duration::from_secs(2)) == Ok(true);
    let _ = worker.join();
    preserved && cleaned
}

fn checked_directory(path: &Path) -> Result<(), ResultStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(ResultStoreError::Corrupt),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|_| ResultStoreError::Unavailable)
        }
        Err(_) => Err(ResultStoreError::Unavailable),
    }
}

fn batch_tick(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("batch-")?;
    let (tick, digest) = rest.split_once('-')?;
    (tick.len() == 20 && result_digest(digest))
        .then(|| tick.parse().ok())
        .flatten()
}

fn result_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn batch_size(path: &Path, limits: ResultStoreLimits) -> Result<(usize, u64), ResultStoreError> {
    let mut count = 0usize;
    let mut bytes = 0u64;
    let enumeration_limit = limits
        .max_entries
        .checked_add(1)
        .ok_or(ResultStoreError::ArithmeticOverflow)?;
    for entry in fs::read_dir(path)
        .map_err(|_| ResultStoreError::Unavailable)?
        .take(enumeration_limit)
    {
        let entry = entry.map_err(|_| ResultStoreError::Unavailable)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| ResultStoreError::Corrupt)?;
        if !result_digest(&name) {
            return Err(ResultStoreError::Corrupt);
        }
        let metadata =
            fs::symlink_metadata(entry.path()).map_err(|_| ResultStoreError::Unavailable)?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(ResultStoreError::Corrupt);
        }
        count = count
            .checked_add(1)
            .ok_or(ResultStoreError::ArithmeticOverflow)?;
        bytes = bytes
            .checked_add(metadata.len())
            .ok_or(ResultStoreError::ArithmeticOverflow)?;
    }
    if count > limits.max_entries || bytes > limits.max_total_bytes as u64 {
        return Err(ResultStoreError::BatchCapacityExceeded);
    }
    Ok((count, bytes))
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), ResultStoreError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| ResultStoreError::Unavailable)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| ResultStoreError::Unavailable)
}

fn read_regular_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, ResultStoreError> {
    let before = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ResultStoreError::NotFound
        } else {
            ResultStoreError::Unavailable
        }
    })?;
    if !before.file_type().is_file()
        || before.file_type().is_symlink()
        || before.len() > limit as u64
    {
        return Err(ResultStoreError::Corrupt);
    }
    let file = fs::File::open(path).map_err(|_| ResultStoreError::Unavailable)?;
    let after = file.metadata().map_err(|_| ResultStoreError::Unavailable)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return Err(ResultStoreError::Corrupt);
        }
    }
    let mut bytes = Vec::new();
    file.take((limit as u64) + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ResultStoreError::Unavailable)?;
    if bytes.is_empty() || bytes.len() > limit {
        return Err(ResultStoreError::Corrupt);
    }
    Ok(bytes)
}

fn monotonic_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn wall_tick() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn do_get(root: &Path, result_id: &str) -> CliResult {
    let id = match ResultId::parse(result_id) {
        Ok(id) => id,
        Err(_) => return CliResult::usage(make_get_error("invalid_result_id")),
    };
    let mut store = match FilesystemResultStore::open(root) {
        Ok(store) => store,
        Err(_) => return CliResult::operation(make_get_error("result_store_unavailable")),
    };
    let bytes = match store.get(&id, wall_tick()) {
        Ok(bytes) => bytes,
        Err(ResultStoreError::NotFound) => {
            return CliResult::operation(make_get_error("result_not_found"))
        }
        Err(_) => return CliResult::operation(make_get_error("invalid_result_content")),
    };
    let result = match InlineResult::decode_canonical(&bytes) {
        Ok(result) => result,
        Err(_) => return CliResult::operation(make_get_error("invalid_result_content")),
    };
    let citations = result
        .citations()
        .iter()
        .map(|citation| format!("\"{}\"", json_escape(citation)))
        .collect::<Vec<_>>()
        .join(",");
    let claims = result
        .claims()
        .iter()
        .map(|claim| {
            format!(
                "{{\"id\":\"{}\",\"text\":\"{}\",\"material\":{},\"locator\":\"{}\",\"content_digest\":\"{}\",\"support\":\"{}\"}}",
                json_escape(claim.id()),
                json_escape(claim.text()),
                claim.material(),
                json_escape(claim.locator()),
                json_escape(claim.content_digest()),
                json_escape(claim.support())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    CliResult::success(make_envelope(
        "get",
        "ok",
        &format!(
            "{{\"result_id\":\"{}\",\"answer\":\"{}\",\"citations\":[{}],\"claims\":[{}]}}",
            json_escape(result_id),
            json_escape(result.answer()),
            citations,
            claims
        ),
        &[],
        &[],
        "{\"sources\":[\"local-content-addressed-result-store\"]}",
        "{}",
    ))
}

fn production_headless_receipt(
    request: &DelegationRequest,
    offline: bool,
    settings: Option<&Settings>,
    trust_current_root: bool,
    controls: &mut ExperimentalControls,
) -> Result<DelegationReceipt, ConnectedSetupFailure> {
    production_headless_receipt_at(
        Path::new("."),
        request,
        offline,
        settings,
        trust_current_root,
        false,
        None,
        controls,
    )
}

#[allow(clippy::too_many_arguments)]
fn production_headless_receipt_at(
    root: &Path,
    request: &DelegationRequest,
    offline: bool,
    settings: Option<&Settings>,
    trust_current_root: bool,
    allow_tui_selection: bool,
    cancellation: Option<ConnectedCancellation>,
    controls: &mut ExperimentalControls,
) -> Result<DelegationReceipt, ConnectedSetupFailure> {
    if offline {
        let profiles = offline_registry(request.profile())?;
        let account = ConfiguredAccount {
            account_ref: "offline".to_owned(),
            credential: SafeAccountReference::new("offline")
                .map_err(|_| ConnectedSetupFailure::RuntimeInvariant)?,
        };
        let sqz = AgentSqzAdapter::new(
            ConnectedSqzPort::Off,
            SqzPolicy::PublicOff,
            request.max_output_bytes(),
        );
        let provider = OfflineProvider;
        let mut results = MemoryResultStore::new(1, 1, 1, 1)
            .map_err(|_| ConnectedSetupFailure::RuntimeInvariant)?;
        return AgentRuntime::new(
            AgentRuntimeConfig::new(true, 8)
                .map_err(|_| ConnectedSetupFailure::RuntimeInvariant)?,
        )
        .invoke(
            request,
            AgentRuntimeAuthority::new(true, false, false),
            &profiles,
            || RuntimePorts::new(&account, &provider, &sqz, &mut results),
            0,
        )
        .map_err(|_| ConnectedSetupFailure::RuntimeInvariant);
    }
    let settings = connected_project_settings(settings)
        .ok_or(ConnectedSetupFailure::MissingConnectedSettings)?;
    if !trust_current_root {
        return Err(ConnectedSetupFailure::ProjectTrustRequired);
    }
    let descriptor = ConfiguredAgentDescriptor::from_environment()?;
    let profiles = if allow_tui_selection {
        descriptor.registry_for_tui_request(request)?
    } else {
        descriptor.registry()?
    };
    let profile = profiles
        .get(request.profile())
        .map_err(|_| ConnectedSetupFailure::InvalidProviderDescriptor)?;
    let mut config = ConnectedConfig::from_environment(profile, settings.sqz)?;
    if let Some(cancellation) = cancellation.as_ref() {
        config.provider = config
            .provider
            .with_cancellation(Arc::clone(&cancellation.flag));
    }
    let Ok(runtime_config) = AgentRuntimeConfig::new(true, 8) else {
        return Err(ConnectedSetupFailure::RuntimeInvariant);
    };
    let runtime = AgentRuntime::new(runtime_config);
    let mut results =
        FilesystemResultStore::open(root).map_err(|_| ConnectedSetupFailure::ResultStoreFailed)?;
    let now = wall_tick();
    let mut grounded_request = grounded_request_with(root, request, controls)?;
    loop {
        let sqz_port = config
            .sqz
            .clone()
            .with_grounded_task(request.prompt(), controls.response_sources);
        let sqz = AgentSqzAdapter::new(
            sqz_port,
            config.sqz_policy,
            grounded_request.max_output_bytes(),
        );
        let mut cancellable_results = CancellableResultStore {
            inner: &mut results,
            cancellation: cancellation.as_ref(),
        };
        let receipt = runtime
            .invoke(
                &grounded_request,
                AgentRuntimeAuthority::new(false, trust_current_root, false),
                &profiles,
                || {
                    RuntimePorts::new(
                        &config.account,
                        &config.provider,
                        &sqz,
                        &mut cancellable_results,
                    )
                },
                now,
            )
            .map_err(|_| ConnectedSetupFailure::RuntimeInvariant)?;
        if cancellation
            .as_ref()
            .is_some_and(ConnectedCancellation::is_cancelled)
            && !matches!(
                receipt.outcome(),
                InvocationOutcome::Incomplete {
                    failure: AgentFailure::Cancelled,
                    ..
                }
            )
        {
            return cancelled_receipt(&grounded_request, &profiles, &config.account);
        }
        let Some((next_controls, next_grounded_request)) =
            next_changed_grounded_request(root, request, controls, &receipt, &grounded_request)?
        else {
            return Ok(receipt);
        };
        *controls = next_controls;
        grounded_request = next_grounded_request;
    }
}

fn next_changed_grounded_request(
    root: &Path,
    request: &DelegationRequest,
    controls: &ExperimentalControls,
    receipt: &DelegationReceipt,
    grounded_request: &DelegationRequest,
) -> Result<Option<(ExperimentalControls, DelegationRequest)>, ConnectedSetupFailure> {
    let mut next_controls = *controls;
    while let Some(next_limit) =
        next_sqz_fidelity_source_limit(receipt, next_controls.max_sources())
    {
        next_controls.effective_max_sources = Some(next_limit);
        let next_grounded_request = grounded_request_with(root, request, &next_controls)?;
        if next_grounded_request.prompt() != grounded_request.prompt() {
            return Ok(Some((next_controls, next_grounded_request)));
        }
    }
    Ok(None)
}

fn next_sqz_fidelity_source_limit(
    receipt: &DelegationReceipt,
    current_limit: usize,
) -> Option<usize> {
    if current_limit <= 1
        || !matches!(
            receipt.outcome(),
            InvocationOutcome::Incomplete {
                failure: AgentFailure::GroundingFailed,
                ..
            }
        )
    {
        return None;
    }
    let input = receipt.sqz_stages().input()?;
    if input.failure_reason != Some(SqzFailureReason::MissingFidelityAnchors)
        || input.missing_fidelity_anchor_ids.is_empty()
        || input
            .missing_fidelity_anchor_ids
            .iter()
            .any(|anchor| !anchor.starts_with("source-"))
    {
        return None;
    }
    Some((current_limit / 2).max(1))
}

fn cancelled_receipt(
    request: &DelegationRequest,
    profiles: &AgentProfileRegistry,
    account: &ConfiguredAccount,
) -> Result<DelegationReceipt, ConnectedSetupFailure> {
    let provider = CancelledProvider;
    let sqz = AgentSqzAdapter::new(
        ConnectedSqzPort::Off,
        SqzPolicy::PublicOff,
        request.max_output_bytes(),
    );
    let mut results =
        MemoryResultStore::new(1, 1, 1, 1).map_err(|_| ConnectedSetupFailure::RuntimeInvariant)?;
    AgentRuntime::new(
        AgentRuntimeConfig::new(true, 8).map_err(|_| ConnectedSetupFailure::RuntimeInvariant)?,
    )
    .invoke(
        request,
        AgentRuntimeAuthority::new(false, true, false),
        profiles,
        || RuntimePorts::new(account, &provider, &sqz, &mut results),
        0,
    )
    .map_err(|_| ConnectedSetupFailure::RuntimeInvariant)
}

#[cfg(test)]
fn connected_cancellation_contract(root: &Path) -> bool {
    let publication_root = root.join("published");
    if fs::create_dir(&publication_root).is_err() {
        return false;
    }
    let publication = ConnectedCancellation::new();
    let mut published_store = match FilesystemResultStore::open(&publication_root) {
        Ok(store) => store,
        Err(_) => return false,
    };
    let published_id = {
        let mut guarded = CancellableResultStore {
            inner: &mut published_store,
            cancellation: Some(&publication),
        };
        match guarded.put_batch(&[b"published-result"], wall_tick()) {
            Ok(mut ids) => ids.remove(0),
            Err(_) => return false,
        }
    };
    publication.cancel();
    let publication_won = !publication.is_cancelled()
        && published_store
            .get(&published_id, wall_tick())
            .is_ok_and(|bytes| bytes == b"published-result");

    let worker_root = root.to_owned();
    let mut task = ConnectedTuiTask::start_with(move |cancellation| {
        while !cancellation.is_cancelled() {
            thread::yield_now();
        }
        let mut store = match FilesystemResultStore::open(&worker_root) {
            Ok(store) => store,
            Err(_) => return CliResult::operation(make_p_error("result_store_unavailable")),
        };
        let mut guarded = CancellableResultStore {
            inner: &mut store,
            cancellation: Some(&cancellation),
        };
        if guarded.put_batch(&[b"late-result"], wall_tick()).is_ok() {
            return CliResult::success(make_p_error("late_result_published"));
        }
        let profiles = match offline_registry("cancelled-agent") {
            Ok(profiles) => profiles,
            Err(error) => return CliResult::operation(make_p_error(error.as_str())),
        };
        let account = ConfiguredAccount {
            account_ref: "offline".to_owned(),
            credential: SafeAccountReference::new("offline").expect("fixture account"),
        };
        let request = DelegationRequest::new(
            "cancelled-agent",
            "cancelled connected work",
            DelegationOptions::new(),
        )
        .expect("fixture request");
        match cancelled_receipt(&request, &profiles, &account) {
            Ok(receipt) => p_receipt_result(receipt),
            Err(error) => CliResult::operation(make_p_error(error.as_str())),
        }
    });
    task.cancel();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let output = loop {
        if let Some(output) = task.finished() {
            break output;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        thread::yield_now();
    };
    let empty = FilesystemResultStore::open(root)
        .and_then(|store| store.batches(wall_tick()))
        .is_ok_and(|batches| batches.is_empty());
    publication_won
        && empty
        && output.exit_code == TypedExit::Operation.code()
        && output.output.contains("\"failure\":\"cancelled\"")
        && !output.output.contains("late_result_published")
}

fn do_tui_connected_query(
    root: &Path,
    query: String,
    settings: &Settings,
    trust_current_root: bool,
    selection: (Option<String>, Option<String>, Option<String>),
    cancellation: Option<ConnectedCancellation>,
) -> CliResult {
    let descriptor = match ConfiguredAgentDescriptor::from_environment() {
        Ok(descriptor) => descriptor,
        Err(failure) => return CliResult::operation(make_p_error(failure.as_str())),
    };
    let (agent, model, reasoning) = selection;
    let mut options = DelegationOptions::new();
    options.model_override = model;
    options.reasoning_override = match reasoning {
        Some(reasoning) => match ReasoningLevel::parse(&reasoning) {
            Ok(reasoning) => Some(reasoning),
            Err(_) => return CliResult::usage(make_p_error("invalid_reasoning")),
        },
        None => None,
    };
    options.hard_total_token_ceiling = settings
        .connected_agent_task_token_ceiling
        .map(|value| value as usize);
    let profile = agent.unwrap_or_else(|| descriptor.profile.name().to_owned());
    let request = match DelegationRequest::new(profile, query, options) {
        Ok(request) => request,
        Err(_) => return CliResult::usage(make_p_error("invalid_identity")),
    };
    let mut controls = ExperimentalControls::default();
    let receipt = match production_headless_receipt_at(
        root,
        &request,
        false,
        Some(settings),
        trust_current_root,
        true,
        cancellation,
        &mut controls,
    ) {
        Ok(receipt) => receipt,
        Err(failure) => return CliResult::operation(make_p_error(failure.as_str())),
    };
    p_receipt_result_with(receipt, &controls)
}

#[cfg(test)]
fn p_receipt_result(receipt: DelegationReceipt) -> CliResult {
    p_receipt_result_with(receipt, &ExperimentalControls::default())
}

fn p_receipt_result_with(receipt: DelegationReceipt, controls: &ExperimentalControls) -> CliResult {
    let data = format!(
        "{{\"controls\":{},\"receipt\":{}}}",
        controls.controls_json(),
        receipt.to_json()
    );
    let metrics = format!("{{\"controls\":{}}}", controls.controls_json());
    match receipt.outcome() {
        InvocationOutcome::Complete { .. }
            if controls.response_sources.is_some_and(|limit| {
                receipt
                    .inline_result()
                    .is_some_and(|result| result.citations().len() > limit)
            }) =>
        {
            CliResult::operation(make_envelope(
                "p",
                "error",
                &data,
                &[],
                &["response_source_limit_exceeded".to_owned()],
                "{}",
                &metrics,
            ))
        }
        InvocationOutcome::Complete { .. } => {
            CliResult::success(make_envelope("p", "ok", &data, &[], &[], "{}", &metrics))
        }
        InvocationOutcome::Incomplete { failure, .. } => CliResult::operation(make_envelope(
            "p",
            "error",
            &data,
            &[],
            &[agent_failure_code(*failure).to_owned()],
            "{}",
            &metrics,
        )),
    }
}

const fn agent_failure_code(failure: AgentFailure) -> &'static str {
    match failure {
        AgentFailure::AgentDisabled => "agent_disabled",
        AgentFailure::ExplicitOffline => "explicit_offline",
        AgentFailure::ProjectUntrusted => "project_untrusted",
        AgentFailure::UnknownProfile => "unknown_profile",
        AgentFailure::UnknownProvider => "unknown_provider",
        AgentFailure::UnknownModel => "unknown_model",
        AgentFailure::MissingAuth => "missing_auth",
        AgentFailure::ProviderUnavailable => "provider_unavailable",
        AgentFailure::ProviderFailed => "provider_failed",
        AgentFailure::Timeout => "timeout",
        AgentFailure::Cancelled => "cancelled",
        AgentFailure::DepthExceeded => "depth_exceeded",
        AgentFailure::DeniedTool => "denied_tool",
        AgentFailure::InvalidOutput => "invalid_output",
        AgentFailure::SqzInputFailed => "sqz_input_failed",
        AgentFailure::SqzOutputFailed => "sqz_output_failed",
        AgentFailure::ResultStoreFailed => "result_store_failed",
        AgentFailure::DlpRejected => "dlp_rejected",
        AgentFailure::TokenBudgetUnattested => "token_budget_unattested",
        AgentFailure::TokenCeilingExceeded => "token_ceiling_exceeded",
        AgentFailure::GroundingFailed => "grounding_failed",
        AgentFailure::ClaimUnsupported => "claim_unsupported",
    }
}

fn do_headless_p(args: Vec<String>) -> CliResult {
    do_headless_p_with(args, production_headless_receipt)
}

fn do_headless_p_with(
    args: Vec<String>,
    execute: impl FnOnce(
        &DelegationRequest,
        bool,
        Option<&Settings>,
        bool,
        &mut ExperimentalControls,
    ) -> Result<DelegationReceipt, ConnectedSetupFailure>,
) -> CliResult {
    // Reject any literal credential/key flags with typed usage (no env, no secret reflection)
    for arg in &args {
        if matches!(
            arg.as_str(),
            "--api-key" | "--apikey" | "--key" | "--credential" | "--credentials"
        ) {
            return CliResult::usage(make_p_error("forbidden_credential_flag"));
        }
    }

    let mut agent: Option<String> = None;
    let mut reasoning: Option<String> = None;
    let mut tools_str: Option<String> = None;
    let mut provider: Option<String> = None;
    let mut model: Option<String> = None;
    let mut no_session = false;
    let mut offline = false;
    let mut trust_current_root = false;
    let mut controls = ExperimentalControls::default();
    let mut positionals: Vec<String> = vec![];
    let mut prompt_seen = false;
    let mut i = 0usize;
    while i < args.len() {
        let arg = &args[i];
        if arg.starts_with("--") {
            if prompt_seen {
                return CliResult::usage(make_p_error("unknown_option"));
            }
            match arg.as_str() {
                "--agent" => {
                    if agent.is_some() {
                        return CliResult::usage(make_p_error("duplicate_option"));
                    }
                    i += 1;
                    if i >= args.len() || args[i].starts_with("--") {
                        return CliResult::usage(make_p_error("missing_value"));
                    }
                    let val = args[i].clone();
                    if val.trim().is_empty() {
                        return CliResult::usage(make_p_error("missing_value"));
                    }
                    agent = Some(val);
                }
                "--reasoning" => {
                    if reasoning.is_some() {
                        return CliResult::usage(make_p_error("duplicate_option"));
                    }
                    i += 1;
                    if i >= args.len() || args[i].starts_with("--") {
                        return CliResult::usage(make_p_error("missing_value"));
                    }
                    let val = args[i].clone();
                    if val.trim().is_empty() {
                        return CliResult::usage(make_p_error("missing_value"));
                    }
                    reasoning = Some(val);
                }
                "--tools" => {
                    if tools_str.is_some() {
                        return CliResult::usage(make_p_error("duplicate_option"));
                    }
                    i += 1;
                    if i >= args.len() || args[i].starts_with("--") {
                        return CliResult::usage(make_p_error("missing_value"));
                    }
                    let val = args[i].clone();
                    if val.trim().is_empty() {
                        return CliResult::usage(make_p_error("missing_value"));
                    }
                    tools_str = Some(val);
                }
                "--provider" => {
                    if provider.is_some() {
                        return CliResult::usage(make_p_error("duplicate_option"));
                    }
                    i += 1;
                    if i >= args.len() || args[i].starts_with("--") {
                        return CliResult::usage(make_p_error("missing_value"));
                    }
                    let val = args[i].clone();
                    if val.trim().is_empty() {
                        return CliResult::usage(make_p_error("missing_value"));
                    }
                    provider = Some(val);
                }
                "--model" => {
                    if model.is_some() {
                        return CliResult::usage(make_p_error("duplicate_option"));
                    }
                    i += 1;
                    if i >= args.len() || args[i].starts_with("--") {
                        return CliResult::usage(make_p_error("missing_value"));
                    }
                    let val = args[i].clone();
                    if val.trim().is_empty() {
                        return CliResult::usage(make_p_error("missing_value"));
                    }
                    model = Some(val);
                }
                "--no-session" => {
                    if no_session {
                        return CliResult::usage(make_p_error("duplicate_option"));
                    }
                    no_session = true;
                }
                "--offline" => {
                    if offline {
                        return CliResult::usage(make_p_error("duplicate_option"));
                    }
                    offline = true;
                }
                "--trust-current-root" => {
                    if trust_current_root {
                        return CliResult::usage(make_p_error("duplicate_option"));
                    }
                    trust_current_root = true;
                }
                "--max-sources" | "--dependency-depth" | "--response-sources"
                | "--excerpt-bytes" => {
                    let (slot, maximum) = match arg.as_str() {
                        "--max-sources" => (&mut controls.max_sources, QUERY_RESULT_LIMIT),
                        "--dependency-depth" => {
                            (&mut controls.dependency_depth, DEPENDENCY_DEPTH_LIMIT)
                        }
                        "--response-sources" => {
                            (&mut controls.response_sources, QUERY_RESULT_LIMIT)
                        }
                        _ => (&mut controls.excerpt_bytes, EXCERPT_BYTE_LIMIT),
                    };
                    if slot.is_some() {
                        return CliResult::usage(make_p_error("duplicate_option"));
                    }
                    i += 1;
                    let Some(value) = args.get(i) else {
                        return CliResult::usage(make_p_error("missing_value"));
                    };
                    let Ok(value) = value.parse::<usize>() else {
                        return CliResult::usage(make_p_error("invalid_control"));
                    };
                    if (arg != "--dependency-depth" && value == 0) || value > maximum {
                        return CliResult::usage(make_p_error("invalid_control"));
                    }
                    *slot = Some(value);
                }
                _ => {
                    return CliResult::usage(make_p_error("unknown_option"));
                }
            }
        } else {
            if prompt_seen {
                return CliResult::usage(make_p_error("too_many_positional"));
            }
            positionals.push(arg.clone());
            prompt_seen = true;
        }
        i += 1;
    }

    if positionals.len() != 1 {
        return CliResult::usage(make_p_error(if positionals.is_empty() {
            "missing_prompt"
        } else {
            "too_many_positional"
        }));
    }
    let prompt = positionals.into_iter().next().unwrap();
    if prompt.trim().is_empty() {
        return CliResult::usage(make_p_error("missing_prompt"));
    }

    let agent = match agent {
        Some(a) if !a.trim().is_empty() => a,
        _ => return CliResult::usage(make_p_error("missing_agent")),
    };

    let reasoning_level = match reasoning {
        Some(r) => match ReasoningLevel::parse(&r) {
            Ok(rl) => Some(rl),
            Err(_) => return CliResult::usage(make_p_error("invalid_reasoning")),
        },
        None => None,
    };

    let tool_policy = if let Some(ts) = tools_str {
        match parse_tools(&ts) {
            Ok(p) => p,
            Err(detail) => return CliResult::usage(make_p_error(&detail)),
        }
    } else {
        ToolPolicy::read_only_default()
    };

    let project_settings = if offline {
        None
    } else {
        load_settings(Path::new(".bran/settings.conf"))
            .ok()
            .flatten()
    };

    // Validate identities via existing core ctor.
    let mut del_opts = DelegationOptions::new();
    del_opts.provider_override = provider;
    del_opts.model_override = model;
    del_opts.reasoning_override = reasoning_level;
    del_opts.tool_policy = tool_policy;
    del_opts.no_session = no_session;
    if !offline {
        del_opts.hard_total_token_ceiling = project_settings
            .as_ref()
            .and_then(|settings| settings.connected_agent_task_token_ceiling)
            .map(|value| value as usize);
    }

    // Production identities come only from explicit provider-neutral
    // descriptors. Deterministic synthetic profiles exist in test builds only.
    let configured_registry = if offline {
        offline_registry(&agent)
    } else {
        ConfiguredAgentDescriptor::from_environment().and_then(|descriptor| descriptor.registry())
    };
    let apr = match configured_registry {
        Ok(registry) => registry,
        Err(failure) => {
            #[cfg(test)]
            {
                let _ = failure;
                synthetic_builtin_profiles()
            }
            #[cfg(not(test))]
            {
                return CliResult::operation(make_p_error(failure.as_str()));
            }
        }
    };
    let profile = match apr.get(&agent) {
        Ok(p) => p,
        Err(_) => return CliResult::usage(make_p_error("unknown_profile")),
    };
    if del_opts
        .provider_override
        .as_deref()
        .is_some_and(|value| !apr.provider_registry().contains(value))
    {
        return CliResult::usage(make_p_error("unknown_provider"));
    }
    if let Some(model) = del_opts.model_override.as_deref() {
        let provider = del_opts
            .provider_override
            .as_deref()
            .unwrap_or(profile.provider());
        if apr
            .model_registry()
            .require_for_provider(provider, model)
            .is_err()
        {
            return CliResult::usage(make_p_error("unknown_model"));
        }
    }

    let del_req = match DelegationRequest::new(agent, prompt, del_opts) {
        Ok(request) => request,
        Err(_) => return CliResult::usage(make_p_error("invalid_identity")),
    };
    let receipt = match execute(
        &del_req,
        offline,
        project_settings.as_ref(),
        trust_current_root,
        &mut controls,
    ) {
        Ok(receipt) => receipt,
        Err(failure) => return CliResult::operation(make_p_error(failure.as_str())),
    };
    p_receipt_result_with(receipt, &controls)
}

struct CliResult {
    output: String,
    exit_code: ExitCode,
    is_error: bool,
    #[cfg_attr(not(test), allow(dead_code))]
    is_interactive: bool,
}

impl CliResult {
    fn success(output: String) -> Self {
        Self {
            output,
            exit_code: TypedExit::Success.code(),
            is_error: false,
            is_interactive: false,
        }
    }

    fn usage(output: String) -> Self {
        Self {
            output,
            exit_code: TypedExit::Usage.code(),
            is_error: true,
            is_interactive: false,
        }
    }

    fn operation(output: String) -> Self {
        Self {
            output,
            exit_code: TypedExit::Operation.code(),
            is_error: true,
            is_interactive: false,
        }
    }

    fn write_to_stdio(self) -> ExitCode {
        if self.is_error {
            eprintln!("{}", self.output);
        } else {
            println!("{}", self.output);
        }
        self.exit_code
    }
}

#[cfg(test)]
mod tests {
    use super::{
        derive_bundle_from_snapshot, AgentFailure, AgentRuntime, AgentRuntimeAuthority,
        AgentRuntimeConfig, AgentSqzAdapter, CliApp, ExitCode, InvocationOutcome,
        MemoryResultStore, RuntimePorts, SqzPolicy, TypedExit, MISSING_COMMAND_ERROR, SMOKE_OUTPUT,
        UNKNOWN_COMMAND_ERROR,
    };
    use bran_core::bundle::ParseStatus;
    use bran_core::metadata::MetadataReport;
    use bran_core::policy::MAX_POLICY_BYTES;
    use bran_core::scan::{ContentIdentity, ScanEntry, ScanSnapshot};
    use std::sync::Arc;

    struct RequestRecorder {
        seen: Arc<std::sync::Mutex<Option<(String, String, bran_core::agent::ReasoningLevel)>>>,
        seen_prompt: Arc<std::sync::Mutex<Option<String>>>,
    }

    impl bran_core::agent::runtime::ProviderPort<bran_core::adapters::SafeAccountReference>
        for RequestRecorder
    {
        fn invoke(
            &self,
            request: &bran_core::agent::runtime::ProviderRequest,
            _credential: &bran_core::adapters::SafeAccountReference,
        ) -> Result<
            bran_core::agent::runtime::ProviderOutput,
            bran_core::agent::runtime::ProviderError,
        > {
            *self.seen.lock().unwrap() = Some((
                request.provider().to_owned(),
                request.model().to_owned(),
                request.requested(),
            ));
            *self.seen_prompt.lock().unwrap() = Some(request.prompt().to_owned());
            Err(bran_core::agent::runtime::ProviderError::Cancelled)
        }
    }

    fn write_stopword_distractors(root: &std::path::Path) {
        let directory = root.join("stopword-distractors");
        std::fs::create_dir(&directory).unwrap();
        for index in 0..40 {
            let document = format!(
                "---\ntype: concept\ntitle: About and for from the this with\nokf_status: active\ntags: about\ntimestamp: 2026-07-20T00:00:00Z\nresource: test://stopword-{index}\npublic_boundary: safe\n---\nnoise\n"
            );
            std::fs::write(directory.join(format!("distractor-{index}.md")), document).unwrap();
        }
    }

    fn common_term_query() -> String {
        "about and for from the this with seed".to_owned()
    }

    fn assert_help_alias(alias: &str) {
        let help = CliApp::run([alias]);
        assert_eq!(help.exit_code, ExitCode::SUCCESS);
        assert!(!help.is_error);
        assert!(help.output.contains("Usage: bran <command> [arguments]"));
        assert!(help.output.contains("packet <repo-root> <request>"));
        assert!(help.output.contains("-p [options] <request>"));
    }

    fn assert_invalid_packet_args(args: Vec<&str>) {
        let invalid = CliApp::run(args.into_iter().map(str::to_owned));
        assert_eq!(invalid.exit_code, TypedExit::Usage.code());
        assert!(
            invalid.output.contains("invalid_control")
                || invalid.output.contains("duplicate_option")
        );
    }

    fn write_ranked_documents(directory: &std::path::Path) {
        for index in 0..33 {
            std::fs::write(
                directory.join(format!("rank-{index:02}.md")),
                format!(
                    "---\ntype: concept\ntitle: ranktarget partial {index}\nokf_status: active\ntags: partial\ntimestamp: 2026-07-20T00:00:00Z\nresource: test://rank-{index}\npublic_boundary: safe\n---\nnoise\n"
                ),
            )
            .unwrap();
        }
    }

    fn assert_contains_all(output: &str, expected: &[&str]) {
        for value in expected {
            assert!(output.contains(value));
        }
    }

    fn assert_skill_instructions(skill: &str) {
        assert!(
            skill.contains("BRAN locates and cites repository evidence")
                && skill.contains("outer agent owns all decisions and changes")
                && skill.contains("Default to one read-only packet")
                && skill.contains("Use `bran query")
                && skill.contains("only for a focused follow-up")
                && skill.contains(
                    "Use connected `bran -p` only when a connected profile is configured"
                )
                && skill.contains("current repository is explicitly trusted")
                && skill.contains("Report failures and unavailable fields honestly")
                && skill.contains("preserve provenance and citations")
                && skill.contains("Run `bran -h` for command and option help")
                && skill.contains("Never pass credentials on the command")
        );
    }

    fn assert_numbered_rules(preamble: &str) {
        for rule in 1..=5 {
            assert!(preamble
                .lines()
                .any(|line| line.starts_with(&format!("{rule}. "))));
        }
    }

    struct DoctorEnvironment {
        previous: [(&'static str, Option<std::ffi::OsString>); 7],
    }

    impl DoctorEnvironment {
        fn ready(host_executable: &std::path::Path, host_digest: &str) -> Self {
            let names = [
                "BRAN_AGENT_PROFILE",
                "BRAN_AGENT_PROVIDER",
                "BRAN_AGENT_MODEL",
                "BRAN_AGENT_REASONING",
                "BRAN_AGENT_ACCOUNT_REF",
                "BRAN_EXTERNAL_HOST_EXECUTABLE",
                "BRAN_EXTERNAL_HOST_SHA256",
            ];
            let previous = names.map(|name| (name, std::env::var_os(name)));
            for (name, value) in [
                ("BRAN_AGENT_PROFILE", "local-agent"),
                ("BRAN_AGENT_PROVIDER", "local-provider"),
                ("BRAN_AGENT_MODEL", "local-model"),
                ("BRAN_AGENT_REASONING", "medium"),
                ("BRAN_AGENT_ACCOUNT_REF", "local-account-reference"),
            ] {
                std::env::set_var(name, value);
            }
            std::env::set_var("BRAN_EXTERNAL_HOST_EXECUTABLE", host_executable);
            std::env::set_var("BRAN_EXTERNAL_HOST_SHA256", host_digest);
            Self { previous }
        }
    }

    impl Drop for DoctorEnvironment {
        fn drop(&mut self) {
            for (name, value) in &self.previous {
                if let Some(value) = value {
                    std::env::set_var(name, value);
                } else {
                    std::env::remove_var(name);
                }
            }
        }
    }

    #[test]
    fn p1_cli() {
        let smoke = CliApp::run(["smoke".to_owned()]);
        assert_eq!(smoke.output, SMOKE_OUTPUT);
        assert_eq!(smoke.exit_code, ExitCode::SUCCESS);
        assert!(!smoke.is_error);

        assert_help_alias("-h");
        assert_help_alias("--help");
        assert_help_alias("help");

        for alias in ["-V", "--version"] {
            let version = CliApp::run([alias.to_owned()]);
            assert_eq!(version.output, "bran 0.1.0");
            assert_eq!(version.exit_code, ExitCode::SUCCESS);
            assert!(!version.is_error);
        }

        let version_extra = CliApp::run(["--version".to_owned(), "extra".to_owned()]);
        assert_eq!(version_extra.output, UNKNOWN_COMMAND_ERROR);
        assert_eq!(version_extra.exit_code, TypedExit::Usage.code());
        assert!(version_extra.is_error);

        let missing = CliApp::run(Vec::<String>::new());
        assert_eq!(missing.output, MISSING_COMMAND_ERROR);
        assert_eq!(missing.exit_code, TypedExit::Usage.code());
        assert!(missing.is_error);

        let unknown = CliApp::run(["other".to_owned()]);
        assert_eq!(unknown.output, UNKNOWN_COMMAND_ERROR);
        assert_eq!(unknown.exit_code, TypedExit::Usage.code());
        assert!(unknown.is_error);

        let extra = CliApp::run(["smoke".to_owned(), "extra".to_owned()]);
        assert_eq!(extra.output, UNKNOWN_COMMAND_ERROR);
        assert_eq!(extra.exit_code, TypedExit::Usage.code());
        assert!(extra.is_error);

        let missing_query1 = CliApp::run(vec!["query".to_owned(), "ROOT".to_owned()]);
        assert_eq!(
            missing_query1.output,
            super::make_query_error("missing_query")
        );
        assert_eq!(missing_query1.exit_code, TypedExit::Usage.code());
        assert!(missing_query1.is_error);

        let missing_query2 = CliApp::run(vec![
            "query".to_owned(),
            "ROOT".to_owned(),
            " \t".to_owned(),
        ]);
        assert_eq!(
            missing_query2.output,
            super::make_query_error("missing_query")
        );
        assert_eq!(missing_query2.exit_code, TypedExit::Usage.code());
        assert!(missing_query2.is_error);

        let missing_packet1 = CliApp::run(vec!["packet".to_owned(), "ROOT".to_owned()]);
        assert_eq!(
            missing_packet1.output,
            super::make_packet_error("missing_query")
        );
        assert_eq!(missing_packet1.exit_code, TypedExit::Usage.code());
        assert!(missing_packet1.is_error);

        let missing_packet2 = CliApp::run(vec![
            "packet".to_owned(),
            "ROOT".to_owned(),
            " \t".to_owned(),
        ]);
        assert_eq!(
            missing_packet2.output,
            super::make_packet_error("missing_query")
        );
        assert_eq!(missing_packet2.exit_code, TypedExit::Usage.code());
        assert!(missing_packet2.is_error);

        let source = "---\ntype: [\n---\nbody\n";
        let mut snapshot = ScanSnapshot::default();
        snapshot.entries.insert(
            "doc.md".to_owned(),
            Arc::new(ScanEntry {
                identity: ContentIdentity::from_bytes(source.as_bytes()),
                source: Arc::from(source.as_bytes()),
                metadata: MetadataReport {
                    warnings: vec!["malformed-metadata: invalid yaml".to_owned()],
                    ..MetadataReport::default()
                },
            }),
        );
        let bundle = derive_bundle_from_snapshot(&snapshot).expect("valid bundle");
        let frontmatter = bundle.docs().get("doc.md").expect("document").frontmatter();
        assert_eq!(frontmatter.raw(), "---\ntype: [\n---\n");
        assert!(
            matches!(frontmatter.status(), ParseStatus::Malformed { reason } if reason == "invalid yaml")
        );
        assert_eq!(frontmatter.parsed(), None);

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let non_utf8 = CliApp::run([std::ffi::OsString::from_vec(vec![0xff])]);
            assert_eq!(non_utf8.output, UNKNOWN_COMMAND_ERROR);
            assert_eq!(non_utf8.exit_code, TypedExit::Usage.code());
            assert!(non_utf8.is_error);
        }
    }

    #[test]
    fn agents_list_without_descriptor_is_graceful() {
        let result = super::unconfigured_agents_list();
        assert_eq!(result.exit_code, ExitCode::SUCCESS);
        assert!(!result.is_error);
        assert!(result.output.contains("\"status\":\"ok\""));
        assert!(result.output.contains("\"agents\":[]"));
        assert!(result.output.contains("provider_descriptor_not_configured"));
        assert!(result.output.contains("BRAN_AGENT_PROFILE"));
    }

    #[test]
    fn p3_headless_cli_and_maintain_lifecycle() {
        // One sequential stdlib-temp-dir journey. No tables, no other names.
        let base = std::env::temp_dir();
        let unique = format!(
            "bran-p3-headless-cli-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = base.join(&unique);
        std::fs::create_dir_all(&root).expect("temp root");
        std::fs::create_dir_all(root.join(".bran")).unwrap();
        std::fs::write(root.join(".bran/policy.yaml"), minimal_valid_policy()).unwrap();
        std::fs::write(root.join(".branignore"), ".bran/\n").unwrap();

        let tgt = "repl.txt".to_owned();
        let rep = "p3-replacement-bytes-exact\n".to_owned();
        let proot = root.to_string_lossy().into_owned();

        let seed_document = "---\ntype: concept\ntitle: Seed\nokf_status: active\ntags: [p3, packet]\ntimestamp: 2026-07-19T00:00:00Z\nresource: test://seed\npublic_boundary: safe\ndependency: dep.md\n---\nseed-full-body-sentinel [dependency](dep.md)\n# Citations\nref\n";
        let dependency_document = "---\ntype: concept\ntitle: Dep\nokf_status: active\ntags: [p3, packet]\ntimestamp: 2026-07-19T00:00:00Z\nresource: test://dep\npublic_boundary: safe\n---\ndep [reference](x)\n# Citations\nref\n";
        std::fs::write(root.join("seed.md"), seed_document.as_bytes()).unwrap();
        std::fs::write(root.join("dep.md"), dependency_document.as_bytes()).unwrap();

        // Slice 5.4 extends this existing setup journey: both doctors are
        // read-only, and agent readiness is based on a real packet result.
        let settings_path = base.join(format!("{unique}-settings.conf"));
        bran_tui::apply_settings(&settings_path, &bran_tui::quick_safe_config()).unwrap();
        let skill_path = base.join(format!("{unique}-SKILL.md"));
        std::fs::write(&skill_path, "# local agent skill\n").unwrap();
        let onboarding_doctor =
            super::do_doctor_at("--onboarding", &settings_path, Some(&skill_path));
        assert_eq!(onboarding_doctor.exit_code, ExitCode::SUCCESS);
        assert!(onboarding_doctor.output.contains("\"ready\":true"));
        assert!(onboarding_doctor
            .output
            .contains("\"connected_task_total_token_ceiling\":null"));
        let agent_doctor = super::do_doctor_at("--agent", &settings_path, Some(&skill_path));
        assert_eq!(agent_doctor.exit_code, TypedExit::Validation.code());
        assert!(agent_doctor.output.contains("\"ready\":false"));
        assert!(agent_doctor.output.contains("\"local_setup_ready\":true"));
        assert!(agent_doctor
            .output
            .contains("\"connected_execution_status\":\"unavailable\""));
        assert!(agent_doctor
            .output
            .contains("\"packet_round_trip\":\"available\""));
        assert!(agent_doctor
            .output
            .contains("\"host_attestation\":\"unavailable\""));
        assert!(agent_doctor.output.contains("\"provider_calls\":0"));

        let host_executable = std::env::current_exe().unwrap();
        let host_digest = bran_core::agent::result_store::ResultId::sha256(
            &std::fs::read(&host_executable).unwrap(),
        )
        .value()
        .to_owned();
        let doctor_environment = DoctorEnvironment::ready(&host_executable, &host_digest);
        let mut ready_settings = bran_tui::quick_safe_config();
        ready_settings.profile = bran_tui::OperatingProfile::ConnectedAgent;
        ready_settings.sqz = false;
        bran_tui::apply_settings(&settings_path, &ready_settings).unwrap();
        let ready_doctor = super::do_doctor_at("--agent", &settings_path, Some(&skill_path));
        assert_eq!(ready_doctor.exit_code, TypedExit::Validation.code());
        assert!(ready_doctor.output.contains("\"ready\":false"));
        assert!(ready_doctor
            .output
            .contains("\"connected_execution_ready\":false"));
        assert!(ready_doctor
            .output
            .contains("\"connected_execution_status\":\"ready_to_attempt\""));
        assert!(ready_doctor
            .output
            .contains("\"connected_agent_runtime\":\"available\""));
        assert!(ready_doctor
            .output
            .contains("\"host_attestation\":\"unavailable\""));
        assert!(ready_doctor.output.contains("\"provider_calls\":0"));

        std::env::set_var("BRAN_EXTERNAL_HOST_SHA256", "0".repeat(64));
        let invalid_doctor = super::do_doctor_at("--agent", &settings_path, Some(&skill_path));
        assert_eq!(invalid_doctor.exit_code, TypedExit::Validation.code());
        assert!(invalid_doctor.output.contains("\"ready\":false"));
        assert!(invalid_doctor
            .output
            .contains("\"connected_execution_ready\":false"));
        assert!(invalid_doctor
            .output
            .contains("\"connected_agent_runtime\":\"unavailable\""));
        drop(doctor_environment);

        let packet_result =
            CliApp::run(vec!["packet".to_owned(), proot.clone(), "seed".to_owned()]);
        assert_eq!(packet_result.exit_code, ExitCode::SUCCESS);
        assert!(!packet_result.is_error);
        assert!(
            packet_result.output.contains("\"command\":\"packet\"")
                && packet_result.output.contains("\"status\":\"ok\"")
        );
        assert!(packet_result
            .output
            .contains("\"selected_locators\":[\"dep.md\",\"seed.md\"]"));
        assert!(packet_result
            .output
            .contains("\"locator\":\"dep.md\",\"reason\":\"declared_dependency\""));
        assert!(packet_result
            .output
            .contains("\"locator\":\"seed.md\",\"reason\":\"metadata_seed\""));
        assert!(packet_result
            .output
            .contains("\"source_rankings\":[{\"locator\":\"seed.md\",\"rank\":1,\"score\":"));
        assert!(packet_result
            .output
            .contains("\"seed_ids\":[\"n:7:a346da01e22eda4be56ecdeffc2f0bb4ce2004c4de8549ed\"]"));
        assert!(packet_result
            .output
            .contains("\"admitted_dependency_ids\":[\"n:6:64eee9fc1a415ae7ee3cea2af4c15b15206bc0d26c8ca9fb\"]"));
        assert!(packet_result
            .output
            .contains("\"selected_ids\":[\"n:6:64eee9fc1a415ae7ee3cea2af4c15b15206bc0d26c8ca9fb\",\"n:7:a346da01e22eda4be56ecdeffc2f0bb4ce2004c4de8549ed\"]"));
        assert!(!packet_result.output.contains("seed-full-body-sentinel"));
        let metric = |name: &str| {
            let value = packet_result
                .output
                .split_once(&format!("\"{name}\":"))
                .unwrap()
                .1;
            value
                .split(|character: char| !character.is_ascii_digit())
                .next()
                .unwrap()
                .parse::<usize>()
                .unwrap()
        };
        let source_bytes = seed_document.len() + dependency_document.len();
        assert_eq!(metric("candidate_source_bytes"), source_bytes);
        assert_eq!(metric("selected_source_bytes"), source_bytes);
        assert_eq!(metric("context_bytes_avoided"), 0);
        assert_eq!(metric("raw_bytes"), metric("encoded_packet_bytes"));
        assert_eq!(
            metric("estimated_tokens"),
            metric("encoded_packet_bytes").div_ceil(4)
        );
        assert!(packet_result
            .output
            .contains("\"runtime_token_ceiling\":null"));
        assert!(packet_result
            .output
            .contains("\"actual_model_input_tokens\":\"unavailable\""));
        assert!(packet_result.output.contains("\"payload\":"));
        assert!(packet_result.output.contains("\"status\":\"off\""));
        assert!(packet_result
            .output
            .contains("\"actual_input_tokens\":null"));
        assert!(packet_result
            .output
            .contains("\"actual_output_tokens\":null"));

        let controlled_packet = CliApp::run(vec![
            "packet".to_owned(),
            "--max-sources".to_owned(),
            "1".to_owned(),
            "--dependency-depth".to_owned(),
            "0".to_owned(),
            "--response-sources".to_owned(),
            "1".to_owned(),
            "--excerpt-bytes".to_owned(),
            "8".to_owned(),
            proot.clone(),
            "seed".to_owned(),
        ]);
        assert_eq!(controlled_packet.exit_code, ExitCode::SUCCESS);
        assert!(controlled_packet
            .output
            .contains("\"requested\":{\"max_sources\":1,\"dependency_depth\":0,\"response_sources\":1,\"excerpt_bytes\":8}"));
        assert!(controlled_packet
            .output
            .contains("\"selected_locators\":[\"seed.md\"]"));
        assert!(controlled_packet.output.contains("excerpt: seed-ful"));
        assert!(!packet_result.output.contains("excerpt: "));
        assert!(controlled_packet.output.contains("\"excerpt_bytes\":8"));
        let response_only_packet = CliApp::run(vec![
            "packet".to_owned(),
            "--response-sources".to_owned(),
            "1".to_owned(),
            proot.clone(),
            "seed".to_owned(),
        ]);
        assert_eq!(response_only_packet.exit_code, ExitCode::SUCCESS);
        assert!(response_only_packet
            .output
            .contains("\"selected_locators\":[\"dep.md\",\"seed.md\"]"));
        assert_invalid_packet_args(vec!["packet", "--max-sources", "0", &proot, "seed"]);
        assert_invalid_packet_args(vec!["packet", "--dependency-depth", "5", &proot, "seed"]);
        assert_invalid_packet_args(vec!["packet", "--response-sources", "x", &proot, "seed"]);
        assert_invalid_packet_args(vec![
            "packet",
            "--excerpt-bytes",
            "1",
            "--excerpt-bytes",
            "2",
            &proot,
            "seed",
        ]);
        let mut bounded_settings = bran_tui::quick_safe_config();
        bounded_settings.sqz = false;
        bounded_settings.connected_agent_task_token_ceiling = Some(100);
        std::fs::create_dir_all(root.join(".bran")).unwrap();
        bran_tui::apply_settings(&root.join(".bran/settings.conf"), &bounded_settings).unwrap();
        std::fs::write(
            root.join("a-earlier.md"),
            "---\ntype: concept\ntitle: rankedlocator helper\nokf_status: active\npublic_boundary: safe\n---\nignored\n",
        )
        .unwrap();
        std::fs::write(
            root.join("z-later.md"),
            "---\ntype: concept\ntitle: Later\nokf_status: active\ntags: rankedlocator\ntimestamp: 2026-07-20T00:00:00Z\npublic_boundary: safe\n---\nranked-full-body-sentinel\n",
        )
        .unwrap();
        let ranked_budget = CliApp::run(vec![
            "packet".to_owned(),
            proot.clone(),
            "rankedlocator".to_owned(),
        ]);
        assert_eq!(
            ranked_budget.exit_code,
            ExitCode::SUCCESS,
            "{}",
            ranked_budget.output
        );
        assert!(ranked_budget
            .output
            .contains("\"selected_locators\":[\"z-later.md\"]"));
        assert!(ranked_budget
            .output
            .contains("\"locator\":\"z-later.md\",\"rank\":1,\"score\":{\"exact\":1"));
        assert!(!ranked_budget.output.contains("a-earlier.md\"]"));
        assert!(!ranked_budget.output.contains("ranked-full-body-sentinel"));
        std::fs::remove_file(root.join("a-earlier.md")).unwrap();
        std::fs::remove_file(root.join("z-later.md")).unwrap();
        std::fs::write(
            root.join("exact-title.md"),
            "---\ntype: concept\ntitle: parentmatch\nokf_status: deprecated\npublic_boundary: safe\n---\nexact title\n",
        )
        .unwrap();
        std::fs::write(
            root.join("parent-status.md"),
            "---\ntype: concept\ntitle: Unrelated status\nokf_status: active\nstatus: parentmatch\npublic_boundary: safe\n---\nstatus match\n",
        )
        .unwrap();
        std::fs::write(
            root.join("parent-okf.md"),
            "---\ntype: concept\ntitle: Unrelated OKF\nokf_status: retiredmatch\npublic_boundary: safe\n---\nOKF match\n",
        )
        .unwrap();
        std::fs::write(
            root.join("parent-boundary.md"),
            "---\ntype: concept\ntitle: Unrelated boundary\nokf_status: active\npublic_boundary: internalmatch\n---\nboundary match\n",
        )
        .unwrap();
        let parent_query = CliApp::run(vec![
            "query".to_owned(),
            proot.clone(),
            "parentmatch retiredmatch internalmatch".to_owned(),
        ]);
        assert_eq!(parent_query.exit_code, ExitCode::SUCCESS);
        assert!(parent_query.output.contains(
            "\"selected_locators\":[\"exact-title.md\",\"parent-boundary.md\",\"parent-okf.md\",\"parent-status.md\"]"
        ));
        assert!(parent_query.output.contains(
            "\"source_rankings\":[{\"locator\":\"exact-title.md\",\"rank\":1,\"score\":{\"exact\":1"
        ));
        assert!(parent_query.output.contains("partial:okf_status"));
        assert!(parent_query.output.contains("partial:public_boundary"));
        assert!(parent_query.output.contains("partial:status"));
        assert!(!parent_query.output.contains("metadata: path="));
        std::fs::remove_file(root.join("exact-title.md")).unwrap();
        std::fs::remove_file(root.join("parent-status.md")).unwrap();
        std::fs::remove_file(root.join("parent-okf.md")).unwrap();
        std::fs::remove_file(root.join("parent-boundary.md")).unwrap();

        let mut sqz_settings = bran_tui::quick_safe_config();
        sqz_settings.sqz = true;
        bran_tui::apply_settings(&root.join(".bran/settings.conf"), &sqz_settings).unwrap();
        std::env::set_var("BRAN_P3_PACKET_SQZ_FIXTURE", "1");
        let sqz_packet = CliApp::run(vec!["packet".to_owned(), proot.clone(), "seed".to_owned()]);
        std::env::remove_var("BRAN_P3_PACKET_SQZ_FIXTURE");
        assert_eq!(
            sqz_packet.exit_code,
            ExitCode::SUCCESS,
            "{}",
            sqz_packet.output
        );
        assert!(sqz_packet.output.contains("\"policy\":\"public-on\""));
        assert!(sqz_packet.output.contains("\"status\":\"not-beneficial\""));
        assert!(sqz_packet.output.contains("\"returned_identity\":{"));
        assert!(sqz_packet.output.contains("\"actual_input_tokens\":null"));
        assert!(sqz_packet.output.contains("\"actual_output_tokens\":null"));
        assert!(!sqz_packet.output.contains("sqz-applied-v1"));
        std::fs::remove_file(root.join(".bran/settings.conf")).unwrap();

        let natural_query = CliApp::run(vec![
            "query".to_owned(),
            proot.clone(),
            "tell me about seed".to_owned(),
        ]);
        assert_eq!(natural_query.exit_code, ExitCode::SUCCESS);
        assert!(natural_query
            .output
            .contains("\"selected_locators\":[\"dep.md\",\"seed.md\"]"));
        let ranked = root.join("ranked");
        std::fs::create_dir(&ranked).unwrap();
        write_ranked_documents(&ranked);
        std::fs::write(
            ranked.join("zz-exact.md"),
            "---\ntype: concept\ntitle: ranktarget\nokf_status: active\ntags: exact\ntimestamp: 2026-07-20T00:00:00Z\nresource: test://rank-exact\npublic_boundary: safe\n---\nnoise\n",
        )
        .unwrap();
        let ranked_query = CliApp::run(vec![
            "query".to_owned(),
            proot.clone(),
            "ranktarget".to_owned(),
        ]);
        let ranked_query_repeat = CliApp::run(vec![
            "query".to_owned(),
            proot.clone(),
            "ranktarget".to_owned(),
        ]);
        assert_eq!(ranked_query.exit_code, ExitCode::SUCCESS);
        assert_eq!(ranked_query.output, ranked_query_repeat.output);
        let rankings = ranked_query
            .output
            .split_once("\"source_rankings\":[")
            .unwrap()
            .1
            .split_once("],\"candidate_source_bytes\"")
            .unwrap()
            .0;
        assert!(rankings
            .starts_with("{\"locator\":\"ranked/zz-exact.md\",\"rank\":1,\"score\":{\"exact\":1"));
        assert_eq!(
            rankings.matches("\"rank\":").count(),
            super::QUERY_RESULT_LIMIT
        );
        std::fs::remove_dir_all(&ranked).unwrap();
        write_stopword_distractors(&root);
        let bounded_stopword_query =
            CliApp::run(vec!["query".to_owned(), proot.clone(), common_term_query()]);
        assert_eq!(bounded_stopword_query.exit_code, ExitCode::SUCCESS);
        assert!(bounded_stopword_query
            .output
            .contains("\"selected_locators\":[\"dep.md\",\"seed.md\"]"));
        assert!(!bounded_stopword_query
            .output
            .contains("stopword-distractors"));
        std::fs::remove_dir_all(root.join("stopword-distractors")).unwrap();
        let multi_packet = CliApp::run(vec![
            "packet".to_owned(),
            proot.clone(),
            "seed dep".to_owned(),
        ]);
        assert_eq!(multi_packet.exit_code, ExitCode::SUCCESS);
        assert!(multi_packet
            .output
            .contains("\"selected_locators\":[\"dep.md\",\"seed.md\"]"));
        assert!(multi_packet.output.contains("\"selected_ids\":["));

        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::create_dir(root.join("tests")).unwrap();
        std::fs::write(
            root.join("task-retry.md"),
            "---\ntype: task\ntitle: Account-scoped retry queue deduplication\nokf_status: active\ntags:\n  - retry\n  - queue\ntimestamp: 2026-07-20T00:00:00Z\nresource: test://task-retry\npublic_boundary: safe\nimplementation:\n  - src/worker.rs\nvalidation:\n  - tests/worker_check.rs\n---\nCanonical task contract.\n# Citations\nref\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/worker.rs"),
            "// ---\n// type: implementation\n// title: Delivery worker\n// okf_status: active\n// tags:\n//   - worker\n// timestamp: 2026-07-20T00:00:00Z\n// resource: test://worker\n// public_boundary: safe\n// ---\nfn group_by_owner() {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("tests/worker_check.rs"),
            "// ---\n// type: check\n// title: Owner isolation check\n// okf_status: active\n// tags:\n//   - visible-check\n// timestamp: 2026-07-20T00:00:00Z\n// resource: test://worker-check\n// public_boundary: safe\n// ---\nfn verifies_owner_isolation() {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("distractor.md"),
            "---\ntype: concept\ntitle: Unrelated incident note\nokf_status: active\ntags: unrelated\ntimestamp: 2026-07-20T00:00:00Z\nresource: test://distractor\npublic_boundary: safe\n---\naccount retry queue deduplication\n",
        )
        .unwrap();
        let okf_query = CliApp::run(vec![
            "query".to_owned(),
            proot.clone(),
            "account retry queue deduplication".to_owned(),
        ]);
        assert_eq!(okf_query.exit_code, ExitCode::SUCCESS);
        // The metadata-exact task contract ranks first; the document whose
        // body contains the full phrase now ranks second via body content.
        assert!(okf_query.output.contains(
            "\"selected_locators\":[\"distractor.md\",\"src/worker.rs\",\"task-retry.md\",\"tests/worker_check.rs\"]"
        ));
        assert!(okf_query.output.contains(
            "\"source_rankings\":[{\"locator\":\"task-retry.md\",\"rank\":1,\"score\":{\"exact\":2"
        ));
        assert!(okf_query.output.contains("partial:body"));
        let okf_packet = CliApp::run(vec![
            "packet".to_owned(),
            proot.clone(),
            "account retry queue deduplication".to_owned(),
        ]);
        assert_eq!(okf_packet.exit_code, ExitCode::SUCCESS);
        assert!(okf_packet
            .output
            .contains("\"locator\":\"task-retry.md\",\"reason\":\"metadata_seed\""));
        assert!(okf_packet
            .output
            .contains("\"locator\":\"distractor.md\",\"reason\":\"metadata_seed\""));
        assert!(okf_packet
            .output
            .contains("\"locator\":\"src/worker.rs\",\"reason\":\"declared_implementation\""));
        assert!(okf_packet
            .output
            .contains("\"locator\":\"tests/worker_check.rs\",\"reason\":\"declared_validation\""));
        let okf_estimate = okf_packet
            .output
            .split_once("\"estimated_tokens\":")
            .unwrap()
            .1
            .split(|character: char| !character.is_ascii_digit())
            .next()
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert!(okf_estimate <= 8_500);
        assert!(okf_packet
            .output
            .contains("\"token_estimate_method\":\"bytes-divided-by-four-ceiling\""));
        assert!(okf_packet
            .output
            .contains("\"actual_model_input_tokens\":\"unavailable\""));
        let line_comment_excerpt = CliApp::run(vec![
            "packet".to_owned(),
            "--max-sources".to_owned(),
            "1".to_owned(),
            "--excerpt-bytes".to_owned(),
            "8".to_owned(),
            proot.clone(),
            "Delivery worker".to_owned(),
        ]);
        assert_eq!(line_comment_excerpt.exit_code, ExitCode::SUCCESS);
        assert!(line_comment_excerpt.output.contains("excerpt: fn group"));
        std::fs::write(
            root.join("block.c"),
            "/* ---\n * type: implementation\n * title: Block excerpt\n * okf_status: active\n * public_boundary: safe\n * ---\n */\nint block_body_sentinel;\n",
        )
        .unwrap();
        let block_comment_excerpt = CliApp::run(vec![
            "packet".to_owned(),
            "--max-sources".to_owned(),
            "1".to_owned(),
            "--dependency-depth".to_owned(),
            "0".to_owned(),
            "--excerpt-bytes".to_owned(),
            "64".to_owned(),
            proot.clone(),
            "Block excerpt".to_owned(),
        ]);
        assert_eq!(block_comment_excerpt.exit_code, ExitCode::SUCCESS);
        assert!(block_comment_excerpt
            .output
            .contains("excerpt: int block_body_sentinel;"));
        assert!(!block_comment_excerpt.output.contains("excerpt: /* ---"));
        std::fs::write(
            root.join("boundary-conflict.md"),
            "---\ntype: concept\ntitle: boundaryconflict\nokf_status: active\npublic_boundary: safe\npublic_boundary: internal\npublic_boundary: private\n---\nconflicted-body-sentinel\n",
        )
        .unwrap();
        std::fs::write(
            root.join("boundary-missing.md"),
            "---\ntype: concept\ntitle: boundarymissing\nokf_status: active\n---\nmissing-body-sentinel\n",
        )
        .unwrap();
        let closed_excerpts = CliApp::run(vec![
            "packet".to_owned(),
            "--max-sources".to_owned(),
            "2".to_owned(),
            "--dependency-depth".to_owned(),
            "0".to_owned(),
            "--excerpt-bytes".to_owned(),
            "64".to_owned(),
            proot.clone(),
            "boundaryconflict boundarymissing".to_owned(),
        ]);
        assert_eq!(closed_excerpts.exit_code, ExitCode::SUCCESS);
        assert!(closed_excerpts.output.contains("\"excerpt_bytes\":0"));
        assert!(!closed_excerpts.output.contains("conflicted-body-sentinel"));
        assert!(!closed_excerpts.output.contains("missing-body-sentinel"));
        std::fs::remove_file(root.join("block.c")).unwrap();
        std::fs::remove_file(root.join("boundary-conflict.md")).unwrap();
        std::fs::remove_file(root.join("boundary-missing.md")).unwrap();
        std::fs::remove_file(root.join("task-retry.md")).unwrap();
        std::fs::remove_file(root.join("distractor.md")).unwrap();
        std::fs::remove_dir_all(root.join("src")).unwrap();
        std::fs::remove_dir_all(root.join("tests")).unwrap();

        // proposal zero mutation
        let pres = CliApp::run(vec![
            "maintain".to_owned(),
            "propose".to_owned(),
            proot.clone(),
            tgt.clone(),
            rep.clone(),
        ]);
        assert!(pres.output.contains("\"command\":\"maintain.propose\""));
        assert!(pres.output.contains("\"status\":\"ok\""));
        assert_eq!(pres.exit_code, ExitCode::SUCCESS);
        assert!(!pres.is_error);
        assert!(!root.join(&tgt).exists(), "propose must zero-mutate");
        let key = "\"digest\":\"";
        let start = pres.output.find(key).expect("propose returns digest") + key.len();
        let dig = pres.output[start..]
            .split('"')
            .next()
            .expect("digest terminator")
            .to_owned();

        // missing marker refusal (code 2, no mutation)
        let miss = CliApp::run(vec![
            "maintain".to_owned(),
            "apply".to_owned(),
            proot.clone(),
            tgt.clone(),
            rep.clone(),
            dig.clone(),
            "auth".to_owned(),
        ]);
        assert!(miss.output.contains("missing_fixture_authority_marker"));
        assert_eq!(miss.exit_code, TypedExit::Usage.code());
        assert!(miss.is_error);
        assert!(!root.join(&tgt).exists());

        // prepare marker + valid strict doc (one dir sequential)
        std::fs::write(
            root.join(".bran-fixture-authority"),
            b"bran-cli-fixture-v1\n",
        )
        .unwrap();
        let valid_doc = "---\ntype: concept\ntitle: P3 Headless\nokf_status: active\ntags: [p3, headless]\ntimestamp: 2026-07-19T00:00:00Z\nresource: test://p3\npublic_boundary: safe\n---\nBody [link](x).\n# Citations\nref\n";
        std::fs::write(root.join("p3.md"), valid_doc.as_bytes()).unwrap();

        let stale_target = "stale.txt".to_owned();
        std::fs::write(root.join(&stale_target), b"initial-source-at-propose\n").unwrap();
        let stale_proposal = CliApp::run(vec![
            "maintain".to_owned(),
            "propose".to_owned(),
            proot.clone(),
            stale_target.clone(),
            "stale-replacement\n".to_owned(),
        ]);
        assert!(stale_proposal.output.contains("\"status\":\"ok\""));
        let digest_key = "\"digest\":\"";
        let stale_digest_start = stale_proposal
            .output
            .find(digest_key)
            .expect("stale propose digest")
            + digest_key.len();
        let stale_digest = stale_proposal.output[stale_digest_start..]
            .split('"')
            .next()
            .expect("digest terminator")
            .to_owned();
        std::fs::write(root.join(&stale_target), b"changed-source-after-propose\n").unwrap();
        let stale_apply = CliApp::run(vec![
            "maintain".to_owned(),
            "apply".to_owned(),
            proot.clone(),
            stale_target.clone(),
            "stale-replacement\n".to_owned(),
            stale_digest.clone(),
            "bran-cli-fixture-v1".to_owned(),
        ]);
        assert!(stale_apply.output.contains("stale_source"));
        assert!(!stale_apply.output.contains("digest_mismatch"));
        assert_eq!(stale_apply.exit_code, TypedExit::Operation.code());
        assert!(stale_apply.is_error);
        assert_eq!(
            std::fs::read(root.join(&stale_target)).unwrap(),
            b"changed-source-after-propose\n"
        );

        let malformed_digest = format!("{}|s:", stale_digest.rsplit_once("|s:").unwrap().0);
        let malformed_apply = CliApp::run(vec![
            "maintain".to_owned(),
            "apply".to_owned(),
            proot.clone(),
            stale_target,
            "stale-replacement\n".to_owned(),
            malformed_digest,
            "bran-cli-fixture-v1".to_owned(),
        ]);
        assert!(malformed_apply.output.contains("digest_mismatch"));
        assert!(!malformed_apply.output.contains("stale_source"));
        assert_eq!(malformed_apply.exit_code, TypedExit::Usage.code());

        let malformed_nonempty_digest =
            format!("{}|s:junk", stale_digest.rsplit_once("|s:").unwrap().0);
        let malformed_nonempty_apply = CliApp::run(vec![
            "maintain".to_owned(),
            "apply".to_owned(),
            proot.clone(),
            "stale.txt".to_owned(),
            "stale-replacement\n".to_owned(),
            malformed_nonempty_digest,
            "bran-cli-fixture-v1".to_owned(),
        ]);
        assert!(malformed_nonempty_apply.output.contains("digest_mismatch"));
        assert!(!malformed_nonempty_apply.output.contains("stale_source"));
        assert_eq!(malformed_nonempty_apply.exit_code, TypedExit::Usage.code());

        // bad digest refusal (code 2, no mutation)
        let badd = CliApp::run(vec![
            "maintain".to_owned(),
            "apply".to_owned(),
            proot.clone(),
            tgt.clone(),
            rep.clone(),
            "bad-digest-not-match".to_owned(),
            "auth".to_owned(),
        ]);
        assert!(badd.output.contains("digest_mismatch"));
        assert_eq!(badd.exit_code, TypedExit::Usage.code());
        assert!(badd.is_error);
        assert!(!root.join(&tgt).exists());

        // authorized exact-digest apply writes and returns validation-passed receipt (0)
        let app = CliApp::run(vec![
            "maintain".to_owned(),
            "apply".to_owned(),
            proot.clone(),
            tgt.clone(),
            rep.clone(),
            dig.clone(),
            "bran-cli-fixture-v1".to_owned(),
        ]);
        assert_eq!(app.exit_code, ExitCode::SUCCESS);
        assert!(!app.is_error);
        assert!(app.output.contains("\"command\":\"maintain.apply\""));
        assert!(app.output.contains("\"status\":\"ok\""));
        assert!(app
            .output
            .contains("proposed -> applied -> validation-passed"));
        assert!(app
            .output
            .contains(&format!("\"proposal_digest\":\"{}\"", dig)));
        assert!(root.join(&tgt).exists());
        let ondisk = std::fs::read(root.join(&tgt)).unwrap();
        assert_eq!(ondisk, rep.as_bytes());

        // revalidate succeeds
        let rev = CliApp::run(vec![
            "maintain".to_owned(),
            "revalidate".to_owned(),
            proot.clone(),
        ]);
        assert_eq!(rev.exit_code, ExitCode::SUCCESS);
        assert!(!rev.is_error);
        assert!(rev.output.contains("\"command\":\"maintain.revalidate\""));
        assert!(rev.output.contains("\"status\":\"ok\""));

        // p3_headless_cli_and_maintain_lifecycle extension only (no new test/table): headless identical under t/f;
        // exact tui interactive success; non-tty tui unavailable; usage remains noninteractive.
        let s1 = super::CliApp::run_for_terminal(vec!["smoke".to_owned()], true);
        let s2 = super::CliApp::run_for_terminal(vec!["smoke".to_owned()], false);
        assert_eq!(s1.output, s2.output);
        assert_eq!(s1.exit_code, s2.exit_code);
        assert_eq!(s1.is_error, s2.is_error);
        assert!(!s1.is_interactive && !s2.is_interactive);
        let tu = super::CliApp::run_for_terminal(vec!["tui".to_owned()], true);
        assert!(tu.output.is_empty());
        assert!(tu.is_interactive);
        assert_eq!(tu.exit_code, ExitCode::SUCCESS);
        assert!(!tu.is_error);
        let tui_non_tty = super::CliApp::run_for_terminal(vec!["tui".to_owned()], false);
        assert!(tui_non_tty.output.contains("\"command\":\"tui\""));
        assert!(tui_non_tty.output.contains("\"status\":\"error\""));
        assert!(tui_non_tty.output.contains("tui_unavailable_non_tty"));
        assert_eq!(tui_non_tty.exit_code, TypedExit::Operation.code());
        assert!(tui_non_tty.is_error);
        assert!(!tui_non_tty.is_interactive);
        let tui_extra =
            super::CliApp::run_for_terminal(vec!["tui".to_owned(), "extra".to_owned()], true);
        assert_eq!(tui_extra.exit_code, TypedExit::Usage.code());
        assert!(tui_extra.is_error);
        assert!(!tui_extra.is_interactive);
        assert!(matches!(
            super::map_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('q'),
                crossterm::event::KeyModifiers::NONE
            )),
            Some(bran_tui::TuiEvent::Input('q'))
        ));
        assert!(matches!(
            super::map_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Esc,
                crossterm::event::KeyModifiers::NONE
            )),
            Some(bran_tui::TuiEvent::Escape)
        ));
        let capability_calls = std::cell::Cell::new(0);
        let mut capability_cache = super::CapabilityCache::default();
        let identity = super::CapabilityIdentity {
            sqz_requested: false,
            environment: std::array::from_fn(|_| None),
        };
        let capability = bran_tui::CapabilityProbe::default();
        capability_cache.resolve_with(identity.clone(), || {
            capability_calls.set(capability_calls.get() + 1);
            capability
        });
        capability_cache.resolve_with(identity.clone(), || {
            capability_calls.set(capability_calls.get() + 1);
            capability
        });
        let mut changed_identity = identity;
        changed_identity.sqz_requested = true;
        capability_cache.resolve_with(changed_identity, || {
            capability_calls.set(capability_calls.get() + 1);
            capability
        });
        assert_eq!(capability_calls.get(), 2);
        assert!(matches!(
            super::map_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('q'),
                crossterm::event::KeyModifiers::NONE
            )),
            Some(bran_tui::TuiEvent::Input('q'))
        ));
        let mut tui_app = bran_tui::TuiApp::default();
        tui_app.update_suggestions(super::tui_candidates(&tui_app));
        assert!(!tui_app.suggestions.is_empty());
        tui_app.step = bran_tui::OnboardingStep::Guardrails;
        tui_app.input = "tokens=".to_owned();
        tui_app.update_suggestions(super::tui_candidates(&tui_app));
        assert!(tui_app.suggestions.contains(&"tokens=".to_owned()));
        tui_app.handle(bran_tui::TuiEvent::Resize { columns: 40 });
        assert_eq!(tui_app.columns, Some(40));
        let r1 = super::CliApp::run_for_terminal(
            vec![
                "maintain".to_owned(),
                "revalidate".to_owned(),
                proot.clone(),
            ],
            true,
        );
        let r2 = super::CliApp::run_for_terminal(
            vec![
                "maintain".to_owned(),
                "revalidate".to_owned(),
                proot.clone(),
            ],
            false,
        );
        assert_eq!(r1.output, r2.output);
        assert!(!r1.is_interactive && !r2.is_interactive);

        let skill = include_str!("../../../skill/use-bran/SKILL.md");
        let readme = include_str!("../../../examples/headless/README.md");
        let pins_forms = |artifact: &str| {
            artifact.contains("bran packet <repo-root> \"<request>\"")
                && artifact.contains("bran query <repo-root> \"<request>\"")
                && artifact.contains("bran check <repo-root> <profile>")
                && artifact.contains("bran maintain propose <repo-root> <target> <replacement>")
                && artifact.contains(
                    "bran maintain apply <repo-root> <target> <replacement> <digest> <authority>",
                )
                && artifact.contains("bran maintain revalidate <repo-root>")
        };
        let is_public = |artifact: &str| {
            let lower = artifact.to_ascii_lowercase();
            !lower.contains("openai")
                && !lower.contains("codex")
                && !lower.contains("devpost")
                && !lower.contains("gpt")
                && !lower.contains("grok")
                && !lower.contains("terra")
                && !lower.contains("luna")
                && !lower.contains("sol")
                && !lower.contains("alphazede")
        };
        assert!(pins_forms(readme));
        assert!(is_public(skill) && is_public(readme));
        assert_skill_instructions(skill);
        let help = CliApp::run(["-h"]);
        assert_contains_all(
            &help.output,
            &[
                "--agent <profile>",
                "--reasoning <level>",
                "--tools <read,search>",
                "--provider <provider>",
                "--model <model>",
                "--no-session",
                "--offline",
                "--trust-current-root",
            ],
        );

        // Slice 3.4 p-headless asserts only (compact, inside existing test, zero new test names)
        let ex1 = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "sol".to_owned(),
            "tell me about bran".to_owned(),
        ]);
        assert!(
            ex1.output.contains("\"command\":\"p\"") && ex1.output.contains("\"status\":\"error\"")
        );
        assert!(ex1.output.contains("missing_connected_settings"));
        assert!(!ex1.output.contains("agent_disabled"));
        assert_eq!(ex1.exit_code, TypedExit::Operation.code());
        assert!(ex1.is_error);
        let ex2 = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "luna".to_owned(),
            "--reasoning".to_owned(),
            "medium".to_owned(),
            "--tools".to_owned(),
            "read,search".to_owned(),
            "--no-session".to_owned(),
            "--provider".to_owned(),
            "fixture-provider".to_owned(),
            "--model".to_owned(),
            "fixture-luna".to_owned(),
            "q".to_owned(),
        ]);
        assert!(ex2.output.contains("missing_connected_settings"));
        assert!(!ex2.output.contains("agent_disabled"));
        // exact reasoning rejection
        let bad_r = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "sol".to_owned(),
            "--reasoning".to_owned(),
            "Medium".to_owned(),
            "hi".to_owned(),
        ]);
        assert!(bad_r.output.contains("invalid_reasoning"));
        assert_eq!(bad_r.exit_code, TypedExit::Usage.code());
        let max_reasoning = super::do_headless_p_with(
            vec![
                "--agent".to_owned(),
                "luna".to_owned(),
                "--reasoning".to_owned(),
                "max".to_owned(),
                "--response-sources".to_owned(),
                "3".to_owned(),
                "seed".to_owned(),
            ],
            |request, _, _, _, controls| {
                assert_eq!(
                    request.reasoning_override(),
                    Some(bran_core::agent::ReasoningLevel::Max)
                );
                assert_eq!(controls.response_sources, Some(3));
                let connected_grounded =
                    super::grounded_request_with(&root, request, controls).unwrap();
                let task_marker = "task:\nseed\n\nrepository-evidence-packet:\n";
                let task_offset = connected_grounded.prompt().find(task_marker).unwrap();
                let preamble = &connected_grounded.prompt()[..task_offset];
                assert!(preamble.contains("at most 3 recommended repository-relative paths"));
                assert_eq!(
                    preamble
                        .lines()
                        .filter(|line| {
                            let bytes = line.as_bytes();
                            bytes.len() >= 3
                                && bytes[0].is_ascii_digit()
                                && bytes[1] == b'.'
                                && bytes[2] == b' '
                        })
                        .count(),
                    6
                );
                assert!(preamble.contains("Treat every factual statement as material"));
                Ok(bran_core::agent::synthetic::connected_receipt_for(
                    request, false,
                ))
            },
        );
        assert_eq!(max_reasoning.exit_code, TypedExit::Success.code());
        assert!(max_reasoning.output.contains("\"value\":\"max\""));
        let response_source_overflow = super::do_headless_p_with(
            vec![
                "--agent".to_owned(),
                "luna".to_owned(),
                "--response-sources".to_owned(),
                "1".to_owned(),
                "seed".to_owned(),
            ],
            |request, _, _, _, _| {
                let receipt = bran_core::agent::synthetic::connected_receipt_for(request, false);
                let inline = bran_core::agent::receipt::InlineResult::new(
                    "synthetic connected answer",
                    ["seed.md", "dep.md"],
                )
                .unwrap();
                let stored_ref = bran_core::agent::receipt::StoredResultRef::new(
                    bran_core::agent::result_store::ResultId::sha256(&inline.encode_canonical()),
                    [],
                )
                .unwrap();
                Ok(bran_core::agent::receipt::DelegationReceipt::new(
                    bran_core::agent::receipt::DelegationReceiptParts {
                        outcome: receipt.outcome().clone(),
                        requested: receipt.requested().clone(),
                        effective: receipt.effective().clone(),
                        sqz: receipt.sqz_stages().clone(),
                        inline_result: Some(inline),
                        stored_ref: Some(stored_ref),
                        no_session: receipt.no_session(),
                        provenance: receipt.provenance().to_vec(),
                    },
                )
                .unwrap())
            },
        );
        assert_eq!(
            response_source_overflow.exit_code,
            TypedExit::Operation.code()
        );
        assert!(response_source_overflow
            .output
            .contains("response_source_limit_exceeded"));
        assert!(response_source_overflow
            .output
            .contains("\"status\":\"error\""));
        let invalid_p_control = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "sol".to_owned(),
            "--response-sources".to_owned(),
            "0".to_owned(),
            "q".to_owned(),
        ]);
        assert_eq!(invalid_p_control.exit_code, TypedExit::Usage.code());
        assert!(invalid_p_control.output.contains("invalid_control"));
        // denied tool rejection
        let bad_t = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "sol".to_owned(),
            "--tools".to_owned(),
            "read,write".to_owned(),
            "hi".to_owned(),
        ]);
        assert!(bad_t.output.contains("denied_tool"));
        assert_eq!(bad_t.exit_code, TypedExit::Usage.code());
        let empty_t = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "sol".to_owned(),
            "--tools".to_owned(),
            "read,,search".to_owned(),
            "hi".to_owned(),
        ]);
        assert!(empty_t.output.contains("empty_tools"));
        assert_eq!(empty_t.exit_code, TypedExit::Usage.code());
        let bad_profile = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "nova".to_owned(),
            "hi".to_owned(),
        ]);
        assert!(bad_profile.output.contains("unknown_profile"));
        let bad_provider = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "sol".to_owned(),
            "--provider".to_owned(),
            "other-provider".to_owned(),
            "hi".to_owned(),
        ]);
        assert!(bad_provider.output.contains("unknown_provider"));
        let bad_model = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "sol".to_owned(),
            "--model".to_owned(),
            "other-model".to_owned(),
            "hi".to_owned(),
        ]);
        assert!(bad_model.output.contains("unknown_model"));
        // missing agent/prompt
        let miss_a = CliApp::run(vec!["-p".to_owned(), "justprompt".to_owned()]);
        assert!(miss_a.output.contains("missing_agent"));
        let miss_p = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "sol".to_owned(),
        ]);
        assert!(miss_p.output.contains("missing_prompt"));
        // unknown/duplicate option
        let unk = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "sol".to_owned(),
            "--xyz".to_owned(),
            "hi".to_owned(),
        ]);
        assert!(unk.output.contains("unknown_option"));
        let dup = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "sol".to_owned(),
            "--agent".to_owned(),
            "luna".to_owned(),
            "hi".to_owned(),
        ]);
        assert!(dup.output.contains("duplicate_option"));
        // no-session receipt + missing value
        let ns = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "sol".to_owned(),
            "--no-session".to_owned(),
            "q".to_owned(),
        ]);
        assert!(ns.output.contains("missing_connected_settings"));
        let mv = CliApp::run(vec!["-p".to_owned(), "--agent".to_owned()]);
        assert!(mv.output.contains("missing_value"));

        let external_initializations = std::cell::Cell::new(0);
        let mut core_sqz_settings = bran_tui::quick_safe_config();
        core_sqz_settings.profile = bran_tui::OperatingProfile::CoreSqz;
        let core_sqz_gate = super::initialize_connected(Some(&core_sqz_settings), |_| {
            external_initializations.set(external_initializations.get() + 1);
            Some(())
        });
        assert!(core_sqz_gate.is_none());
        assert_eq!(external_initializations.get(), 0);
        let mut connected_settings = bran_tui::quick_safe_config();
        connected_settings.profile = bran_tui::OperatingProfile::ConnectedAgent;
        connected_settings.sqz = true;
        let connected_gate = super::initialize_connected(Some(&connected_settings), |_| {
            external_initializations.set(external_initializations.get() + 1);
            Some(())
        });
        assert!(connected_gate.is_some());
        assert_eq!(external_initializations.get(), 1);
        let (sqz_off_port, sqz_off_policy) = super::configured_sqz(false).unwrap();
        assert!(matches!(sqz_off_port, super::ConnectedSqzPort::Off));
        assert_eq!(sqz_off_policy, bran_core::adapters::SqzPolicy::PublicOff);

        let invariant_request = bran_core::agent::delegate::DelegationRequest::new(
            "sol",
            "typed runtime invariant",
            bran_core::agent::delegate::DelegationOptions::new(),
        )
        .unwrap();
        let invariant_receipt = super::runtime_receipt_or_incomplete(
            &invariant_request,
            Err(bran_core::agent::coordinator::AgentRuntimeInternalError::ReceiptInvariant),
        );
        assert!(matches!(
            invariant_receipt.outcome(),
            bran_core::agent::runtime::InvocationOutcome::Incomplete { .. }
        ));
        assert!(invariant_receipt.inline_result().is_none());
        assert!(invariant_receipt.stored_result_ref().is_none());

        let grounding_request = bran_core::agent::delegate::DelegationRequest::new(
            "sol",
            "tell me about seed",
            bran_core::agent::delegate::DelegationOptions::new(),
        )
        .unwrap();
        let grounded = super::grounded_request(&root, &grounding_request).unwrap();
        let task_marker = "task:\ntell me about seed\n\nrepository-evidence-packet:\n";
        let task_offset = grounded.prompt().find(task_marker).unwrap();
        let preamble = &grounded.prompt()[..task_offset];
        assert_eq!(preamble, super::CONNECTED_AGENT_PREAMBLE);
        assert_eq!(
            preamble
                .lines()
                .filter(|line| {
                    let bytes = line.as_bytes();
                    bytes.len() >= 3
                        && bytes[0].is_ascii_digit()
                        && bytes[1] == b'.'
                        && bytes[2] == b' '
                })
                .count(),
            6
        );
        assert_numbered_rules(preamble);
        assert!(grounded
            .prompt()
            .starts_with(super::CONNECTED_AGENT_PREAMBLE));
        assert!(grounded.prompt().contains("task:\ntell me about seed"));
        assert!(grounded.prompt().contains("repository-evidence-packet:"));
        assert!(grounded.prompt().contains("seed.md") || grounded.prompt().contains("p3.md"));
        assert!(grounded.prompt().contains("rank: 1"));
        assert!(!grounded.prompt().contains("seed-full-body-sentinel"));
        assert!(grounded.prompt().len() <= 65_536);
        let response_limited_grounding = super::grounded_request_with(
            &root,
            &grounding_request,
            &super::ExperimentalControls {
                max_sources: None,
                effective_max_sources: None,
                dependency_depth: None,
                response_sources: Some(1),
                excerpt_bytes: None,
            },
        )
        .unwrap();
        let response_limited_contract = response_limited_grounding.grounding_contract().unwrap();
        assert!(response_limited_contract.admits_citation("seed.md"));
        assert!(response_limited_contract.admits_citation("dep.md"));
        let response_task_offset = response_limited_grounding
            .prompt()
            .find(task_marker)
            .unwrap();
        let response_preamble = &response_limited_grounding.prompt()[..response_task_offset];
        assert!(response_preamble.contains("at most 1 recommended repository-relative paths"));
        assert!(response_preamble.contains("one short relevance reason per path"));
        assert!(response_preamble.contains("confidence and missing evidence"));
        assert!(response_preamble.contains("no implementation analysis"));
        assert_eq!(
            response_preamble
                .lines()
                .filter(|line| {
                    let bytes = line.as_bytes();
                    bytes.len() >= 3
                        && bytes[0].is_ascii_digit()
                        && bytes[1] == b'.'
                        && bytes[2] == b' '
                })
                .count(),
            6
        );
        assert!(response_limited_grounding
            .prompt()
            .contains("task:\ntell me about seed\n\nrepository-evidence-packet:\n"));
        let grounding_contract = grounded.grounding_contract().unwrap();
        assert!(grounding_contract.admits_citation("seed.md"));
        assert!(grounding_contract.admits_citation("dep.md"));
        assert!(!grounding_contract.admits_citation("not-in-packet.md"));
        assert!(grounding_contract.preservation_anchors().contains(
            &bran_core::packet::PreservationAnchor::new("task", grounding_request.prompt())
                .unwrap()
        ));
        let oversized_root = root.join("oversized-grounding");
        std::fs::create_dir(&oversized_root).unwrap();
        std::fs::write(
            oversized_root.join("only.md"),
            format!(
                "---\ntype: concept\ntitle: Oversized\n---\nneedle {}",
                "x".repeat(70_000)
            ),
        )
        .unwrap();
        let oversized_request = bran_core::agent::delegate::DelegationRequest::new(
            "sol",
            "needle",
            bran_core::agent::delegate::DelegationOptions::new(),
        )
        .unwrap();
        // A term that exists only in the document body is now groundable
        // (issue #19), and the 70 KiB body must not bloat the packet: the
        // evidence is the metadata descriptor, not the raw body.
        let grounded_oversized = super::grounded_request(&oversized_root, &oversized_request)
            .expect("body-only content must be groundable");
        assert!(grounded_oversized
            .grounding_contract()
            .unwrap()
            .admits_citation("only.md"));
        assert!(grounded_oversized.prompt().len() < 2_000);
        let configured_descriptor = super::ConfiguredAgentDescriptor {
            profile: bran_core::agent::AgentProfile::new(
                "local-agent",
                "local-provider",
                "local-model",
                "local-account",
                bran_core::agent::ReasoningLevel::Medium,
                bran_core::agent::ToolPolicy::read_only_default(),
            )
            .unwrap(),
        };
        let configured_registry = configured_descriptor.registry().unwrap();
        assert_eq!(
            configured_registry.get("local-agent").unwrap().model(),
            "local-model"
        );
        let long_task = format!(
            "{}literal\n\nrepository-evidence-packet:\ninside task exact tail",
            "seed ".repeat(120)
        );
        assert!(long_task.len() > 512);
        let long_request = bran_core::agent::delegate::DelegationRequest::new(
            "local-agent",
            &long_task,
            bran_core::agent::delegate::DelegationOptions::new(),
        )
        .unwrap();
        let long_grounded = super::grounded_request(&root, &long_request).unwrap();
        let long_anchors = long_grounded
            .grounding_contract()
            .unwrap()
            .preservation_anchors();
        // Body-content ranking (#19) admits repl.txt to the packet: its body
        // "p3-replacement-bytes-exact" matches the task term "exact", so the
        // grounding contract now carries three sources plus three digests.
        assert_eq!(long_anchors.len(), 8);
        assert_eq!(long_anchors[6].id(), "task-000");
        assert_eq!(long_anchors[6].value(), &long_task[..512]);
        assert_eq!(long_anchors[7].id(), "task-001");
        assert_eq!(long_anchors[7].value(), &long_task[512..]);
        let mut tui_options = bran_core::agent::delegate::DelegationOptions::new();
        tui_options.model_override = Some("alternate-model".to_owned());
        tui_options.reasoning_override = Some(bran_core::agent::ReasoningLevel::Max);
        let tui_request = bran_core::agent::delegate::DelegationRequest::new(
            "alternate-agent",
            "tell me about seed",
            tui_options,
        )
        .unwrap();
        let tui_registry = configured_descriptor
            .registry_for_tui_request(&tui_request)
            .unwrap();
        let tui_profile = tui_registry.get("alternate-agent").unwrap();
        assert_eq!(tui_profile.provider(), "local-provider");
        assert_eq!(tui_profile.model(), "local-model");
        assert_eq!(tui_profile.account_handle(), "local-account");
        assert!(tui_registry
            .model_registry()
            .contains_for_provider("local-provider", "alternate-model"));
        let grounded_tui_request = super::grounded_request(&root, &tui_request).unwrap();
        let seen = Arc::new(std::sync::Mutex::new(None));
        let seen_prompt = Arc::new(std::sync::Mutex::new(None));
        let recorder = RequestRecorder {
            seen: Arc::clone(&seen),
            seen_prompt: Arc::clone(&seen_prompt),
        };
        let account = super::ConfiguredAccount {
            account_ref: "local-account".to_owned(),
            credential: bran_core::adapters::SafeAccountReference::new("local-account").unwrap(),
        };
        let dropping_sqz = AgentSqzAdapter::new(
            super::ConnectedSqzPort::FixtureDropsTask,
            SqzPolicy::PublicOn,
            long_grounded.max_output_bytes(),
        );
        let mut store = MemoryResultStore::new(1, 1, 1, 1).unwrap();
        let runtime = AgentRuntime::new(AgentRuntimeConfig::new(true, 8).unwrap());
        let mismatched_sqz = AgentSqzAdapter::new(
            super::ConnectedSqzPort::Fixture.with_grounded_task("different task", None),
            SqzPolicy::PublicOn,
            long_grounded.max_output_bytes(),
        );
        let mismatched = runtime
            .invoke(
                &long_grounded,
                AgentRuntimeAuthority::new(false, true, false),
                &configured_registry,
                || RuntimePorts::new(&account, &recorder, &mismatched_sqz, &mut store),
                0,
            )
            .unwrap();
        assert!(matches!(
            mismatched.outcome(),
            InvocationOutcome::Incomplete {
                failure: AgentFailure::SqzInputFailed,
                ..
            }
        ));
        assert!(seen.lock().unwrap().is_none());
        let dropped = runtime
            .invoke(
                &long_grounded,
                AgentRuntimeAuthority::new(false, true, false),
                &configured_registry,
                || RuntimePorts::new(&account, &recorder, &dropping_sqz, &mut store),
                0,
            )
            .unwrap();
        assert!(
            matches!(
                dropped.outcome(),
                InvocationOutcome::Incomplete {
                    failure: AgentFailure::GroundingFailed,
                    ..
                }
            ),
            "unexpected dropped-task outcome: {:?}",
            dropped.outcome()
        );
        let dropped_input = dropped.sqz_stages().input().unwrap();
        assert!(dropped_input
            .required_fidelity_anchor_ids
            .contains(&"task-000".to_owned()));
        assert!(dropped_input
            .missing_fidelity_anchor_ids
            .contains(&"task-000".to_owned()));
        assert_eq!(super::next_sqz_fidelity_source_limit(&dropped, 32), None);
        let mut source_only_input = dropped_input.clone();
        source_only_input.missing_fidelity_anchor_ids = vec!["source-7".to_owned()];
        let source_only_receipt = bran_core::agent::receipt::DelegationReceipt::new(
            bran_core::agent::receipt::DelegationReceiptParts {
                outcome: dropped.outcome().clone(),
                requested: dropped.requested().clone(),
                effective: dropped.effective().clone(),
                sqz: bran_core::agent::receipt::SqzStages::new(Some(source_only_input), None),
                inline_result: None,
                stored_ref: None,
                no_session: dropped.no_session(),
                provenance: dropped.provenance().to_vec(),
            },
        )
        .unwrap();
        assert_eq!(
            super::next_sqz_fidelity_source_limit(&source_only_receipt, 32),
            Some(16)
        );
        assert_eq!(
            super::next_sqz_fidelity_source_limit(&source_only_receipt, 2),
            Some(1)
        );
        assert_eq!(
            super::next_sqz_fidelity_source_limit(&source_only_receipt, 1),
            None
        );
        let backoff_root = root.join("sqz-backoff");
        std::fs::create_dir(&backoff_root).unwrap();
        for index in 0..10 {
            std::fs::write(
                backoff_root.join(format!("source-{index}.md")),
                format!(
                    "---\ntype: concept\ntitle: Shared topic {index}\nokf_status: active\ntags: shared-topic\npublic_boundary: safe\n---\nshared-topic evidence {index}\n"
                ),
            )
            .unwrap();
        }
        let backoff_request = bran_core::agent::delegate::DelegationRequest::new(
            "sol",
            "shared-topic",
            bran_core::agent::delegate::DelegationOptions::new(),
        )
        .unwrap();
        let default_controls = super::ExperimentalControls::default();
        let initial_backoff_request =
            super::grounded_request_with(&backoff_root, &backoff_request, &default_controls)
                .unwrap();
        let sixteen_controls = super::ExperimentalControls {
            effective_max_sources: Some(16),
            ..Default::default()
        };
        let sixteen_source_request =
            super::grounded_request_with(&backoff_root, &backoff_request, &sixteen_controls)
                .unwrap();
        assert_eq!(
            initial_backoff_request.prompt(),
            sixteen_source_request.prompt()
        );
        let (backed_off_controls, backed_off_request) = super::next_changed_grounded_request(
            &backoff_root,
            &backoff_request,
            &default_controls,
            &source_only_receipt,
            &initial_backoff_request,
        )
        .unwrap()
        .unwrap();
        assert_eq!(backed_off_controls.effective_max_sources, Some(8));
        assert_ne!(
            backed_off_request.prompt(),
            initial_backoff_request.prompt()
        );
        let adaptive_controls = super::ExperimentalControls {
            effective_max_sources: Some(4),
            ..Default::default()
        };
        assert!(adaptive_controls
            .controls_json()
            .contains("\"requested\":{\"max_sources\":null"));
        assert!(adaptive_controls
            .controls_json()
            .contains("\"effective\":{\"max_sources\":4"));
        assert!(seen.lock().unwrap().is_none());
        let preserving_sqz = AgentSqzAdapter::new(
            super::ConnectedSqzPort::Fixture.with_grounded_task(&long_task, None),
            SqzPolicy::PublicOn,
            long_grounded.max_output_bytes(),
        );
        let _ = runtime.invoke(
            &long_grounded,
            AgentRuntimeAuthority::new(false, true, false),
            &configured_registry,
            || RuntimePorts::new(&account, &recorder, &preserving_sqz, &mut store),
            0,
        );
        let preserved_prompt = seen_prompt.lock().unwrap().take().unwrap();
        seen.lock().unwrap().take();
        let grounded_prefix = super::grounded_prompt_prefix(&long_task, None);
        assert!(preserved_prompt.starts_with(&grounded_prefix));
        assert_ne!(preserved_prompt, long_grounded.prompt());
        let original_evidence = long_grounded
            .prompt()
            .strip_prefix(&grounded_prefix)
            .unwrap();
        assert_eq!(
            preserved_prompt.strip_prefix(&grounded_prefix).unwrap(),
            format!(
                "sqz-applied-v1\n{}",
                original_evidence
                    .lines()
                    .filter(|line| {
                        line.starts_with("locator=") || line.starts_with("content-digest-sha256:")
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        );
        assert_eq!(
            preserved_prompt
                .matches("repository-evidence-packet:")
                .count(),
            2
        );
        let sqz = AgentSqzAdapter::new(
            super::ConnectedSqzPort::Off,
            SqzPolicy::PublicOff,
            grounded_tui_request.max_output_bytes(),
        );
        let _ = runtime.invoke(
            &grounded_tui_request,
            AgentRuntimeAuthority::new(false, true, false),
            &tui_registry,
            || RuntimePorts::new(&account, &recorder, &sqz, &mut store),
            0,
        );
        assert_eq!(
            *seen.lock().unwrap(),
            Some((
                "local-provider".to_owned(),
                "alternate-model".to_owned(),
                bran_core::agent::ReasoningLevel::Max,
            ))
        );
        assert_eq!(
            *seen_prompt.lock().unwrap(),
            Some(grounded_tui_request.prompt().to_owned())
        );
        assert!(!grounded_tui_request.prompt().contains("sqz-applied-v1\n"));
        let opaque = super::opaque_account_handle("local-account-reference");
        assert!(opaque.starts_with("account-") && !opaque.contains("local-account-reference"));
        assert_eq!(
            opaque,
            super::opaque_account_handle("local-account-reference")
        );
        assert!(!bran_core::adapters::is_public_dlp_safe(
            "api_key=public-boundary-fixture"
        ));
        assert_eq!(
            super::ConnectedSetupFailure::MissingProviderDescriptor.as_str(),
            "missing_provider_descriptor"
        );
        assert_eq!(
            super::ConnectedSetupFailure::MissingAuthReference.as_str(),
            "missing_auth_reference"
        );
        assert_eq!(
            super::ConnectedSetupFailure::MissingSqz.as_str(),
            "missing_sqz_configuration"
        );
        assert_eq!(
            super::ConnectedSetupFailure::ProjectTrustRequired.as_str(),
            "project_trust_required"
        );

        let conn = super::do_headless_p_with(
            vec![
                "--agent".to_owned(),
                "sol".to_owned(),
                "--trust-current-root".to_owned(),
                "sol-connected-exact".to_owned(),
            ],
            |request, _, _, trusted, _| {
                assert!(trusted);
                Ok(bran_core::agent::synthetic::connected_receipt_for(
                    request, true,
                ))
            },
        );
        assert!(
            conn.output.contains("\"command\":\"p\"") && conn.output.contains("\"status\":\"ok\"")
        );
        assert_eq!(conn.exit_code, TypedExit::Success.code());
        assert!(!conn.is_error);
        assert!(conn.output.contains("\"state\":\"complete\""));
        assert!(conn.output.contains("\"profile_name\":\"sol\""));
        assert!(conn.output.contains("\"value\":\"fixture-sol\""));
        assert!(!conn.output.contains("sol-connected-exact")); // canonical omits original prompt

        let stored_receipt =
            bran_core::agent::synthetic::connected_receipt_for(&invariant_request, true);
        let stored_payload = stored_receipt.inline_result().unwrap().encode_canonical();
        let stored_id = stored_receipt
            .stored_result_ref()
            .unwrap()
            .result_id()
            .clone();
        assert_eq!(
            stored_id,
            bran_core::agent::result_store::ResultId::sha256(&stored_payload)
        );
        let mut durable = super::FilesystemResultStore::open(&root).unwrap();
        let now = super::wall_tick();
        let artifact = b"exact artifact bytes";
        let artifact_id = bran_core::agent::result_store::ResultId::sha256(artifact);
        let persisted = bran_core::agent::result_store::ResultStore::put_batch(
            &mut durable,
            &[stored_payload.as_slice(), artifact.as_slice()],
            now,
        )
        .unwrap();
        assert_eq!(
            persisted.as_slice(),
            &[stored_id.clone(), artifact_id.clone()]
        );
        assert_eq!(
            bran_core::agent::result_store::ResultStore::get(&mut durable, &artifact_id, now)
                .unwrap(),
            artifact
        );
        let publication_root = root.join("failed-publication");
        std::fs::create_dir(&publication_root).unwrap();
        assert!(super::failed_publication_preserves_prior(
            &publication_root,
            now
        ));
        let recovery_root = root.join("concurrent-recovery");
        std::fs::create_dir(&recovery_root).unwrap();
        assert!(super::concurrent_recovery_preserves_live_transaction(
            &recovery_root,
            now
        ));
        let cancellation_root = root.join("connected-cancellation");
        std::fs::create_dir(&cancellation_root).unwrap();
        assert!(super::connected_cancellation_contract(&cancellation_root));
        let fetched = super::do_get(&root, &stored_id.to_string());
        assert_eq!(fetched.exit_code, ExitCode::SUCCESS);
        assert!(fetched.output.contains("synthetic connected answer"));
        let traversal = super::do_get(&root, "sha256:../../settings.conf");
        assert!(traversal.output.contains("invalid_result_id"));
        let expired = bran_core::agent::receipt::InlineResult::new("expired", ["seed.md"])
            .unwrap()
            .encode_canonical();
        let expired_id = bran_core::agent::result_store::ResultStore::put_batch(
            &mut durable,
            &[expired.as_slice()],
            1,
        )
        .unwrap()
        .remove(0);
        assert_eq!(
            bran_core::agent::result_store::ResultStore::get(
                &mut durable,
                &expired_id,
                1 + super::RESULT_STORE_LIMITS.max_age_ticks,
            ),
            Err(bran_core::agent::result_store::ResultStoreError::NotFound)
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let batch = durable.batches(now).unwrap().pop().unwrap().1;
            let linked_id = bran_core::agent::result_store::ResultId::sha256(b"linked");
            let linked_path = batch.join(linked_id.value());
            symlink(root.join("seed.md"), &linked_path).unwrap();
            assert_eq!(
                bran_core::agent::result_store::ResultStore::get(&mut durable, &linked_id, now,),
                Err(bran_core::agent::result_store::ResultStoreError::Corrupt)
            );
            std::fs::remove_file(linked_path).unwrap();
        }

        let unatt = super::do_headless_p_with(
            vec![
                "--agent".to_owned(),
                "luna".to_owned(),
                "--provider".to_owned(),
                "fixture-provider".to_owned(),
                "--model".to_owned(),
                "fixture-luna".to_owned(),
                "--reasoning".to_owned(),
                "medium".to_owned(),
                "luna-unatt".to_owned(),
            ],
            |request, _, _, _, _| {
                Ok(bran_core::agent::synthetic::connected_receipt_for(
                    request, false,
                ))
            },
        );
        assert!(unatt.output.contains("\"status\":\"ok\""));
        assert_eq!(unatt.exit_code, TypedExit::Success.code());
        assert!(unatt.output.contains("\"profile_name\":\"luna\""));
        assert!(unatt.output.contains("\"value\":\"fixture-luna\""));
        assert!(unatt.output.contains("\"value\":\"medium\""));
        assert!(unatt.output.contains("unavailable"));
        assert!(!unatt.output.contains("luna-unatt"));

        let off = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "sol".to_owned(),
            "--offline".to_owned(),
            "off-prompt".to_owned(),
        ]);
        assert!(off.output.contains("explicit_offline"));
        assert_eq!(off.exit_code, TypedExit::Operation.code());
        assert!(off.is_error);
        assert!(!off.output.contains("off-prompt"));

        // absence of secret material (and no credential reflection)
        let sec = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "sol".to_owned(),
            "secret api-key credential".to_owned(),
        ]);
        let slow = sec.output.to_ascii_lowercase();
        assert!(
            !slow.contains("key")
                && !slow.contains("credential")
                && !slow.contains("secret")
                && !slow.contains("api-key")
        );
        let forbidden = CliApp::run(vec![
            "-p".to_owned(),
            "--agent".to_owned(),
            "sol".to_owned(),
            "--credentials".to_owned(),
            "hi".to_owned(),
        ]);
        assert!(forbidden.output.contains("forbidden_credential_flag"));
        std::fs::remove_file(settings_path).unwrap();
        std::fs::remove_file(skill_path).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    // --- Slice 2.1-B: policy CLI safety and proof tests ---

    fn scratch_check_root(prefix: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let mut dir = std::env::temp_dir();
        dir.push(format!("bran-cli-check-{}-{}", prefix, std::process::id()));
        let bran_dir = dir.join(".bran");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&bran_dir).expect("create scratch dir");
        (dir, bran_dir)
    }

    fn minimal_valid_policy() -> String {
        "schema_version: \"1\"\n".to_owned()
    }

    fn write_valid_doc(dir: &std::path::Path, name: &str) {
        std::fs::write(
            dir.join(name),
            format!(
                "---\ntype: concept\ntitle: {name}\nokf_status: active\ntags: cli-check\ntimestamp: 2026-07-20T00:00:00Z\nresource: test://{name}\npublic_boundary: safe\n---\nbody\n"
            ),
        )
        .unwrap();
    }

    // ── check_default_repository_file ──

    #[test]
    fn check_default_repository_file_and_injected_stdin_match() {
        let (root, bran_dir) = scratch_check_root("repo-stdin-match");
        write_valid_doc(&root, "doc.md");
        let policy_raw = minimal_valid_policy();
        std::fs::write(bran_dir.join("policy.yaml"), &policy_raw).unwrap();

        let repo_result = CliApp::run(vec![
            "check".to_owned(),
            root.to_string_lossy().into_owned(),
            "bran-strict".to_owned(),
        ]);
        let stdin_result = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "bran-strict".to_owned(),
            ],
            policy_raw.as_bytes(),
        );

        assert_eq!(repo_result.exit_code, stdin_result.exit_code);
        assert!(!repo_result.is_error);
        assert!(!stdin_result.is_error);
        // Source-identity assertions
        assert!(repo_result
            .output
            .contains("\"policy_source\":\"repository-file\""));
        assert!(stdin_result.output.contains("\"policy_source\":\"stdin\""));
        // Byte-identical after normalizing policy_source
        let repo_normalized = repo_result.output.replace(
            "\"policy_source\":\"repository-file\"",
            "\"policy_source\":\"__NORM__\"",
        );
        let stdin_normalized = stdin_result.output.replace(
            "\"policy_source\":\"stdin\"",
            "\"policy_source\":\"__NORM__\"",
        );
        assert_eq!(
            repo_normalized, stdin_normalized,
            "complete JSON differs beyond policy_source"
        );
        // stdin does not create .bran/policy.yaml and repo is unchanged
        assert!(
            !bran_dir.join("policy.yaml").exists()
                || std::fs::read_to_string(bran_dir.join("policy.yaml")).unwrap() == policy_raw
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn check_ignores_oversized_non_candidate_before_profile_evaluation() {
        let (root, _) = scratch_check_root("oversized-non-candidate");
        write_valid_doc(&root, "doc.md");
        std::fs::write(root.join("review.html"), vec![b'x'; 256 * 1024 + 1]).unwrap();

        let result = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "bran-strict".to_owned(),
            ],
            minimal_valid_policy().as_bytes(),
        );

        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(!result.output.contains("scan_error"));
        assert!(result
            .output
            .contains("\"selected_profile\":\"bran-strict\""));
        assert!(result
            .output
            .contains("\"okf_compatibility\":{\"profile\":"));
        assert!(result.output.contains("\"bran_strict\":{\"profile\":"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn check_okf_v0_2_profile_end_to_end() {
        let (root, _) = scratch_check_root("okf-v0-2-profile");
        // Nested v0.2 families: the scanner's flat parser cannot represent
        // them and warns, which exercises the structural re-parse in
        // derive_bundle_from_snapshot.
        std::fs::write(
            root.join("doc.md"),
            "---\ntype: Concept\ngenerated:\n  by: agent/1\n  at: 2026-07-01T00:00:00Z\nverified:\n  - by: human:alice\n    at: 2026-07-02T00:00:00Z\n---\nbody\n",
        )
        .unwrap();

        let result = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "okf-v0.2".to_owned(),
            ],
            minimal_valid_policy().as_bytes(),
        );

        assert!(!result.is_error);
        assert_eq!(result.exit_code, TypedExit::Success.code());
        assert!(result.output.contains("\"selected_profile\":\"okf-v0.2\""));
        assert!(result.output.contains(
            "\"okf_v0_2\":{\"profile\":\"okf-v0.2\",\"status\":\"pass\",\"diagnostics\":[]}"
        ));
        assert!(result
            .output
            .contains("\"okf_compatibility\":{\"profile\":\"okf-v0.1\""));
        assert!(result
            .output
            .contains("\"bran_strict\":{\"profile\":\"bran-strict\""));
        assert!(result.output.contains("\"selected_passed\":true"));
        assert!(result.output.contains("\"exit_code\":0"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn check_stdin_no_side_effects() {
        let (root, bran_dir) = scratch_check_root("stdin-no-side");
        write_valid_doc(&root, "doc.md");
        let policy_raw = minimal_valid_policy();
        // Do NOT write policy.yaml to disk — stdin only
        let before = snapshot_file_list(&root);

        let result = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "bran-strict".to_owned(),
            ],
            policy_raw.as_bytes(),
        );

        // Still works (test that we didn't crash)
        assert!(result.output.contains("\"command\":\"check\""));
        // No .bran/policy.yaml was created
        assert!(!bran_dir.join("policy.yaml").exists());
        // File list unchanged
        let after = snapshot_file_list(&root);
        assert_eq!(before, after);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn snapshot_file_list(dir: &std::path::Path) -> Vec<String> {
        let mut files = Vec::new();
        fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let rel = entry
                    .path()
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if entry.file_type().unwrap().is_dir() {
                    out.push(format!("{rel}/"));
                    walk(&entry.path(), out);
                } else {
                    out.push(rel);
                }
            }
        }
        walk(dir, &mut files);
        files.sort();
        files
    }

    // ── check_negative_exit_codes ──

    fn assert_check_usage(args: Vec<String>) {
        let result = CliApp::run(args);
        assert_eq!(
            result.exit_code,
            TypedExit::Usage.code(),
            "expected usage exit for: {}",
            result.output
        );
    }

    #[test]
    fn check_negative_usage_exit_codes() {
        // missing default policy (no .bran/policy.yaml)
        let (root, _) = scratch_check_root("neg-usage");
        write_valid_doc(&root, "doc.md");
        assert_check_usage(vec![
            "check".to_owned(),
            root.to_string_lossy().into_owned(),
            "bran-strict".to_owned(),
        ]);

        // empty stdin
        let result_empty = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "bran-strict".to_owned(),
            ],
            b"",
        );
        assert_eq!(result_empty.exit_code, TypedExit::Usage.code());
        assert!(result_empty
            .output
            .contains("policy_missing_schema_version"));

        // limit+1 oversized stdin
        let oversized = vec![b'x'; MAX_POLICY_BYTES + 1];
        let result_oversized = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "bran-strict".to_owned(),
            ],
            &oversized,
        );
        assert_eq!(result_oversized.exit_code, TypedExit::Usage.code());
        assert!(result_oversized.output.contains("policy_oversized"));

        // invalid UTF-8
        let result_utf8 = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "bran-strict".to_owned(),
            ],
            &[0xff, 0xfe, 0xfd],
        );
        assert_eq!(result_utf8.exit_code, TypedExit::Usage.code());
        assert!(result_utf8.output.contains("policy_utf8"));

        // malformed YAML
        let result_malformed = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "bran-strict".to_owned(),
            ],
            b"schema_version: [}\n",
        );
        assert_eq!(result_malformed.exit_code, TypedExit::Usage.code());
        assert!(result_malformed.output.contains("policy_malformed_yaml"));

        // Valid YAML with the wrong schema shape names the field and expected shape.
        let result_shape = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "bran-strict".to_owned(),
            ],
            b"schema_version: \"1\"\ndocument_coverage:\n  native_bundle: scalar\n",
        );
        assert_eq!(result_shape.exit_code, TypedExit::Usage.code());
        assert!(result_shape.output.contains("policy_schema_invalid"));
        assert!(result_shape
            .output
            .contains("document_coverage.native_bundle"));

        // unknown field
        let result_uk = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "bran-strict".to_owned(),
            ],
            b"schema_version: \"1\"\nnot_a_field: x\n",
        );
        assert_eq!(result_uk.exit_code, TypedExit::Usage.code());
        assert!(result_uk.output.contains("policy_unknown_field"));

        // unsafe path (embedded in error via load, not via stdin)
        std::fs::write(root.join(".bran/policy.yaml"), "schema_version: \"1\"\n").unwrap();
        let result_ok = CliApp::run(vec![
            "check".to_owned(),
            root.to_string_lossy().into_owned(),
            "bran-strict".to_owned(),
        ]);
        // This should succeed since the policy file is valid
        assert!(!result_ok.is_error || result_ok.exit_code == TypedExit::Validation.code());

        // unsupported version
        let result_ver = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "bran-strict".to_owned(),
            ],
            b"schema_version: \"99\"\n",
        );
        assert_eq!(result_ver.exit_code, TypedExit::Usage.code());
        assert!(result_ver.output.contains("policy_unsupported_version"));

        // duplicate key
        let result_dup = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "bran-strict".to_owned(),
            ],
            b"schema_version: \"1\"\nschema_version: \"1\"\n",
        );
        assert_eq!(result_dup.exit_code, TypedExit::Usage.code());
        assert!(result_dup.output.contains("policy_duplicate_key"));

        // extra args
        assert_check_usage(vec![
            "check".to_owned(),
            root.to_string_lossy().into_owned(),
            "bran-strict".to_owned(),
            "extra".to_owned(),
        ]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn query_without_native_policy_is_explicitly_unavailable() {
        let root =
            std::env::temp_dir().join(format!("bran-query-no-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("readme.md"), "hello\n").unwrap();

        let result = CliApp::run(vec![
            "query".to_owned(),
            root.to_string_lossy().into_owned(),
            "anything".to_owned(),
        ]);

        assert_eq!(result.exit_code, ExitCode::SUCCESS);
        assert!(!result.is_error);
        assert!(result.output.contains("\"status\":\"unavailable\""));
        assert!(result.output.contains("\"bran_status\":\"unavailable\""));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn query_skips_oversized_and_ignored_files_with_warning() {
        let root =
            std::env::temp_dir().join(format!("bran-query-robust-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".bran")).unwrap();
        std::fs::create_dir_all(root.join(".bearing/focus")).unwrap();
        std::fs::create_dir_all(root.join("ignored")).unwrap();
        std::fs::write(root.join(".bran/policy.yaml"), minimal_valid_policy()).unwrap();
        std::fs::write(root.join(".branignore"), ".bran/\n").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::write(root.join("ignored/private.md"), "oversized target\n").unwrap();
        std::fs::write(root.join(".bearing/focus/log.txt"), vec![b'x'; 1_048_577]).unwrap();
        std::fs::write(root.join("large.log"), vec![b'x'; 1_048_577]).unwrap();
        std::fs::write(
            root.join("visible.md"),
            "---\ntitle: Oversized target\n---\nvisible\n",
        )
        .unwrap();

        let result = CliApp::run(vec![
            "query".to_owned(),
            root.to_string_lossy().into_owned(),
            "oversized target".to_owned(),
        ]);

        assert_eq!(result.exit_code, ExitCode::SUCCESS, "{}", result.output);
        assert!(!result.is_error);
        assert!(result.output.contains("\"status\":\"ok\""));
        assert!(result
            .output
            .contains("\"selected_locators\":[\"visible.md\"]"));
        assert!(result.output.contains("OversizedInput"));
        assert!(result.output.contains("large.log"));
        assert!(!result.output.contains("scan_error"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn query_ranks_body_phrase_document_first() {
        // A phrase that appears only in a document body must retrieve that
        // document ahead of any path-token coincidence (issue #19).
        let root =
            std::env::temp_dir().join(format!("bran-query-body-phrase-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".bran")).unwrap();
        std::fs::write(root.join(".bran/policy.yaml"), minimal_valid_policy()).unwrap();
        std::fs::write(
            root.join("canonical.md"),
            "---\ntype: concept\ntitle: Canonical anchor\ntags: anchor\n---\nThe proposal builds on the compatible superset not a fork idea.\n",
        )
        .unwrap();
        std::fs::write(
            root.join("not-unrelated.md"),
            "---\ntype: concept\ntitle: Unrelated incident\ntags: incident\n---\nno matching content here\n",
        )
        .unwrap();

        let result = CliApp::run(vec![
            "query".to_owned(),
            root.to_string_lossy().into_owned(),
            "compatible superset not a fork".to_owned(),
        ]);

        assert_eq!(result.exit_code, ExitCode::SUCCESS, "{}", result.output);
        assert!(result.output.contains(
            "\"source_rankings\":[{\"locator\":\"canonical.md\",\"rank\":1,\"score\":{\"exact\":0,\"partial\":3"
        ));
        assert!(result.output.contains("partial:body"));
        // The generic term "not" is stop-listed (issue #18); the remaining
        // body phrase must still outrank any path-token coincidence and rank
        // the full-phrase document first.
        assert!(!result
            .output
            .contains("\"locator\":\"not-unrelated.md\",\"rank\":1"));
        assert!(!result.output.contains("unmatched_query_terms"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn query_returns_empty_when_high_specificity_entity_unmatched() {
        // A hyphenated identifier that matches no document must not present
        // generic sub-token matches as ordinary evidence: no rankings and a
        // warning naming the whole unit (issue #18).
        let root = std::env::temp_dir().join(format!(
            "bran-query-unmatched-entity-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".bran")).unwrap();
        std::fs::write(root.join(".bran/policy.yaml"), minimal_valid_policy()).unwrap();
        std::fs::write(
            root.join("notes.md"),
            "---\ntype: concept\ntitle: Notes\n---\nGeneric notes about the nonexistent collector wrapper and runner.\n",
        )
        .unwrap();

        let result = CliApp::run(vec![
            "query".to_owned(),
            root.to_string_lossy().into_owned(),
            "zzq-entity-unit-8873".to_owned(),
        ]);

        assert_eq!(result.exit_code, ExitCode::SUCCESS, "{}", result.output);
        assert!(result.output.contains("\"source_rankings\":[],"));
        assert!(!result.output.contains("\"locator\":\"notes.md\""));
        assert!(result
            .output
            .contains("unmatched_query_terms: zzq-entity-unit-8873"));

        // The same holds when generic words around the missing entity match
        // bodies: their weak matches must not be presented as evidence either.
        let diluted = CliApp::run(vec![
            "query".to_owned(),
            root.to_string_lossy().into_owned(),
            "Where are the zzq-entity-unit-8873 wrapper, runner, and tests documented?".to_owned(),
        ]);
        assert_eq!(diluted.exit_code, ExitCode::SUCCESS, "{}", diluted.output);
        assert!(diluted.output.contains("\"source_rankings\":[],"));
        assert!(!diluted.output.contains("\"locator\":\"notes.md\""));
        assert!(diluted
            .output
            .contains("unmatched_query_terms: zzq-entity-unit-8873"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sub_token_does_not_count_as_entity_match() {
        // Only the whole identifier unit matches: a document holding just its
        // sub-tokens as separate words must not be presented as evidence for
        // the entity (issue #18).
        let root = std::env::temp_dir().join(format!(
            "bran-query-sub-token-entity-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".bran")).unwrap();
        std::fs::write(root.join(".bran/policy.yaml"), minimal_valid_policy()).unwrap();
        std::fs::write(
            root.join("home.md"),
            "---\ntype: concept\ntitle: Home\n---\nThe zzq-entity-unit-8873 wrapper and runner are implemented here.\n",
        )
        .unwrap();
        std::fs::write(
            root.join("noise.md"),
            "---\ntype: concept\ntitle: Noise\n---\nThe entity unit wrapper notes live here.\n",
        )
        .unwrap();

        let result = CliApp::run(vec![
            "query".to_owned(),
            root.to_string_lossy().into_owned(),
            "zzq-entity-unit-8873".to_owned(),
        ]);

        assert_eq!(result.exit_code, ExitCode::SUCCESS, "{}", result.output);
        assert!(result.output.contains(
            "\"source_rankings\":[{\"locator\":\"home.md\",\"rank\":1,\"score\":{\"exact\":1"
        ));
        assert!(result.output.contains("exact:body"));
        assert!(!result.output.contains("\"locator\":\"noise.md\""));
        assert!(!result.output.contains("unmatched_query_terms"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn query_warns_on_unmatched_query_terms() {
        // A high-specificity term that matches no document must surface an
        // explicit warning instead of silently returning diluted generic
        // matches (issue #18).
        let root =
            std::env::temp_dir().join(format!("bran-query-unmatched-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".bran")).unwrap();
        std::fs::write(root.join(".bran/policy.yaml"), minimal_valid_policy()).unwrap();
        std::fs::write(
            root.join("actuator.md"),
            "---\ntype: concept\ntitle: Actuator\n---\nGeneric actuator notes.\n",
        )
        .unwrap();

        let diluted = CliApp::run(vec![
            "query".to_owned(),
            root.to_string_lossy().into_owned(),
            "zephyrite actuator".to_owned(),
        ]);
        assert_eq!(diluted.exit_code, ExitCode::SUCCESS, "{}", diluted.output);
        assert!(diluted
            .output
            .contains("\"locator\":\"actuator.md\",\"rank\":1"));
        assert!(diluted.output.contains("unmatched_query_terms: zephyrite"));

        let miss = CliApp::run(vec![
            "query".to_owned(),
            root.to_string_lossy().into_owned(),
            "zephyrite".to_owned(),
        ]);
        assert_eq!(miss.exit_code, ExitCode::SUCCESS, "{}", miss.output);
        assert!(miss.output.contains("unmatched_query_terms: zephyrite"));
        assert!(miss.output.contains("\"source_rankings\":[],"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn check_coverage_policy_error_is_typed_and_non_echoing() {
        let invalid = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                "/nonexistent".to_owned(),
                "bran-strict".to_owned(),
            ],
            b"schema_version: \"1\"\ncoverage:\n  - canoncal\n",
        );
        assert_eq!(invalid.exit_code, TypedExit::Usage.code());
        assert!(invalid.output.contains("policy_invalid_coverage_class"));
        assert!(!invalid.output.contains("canoncal"));

        let (root, _) = scratch_check_root("coverage-allowed");
        write_valid_doc(&root, "doc.md");
        let allowed = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "bran-strict".to_owned(),
            ],
            b"schema_version: \"1\"\ncoverage:\n  - canonical\n",
        );
        assert!(!allowed.is_error);
        assert!(!allowed.output.contains("policy_invalid_coverage_class"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── policy_max_bytes_not_oversized ──

    #[test]
    fn policy_max_bytes_not_oversized() {
        let (root, _) = scratch_check_root("max-bytes");
        write_valid_doc(&root, "doc.md");
        // Exactly MAX_POLICY_BYTES of valid-ish YAML
        let prefix = "schema_version: \"1\"\n# ";
        let fill = MAX_POLICY_BYTES - prefix.len();
        let mut payload = prefix.to_owned();
        for _ in 0..fill {
            payload.push('#');
        }
        assert_eq!(payload.len(), MAX_POLICY_BYTES);
        let result = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "bran-strict".to_owned(),
            ],
            payload.as_bytes(),
        );
        // Must NOT be classified oversized; may fail for content but not size
        assert!(!result.output.contains("policy_oversized"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── check_secret_sentinels ──

    #[test]
    fn check_secret_sentinels_never_echo() {
        let secret = "SECRET-SENTINEL-9977";
        // unknown key containing the sentinel
        let result_uk = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                "/nonexistent".to_owned(),
                "bran-strict".to_owned(),
            ],
            format!("schema_version: \"1\"\n{secret}: x\n").as_bytes(),
        );
        assert!(!result_uk.output.contains(secret));
        assert!(result_uk.output.contains("policy_unknown_field"));

        // duplicate key containing sentinel
        let result_dk = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                "/nonexistent".to_owned(),
                "bran-strict".to_owned(),
            ],
            format!("schema_version: \"1\"\n{secret}: a\n{secret}: b\n").as_bytes(),
        );
        assert!(!result_dk.output.contains(secret));
        assert!(result_dk.output.contains("policy_duplicate_key"));

        // unsafe path containing sentinel
        let result_up = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                "/nonexistent".to_owned(),
                "bran-strict".to_owned(),
            ],
            format!("schema_version: \"1\"\ndocument_coverage:\n  roots:\n    - /{secret}\n")
                .as_bytes(),
        );
        assert!(!result_up.output.contains(secret));
        assert!(result_up.output.contains("policy_unsafe_document_path"));

        // unsupported version containing sentinel
        let result_ver = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                "/nonexistent".to_owned(),
                "bran-strict".to_owned(),
            ],
            format!("schema_version: \"{secret}\"\n").as_bytes(),
        );
        assert!(!result_ver.output.contains(secret));
        assert!(result_ver.output.contains("policy_unsupported_version"));

        // malformed construct containing sentinel
        let result_ma = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                "/nonexistent".to_owned(),
                "bran-strict".to_owned(),
            ],
            format!("schema_version: {{{secret}\n").as_bytes(),
        );
        assert!(!result_ma.output.contains(secret));
        assert!(result_ma.output.contains("policy_malformed_yaml"));
    }

    // ── check_policy_before_scan ──

    #[test]
    fn check_policy_error_before_scan() {
        // Use nonexistent root + invalid policy
        let result = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                "/nonexistent-bran-root".to_owned(),
                "bran-strict".to_owned(),
            ],
            b"schema_version: \"99\"\n",
        );
        // Must exit with policy error before scan error
        assert_eq!(result.exit_code, TypedExit::Usage.code());
        assert!(result.output.contains("policy_unsupported_version"));
        assert!(!result.output.contains("scan_error"));
    }

    // ── policy_changes_diagnostics ──

    #[test]
    fn policy_rule_changes_strict_diagnostics() {
        let (root, bran_dir) = scratch_check_root("policy-changes");
        write_valid_doc(&root, "doc.md");
        std::fs::write(
            root.join("extra.md"),
            "---\ntype: concept\ntitle: Extra\nokf_status: active\ntags: cli-check\ntimestamp: 2026-07-20T00:00:00Z\nresource: test://extra\npublic_boundary: safe\n---\nbody\n",
        )
        .unwrap();

        // Baseline: minimal policy
        let minimal = minimal_valid_policy();
        std::fs::write(bran_dir.join("policy.yaml"), &minimal).unwrap();
        let baseline = CliApp::run(vec![
            "check".to_owned(),
            root.to_string_lossy().into_owned(),
            "bran-strict".to_owned(),
        ]);

        // Stricter policy: require an additional frontmatter field
        let stricter = "schema_version: \"1\"\nfrontmatter:\n  required:\n    - type\n    - title\n    - okf_status\n    - tags\n    - timestamp\n    - resource\n    - public_boundary\n    - extra_required\n";
        let stricter_result = CliApp::run_with_stdin(
            vec![
                "check".to_owned(),
                "--policy-stdin".to_owned(),
                root.to_string_lossy().into_owned(),
                "bran-strict".to_owned(),
            ],
            stricter.as_bytes(),
        );

        // The stricter policy should produce different (more) diagnostics
        assert_ne!(baseline.output, stricter_result.output);
        // Both should pass if baseline was clean (valid docs satisfy stricter too depends)
        // At minimum, the diagnostics arrays differ
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── policy_stdin_ignored_for_non_stdin_commands ──

    #[test]
    fn policy_stdin_injection_ignored_for_non_check() {
        // run_with_stdin passes bytes, but non-check commands ignore it
        let smoke = CliApp::run_with_stdin(vec!["smoke".to_owned()], b"garbage");
        assert_eq!(smoke.output, SMOKE_OUTPUT);
        assert_eq!(smoke.exit_code, ExitCode::SUCCESS);

        let help = CliApp::run_with_stdin(vec!["-h".to_owned()], b"garbage");
        assert!(help.output.contains("Usage: bran"));
        assert_eq!(help.exit_code, ExitCode::SUCCESS);
    }

    // ── check_extra_arg_too_many_args ──

    #[test]
    fn check_extra_arg_too_many_args() {
        let (root, bran_dir) = scratch_check_root("extra-arg");
        write_valid_doc(&root, "doc.md");
        let policy_raw = minimal_valid_policy();
        std::fs::write(bran_dir.join("policy.yaml"), &policy_raw).unwrap();

        let result = CliApp::run(vec![
            "check".to_owned(),
            root.to_string_lossy().into_owned(),
            "bran-strict".to_owned(),
            "extra".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Usage.code());
        assert!(result.output.contains("too_many_args"));
        assert!(!result.output.contains("invalid_policy_stdin_flag"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── check_invalid_policy_stdin_flag_misplaced ──

    #[test]
    fn check_invalid_policy_stdin_flag_misplaced() {
        let policy_raw = minimal_valid_policy();
        let root_path = "/nonexistent-check-policy-stdin-flag";

        let cases: &[&[&str]] = &[
            &["check", root_path, "--policy-stdin", "bran-strict"],
            &["check", root_path, "bran-strict", "--policy-stdin"],
            &[
                "check",
                "--policy-stdin",
                "--policy-stdin",
                root_path,
                "bran-strict",
            ],
            &["check", "--policy-stdin", root_path, "--policy-stdin"],
            &["check", root_path, "--policy-stdin", "bran-strict", "extra"],
        ];
        for args in cases {
            let result =
                CliApp::run_with_stdin(args.iter().map(|s| s.to_string()), policy_raw.as_bytes());
            assert_eq!(
                result.exit_code,
                TypedExit::Usage.code(),
                "expected usage exit for args: {:?}, got: {}",
                args,
                result.output
            );
            assert!(
                result.output.contains("invalid_policy_stdin_flag"),
                "missing invalid_policy_stdin_flag for args: {:?}, got: {}",
                args,
                result.output
            );
        }
    }

    // --- body-preserved CLI tests ---

    fn body_preserved_fixture() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bran-bp-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn write_bp_file(dir: &std::path::Path, name: &str, content: &[u8]) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn body_preserved_cli_passes_identical_bodies() {
        let root = body_preserved_fixture();
        write_bp_file(&root, "orig.md", b"---\ntitle: A\n---\n# Hello\n");
        write_bp_file(&root, "mig.md", b"---\ntitle: B\n---\n# Hello\n");
        write_bp_file(
            &root,
            "bp.json",
            b"{\"files\":[{\"original_path\":\"orig.md\",\"migrated_path\":\"mig.md\"}]}",
        );
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "bp.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, ExitCode::SUCCESS);
        assert!(result.output.contains("\"status\":\"ok\""));
        assert!(result.output.contains("\"passed\":true"));
        assert!(result.output.contains("\"files_checked\":1"));
        assert!(result.output.contains("\"findings_count\":0"));
        assert!(result
            .output
            .contains("\"sources\":[\"bran-core\",\"body-preservation\"]"));
    }

    #[test]
    fn body_preserved_cli_changed_body_exits_one() {
        let root = body_preserved_fixture();
        write_bp_file(&root, "orig.md", b"# Hello\n");
        write_bp_file(&root, "mig.md", b"# World\n");
        write_bp_file(
            &root,
            "bp.json",
            b"{\"files\":[{\"original_path\":\"orig.md\",\"migrated_path\":\"mig.md\"}]}",
        );
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "bp.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(result.output.contains("\"status\":\"failed\""));
        assert!(result.output.contains("\"passed\":false"));
        assert!(result.output.contains("\"findings_count\":1"));
        assert!(result.output.contains("body_changed"));
    }

    #[test]
    fn body_preserved_cli_deterministic_repeat() {
        let root = body_preserved_fixture();
        write_bp_file(&root, "a.md", b"# A\n");
        write_bp_file(&root, "b.md", b"# B\n");
        write_bp_file(&root, "ma.md", b"# A\n");
        write_bp_file(&root, "mb.md", b"# B\n");
        write_bp_file(
            &root,
            "bp.json",
            b"{\"files\":[{\"original_path\":\"a.md\",\"migrated_path\":\"ma.md\"},{\"original_path\":\"b.md\",\"migrated_path\":\"mb.md\"}]}",
        );
        let a1 = [
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "bp.json".to_owned(),
        ];
        let a2 = a1.clone();
        let r1 = CliApp::run(a1);
        let r2 = CliApp::run(a2);
        assert_eq!(r1.output, r2.output);
    }

    #[test]
    fn body_preserved_cli_unicode_json_path_works() {
        let root = body_preserved_fixture();
        write_bp_file(&root, "répo.md", b"# Bonjour\n");
        write_bp_file(&root, "migré.md", b"# Bonjour\n");
        write_bp_file(
            &root,
            "bp.json",
            b"{\"files\":[{\"original_path\":\"r\xc3\xa9po.md\",\"migrated_path\":\"migr\xc3\xa9.md\"}]}",
        );
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "bp.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, ExitCode::SUCCESS);
        assert!(result.output.contains("\"status\":\"ok\""));
    }

    #[test]
    fn body_preserved_cli_missing_or_extra_args_usage_exit_two() {
        let root = body_preserved_fixture();
        let missing_manifest = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
        ]);
        assert_eq!(missing_manifest.exit_code, TypedExit::Usage.code());
        assert!(missing_manifest.output.contains("missing_manifest"));
        let extra = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "bp.json".to_owned(),
            "extra".to_owned(),
        ]);
        assert_eq!(extra.exit_code, TypedExit::Usage.code());
        assert!(extra.output.contains("too_many_args"));
        let no_args = CliApp::run(["body-preserved".to_owned()]);
        assert_eq!(no_args.exit_code, TypedExit::Usage.code());
        assert!(no_args.output.contains("missing_root"));
    }

    #[test]
    fn body_preserved_cli_unsafe_manifest_path_safe_exit_one() {
        let root = body_preserved_fixture();
        // absolute path inside JSON is rejected; the report finds it unsafe
        write_bp_file(&root, "orig.md", b"body\n");
        write_bp_file(&root, "mig.md", b"body\n");
        write_bp_file(
            &root,
            "bp.json",
            b"{\"files\":[{\"original_path\":\"orig.md\",\"migrated_path\":\"/etc/passwd\"}]}",
        );
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "bp.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(result.output.contains("\"status\":\"failed\""));
        assert!(!result.output.contains("passwd"));
    }

    #[test]
    fn body_preserved_cli_traversal_manifest_path_safe_exit_one() {
        let root = body_preserved_fixture();
        write_bp_file(&root, "orig.md", b"body\n");
        write_bp_file(&root, "mig.md", b"body\n");
        write_bp_file(
            &root,
            "bp.json",
            b"{\"files\":[{\"original_path\":\"orig.md\",\"migrated_path\":\"../escape.md\"}]}",
        );
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "bp.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(!result.output.contains("escape"));
    }

    #[test]
    fn body_preserved_cli_symlink_manifest_rejected() {
        let root = body_preserved_fixture();
        write_bp_file(&root, "real.json", b"{\"files\":[]}");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("real.json", root.join("bp.json")).unwrap();
        }
        #[cfg(not(unix))]
        {
            std::fs::write(root.join("bp.json"), b"{\"files\":[]}").unwrap();
        }
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "bp.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(result.output.contains("body_preserved_validation_failed"));
    }

    #[test]
    fn body_preserved_cli_oversized_manifest_exit_one() {
        let root = body_preserved_fixture();
        let big = vec![b' '; 2 * 1024 * 1024 + 1];
        std::fs::write(root.join("big.json"), &big).unwrap();
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "big.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(result.output.contains("body_preserved_validation_failed"));
    }

    #[test]
    fn body_preserved_cli_nonexistent_root_exit_three() {
        let result = CliApp::run([
            "body-preserved".to_owned(),
            "/nonexistent-root-zzz-bp".to_owned(),
            "bp.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Operation.code());
        assert!(result.output.contains("io_unavailable"));
    }

    #[test]
    fn body_preserved_cli_no_secrets_or_os_strings_in_output() {
        let root = body_preserved_fixture();
        write_bp_file(&root, "orig.md", b"body\n");
        write_bp_file(&root, "mig.md", b"body\n");
        write_bp_file(
            &root,
            "bp.json",
            b"{\"files\":[{\"original_path\":\"orig.md\",\"migrated_path\":\"missing.md\"}]}",
        );
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "bp.json".to_owned(),
        ]);
        assert!(result.output.contains("\"status\":\"failed\""));
        // never echoes raw manifest JSON body content
        assert!(!result.output.contains("missing.md"));
    }

    #[test]
    fn body_preserved_cli_read_only_no_writes() {
        let root = body_preserved_fixture();
        write_bp_file(&root, "orig.md", b"# H\n");
        write_bp_file(&root, "mig.md", b"# H\n");
        write_bp_file(
            &root,
            "bp.json",
            b"{\"files\":[{\"original_path\":\"orig.md\",\"migrated_path\":\"mig.md\"}]}",
        );
        // snapshot before
        let before: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (e.file_name(), e.metadata().unwrap().modified().unwrap())
            })
            .collect();
        CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "bp.json".to_owned(),
        ]);
        // snapshot after
        let after: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (e.file_name(), e.metadata().unwrap().modified().unwrap())
            })
            .collect();
        assert_eq!(before, after);
    }

    // --- 2.1-C remediation CLI tests ---

    #[test]
    fn body_preserved_cli_missing_manifest_exit_one_no_echo() {
        let root = body_preserved_fixture();
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "nope.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(result.output.contains("body_preserved_validation_failed"));
        assert!(!result.output.contains("nope.json"));
    }

    #[test]
    fn body_preserved_cli_directory_manifest_exit_one_no_echo() {
        let root = body_preserved_fixture();
        std::fs::create_dir(root.join("subdir")).unwrap();
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "subdir".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(result.output.contains("body_preserved_validation_failed"));
        assert!(!result.output.contains("subdir"));
    }

    #[test]
    fn body_preserved_cli_absolute_manifest_path_exit_one_no_echo() {
        let root = body_preserved_fixture();
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "/etc/passwd".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(result.output.contains("body_preserved_validation_failed"));
        assert!(!result.output.contains("passwd"));
    }

    #[test]
    fn body_preserved_cli_windows_style_manifest_exit_one_no_echo() {
        let root = body_preserved_fixture();
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "C:\\foo\\bar.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(result.output.contains("body_preserved_validation_failed"));
        assert!(!result.output.contains("C:"));
    }

    #[test]
    fn body_preserved_cli_unc_manifest_exit_one_no_echo() {
        let root = body_preserved_fixture();
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "\\\\server\\share\\file.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(result.output.contains("body_preserved_validation_failed"));
        assert!(!result.output.contains("server"));
    }

    #[test]
    fn body_preserved_cli_nul_manifest_exit_one_no_echo() {
        let root = body_preserved_fixture();
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "bad\0.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(result.output.contains("body_preserved_validation_failed"));
    }

    #[test]
    #[cfg(unix)]
    fn body_preserved_cli_invalid_utf8_manifest_exits_two() {
        use std::os::unix::ffi::OsStrExt;
        let root = body_preserved_fixture();
        let invalid = std::ffi::OsStr::from_bytes(&[0xFF, 0xFE]);
        let args = [
            std::ffi::OsString::from("body-preserved"),
            root.as_os_str().to_os_string(),
            invalid.to_os_string(),
        ];
        let result = CliApp::run(args);
        assert_eq!(result.exit_code, TypedExit::Usage.code());
    }

    #[test]
    fn body_preserved_cli_wrong_digest_exit_one_original_body_hash_mismatch() {
        let root = body_preserved_fixture();
        write_bp_file(&root, "orig.md", b"body\n");
        write_bp_file(&root, "mig.md", b"body\n");
        write_bp_file(
            &root,
            "bp.json",
            b"{\"files\":[{\"original_path\":\"orig.md\",\"migrated_path\":\"mig.md\",\"body_sha256\":\"0000000000000000000000000000000000000000000000000000000000000000\"}]}",
        );
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "bp.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(result.output.contains("original_body_hash_mismatch"));
    }

    #[test]
    fn body_preserved_cli_malformed_secret_bearing_json_no_echo() {
        let root = body_preserved_fixture();
        write_bp_file(&root, "orig.md", b"body\n");
        write_bp_file(&root, "mig.md", b"body\n");
        write_bp_file(
            &root,
            "bp.json",
            b"{\"files\":[{\"original_path\":\"orig.md\",\"migrated_path\":\"mig.md\",\"secret\":\"abc123\",}]}",
        );
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "bp.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(!result.output.contains("abc123"));
        assert!(!result.output.contains("secret"));
    }

    #[test]
    fn body_preserved_cli_exact_2mib_not_oversized() {
        let root = body_preserved_fixture();
        let exact = vec![b' '; 2 * 1024 * 1024];
        std::fs::write(root.join("exact.json"), &exact).unwrap();
        let result = CliApp::run([
            "body-preserved".to_owned(),
            root.to_string_lossy().into_owned(),
            "exact.json".to_owned(),
        ]);
        assert_eq!(result.exit_code, TypedExit::Validation.code());
        assert!(!result.output.contains("oversized"));
        assert!(!result.output.contains("manifest_too_large"));
    }
}
