//! Dependency-free SQZ policy evaluation at the packet boundary.
//!
//! This module deliberately owns no process, filesystem, network, or provider
//! integration. A host supplies those concerns through [`SqzPort`].

use crate::agent::result_store::ResultId;
use crate::packet::{ContextPacket, EvidencePriority, PreservationAnchor};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub const APPROVED_SQZ_SOURCE: &str =
    "approved-cargo-install:sqz-cli=1.1.1+patched-compatible-deps";
pub const APPROVED_SQZ_VERSION: &str = "sqz 1.1.1";
pub const APPROVED_SQZ_SHA256: &str =
    "03c8de9c55f22e3c3e33852972a2a12a8e436d8861736db71c9441804075e722";
pub const SQZ_RECEIPT_SCHEMA_VERSION: &str = "1.0.0";

/// Exact identity claimed by the configured and returned SQZ implementation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqzIdentity {
    pub source: String,
    pub version: String,
    pub sha256: String,
}

impl SqzIdentity {
    pub fn new(
        source: impl Into<String>,
        version: impl Into<String>,
        sha256: impl Into<String>,
    ) -> Self {
        Self {
            source: source.into(),
            version: version.into(),
            sha256: sha256.into(),
        }
    }

    pub fn approved() -> Self {
        Self::new(
            APPROVED_SQZ_SOURCE,
            APPROVED_SQZ_VERSION,
            APPROVED_SQZ_SHA256,
        )
    }

    fn is_approved(&self) -> bool {
        self.source == APPROVED_SQZ_SOURCE
            && self.version == APPROVED_SQZ_VERSION
            && self.sha256 == APPROVED_SQZ_SHA256
    }
}

/// Policy resolved by configuration, not by a call site.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqzPolicy {
    PublicOff,
    PublicOn,
    InternalLocked,
}

/// A provider-neutral port. Implementations may be in-memory or host-owned.
pub trait SqzPort {
    fn compress(&self, input: &str) -> Result<SqzPortOutput, SqzPortError>;
}

impl<T: SqzPort + ?Sized> SqzPort for &T {
    fn compress(&self, input: &str) -> Result<SqzPortOutput, SqzPortError> {
        (**self).compress(input)
    }
}

const EXTERNAL_SQZ_MAX_STDOUT: usize = 1024 * 1024;
const EXTERNAL_SQZ_MAX_STDERR: usize = 64 * 1024;

/// Exact, local SQZ executable pinned to the approved identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalSqzPort {
    executable: PathBuf,
    timeout: Duration,
    #[cfg(test)]
    fixture_mode: bool,
}

impl ExternalSqzPort {
    pub fn new(executable: impl Into<PathBuf>) -> Result<Self, SqzPortError> {
        let port = Self {
            executable: executable.into(),
            timeout: Duration::from_secs(30),
            #[cfg(test)]
            fixture_mode: false,
        };
        port.verify_executable(None)?;
        Ok(port)
    }

    #[cfg(test)]
    pub fn current_test_fixture(timeout: Duration) -> Result<Self, SqzPortError> {
        let (executable, _) = crate::adapters::provider::current_test_fixture_identity()
            .map_err(|_| sqz_unavailable())?;
        Ok(Self {
            executable,
            timeout,
            fixture_mode: true,
        })
    }

