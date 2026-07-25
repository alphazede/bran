//! Strict, std-only external-host v1 provider adapter.
//!
//! Portable `std` cannot atomically bind a path digest to `exec`; verification
//! immediately before and after spawn narrows, but does not eliminate, TOCTOU.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufReader, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::OnceLock;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use crate::agent::result_store::ResultId;
use crate::agent::runtime::{
    ArtifactKind, LosslessArtifact, ProviderError, ProviderExecutionEvidence, ProviderOutput,
    ProviderPort, ProviderRequest, ProviderTokenUsage,
};

const MAX_FRAME: usize = 4 * 1024 * 1024;
const MAX_STDOUT: usize = 2 * MAX_FRAME;
const MAX_STDERR: usize = 64 * 1024;
const MAX_LENGTH_LINE: usize = 8;
const MAX_EXECUTABLE: u64 = 256 * 1024 * 1024;
const DEFAULT_EXTERNAL_HOST_TIMEOUT_SECS: u64 = 30;
const MAX_EXTERNAL_HOST_TIMEOUT_SECS: u64 = 600;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafeAccountReference(String);

impl SafeAccountReference {
    pub fn new(value: impl Into<String>) -> Result<Self, ProviderError> {
        let value = value.into();
        if !valid_name(&value) {
            return Err(ProviderError::Unavailable);
        }
        Ok(Self(value))
    }
    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub struct ExternalHostAdapter {
    executable: PathBuf,
    expected_sha256: String,
    argv: Vec<String>,
    effective_profile: Option<String>,
    timeout: Duration,
    cancellation: Option<Arc<AtomicBool>>,
    #[cfg(test)]
    fixture_mode: bool,
}

impl ExternalHostAdapter {
    pub fn new(
        executable: impl Into<PathBuf>,
        expected_sha256: impl Into<String>,
        argv: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ProviderError> {
        let executable = executable.into();
        let expected_sha256 = expected_sha256.into();
        let argv: Vec<String> = argv.into_iter().map(Into::into).collect();
        if !executable.is_absolute()
            || !valid_sha256(&expected_sha256)
            || argv
                .iter()
                .any(|arg| arg.is_empty() || arg.len() > 256 || arg.contains('\0'))
        {
            return Err(ProviderError::Unavailable);
        }
        let adapter = Self {
            executable,
            expected_sha256,
            argv,
            effective_profile: None,
            timeout: Duration::from_secs(DEFAULT_EXTERNAL_HOST_TIMEOUT_SECS),
            cancellation: None,
            #[cfg(test)]
            fixture_mode: false,
        };
        adapter.verify_executable(None, None)?;
        Ok(adapter)
    }

    pub fn with_profile(mut self, profile: impl Into<String>) -> Result<Self, ProviderError> {
        let profile = profile.into();
        if !valid_name(&profile) {
            return Err(ProviderError::Unavailable);
        }
        self.effective_profile = Some(profile);
        Ok(self)
    }

    pub fn with_cancellation(mut self, cancellation: Arc<AtomicBool>) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    pub fn with_timeout_secs(mut self, seconds: u64) -> Result<Self, ProviderError> {
        if !valid_timeout_secs(seconds) {
            return Err(ProviderError::Unavailable);
        }
        self.timeout = Duration::from_secs(seconds);
        Ok(self)
    }

    #[cfg(test)]
    pub fn current_test_fixture(timeout: Duration) -> Result<Self, ProviderError> {
        let (executable, expected_sha256) = current_test_fixture_identity()?;
        Ok(Self {
            executable,
            expected_sha256,
            argv: [
                "agent::tests::p3_agent_profile_contract",
                "--exact",
                "--nocapture",
                "--quiet",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            effective_profile: None,
            timeout,
            cancellation: None,
            fixture_mode: true,
        })
    }

    fn verify_executable(
        &self,
        deadline: Option<Instant>,
        cancellation: Option<&AtomicBool>,
    ) -> Result<(), ProviderError> {
        #[cfg(test)]
        if self.fixture_mode {
            return Ok(());
        }
        let actual = match deadline {
            Some(deadline) => {
                verified_executable_sha256_by_cancel(&self.executable, deadline, cancellation)?
            }
            None => verified_executable_sha256(&self.executable)?,
        };
        if actual != self.expected_sha256 {
            return Err(ProviderError::Unavailable);
        }
        Ok(())
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .args(&self.argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();
        #[cfg(test)]
        if self.fixture_mode {
            command.env("BRAN_P3_EXTERNAL_HOST_FIXTURE", "1");
        }
        command
    }
}

fn valid_timeout_secs(seconds: u64) -> bool {
    (1..=MAX_EXTERNAL_HOST_TIMEOUT_SECS).contains(&seconds)
}

impl ProviderPort<SafeAccountReference> for ExternalHostAdapter {
    fn invoke(
        &self,
        request: &ProviderRequest,
        account: &SafeAccountReference,
    ) -> Result<ProviderOutput, ProviderError> {
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or(ProviderError::Timeout)?;
        let canonical = request_payload(request, account);
        let digest = ResultId::sha256(&canonical).value().to_owned();
        let cancellation = self.cancellation.as_deref();
        before_cancel(deadline, cancellation)?;
        self.verify_executable(Some(deadline), cancellation)?;
        before_cancel(deadline, cancellation)?;
        let mut child = self
            .command()
            .spawn()
            .map_err(|_| ProviderError::Unavailable)?;
        if let Err(error) = before_cancel(deadline, cancellation)
            .and_then(|_| self.verify_executable(Some(deadline), cancellation))
        {
            terminate(child);
            return Err(error);
        }
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            terminate(child);
            return Err(ProviderError::Unavailable);
        };
        let (frames_tx, frames_rx) = mpsc::channel();
        #[cfg(test)]
        {
            let fixture_mode = self.fixture_mode;
            thread::spawn(move || drain_frames(stdout, frames_tx, fixture_mode))
        };
        #[cfg(not(test))]
        thread::spawn(move || drain_frames(stdout, frames_tx));
        let (stderr_tx, stderr_rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = stderr_tx.send(drain(stderr, MAX_STDERR));
        });
        let (writes_tx, writes_rx) = mpsc::channel::<Vec<u8>>();
        let (write_results_tx, write_results_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut stdin = stdin;
            while let Ok(payload) = writes_rx.recv() {
                let result = write_frame(&mut stdin, &payload);
                let failed = result.is_err();
                if write_results_tx.send(result).is_err() || failed {
                    return;
                }
            }
        });
        let result = (|| {
            write_by(
                &writes_tx,
                &write_results_rx,
                canonical,
                deadline,
                cancellation,
            )?;
            let preflight = receive_cancellable(
                &frames_rx,
                deadline,
                ProviderError::InvalidOutput,
                cancellation,
            )??;
            #[cfg(not(test))]
            let legacy_test_fixture = false;
            #[cfg(test)]
            let legacy_test_fixture = self.fixture_mode;
            let preflight =
                parse_preflight(&preflight, request, account, &digest, legacy_test_fixture)?;
            let execute = execute_payload(&digest, &preflight, request);
            write_by(
                &writes_tx,
                &write_results_rx,
                execute,
                deadline,
                cancellation,
            )?;
            drop(writes_tx);
            let output = receive_cancellable(
                &frames_rx,
                deadline,
                ProviderError::InvalidOutput,
                cancellation,
            )??;
            parse_result(
                &output,
                request,
                account,
                &digest,
                &preflight,
                self.effective_profile.as_deref(),
                legacy_test_fixture,
            )
        })();
        let output = match result {
            Ok(output) => output,
            Err(error) => {
                terminate(child);
                return Err(error);
            }
        };
        let exit = match wait_until(&mut child, &frames_rx, &stderr_rx, deadline, cancellation) {
            Ok(exit) => exit,
            Err(error) => {
                terminate(child);
                return Err(error);
            }
        };
        if !exit.success() {
            return Err(ProviderError::Failed);
        }
        Ok(output)
    }
}

fn before(deadline: Instant) -> Result<(), ProviderError> {
    if Instant::now() < deadline {
        Ok(())
    } else {
        Err(ProviderError::Timeout)
    }
}

fn before_cancel(
    deadline: Instant,
    cancellation: Option<&AtomicBool>,
) -> Result<(), ProviderError> {
    if cancellation.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        Err(ProviderError::Cancelled)
    } else {
        before(deadline)
    }
}