    fn verify_executable(&self, deadline: Option<Instant>) -> Result<(), SqzPortError> {
        #[cfg(test)]
        if self.fixture_mode {
            return Ok(());
        }
        let digest = match deadline {
            Some(deadline) => {
                crate::adapters::provider::verified_executable_sha256_by(&self.executable, deadline)
            }
            None => crate::adapters::provider::verified_executable_sha256(&self.executable),
        }
        .map_err(|error| {
            if error == crate::agent::runtime::ProviderError::Timeout {
                sqz_timeout()
            } else {
                sqz_unavailable()
            }
        })?;
        if digest == APPROVED_SQZ_SHA256 {
            Ok(())
        } else {
            Err(sqz_unavailable())
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();
        #[cfg(test)]
        if self.fixture_mode {
            command.args([
                "agent::tests::p3_agent_profile_contract",
                "--exact",
                "--nocapture",
                "--quiet",
            ]);
            command.env("BRAN_P3_SQZ_FIXTURE", "1");
            return command;
        }
        command.args(["compress", "--mode", "safe", "--no-cache"]);
        command
    }
}

impl SqzPort for ExternalSqzPort {
    fn compress(&self, input: &str) -> Result<SqzPortOutput, SqzPortError> {
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or_else(sqz_timeout)?;
        self.verify_executable(Some(deadline))?;
        if Instant::now() >= deadline {
            return Err(sqz_timeout());
        }
        let mut child = self.command().spawn().map_err(|_| sqz_unavailable())?;
        if let Err(error) = self.verify_executable(Some(deadline)) {
            terminate_sqz(child);
            return Err(error);
        }
        let (Some(mut stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            terminate_sqz(child);
            return Err(sqz_execution_failed());
        };
        let payload = input.as_bytes().to_vec();
        let (stdin_tx, stdin_rx) = mpsc::channel();
        thread::spawn(move || {
            let result = stdin
                .write_all(&payload)
                .and_then(|_| stdin.flush())
                .map_err(|_| sqz_execution_failed());
            let _ = stdin_tx.send(result);
        });
        let (stdout_tx, stdout_rx) = mpsc::channel();
        #[cfg(test)]
        let fixture_mode = self.fixture_mode;
        thread::spawn(move || {
            #[cfg(test)]
            let result = if fixture_mode {
                read_sqz_fixture(stdout)
            } else {
                read_bounded(stdout, EXTERNAL_SQZ_MAX_STDOUT)
            };
            #[cfg(not(test))]
            let result = read_bounded(stdout, EXTERNAL_SQZ_MAX_STDOUT);
            let _ = stdout_tx.send(result);
        });
        let (stderr_tx, stderr_rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = stderr_tx.send(read_bounded(stderr, EXTERNAL_SQZ_MAX_STDERR));
        });

        let result = (|| {
            receive_sqz(&stdin_rx, deadline)??;
            let stdout = receive_sqz(&stdout_rx, deadline)??;
            receive_sqz(&stderr_rx, deadline)??;
            let status = wait_sqz(&mut child, deadline)?;
            if !status.success() {
                return Err(sqz_execution_failed());
            }
            let payload = String::from_utf8(stdout).map_err(|_| sqz_invalid_output())?;
            if payload.trim().is_empty() {
                return Err(sqz_invalid_output());
            }
            Ok(SqzPortOutput::new(payload, SqzIdentity::approved()))
        })();
        if let Err(error) = result {
            terminate_sqz(child);
            return Err(error);
        }
        result
    }
}

fn receive_sqz<T>(rx: &mpsc::Receiver<T>, deadline: Instant) -> Result<T, SqzPortError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(sqz_timeout)?;
    rx.recv_timeout(remaining).map_err(|error| match error {
        mpsc::RecvTimeoutError::Timeout => sqz_timeout(),
        mpsc::RecvTimeoutError::Disconnected => sqz_execution_failed(),
    })
}

fn wait_sqz(child: &mut Child, deadline: Instant) -> Result<ExitStatus, SqzPortError> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Err(_) => return Err(sqz_execution_failed()),
            Ok(None) => {}
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(sqz_timeout)?;
        thread::sleep(remaining.min(Duration::from_millis(1)));
    }
}

fn terminate_sqz(mut child: Child) {
    let _ = child.kill();
    thread::spawn(move || {
        let _ = child.wait();
    });
}

fn read_bounded(mut reader: impl Read, max: usize) -> Result<Vec<u8>, SqzPortError> {
    let mut output = Vec::new();
    reader
        .by_ref()
        .take(max as u64 + 1)
        .read_to_end(&mut output)
        .map_err(|_| sqz_execution_failed())?;
    if output.len() > max {
        Err(sqz_invalid_output())
    } else {
        Ok(output)
    }
}