fn write_by(
    writes: &mpsc::Sender<Vec<u8>>,
    results: &mpsc::Receiver<Result<(), ProviderError>>,
    payload: Vec<u8>,
    deadline: Instant,
    cancellation: Option<&AtomicBool>,
) -> Result<(), ProviderError> {
    before_cancel(deadline, cancellation)?;
    writes.send(payload).map_err(|_| ProviderError::Failed)?;
    receive_cancellable(results, deadline, ProviderError::Failed, cancellation)?
}

fn receive_cancellable<T>(
    rx: &mpsc::Receiver<T>,
    deadline: Instant,
    disconnected: ProviderError,
    cancellation: Option<&AtomicBool>,
) -> Result<T, ProviderError> {
    loop {
        before_cancel(deadline, cancellation)?;
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(ProviderError::Timeout)?;
        match rx.recv_timeout(remaining.min(Duration::from_millis(10))) {
            Ok(value) => return Ok(value),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(disconnected),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn wait_until(
    child: &mut Child,
    frames: &mpsc::Receiver<Result<Vec<u8>, ProviderError>>,
    stderr: &mpsc::Receiver<Result<(), ProviderError>>,
    deadline: Instant,
    cancellation: Option<&AtomicBool>,
) -> Result<ExitStatus, ProviderError> {
    loop {
        before_cancel(deadline, cancellation)?;
        check_streams(frames, stderr)?;
        match child.try_wait() {
            Ok(Some(status)) => {
                finish_streams(frames, stderr, deadline)?;
                return Ok(status);
            }
            Err(_) => return Err(ProviderError::Failed),
            Ok(None) => {}
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Err(ProviderError::Timeout);
        };
        thread::sleep(remaining.min(Duration::from_millis(1)));
    }
}

fn finish_streams(
    frames: &mpsc::Receiver<Result<Vec<u8>, ProviderError>>,
    stderr: &mpsc::Receiver<Result<(), ProviderError>>,
    deadline: Instant,
) -> Result<(), ProviderError> {
    match receive_until_disconnect(frames, deadline)? {
        Some(Err(error)) => return Err(error),
        Some(Ok(_)) => return Err(ProviderError::InvalidOutput),
        None => {}
    }
    loop {
        match receive_until_disconnect(stderr, deadline)? {
            Some(Err(error)) => return Err(error),
            Some(Ok(())) => {}
            None => return Ok(()),
        }
    }
}

fn receive_until_disconnect<T>(
    rx: &mpsc::Receiver<T>,
    deadline: Instant,
) -> Result<Option<T>, ProviderError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or(ProviderError::Timeout)?;
    match rx.recv_timeout(remaining) {
        Ok(value) => Ok(Some(value)),
        Err(mpsc::RecvTimeoutError::Disconnected) => Ok(None),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(ProviderError::Timeout),
    }
}

fn check_streams(
    frames: &mpsc::Receiver<Result<Vec<u8>, ProviderError>>,
    stderr: &mpsc::Receiver<Result<(), ProviderError>>,
) -> Result<(), ProviderError> {
    match frames.try_recv() {
        Ok(Err(error)) => return Err(error),
        Ok(Ok(_)) => return Err(ProviderError::InvalidOutput),
        Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => {}
    }
    match stderr.try_recv() {
        Ok(Err(error)) => Err(error),
        Ok(Ok(())) | Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => Ok(()),
    }
}

fn terminate(mut child: Child) {
    let _ = child.kill();
    // Only the direct configured host is killed. Reaping is detached so
    // descendants retaining inherited pipes cannot hold the caller open.
    thread::spawn(move || {
        let _ = child.wait();
    });
}

fn drain_frames<R: Read>(
    reader: R,
    tx: mpsc::Sender<Result<Vec<u8>, ProviderError>>,
    #[cfg(test)] fixture_mode: bool,
) {
    let mut reader = BufReader::new(reader);
    #[cfg(test)]
    if fixture_mode {
        if let Err(error) = fixture_banner(&mut reader) {
            let _ = tx.send(Err(error));
            return;
        }
    }
    let mut total = 0;
    let mut count = 0;
    loop {
        match read_frame(&mut reader) {
            Ok(Some(frame)) => {
                count += 1;
                if count > 2 {
                    let _ = tx.send(Err(ProviderError::InvalidOutput));
                    return;
                }
                total += frame.len();
                if total > MAX_STDOUT {
                    let _ = tx.send(Err(ProviderError::InvalidOutput));
                    return;
                }
                if tx.send(Ok(frame)).is_err() {
                    return;
                }
                #[cfg(test)]
                if fixture_mode && count == 2 {
                    if let Err(error) = drain(reader, 4096) {
                        let _ = tx.send(Err(error));
                    }
                    return;
                }
            }
            Ok(None) if count == 2 => return,
            Ok(None) => {
                let _ = tx.send(Err(ProviderError::InvalidOutput));
                return;
            }
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        }
    }
}

#[cfg(test)]
fn fixture_banner<R: Read>(reader: &mut R) -> Result<(), ProviderError> {
    let mut line = [0; 16];
    let mut len = read_bounded_line(reader, &mut line)?.ok_or(ProviderError::InvalidOutput)?;
    if &line[..len] == b"\n" {
        len = read_bounded_line(reader, &mut line)?.ok_or(ProviderError::InvalidOutput)?;
    }
    if &line[..len] == b"running 1 test\n" {
        Ok(())
    } else {
        Err(ProviderError::InvalidOutput)
    }
}

fn drain<R: Read>(mut reader: R, max: usize) -> Result<(), ProviderError> {
    let mut total = 0;
    let mut bytes = [0; 4096];
    loop {
        let n = reader.read(&mut bytes).map_err(|_| ProviderError::Failed)?;
        if n == 0 {
            return Ok(());
        }
        total += n;
        if total > max {
            return Err(ProviderError::InvalidOutput);
        }
    }
}
fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), ProviderError> {
    if payload.len() > MAX_FRAME {
        return Err(ProviderError::InvalidOutput);
    }
    writer
        .write_all(payload.len().to_string().as_bytes())
        .and_then(|_| writer.write_all(b"\n"))
        .and_then(|_| writer.write_all(payload))
        .and_then(|_| writer.write_all(b"\n"))
        .and_then(|_| writer.flush())
        .map_err(|_| ProviderError::Failed)
}
fn read_frame<R: Read>(reader: &mut R) -> Result<Option<Vec<u8>>, ProviderError> {
    let mut line = [0; MAX_LENGTH_LINE];
    let Some(n) = read_bounded_line(reader, &mut line)? else {
        return Ok(None);
    };
    let len =
        number(std::str::from_utf8(&line[..n - 1]).map_err(|_| ProviderError::InvalidOutput)?)?;
    if len > MAX_FRAME {
        return Err(ProviderError::InvalidOutput);
    }
    let mut payload = vec![0; len];
    read_protocol_exact(reader, &mut payload)?;
    let mut term = [0];
    read_protocol_exact(reader, &mut term)?;
    if term != [b'\n'] {
        return Err(ProviderError::InvalidOutput);
    }
    Ok(Some(payload))
}

fn read_bounded_line<R: Read>(
    reader: &mut R,
    line: &mut [u8],
) -> Result<Option<usize>, ProviderError> {
    for index in 0..line.len() {
        match reader.read(&mut line[index..=index]) {
            Ok(0) if index == 0 => return Ok(None),
            Ok(0) => return Err(ProviderError::InvalidOutput),
            Err(_) => return Err(ProviderError::Failed),
            Ok(_) if line[index] == b'\n' => return Ok(Some(index + 1)),
            Ok(_) => {}
        }
    }
    Err(ProviderError::InvalidOutput)
}

fn read_protocol_exact<R: Read>(reader: &mut R, bytes: &mut [u8]) -> Result<(), ProviderError> {
    reader.read_exact(bytes).map_err(|error| {
        if error.kind() == ErrorKind::UnexpectedEof {
            ProviderError::InvalidOutput
        } else {
            ProviderError::Failed
        }
    })
}

#[derive(Clone)]
struct Preflight {
    id: String,
    no_session: bool,
    input: Option<usize>,
    max_output: Option<usize>,
    complete: Option<usize>,
    ceiling: Option<usize>,
}
fn optional_number(value: Option<usize>) -> String {
    value.map_or_else(|| "unavailable".to_owned(), |value| value.to_string())
}
fn request_payload(request: &ProviderRequest, account: &SafeAccountReference) -> Vec<u8> {
    fields(&[
        ("stage", "preflight"),
        ("provider", request.provider()),
        ("model", request.model()),
        ("reasoning", request.requested().as_str()),
        ("tool_policy", &tool_policy(request)),
        ("account_ref", account.as_str()),
        ("no_session", &request.no_session().to_string()),
        ("max_output_bytes", &request.max_output_bytes().to_string()),
        (
            "hard_total_token_ceiling",
            &optional_number(request.hard_total_token_ceiling()),
        ),
        (
            "max_output_tokens",
            &optional_number(request.max_output_tokens()),
        ),
        ("prompt", request.prompt()),
    ])
}
fn execute_payload(digest: &str, preflight: &Preflight, request: &ProviderRequest) -> Vec<u8> {
    fields(&[
        ("stage", "execute"),
        ("request_digest", digest),
        ("preflight_id", &preflight.id),
        ("no_session", &preflight.no_session.to_string()),
        ("input_tokens", &optional_number(preflight.input)),
        ("max_output_tokens", &optional_number(preflight.max_output)),
        (
            "exact_complete_tokens",
            &optional_number(preflight.complete),
        ),
        (
            "hard_total_token_ceiling",
            &optional_number(request.hard_total_token_ceiling()),
        ),
    ])
}
fn fields(items: &[(&str, &str)]) -> Vec<u8> {
    let mut result = b"v1\n".to_vec();
    for (key, value) in items {
        result.extend_from_slice(key.as_bytes());
        result.push(b'\n');
        result.extend_from_slice(value.len().to_string().as_bytes());
        result.push(b'\n');
        result.extend_from_slice(value.as_bytes());
    }
    result
}
fn parse(payload: &[u8]) -> Result<BTreeMap<String, Vec<String>>, ProviderError> {
    let mut rest = payload
        .strip_prefix(b"v1\n")
        .ok_or(ProviderError::InvalidOutput)?;
    let mut values = BTreeMap::new();
    while !rest.is_empty() {
        let key_end = rest
            .iter()
            .position(|b| *b == b'\n')
            .ok_or(ProviderError::InvalidOutput)?;
        let key =
            std::str::from_utf8(&rest[..key_end]).map_err(|_| ProviderError::InvalidOutput)?;
        if !valid_key(key) {
            return Err(ProviderError::InvalidOutput);
        }
        rest = &rest[key_end + 1..];
        let n_end = rest
            .iter()
            .position(|b| *b == b'\n')
            .ok_or(ProviderError::InvalidOutput)?;
        let len =
            number(std::str::from_utf8(&rest[..n_end]).map_err(|_| ProviderError::InvalidOutput)?)?;
        rest = &rest[n_end + 1..];
        if len > rest.len() {
            return Err(ProviderError::InvalidOutput);
        }
        let value = std::str::from_utf8(&rest[..len])
            .map_err(|_| ProviderError::InvalidOutput)?
            .to_owned();
        rest = &rest[len..];
        values
            .entry(key.to_owned())
            .or_insert_with(Vec::new)
            .push(value);
    }
    Ok(values)
}
fn strict(
    values: &BTreeMap<String, Vec<String>>,
    allowed: &[&str],
    required: &[&str],
    repeated: &[&str],
) -> Result<(), ProviderError> {
    if values.keys().any(|key| !allowed.contains(&key.as_str()))
        || required
            .iter()
            .any(|key| values.get(*key).is_none_or(|value| value.len() != 1))
        || values.iter().any(|(key, value)| {
            if repeated.contains(&key.as_str()) {
                value.len() > 64
            } else {
                value.len() != 1
            }
        })
    {
        return Err(ProviderError::InvalidOutput);
    }
    Ok(())
}
fn value<'a>(
    values: &'a BTreeMap<String, Vec<String>>,
    key: &str,
) -> Result<&'a str, ProviderError> {
    values
        .get(key)
        .and_then(|v| v.first())
        .map(String::as_str)
        .ok_or(ProviderError::InvalidOutput)
}
fn same(
    values: &BTreeMap<String, Vec<String>>,
    key: &str,
    expected: &str,
) -> Result<(), ProviderError> {
    if value(values, key)? == expected {
        Ok(())
    } else {
        Err(ProviderError::InvalidOutput)
    }
}
fn parse_preflight(
    payload: &[u8],
    request: &ProviderRequest,
    account: &SafeAccountReference,
    digest: &str,
    legacy_test_fixture: bool,
) -> Result<Preflight, ProviderError> {
    let values = parse(payload)?;
    let names = [
        "stage",
        "status",
        "provider",
        "model",
        "reasoning",
        "tool_policy",
        "account_ref",
        "request_digest",
        "preflight_id",
        "no_session",
        "input_tokens",
        "max_output_tokens",
        "exact_complete_tokens",
        "hard_total_token_ceiling",
    ];
    let mut required = names.to_vec();
    if legacy_test_fixture {
        required.retain(|name| *name != "no_session");
    }
    strict(&values, &names, &required, &[])?;
    if value(&values, "status")? != "supported" {
        return Err(ProviderError::Unavailable);
    }
    same(&values, "stage", "preflight-result")?;
    same(&values, "provider", request.provider())?;
    same(&values, "model", request.model())?;
    same(&values, "reasoning", request.requested().as_str())?;
    same(&values, "tool_policy", &tool_policy(request))?;
    same(&values, "account_ref", account.as_str())?;
    same(&values, "request_digest", digest)?;
    if values.contains_key("no_session") {
        same(&values, "no_session", &request.no_session().to_string())?;
    }
    let id = value(&values, "preflight_id")?.to_owned();
    if !valid_name(&id) {
        return Err(ProviderError::InvalidOutput);
    }
    let max_output = match value(&values, "max_output_tokens")? {
        "unavailable" => None,
        value => Some(number(value)?),
    };
    let ceiling = match value(&values, "hard_total_token_ceiling")? {
        "unavailable" => None,
        value => Some(number(value)?),
    };
    if ceiling != request.hard_total_token_ceiling()
        || request
            .max_output_tokens()
            .is_some_and(|requested| max_output != Some(requested))
    {
        return Err(ProviderError::Unavailable);
    }
    let (input, complete) = match ceiling {
        Some(ceiling) => match max_output {
            Some(max_output) => {
                let input = number(value(&values, "input_tokens")?)?;
                let complete = number(value(&values, "exact_complete_tokens")?)?;
                if input == 0
                    || complete
                        != input
                            .checked_add(max_output)
                            .ok_or(ProviderError::InvalidOutput)?
                    || complete > ceiling
                {
                    return Err(ProviderError::Unavailable);
                }
                (Some(input), Some(complete))
            }
            None => {
                if value(&values, "exact_complete_tokens")? != "unavailable" {
                    return Err(ProviderError::Unavailable);
                }
                let input = match value(&values, "input_tokens")? {
                    "unavailable" => None,
                    value => Some(number(value)?),
                };
                (input, None)
            }
        },
        None => {
            if max_output.is_some() || value(&values, "exact_complete_tokens")? != "unavailable" {
                return Err(ProviderError::Unavailable);
            }
            let input = match value(&values, "input_tokens")? {
                "unavailable" => None,
                value => Some(number(value)?),
            };
            (input, None)
        }
    };
    Ok(Preflight {
        id,
        no_session: request.no_session(),
        input,
        max_output,
        complete,
        ceiling,
    })
}
fn parse_result(
    payload: &[u8],
    request: &ProviderRequest,
    account: &SafeAccountReference,
    digest: &str,
    preflight: &Preflight,
    effective_profile: Option<&str>,
    legacy_test_fixture: bool,
) -> Result<ProviderOutput, ProviderError> {
    let values = parse(payload)?;
    let names = [
        "stage",
        "status",
        "request_digest",
        "preflight_id",
        "provider",
        "model",
        "reasoning",
        "tool_policy",
        "account_ref",
        "no_session",
        "input_tokens",
        "max_output_tokens",
        "exact_complete_tokens",
        "hard_total_token_ceiling",
        "actual_input_tokens",
        "actual_output_tokens",
        "answer",
        "citation",
        "provider_run_id",
        "effective_provider",
        "effective_model",
        "effective_reasoning",
        "artifact_kind",
        "artifact_media_type",
        "artifact_id",
        "artifact_bytes",
    ];
    let required = [
        "stage",
        "status",
        "request_digest",
        "preflight_id",
        "provider",
        "model",
        "reasoning",
        "tool_policy",
        "account_ref",
        "no_session",
        "input_tokens",
        "max_output_tokens",
        "exact_complete_tokens",
        "hard_total_token_ceiling",
        "actual_input_tokens",
        "actual_output_tokens",
        "answer",
        "provider_run_id",
    ];
    let mut required = required.to_vec();
    if legacy_test_fixture {
        required.retain(|name| *name != "no_session");
    } else {
        required.extend([
            "effective_provider",
            "effective_model",
            "effective_reasoning",
        ]);
    }
    strict(
        &values,
        &names,
        &required,
        &[
            "citation",
            "artifact_kind",
            "artifact_media_type",
            "artifact_id",
            "artifact_bytes",
        ],
    )?;
    same(&values, "stage", "result")?;
    same(&values, "status", "complete")?;
    same(&values, "request_digest", digest)?;
    same(&values, "preflight_id", &preflight.id)?;
    same(&values, "provider", request.provider())?;
    same(&values, "model", request.model())?;
    same(&values, "reasoning", request.requested().as_str())?;
    same(&values, "tool_policy", &tool_policy(request))?;
    same(&values, "account_ref", account.as_str())?;
    if values.contains_key("no_session") {
        same(&values, "no_session", &preflight.no_session.to_string())?;
    }
    same(&values, "input_tokens", &optional_number(preflight.input))?;
    same(
        &values,
        "max_output_tokens",
        &optional_number(preflight.max_output),
    )?;
    same(
        &values,
        "exact_complete_tokens",
        &optional_number(preflight.complete),
    )?;
    same(
        &values,
        "hard_total_token_ceiling",
        &optional_number(preflight.ceiling),
    )?;
    let actual = |key| match value(&values, key)? {
        "unavailable" => Ok(None),
        value => Ok(Some(number(value)?)),
    };
    let input = actual("actual_input_tokens")?;
    let output = actual("actual_output_tokens")?;
    let needs_input =
        preflight.input.is_some() || preflight.complete.is_some() || preflight.ceiling.is_some();
    let needs_output = preflight.max_output.is_some()
        || preflight.complete.is_some()
        || preflight.ceiling.is_some();
    if needs_input && input.is_none() || needs_output && output.is_none() {
        return Err(ProviderError::InvalidOutput);
    }
    let actual_complete = match (input, output) {
        (Some(input), Some(output)) => Some(
            input
                .checked_add(output)
                .ok_or(ProviderError::InvalidOutput)?,
        ),
        _ => None,
    };
    if preflight
        .max_output
        .zip(output)
        .is_some_and(|(maximum, output)| output > maximum)
        || preflight
            .input
            .zip(input)
            .is_some_and(|(expected, input)| input != expected)
        || preflight
            .complete
            .zip(actual_complete)
            .is_some_and(|(complete, actual_complete)| actual_complete > complete)
        || preflight
            .ceiling
            .zip(actual_complete)
            .is_some_and(|(ceiling, actual_complete)| actual_complete > ceiling)
    {
        return Err(ProviderError::InvalidOutput);
    }
    let answer = value(&values, "answer")?;
    if answer.len() > request.max_output_bytes() {
        return Err(ProviderError::InvalidOutput);
    }
    let citations = values.get("citation").cloned().unwrap_or_default();
    let artifacts = parse_artifacts(&values)?;
    let effective = |key: &str, legacy: &str| -> Result<Option<String>, ProviderError> {
        let reported = values
            .get(key)
            .and_then(|items| items.first())
            .map(String::as_str)
            .or(legacy_test_fixture.then_some(legacy))
            .ok_or(ProviderError::InvalidOutput)?;
        if reported == "unavailable" {
            Ok(None)
        } else if valid_name(reported) {
            Ok(Some(reported.to_owned()))
        } else {
            Err(ProviderError::InvalidOutput)
        }
    };
    let effective_provider = effective("effective_provider", request.provider())?;
    let effective_model = effective("effective_model", request.model())?;
    let effective_reasoning = effective("effective_reasoning", request.requested().as_str())?;
    let evidence = ProviderExecutionEvidence::new(
        effective_profile,
        effective_provider,
        effective_model,
        effective_reasoning,
    )
    .map_err(|_| ProviderError::InvalidOutput)?;
    let output = ProviderOutput::with_effective_execution(
        answer,
        citations,
        Some(value(&values, "provider_run_id")?),
        evidence,
        ProviderTokenUsage {
            actual_input_tokens: input,
            actual_output_tokens: output,
        },
        artifacts,
    )
    .map_err(|_| ProviderError::InvalidOutput)?;
    match preflight.ceiling {
        Some(ceiling) => output
            .with_token_budget_attestation(ceiling)
            .map_err(|_| ProviderError::InvalidOutput),
        None => Ok(output),
    }
}