#[cfg(test)]
fn read_sqz_fixture(mut reader: impl Read) -> Result<Vec<u8>, SqzPortError> {
    fn line(reader: &mut impl Read) -> Result<Vec<u8>, SqzPortError> {
        let mut output = Vec::new();
        for _ in 0..64 {
            let mut byte = [0];
            reader
                .read_exact(&mut byte)
                .map_err(|_| sqz_invalid_output())?;
            output.push(byte[0]);
            if byte[0] == b'\n' {
                return Ok(output);
            }
        }
        Err(sqz_invalid_output())
    }
    let mut banner = line(&mut reader)?;
    if banner == b"\n" {
        banner = line(&mut reader)?;
    }
    if banner != b"running 1 test\n" {
        return Err(sqz_invalid_output());
    }
    let length = String::from_utf8(line(&mut reader)?)
        .map_err(|_| sqz_invalid_output())?
        .trim_end()
        .parse::<usize>()
        .map_err(|_| sqz_invalid_output())?;
    if length > EXTERNAL_SQZ_MAX_STDOUT {
        return Err(sqz_invalid_output());
    }
    let mut output = vec![0; length];
    reader
        .read_exact(&mut output)
        .map_err(|_| sqz_invalid_output())?;
    let mut newline = [0];
    reader
        .read_exact(&mut newline)
        .map_err(|_| sqz_invalid_output())?;
    if newline != [b'\n'] {
        return Err(sqz_invalid_output());
    }
    // The test binary emits its harness trailer after the fixture payload.
    read_bounded(reader, 4096)?;
    Ok(output)
}

fn sqz_unavailable() -> SqzPortError {
    SqzPortError::new(SqzPortErrorCode::Unavailable)
}
fn sqz_timeout() -> SqzPortError {
    SqzPortError::new(SqzPortErrorCode::Timeout)
}
fn sqz_execution_failed() -> SqzPortError {
    SqzPortError::new(SqzPortErrorCode::ExecutionFailed)
}
fn sqz_invalid_output() -> SqzPortError {
    SqzPortError::new(SqzPortErrorCode::InvalidOutput)
}

/// Successful port response. Actual token counts are optional provider telemetry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqzPortOutput {
    pub payload: String,
    pub identity: SqzIdentity,
    pub actual_input_tokens: Option<usize>,
    pub actual_output_tokens: Option<usize>,
}

impl SqzPortOutput {
    pub fn new(payload: impl Into<String>, identity: SqzIdentity) -> Self {
        Self {
            payload: payload.into(),
            identity,
            actual_input_tokens: None,
            actual_output_tokens: None,
        }
    }
}

/// Closed public diagnostic codes prevent provider stderr or secrets entering receipts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqzPortErrorCode {
    Unavailable,
    Timeout,
    ExecutionFailed,
    InvalidOutput,
}

/// A port failure without provider-specific or unbounded text entering the core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SqzPortError {
    pub code: SqzPortErrorCode,
}

impl SqzPortError {
    pub fn new(code: SqzPortErrorCode) -> Self {
        Self { code }
    }
}

/// Configuration fixed when the adapter is created.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqzAdapterConfig {
    pub policy: SqzPolicy,
    pub identity: SqzIdentity,
    pub configured_max_output_bytes: usize,
    pub fidelity_anchors: Vec<PreservationAnchor>,
}

impl SqzAdapterConfig {
    pub fn new(
        policy: SqzPolicy,
        identity: SqzIdentity,
        configured_max_output_bytes: usize,
        fidelity_anchors: Vec<PreservationAnchor>,
    ) -> Self {
        Self {
            policy,
            identity,
            configured_max_output_bytes,
            fidelity_anchors,
        }
    }
}

/// Explicit outcome for one eligible packet evaluation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqzStatus {
    Off,
    Applied,
    NotBeneficial,
    Failed,
}

/// Fidelity-anchor evaluation is explicit even when an output was unavailable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FidelityStatus {
    NotEvaluated,
    RequiredButUnavailable,
    Passed,
    Missing,
}

/// How far DLP evaluation progressed for the returned receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DlpStatus {
    NotEvaluated,
    InputPassed,
    Passed,
    Findings,
}

/// Why a completed evaluation did not produce a valid compressed payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SqzFailureReason {
    ConfiguredIdentityMismatch,
    ReturnedIdentityMismatch,
    PortUnavailable(SqzPortErrorCode),
    FidelityAnchorsUnavailable,
    MissingFidelityAnchors,
    FidelityAnchorMissingFromInput,
    ConflictingFidelityAnchorIds,
    DlpFindings,
    OutputExceedsBound,
}

/// A SHA-256 identity for accepted returned content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqzId {
    pub algorithm: &'static str,
    pub value: String,
}

/// Complete accounting for policy evaluation. Byte-derived token counts are estimates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqzReceipt {
    pub schema_version: &'static str,
    pub configured_identity: SqzIdentity,
    pub returned_identity: Option<SqzIdentity>,
    pub policy: SqzPolicy,
    pub status: SqzStatus,
    pub failure_reason: Option<SqzFailureReason>,
    pub monotonic_call_latency: Duration,
    pub raw_bytes: usize,
    pub candidate_compressed_bytes: Option<usize>,
    pub returned_bytes: usize,
    pub raw_token_estimate_bytes_divided_by_four_ceiling: usize,
    pub candidate_token_estimate_bytes_divided_by_four_ceiling: Option<usize>,
    pub returned_token_estimate_bytes_divided_by_four_ceiling: usize,
    pub actual_input_tokens: Option<usize>,
    pub actual_output_tokens: Option<usize>,
    pub fidelity_status: FidelityStatus,
    pub required_fidelity_anchor_ids: Vec<String>,
    pub missing_fidelity_anchor_ids: Vec<String>,
    pub dlp_status: DlpStatus,
    pub dlp_findings: Vec<String>,
    pub requested_max_output_bytes: usize,
    pub effective_max_output_bytes: usize,
    pub sqz_id: Option<SqzId>,
}

/// The returned payload and its policy receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqzEvaluation {
    pub packet: ContextPacket,
    pub receipt: SqzReceipt,
}

/// Internal locked policy returns this typed failure instead of an original packet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqzError {
    pub receipt: Box<SqzReceipt>,
}

/// Evaluates one packet through a configured policy. Callers cannot override it.
#[derive(Clone, Debug)]
pub struct SqzAdapter<P> {
    port: P,
    config: SqzAdapterConfig,
}