fn parse_artifacts(
    values: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<LosslessArtifact>, ProviderError> {
    let kinds = values
        .get("artifact_kind")
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let media_types = values
        .get("artifact_media_type")
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let ids = values.get("artifact_id").map(Vec::as_slice).unwrap_or(&[]);
    let encoded = values
        .get("artifact_bytes")
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if kinds.len() != media_types.len() || kinds.len() != ids.len() || kinds.len() != encoded.len()
    {
        return Err(ProviderError::InvalidOutput);
    }
    kinds
        .iter()
        .zip(media_types)
        .zip(ids)
        .zip(encoded)
        .map(|(((kind, media_type), id), encoded)| {
            let kind = match kind.as_str() {
                "patch" => ArtifactKind::Patch,
                "json" => ArtifactKind::Json,
                "validation-receipt" => ArtifactKind::ValidationReceipt,
                "opaque" => ArtifactKind::Opaque,
                _ => return Err(ProviderError::InvalidOutput),
            };
            let artifact = LosslessArtifact::new(kind, media_type, decode_hex(encoded)?)
                .map_err(|_| ProviderError::InvalidOutput)?;
            if artifact.id().to_string() != *id {
                return Err(ProviderError::InvalidOutput);
            }
            Ok(artifact)
        })
        .collect()
}

fn decode_hex(value: &str) -> Result<Vec<u8>, ProviderError> {
    if !value.len().is_multiple_of(2)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ProviderError::InvalidOutput);
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte| match byte {
                b'0'..=b'9' => Ok(byte - b'0'),
                b'a'..=b'f' => Ok(byte - b'a' + 10),
                _ => Err(ProviderError::InvalidOutput),
            };
            Ok((digit(pair[0])? << 4) | digit(pair[1])?)
        })
        .collect()
}
fn tool_policy(request: &ProviderRequest) -> String {
    format!(
        "allow:{};deny:{}",
        request
            .tool_policy()
            .allowed()
            .collect::<Vec<_>>()
            .join(","),
        request.tool_policy().denied().collect::<Vec<_>>().join(",")
    )
}
fn number(value: &str) -> Result<usize, ProviderError> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ProviderError::InvalidOutput);
    }
    value.parse().map_err(|_| ProviderError::InvalidOutput)
}
fn valid_key(value: &str) -> bool {
    value.len() <= 64
        && !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}