impl<P> SqzAdapter<P>
where
    P: SqzPort,
{
    pub fn new(port: P, config: SqzAdapterConfig) -> Self {
        Self { port, config }
    }

    pub fn policy(&self) -> SqzPolicy {
        self.config.policy
    }

    /// Evaluates a separately-grounded canonical payload with its explicit
    /// fidelity anchors, without adding another process implementation.
    pub fn evaluate_with_anchors(
        &self,
        packet: ContextPacket,
        requested_max_output_bytes: usize,
        anchors: &[PreservationAnchor],
    ) -> Result<SqzEvaluation, SqzError> {
        let mut config = self.config.clone();
        config.fidelity_anchors.extend_from_slice(anchors);
        SqzAdapter::<&P>::new(&self.port, config).evaluate(packet, requested_max_output_bytes)
    }

    pub fn evaluate(
        &self,
        packet: ContextPacket,
        requested_max_output_bytes: usize,
    ) -> Result<SqzEvaluation, SqzError> {
        let raw_bytes = packet.payload.len();
        let effective_max_output_bytes =
            requested_max_output_bytes.min(self.config.configured_max_output_bytes);
        let anchors = combined_anchors(&packet, &self.config.fidelity_anchors);
        if self.config.policy == SqzPolicy::PublicOff {
            if anchors.conflicting_ids {
                return self.failed(
                    packet,
                    requested_max_output_bytes,
                    effective_max_output_bytes,
                    None,
                    Duration::ZERO,
                    None,
                    FidelityStatus::NotEvaluated,
                    Vec::new(),
                    Vec::new(),
                    SqzFailureReason::ConflictingFidelityAnchorIds,
                );
            }
            let anchor_findings = anchors
                .anchors
                .iter()
                .flat_map(|anchor| dlp_findings(anchor.value()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            if !anchor_findings.is_empty() {
                return self.failed(
                    packet,
                    requested_max_output_bytes,
                    effective_max_output_bytes,
                    None,
                    Duration::ZERO,
                    None,
                    FidelityStatus::NotEvaluated,
                    Vec::new(),
                    anchor_findings,
                    SqzFailureReason::DlpFindings,
                );
            }
            let missing_anchor_ids = anchors
                .anchors
                .iter()
                .filter(|anchor| !packet.payload.contains(anchor.value()))
                .map(|anchor| anchor.id().to_owned())
                .collect::<Vec<_>>();
            if !missing_anchor_ids.is_empty() {
                return self.failed(
                    packet,
                    requested_max_output_bytes,
                    effective_max_output_bytes,
                    None,
                    Duration::ZERO,
                    None,
                    FidelityStatus::Missing,
                    missing_anchor_ids,
                    Vec::new(),
                    SqzFailureReason::FidelityAnchorMissingFromInput,
                );
            }
            let findings = dlp_findings(&packet.payload);
            if !findings.is_empty() {
                return self.failed(
                    packet,
                    requested_max_output_bytes,
                    effective_max_output_bytes,
                    None,
                    Duration::ZERO,
                    None,
                    FidelityStatus::Passed,
                    Vec::new(),
                    findings,
                    SqzFailureReason::DlpFindings,
                );
            }
            return Ok(SqzEvaluation {
                receipt: self.base_receipt(
                    &packet,
                    raw_bytes,
                    raw_bytes,
                    requested_max_output_bytes,
                    effective_max_output_bytes,
                ),
                packet,
            });
        }

        if !self.config.identity.is_approved() {
            return self.failed(
                packet,
                requested_max_output_bytes,
                effective_max_output_bytes,
                None,
                Duration::ZERO,
                None,
                FidelityStatus::NotEvaluated,
                Vec::new(),
                Vec::new(),
                SqzFailureReason::ConfiguredIdentityMismatch,
            );
        }

        if anchors.conflicting_ids {
            return self.failed(
                packet,
                requested_max_output_bytes,
                effective_max_output_bytes,
                None,
                Duration::ZERO,
                None,
                FidelityStatus::NotEvaluated,
                Vec::new(),
                Vec::new(),
                SqzFailureReason::ConflictingFidelityAnchorIds,
            );
        }

        let anchor_dlp = anchors
            .anchors
            .iter()
            .flat_map(|anchor| dlp_findings(anchor.value()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if !anchor_dlp.is_empty() {
            return self.failed(
                packet,
                requested_max_output_bytes,
                effective_max_output_bytes,
                None,
                Duration::ZERO,
                None,
                FidelityStatus::NotEvaluated,
                Vec::new(),
                anchor_dlp,
                SqzFailureReason::DlpFindings,
            );
        }

        let missing_input_anchor_ids = anchors
            .anchors
            .iter()
            .filter(|anchor| !packet.payload.contains(anchor.value()))
            .map(|anchor| anchor.id().to_owned())
            .collect::<Vec<_>>();
        if !missing_input_anchor_ids.is_empty() {
            return self.failed(
                packet,
                requested_max_output_bytes,
                effective_max_output_bytes,
                None,
                Duration::ZERO,
                None,
                FidelityStatus::Missing,
                missing_input_anchor_ids,
                Vec::new(),
                SqzFailureReason::FidelityAnchorMissingFromInput,
            );
        }

        let input_dlp = dlp_findings(&packet.payload);
        if !input_dlp.is_empty() {
            return self.failed(
                packet,
                requested_max_output_bytes,
                effective_max_output_bytes,
                None,
                Duration::ZERO,
                None,
                FidelityStatus::NotEvaluated,
                Vec::new(),
                input_dlp,
                SqzFailureReason::DlpFindings,
            );
        }

        let started = Instant::now();
        let output = self.port.compress(&packet.payload);
        let latency = started.elapsed();
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                return self.failed(
                    packet,
                    requested_max_output_bytes,
                    effective_max_output_bytes,
                    None,
                    latency,
                    None,
                    FidelityStatus::NotEvaluated,
                    Vec::new(),
                    Vec::new(),
                    SqzFailureReason::PortUnavailable(error.code),
                )
            }
        };

        let candidate_bytes = output.payload.len();
        if !output.identity.is_approved() || output.identity != self.config.identity {
            return self.failed(
                packet,
                requested_max_output_bytes,
                effective_max_output_bytes,
                Some(output.identity.clone()),
                latency,
                Some((&output, candidate_bytes, None)),
                FidelityStatus::NotEvaluated,
                Vec::new(),
                Vec::new(),
                SqzFailureReason::ReturnedIdentityMismatch,
            );
        }
        let returned_identity = Some(output.identity.clone());
        if candidate_bytes > effective_max_output_bytes {
            return self.failed(
                packet,
                requested_max_output_bytes,
                effective_max_output_bytes,
                returned_identity,
                latency,
                Some((&output, candidate_bytes, None)),
                FidelityStatus::NotEvaluated,
                Vec::new(),
                Vec::new(),
                SqzFailureReason::OutputExceedsBound,
            );
        }
        let candidate_id = Some(content_id(&output.payload));

        let (fidelity_status, missing_fidelity_anchor_ids) =
            fidelity_status(&output.payload, &anchors.anchors);
        let findings = dlp_findings(&output.payload);
        if fidelity_status == FidelityStatus::RequiredButUnavailable {
            return self.failed(
                packet,
                requested_max_output_bytes,
                effective_max_output_bytes,
                returned_identity,
                latency,
                Some((&output, candidate_bytes, candidate_id)),
                fidelity_status,
                missing_fidelity_anchor_ids,
                findings,
                SqzFailureReason::FidelityAnchorsUnavailable,
            );
        }
        if fidelity_status == FidelityStatus::Missing {
            return self.failed(
                packet,
                requested_max_output_bytes,
                effective_max_output_bytes,
                returned_identity,
                latency,
                Some((&output, candidate_bytes, candidate_id)),
                fidelity_status,
                missing_fidelity_anchor_ids,
                findings,
                SqzFailureReason::MissingFidelityAnchors,
            );
        }
        if !findings.is_empty() {
            return self.failed(
                packet,
                requested_max_output_bytes,
                effective_max_output_bytes,
                returned_identity,
                latency,
                Some((&output, candidate_bytes, candidate_id)),
                fidelity_status,
                missing_fidelity_anchor_ids,
                findings,
                SqzFailureReason::DlpFindings,
            );
        }
        if candidate_bytes >= raw_bytes {
            let returned_id = Some(content_id(&packet.payload));
            let receipt = self.receipt(
                raw_bytes,
                raw_bytes,
                requested_max_output_bytes,
                effective_max_output_bytes,
                returned_identity,
                latency,
                Some((&output, candidate_bytes, returned_id)),
                SqzStatus::NotBeneficial,
                None,
                fidelity_status,
                anchor_ids(&anchors.anchors),
                missing_fidelity_anchor_ids,
                findings,
            );
            return Ok(SqzEvaluation { packet, receipt });
        }

        let receipt = self.receipt(
            raw_bytes,
            candidate_bytes,
            requested_max_output_bytes,
            effective_max_output_bytes,
            returned_identity,
            latency,
            Some((&output, candidate_bytes, candidate_id)),
            SqzStatus::Applied,
            None,
            fidelity_status,
            anchor_ids(&anchors.anchors),
            missing_fidelity_anchor_ids,
            findings,
        );
        Ok(SqzEvaluation {
            packet: ContextPacket {
                payload: output.payload,
                ..packet
            },
            receipt,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn failed(
        &self,
        packet: ContextPacket,
        requested_max_output_bytes: usize,
        effective_max_output_bytes: usize,
        returned_identity: Option<SqzIdentity>,
        latency: Duration,
        candidate: Option<(&SqzPortOutput, usize, Option<SqzId>)>,
        fidelity_status: FidelityStatus,
        missing_fidelity_anchor_ids: Vec<String>,
        dlp_findings: Vec<String>,
        failure_reason: SqzFailureReason,
    ) -> Result<SqzEvaluation, SqzError> {
        let raw_bytes = packet.payload.len();
        let required_fidelity_anchor_ids =
            anchor_ids(&combined_anchors(&packet, &self.config.fidelity_anchors).anchors);
        let receipt = self.receipt(
            raw_bytes,
            raw_bytes,
            requested_max_output_bytes,
            effective_max_output_bytes,
            returned_identity,
            latency,
            candidate,
            SqzStatus::Failed,
            Some(failure_reason),
            fidelity_status,
            required_fidelity_anchor_ids,
            missing_fidelity_anchor_ids,
            dlp_findings,
        );
        if self.config.policy == SqzPolicy::InternalLocked {
            Err(SqzError {
                receipt: Box::new(receipt),
            })
        } else {
            Ok(SqzEvaluation { packet, receipt })
        }
    }

    fn base_receipt(
        &self,
        packet: &ContextPacket,
        raw_bytes: usize,
        returned_bytes: usize,
        requested_max_output_bytes: usize,
        effective_max_output_bytes: usize,
    ) -> SqzReceipt {
        let required_fidelity_anchor_ids =
            anchor_ids(&combined_anchors(packet, &self.config.fidelity_anchors).anchors);
        self.receipt(
            raw_bytes,
            returned_bytes,
            requested_max_output_bytes,
            effective_max_output_bytes,
            None,
            Duration::ZERO,
            None,
            SqzStatus::Off,
            None,
            FidelityStatus::Passed,
            required_fidelity_anchor_ids,
            Vec::new(),
            Vec::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn receipt(
        &self,
        raw_bytes: usize,
        returned_bytes: usize,
        requested_max_output_bytes: usize,
        effective_max_output_bytes: usize,
        returned_identity: Option<SqzIdentity>,
        monotonic_call_latency: Duration,
        candidate: Option<(&SqzPortOutput, usize, Option<SqzId>)>,
        status: SqzStatus,
        failure_reason: Option<SqzFailureReason>,
        fidelity_status: FidelityStatus,
        required_fidelity_anchor_ids: Vec<String>,
        missing_fidelity_anchor_ids: Vec<String>,
        dlp_findings: Vec<String>,
    ) -> SqzReceipt {
        let candidate_compressed_bytes = candidate.as_ref().map(|(_, bytes, _)| *bytes);
        let candidate_token_estimate_bytes_divided_by_four_ceiling =
            candidate_compressed_bytes.map(estimate_tokens);
        let actual_input_tokens = candidate
            .as_ref()
            .and_then(|(output, _, _)| output.actual_input_tokens);
        let actual_output_tokens = candidate
            .as_ref()
            .and_then(|(output, _, _)| output.actual_output_tokens);
        let sqz_id = matches!(status, SqzStatus::Applied | SqzStatus::NotBeneficial)
            .then(|| candidate.and_then(|(_, _, id)| id))
            .flatten();
        let dlp_status = if !dlp_findings.is_empty() {
            DlpStatus::Findings
        } else if matches!(
            failure_reason.as_ref(),
            Some(SqzFailureReason::ConfiguredIdentityMismatch)
                | Some(SqzFailureReason::ConflictingFidelityAnchorIds)
        ) {
            DlpStatus::NotEvaluated
        } else if matches!(
            failure_reason.as_ref(),
            Some(SqzFailureReason::PortUnavailable(_))
                | Some(SqzFailureReason::ReturnedIdentityMismatch)
                | Some(SqzFailureReason::OutputExceedsBound)
        ) {
            DlpStatus::InputPassed
        } else {
            DlpStatus::Passed
        };
        SqzReceipt {
            schema_version: SQZ_RECEIPT_SCHEMA_VERSION,
            configured_identity: self.config.identity.clone(),
            returned_identity,
            policy: self.config.policy,
            status,
            failure_reason,
            monotonic_call_latency,
            raw_bytes,
            candidate_compressed_bytes,
            returned_bytes,
            raw_token_estimate_bytes_divided_by_four_ceiling: estimate_tokens(raw_bytes),
            candidate_token_estimate_bytes_divided_by_four_ceiling,
            returned_token_estimate_bytes_divided_by_four_ceiling: estimate_tokens(returned_bytes),
            actual_input_tokens,
            actual_output_tokens,
            fidelity_status,
            required_fidelity_anchor_ids,
            missing_fidelity_anchor_ids,
            dlp_status,
            dlp_findings,
            requested_max_output_bytes,
            effective_max_output_bytes,
            sqz_id,
        }
    }
}

fn estimate_tokens(bytes: usize) -> usize {
    bytes / 4 + usize::from(!bytes.is_multiple_of(4))
}

fn fidelity_status(output: &str, anchors: &[PreservationAnchor]) -> (FidelityStatus, Vec<String>) {
    if anchors.is_empty() {
        return (FidelityStatus::RequiredButUnavailable, Vec::new());
    }
    let missing = anchors
        .iter()
        .filter(|anchor| !output.contains(anchor.value()))
        .map(|anchor| anchor.id().to_owned())
        .collect::<Vec<_>>();
    if missing.is_empty() {
        (FidelityStatus::Passed, missing)
    } else {
        (FidelityStatus::Missing, missing)
    }
}

struct CombinedAnchors {
    anchors: Vec<PreservationAnchor>,
    conflicting_ids: bool,
}

fn combined_anchors(packet: &ContextPacket, configured: &[PreservationAnchor]) -> CombinedAnchors {
    let mut anchors = BTreeMap::<String, PreservationAnchor>::new();
    let mut conflicting_ids = false;
    for anchor in configured.iter().chain(
        packet
            .items
            .iter()
            .filter(|item| item.priority == EvidencePriority::Required)
            .flat_map(|item| item.preservation_anchors.iter()),
    ) {
        if let Some(existing) = anchors.get(anchor.id()) {
            if existing.value() != anchor.value() {
                conflicting_ids = true;
            }
        } else {
            anchors.insert(anchor.id().to_owned(), anchor.clone());
        }
    }
    CombinedAnchors {
        anchors: anchors.into_values().collect(),
        conflicting_ids,
    }
}

fn anchor_ids(anchors: &[PreservationAnchor]) -> Vec<String> {
    anchors
        .iter()
        .map(|anchor| anchor.id().to_owned())
        .collect()
}

fn content_id(output: &str) -> SqzId {
    let id = ResultId::sha256(output.as_bytes());
    SqzId {
        algorithm: id.algorithm(),
        value: id.value().to_owned(),
    }
}

fn dlp_findings(value: &str) -> Vec<String> {
    let lower = value.to_ascii_lowercase();
    let mut findings = Vec::new();
    if lower.contains("api_key=")
        || lower.contains("api_key:")
        || lower.contains("token=")
        || lower.contains("token:")
        || lower.contains("secret=")
        || lower.contains("secret:")
        || lower.contains("password=")
        || lower.contains("password:")
        || lower.contains("\"api_key\":")
        || lower.contains("\"token\":")
        || lower.contains("\"secret\":")
        || lower.contains("\"password\":")
    {
        findings.push("credential_assignment".to_owned());
    }
    if value.contains("/home/") {
        findings.push("private_home_path".to_owned());
    }
    let prefixed_token = lower
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        })
        .any(|candidate| {
            candidate.len() >= 12
                && ["sk-", "sk_", "xoxb-", "xoxp-", "glpat-", "github_pat_"]
                    .iter()
                    .any(|prefix| candidate.starts_with(prefix))
        });
    if lower.contains("bearer ")
        || lower.contains("ghp_")
        || lower.contains("gho_")
        || prefixed_token
    {
        findings.push("bearer_or_token".to_owned());
    }
    if value.contains("-----BEGIN PRIVATE KEY-----") {
        findings.push("private_key".to_owned());
    }
    findings
}

/// Applies the same deterministic public-boundary DLP checks to exact bytes.
/// Callers reject findings and never rewrite machine-critical artifacts.
pub(crate) fn public_dlp_findings(bytes: &[u8]) -> Vec<String> {
    dlp_findings(&String::from_utf8_lossy(bytes))
}

/// Deterministic, content-free public-boundary decision for descriptors.
pub fn is_public_dlp_safe(value: &str) -> bool {
    dlp_findings(value).is_empty()
}