fn valid_name(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}
pub(crate) fn verified_executable_sha256(path: &Path) -> Result<String, ProviderError> {
    verified_executable_sha256_inner(path, None)
}

pub(crate) fn verified_executable_sha256_until(
    path: &Path,
    deadline: Instant,
) -> Result<String, ProviderError> {
    verified_executable_sha256_inner(path, Some(deadline))
}

pub(crate) fn verified_executable_sha256_by(
    path: &Path,
    deadline: Instant,
) -> Result<String, ProviderError> {
    verified_executable_sha256_by_cancel(path, deadline, None)
}

fn verified_executable_sha256_by_cancel(
    path: &Path,
    deadline: Instant,
    cancellation: Option<&AtomicBool>,
) -> Result<String, ProviderError> {
    before(deadline)?;
    let path = path.to_owned();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(verified_executable_sha256_until(&path, deadline));
    });
    receive_cancellable(
        &receiver,
        deadline,
        ProviderError::Unavailable,
        cancellation,
    )?
}

fn verified_executable_sha256_inner(
    path: &Path,
    deadline: Option<Instant>,
) -> Result<String, ProviderError> {
    if let Some(deadline) = deadline {
        before(deadline)?;
    }
    verify_path(path)?;
    let mut file = File::open(path).map_err(|_| ProviderError::Unavailable)?;
    let metadata = file.metadata().map_err(|_| ProviderError::Unavailable)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_EXECUTABLE {
        return Err(ProviderError::Unavailable);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut chunk = [0; 64 * 1024];
    loop {
        if let Some(deadline) = deadline {
            before(deadline)?;
        }
        let read = file
            .read(&mut chunk)
            .map_err(|_| ProviderError::Unavailable)?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) as u64 > MAX_EXECUTABLE {
            return Err(ProviderError::Unavailable);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    if bytes.len() as u64 != metadata.len()
        || file
            .metadata()
            .map_err(|_| ProviderError::Unavailable)?
            .len()
            != metadata.len()
    {
        return Err(ProviderError::Unavailable);
    }
    verify_path(path)?;
    if let Some(deadline) = deadline {
        before(deadline)?;
    }
    let digest = ResultId::sha256(&bytes).value().to_owned();
    if let Some(deadline) = deadline {
        before(deadline)?;
    }
    Ok(digest)
}

#[cfg(test)]
pub(crate) fn current_test_fixture_identity() -> Result<(PathBuf, String), ProviderError> {
    provider_protocol_self_check()?;
    static IDENTITY: OnceLock<(PathBuf, String)> = OnceLock::new();
    if let Some(identity) = IDENTITY.get() {
        return Ok(identity.clone());
    }
    let executable = std::env::current_exe().map_err(|_| ProviderError::Unavailable)?;
    let identity = (executable.clone(), verified_executable_sha256(&executable)?);
    let _ = IDENTITY.set(identity.clone());
    Ok(IDENTITY.get().cloned().unwrap_or(identity))
}

#[cfg(test)]
fn provider_protocol_self_check() -> Result<(), ProviderError> {
    use std::io::Cursor;

    let cancellation = AtomicBool::new(true);
    if !matches!(
        before_cancel(Instant::now() + Duration::from_secs(1), Some(&cancellation)),
        Err(ProviderError::Cancelled)
    ) {
        return Err(ProviderError::InvalidOutput);
    }
    if valid_timeout_secs(0)
        || !valid_timeout_secs(DEFAULT_EXTERNAL_HOST_TIMEOUT_SECS)
        || valid_timeout_secs(MAX_EXTERNAL_HOST_TIMEOUT_SECS + 1)
    {
        return Err(ProviderError::InvalidOutput);
    }
    let request = ProviderRequest::new(
        "fixture-provider",
        "fixture-model",
        crate::agent::ReasoningLevel::High,
        crate::agent::ToolPolicy::read_only_default(),
        "fixture",
        1024,
        0,
    )
    .map_err(|_| ProviderError::InvalidOutput)?
    .with_no_session(true)
    .with_token_budget(110, 100)
    .map_err(|_| ProviderError::InvalidOutput)?;
    let account = SafeAccountReference::new("fixture-account")?;
    let max_request = ProviderRequest::new(
        "fixture-provider",
        "fixture-model",
        crate::agent::ReasoningLevel::Max,
        crate::agent::ToolPolicy::read_only_default(),
        "fixture",
        1024,
        0,
    )
    .map_err(|_| ProviderError::InvalidOutput)?;
    same(
        &parse(&request_payload(&max_request, &account))?,
        "reasoning",
        "max",
    )?;
    let requested_tool_policy = tool_policy(&request);
    let preflight_payload = |no_session| {
        fields(&[
            ("stage", "preflight-result"),
            ("status", "supported"),
            ("provider", "fixture-provider"),
            ("model", "fixture-model"),
            ("reasoning", "high"),
            ("tool_policy", &requested_tool_policy),
            ("account_ref", "fixture-account"),
            ("request_digest", "fixture-digest"),
            ("preflight_id", "fixture-pf"),
            ("no_session", no_session),
            ("input_tokens", "10"),
            ("max_output_tokens", "100"),
            ("exact_complete_tokens", "110"),
            ("hard_total_token_ceiling", "110"),
        ])
    };
    let preflight = parse_preflight(
        &preflight_payload("true"),
        &request,
        &account,
        "fixture-digest",
        false,
    )?;
    if parse_preflight(
        &preflight_payload("false"),
        &request,
        &account,
        "fixture-digest",
        false,
    )
    .is_ok()
    {
        return Err(ProviderError::InvalidOutput);
    }
    same(
        &parse(&request_payload(&request, &account))?,
        "no_session",
        "true",
    )?;
    same(
        &parse(&execute_payload("fixture-digest", &preflight, &request))?,
        "no_session",
        "true",
    )?;
    let artifact_bytes = [0, 0xff, b'{', b'}'];
    let artifact_id = ResultId::sha256(&artifact_bytes).to_string();
    let result_payload = |no_session| {
        fields(&[
            ("stage", "result"),
            ("status", "complete"),
            ("request_digest", "fixture-digest"),
            ("preflight_id", "fixture-pf"),
            ("provider", "fixture-provider"),
            ("model", "fixture-model"),
            ("reasoning", "high"),
            ("tool_policy", &requested_tool_policy),
            ("account_ref", "fixture-account"),
            ("no_session", no_session),
            ("input_tokens", "10"),
            ("max_output_tokens", "100"),
            ("exact_complete_tokens", "110"),
            ("hard_total_token_ceiling", "110"),
            ("actual_input_tokens", "10"),
            ("actual_output_tokens", "7"),
            ("answer", "fixture answer"),
            ("provider_run_id", "fixture-run"),
            ("effective_provider", "fixture-provider-v2"),
            ("effective_model", "unavailable"),
            ("effective_reasoning", "low"),
            ("artifact_kind", "json"),
            ("artifact_media_type", "application/json"),
            ("artifact_id", &artifact_id),
            ("artifact_bytes", "00ff7b7d"),
        ])
    };
    let payload = result_payload("true");
    let output = parse_result(
        &payload,
        &request,
        &account,
        "fixture-digest",
        &preflight,
        None,
        false,
    )?;
    if output.effective_provider() != Some("fixture-provider-v2")
        || output.effective_model().is_some()
        || output.effective_reasoning() != Some("low")
        || output.artifacts().len() != 1
        || output.artifacts()[0].bytes() != artifact_bytes
        || output.artifacts()[0].id().to_string() != artifact_id
    {
        return Err(ProviderError::InvalidOutput);
    }
    if parse_result(
        &result_payload("false"),
        &request,
        &account,
        "fixture-digest",
        &preflight,
        None,
        false,
    )
    .is_ok()
    {
        return Err(ProviderError::InvalidOutput);
    }

    let unbounded_request = ProviderRequest::new(
        "fixture-provider",
        "fixture-model",
        crate::agent::ReasoningLevel::High,
        crate::agent::ToolPolicy::read_only_default(),
        "fixture",
        1024,
        0,
    )
    .map_err(|_| ProviderError::InvalidOutput)?;
    let unbounded_preflight = fields(&[
        ("stage", "preflight-result"),
        ("status", "supported"),
        ("provider", "fixture-provider"),
        ("model", "fixture-model"),
        ("reasoning", "high"),
        ("tool_policy", &requested_tool_policy),
        ("account_ref", "fixture-account"),
        ("request_digest", "fixture-digest"),
        ("preflight_id", "fixture-pf"),
        ("no_session", "false"),
        ("input_tokens", "unavailable"),
        ("max_output_tokens", "unavailable"),
        ("exact_complete_tokens", "unavailable"),
        ("hard_total_token_ceiling", "unavailable"),
    ]);
    let unbounded_preflight = parse_preflight(
        &unbounded_preflight,
        &unbounded_request,
        &account,
        "fixture-digest",
        false,
    )?;
    let unbounded_result = fields(&[
        ("stage", "result"),
        ("status", "complete"),
        ("request_digest", "fixture-digest"),
        ("preflight_id", "fixture-pf"),
        ("provider", "fixture-provider"),
        ("model", "fixture-model"),
        ("reasoning", "high"),
        ("tool_policy", &requested_tool_policy),
        ("account_ref", "fixture-account"),
        ("no_session", "false"),
        ("input_tokens", "unavailable"),
        ("max_output_tokens", "unavailable"),
        ("exact_complete_tokens", "unavailable"),
        ("hard_total_token_ceiling", "unavailable"),
        ("actual_input_tokens", "unavailable"),
        ("actual_output_tokens", "unavailable"),
        ("answer", "fixture answer"),
        ("provider_run_id", "fixture-run"),
        ("effective_provider", "fixture-provider"),
        ("effective_model", "fixture-model"),
        ("effective_reasoning", "high"),
    ]);
    let unbounded_output = parse_result(
        &unbounded_result,
        &unbounded_request,
        &account,
        "fixture-digest",
        &unbounded_preflight,
        None,
        false,
    )?;
    if unbounded_output.actual_input_tokens().is_some()
        || unbounded_output.actual_output_tokens().is_some()
        || unbounded_output.enforced_token_ceiling().is_some()
    {
        return Err(ProviderError::InvalidOutput);
    }

    let ceiling_request = unbounded_request
        .clone()
        .with_total_token_ceiling(100)
        .map_err(|_| ProviderError::InvalidOutput)?;
    let ceiling_preflight = fields(&[
        ("stage", "preflight-result"),
        ("status", "supported"),
        ("provider", "fixture-provider"),
        ("model", "fixture-model"),
        ("reasoning", "high"),
        ("tool_policy", &requested_tool_policy),
        ("account_ref", "fixture-account"),
        ("request_digest", "fixture-digest"),
        ("preflight_id", "fixture-pf"),
        ("no_session", "false"),
        ("input_tokens", "unavailable"),
        ("max_output_tokens", "unavailable"),
        ("exact_complete_tokens", "unavailable"),
        ("hard_total_token_ceiling", "100"),
    ]);
    let ceiling_preflight = parse_preflight(
        &ceiling_preflight,
        &ceiling_request,
        &account,
        "fixture-digest",
        false,
    )?;
    let ceiling_result = |input, output| {
        fields(&[
            ("stage", "result"),
            ("status", "complete"),
            ("request_digest", "fixture-digest"),
            ("preflight_id", "fixture-pf"),
            ("provider", "fixture-provider"),
            ("model", "fixture-model"),
            ("reasoning", "high"),
            ("tool_policy", &requested_tool_policy),
            ("account_ref", "fixture-account"),
            ("no_session", "false"),
            ("input_tokens", "unavailable"),
            ("max_output_tokens", "unavailable"),
            ("exact_complete_tokens", "unavailable"),
            ("hard_total_token_ceiling", "100"),
            ("actual_input_tokens", input),
            ("actual_output_tokens", output),
            ("answer", "fixture answer"),
            ("provider_run_id", "fixture-run"),
            ("effective_provider", "fixture-provider"),
            ("effective_model", "fixture-model"),
            ("effective_reasoning", "high"),
        ])
    };
    if parse_result(
        &ceiling_result("10", "7"),
        &ceiling_request,
        &account,
        "fixture-digest",
        &ceiling_preflight,
        None,
        false,
    )?
    .enforced_token_ceiling()
        != Some(100)
    {
        return Err(ProviderError::InvalidOutput);
    }
    if parse_result(
        &ceiling_result("unavailable", "7"),
        &ceiling_request,
        &account,
        "fixture-digest",
        &ceiling_preflight,
        None,
        false,
    )
    .is_ok()
        || parse_result(
            &ceiling_result("60", "41"),
            &ceiling_request,
            &account,
            "fixture-digest",
            &ceiling_preflight,
            None,
            false,
        )
        .is_ok()
    {
        return Err(ProviderError::InvalidOutput);
    }

    let max_artifact_bytes = vec![0xab; 1_048_576];
    let max_artifact_id = ResultId::sha256(&max_artifact_bytes).to_string();
    let max_artifact_hex = "ab".repeat(max_artifact_bytes.len());
    let max_artifact_result = fields(&[
        ("stage", "result"),
        ("status", "complete"),
        ("request_digest", "fixture-digest"),
        ("preflight_id", "fixture-pf"),
        ("provider", "fixture-provider"),
        ("model", "fixture-model"),
        ("reasoning", "high"),
        ("tool_policy", &requested_tool_policy),
        ("account_ref", "fixture-account"),
        ("no_session", "false"),
        ("input_tokens", "unavailable"),
        ("max_output_tokens", "unavailable"),
        ("exact_complete_tokens", "unavailable"),
        ("hard_total_token_ceiling", "unavailable"),
        ("actual_input_tokens", "unavailable"),
        ("actual_output_tokens", "unavailable"),
        ("answer", "fixture answer"),
        ("provider_run_id", "fixture-run"),
        ("effective_provider", "fixture-provider"),
        ("effective_model", "fixture-model"),
        ("effective_reasoning", "high"),
        ("artifact_kind", "opaque"),
        ("artifact_media_type", "application/octet-stream"),
        ("artifact_id", &max_artifact_id),
        ("artifact_bytes", &max_artifact_hex),
    ]);
    let max_artifact_output = parse_result(
        &max_artifact_result,
        &unbounded_request,
        &account,
        "fixture-digest",
        &unbounded_preflight,
        None,
        false,
    )?;
    let mut framed = Vec::new();
    write_frame(&mut framed, b"preflight")?;
    write_frame(&mut framed, &max_artifact_result)?;
    let (tx, rx) = mpsc::channel();
    drain_frames(Cursor::new(framed), tx, false);
    let frames = rx.into_iter().collect::<Vec<_>>();
    if max_artifact_output.artifacts()[0].bytes() != max_artifact_bytes
        || frames.len() != 2
        || frames.iter().any(Result::is_err)
        || !matches!(
            read_frame(&mut Cursor::new(
                format!("{}\n", MAX_FRAME + 1).into_bytes()
            )),
            Err(ProviderError::InvalidOutput)
        )
    {
        return Err(ProviderError::InvalidOutput);
    }

    for suffix in [b"1\nx\n".as_slice(), b"trailing".as_slice()] {
        let mut framed = Vec::new();
        write_frame(&mut framed, b"one")?;
        write_frame(&mut framed, b"two")?;
        framed.extend_from_slice(suffix);
        let (tx, rx) = mpsc::channel();
        drain_frames(Cursor::new(framed), tx, false);
        let results = rx.into_iter().collect::<Vec<_>>();
        if results.len() != 3 || !matches!(results[2], Err(ProviderError::InvalidOutput)) {
            return Err(ProviderError::InvalidOutput);
        }
    }
    Ok(())
}

fn verify_path(path: &Path) -> Result<(), ProviderError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ProviderError::Unavailable)?;
    if !path.is_absolute()
        || metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() > MAX_EXECUTABLE
        || fs::canonicalize(path).map_err(|_| ProviderError::Unavailable)? != path
    {
        return Err(ProviderError::Unavailable);
    }
    Ok(())
}
