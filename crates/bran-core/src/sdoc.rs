//! StrictDoc bridge v2 consumer for read-only `.sdoc` discovery.
//!
//! StrictDoc remains the SDoc grammar authority. This module invokes only an
//! explicitly configured absolute bridge command and validates its versioned
//! JSON receipt before exposing it to later graph and ranking work.

use crate::agent::result_store::ResultId;
use crate::scan::ScanFailure;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_SDOC_FILES: usize = 10_000;
const MAX_SDOC_FILE_BYTES: usize = 1024 * 1024;
const MAX_SDOC_TOTAL_BYTES: usize = 64 * 1024 * 1024;

pub const BRIDGE_PROTOCOL: &str = "alphazede.strictdoc.bridge";
pub const BRIDGE_SCHEMA_VERSION: &str = "2";
pub const BRIDGE_MODE: &str = "sdoc";
pub const STRICTDOC_API: &str = "strictdoc.api";
pub const STRICTDOC_VERSION: &str = "0.29.0";
pub const STRICTDOC_ARTIFACT_SHA256: &str =
    "fae511b228952ee5e1ff765650ac2701526ce39e32a6686f53ef384621486a90";
pub const STRICTDOC_REQUIREMENTS_SHA256: &str =
    "77b879886d9856ca748e181b592e78efa377d432953ec52d6e61803cf10ef9c8";

/// Fixed startup bootstrap for the pinned interpreter, invoked under `-I -S -B`.
///
/// `-I -S` starts the interpreter isolated and without `site`, so no
/// `sitecustomize`, `usercustomize`, `.pth`, inherited `PYTHONPATH`, or user
/// site-packages code can run before BRAN's verification. `-B` stops the
/// interpreter writing `__pycache__/*.pyc` back into the pinned import root:
/// BRAN must never mutate the tree whose closure it just verified, and a `.pyc`
/// written there would be unpinned executable input on the next run.
/// This constant then appends the pinned site-packages directory (appending
/// keeps the standard library ahead of it) and executes the hashed bridge,
/// which verifies every locked distribution RECORD before importing StrictDoc.
/// The directory and bridge arrive on `argv`, never interpolated into this
/// source.
const BRIDGE_BOOTSTRAP: &str = concat!(
    "import sys\n",
    "sys.path.append(sys.argv[1])\n",
    "sys.argv = sys.argv[2:]\n",
    "exec(compile(open(sys.argv[0], \"rb\").read(), sys.argv[0], \"exec\"), ",
    "{\"__name__\": \"__main__\", \"__file__\": sys.argv[0]})\n",
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdocBridgeConfig {
    program: PathBuf,
    /// The real interpreter file the packaged `program` launcher execs. BRAN
    /// runs this path directly so startup is isolated, and hashes it here so
    /// the pin is independent of anything the interpreter reports about itself.
    interpreter: PathBuf,
    bridge: PathBuf,
    wheel: PathBuf,
    wheelhouse: PathBuf,
    /// The pinned import root appended by [`BRIDGE_BOOTSTRAP`] after startup.
    /// The hashed bridge verifies the files a locked distribution RECORD
    /// claims; [`site_closure_digest`] independently rejects everything under
    /// this root that no pin claims, so nothing unpinned can be imported.
    site_packages: PathBuf,
    hashes: PathBuf,
    python_sha256: String,
    interpreter_sha256: String,
    bridge_sha256: String,
    wheelhouse_sha256: String,
    site_packages_sha256: String,
    hashes_sha256: String,
}

impl SdocBridgeConfig {
    /// Production configuration is compiled into the packaged `.deb`; callers
    /// cannot select an interpreter, bridge, wheel, or expected identity.
    pub fn pinned_installation() -> Result<Self, SdocError> {
        let config = Self {
            program: pinned_path(option_env!("AZREQ_STRICTDOC_PYTHON"))?,
            interpreter: pinned_path(option_env!("AZREQ_STRICTDOC_INTERPRETER"))?,
            bridge: pinned_path(option_env!("AZREQ_STRICTDOC_BRIDGE"))?,
            wheel: pinned_path(option_env!("AZREQ_STRICTDOC_WHEEL"))?,
            wheelhouse: pinned_path(option_env!("AZREQ_STRICTDOC_WHEELHOUSE"))?,
            site_packages: pinned_path(option_env!("AZREQ_STRICTDOC_SITE_PACKAGES"))?,
            hashes: pinned_path(option_env!("AZREQ_STRICTDOC_HASHES"))?,
            python_sha256: pinned_digest(option_env!("AZREQ_STRICTDOC_PYTHON_SHA256"))?,
            interpreter_sha256: pinned_digest(option_env!("AZREQ_STRICTDOC_INTERPRETER_SHA256"))?,
            bridge_sha256: pinned_digest(option_env!("AZREQ_STRICTDOC_BRIDGE_SHA256"))?,
            wheelhouse_sha256: pinned_digest(option_env!("AZREQ_STRICTDOC_WHEELHOUSE_SHA256"))?,
            site_packages_sha256: pinned_digest(option_env!(
                "AZREQ_STRICTDOC_SITE_PACKAGES_SHA256"
            ))?,
            hashes_sha256: pinned_digest(option_env!("AZREQ_STRICTDOC_HASHES_SHA256"))?,
        };
        config.verify()?;
        Ok(config)
    }

    /// Test-only construction of an unpinned bridge, so behaviour tests can run
    /// a fixture without production pins. The dedicated `test-fixtures` feature
    /// keeps it out of every shipped binary — `bran-cli` enables it only through
    /// its dev-dependency — while staying available in `--release` test builds,
    /// which `debug_assertions` was not.
    #[cfg(any(test, feature = "test-fixtures"))]
    #[doc(hidden)]
    pub fn new(
        program: impl Into<PathBuf>,
        bridge: impl Into<PathBuf>,
        wheel: impl Into<PathBuf>,
    ) -> Result<Self, SdocError> {
        let config = Self {
            program: program.into(),
            interpreter: PathBuf::new(),
            bridge: bridge.into(),
            wheel: wheel.into(),
            wheelhouse: PathBuf::new(),
            site_packages: PathBuf::new(),
            hashes: PathBuf::new(),
            python_sha256: String::new(),
            interpreter_sha256: String::new(),
            bridge_sha256: String::new(),
            wheelhouse_sha256: String::new(),
            site_packages_sha256: String::new(),
            hashes_sha256: String::new(),
        };
        if !config.program.is_absolute() {
            return Err(SdocError::RelativeBridgeProgram);
        }
        if !config.bridge.is_absolute() || !config.wheel.is_absolute() {
            return Err(SdocError::RelativeBridgeInput);
        }
        Ok(config)
    }

    fn verify(&self) -> Result<(), SdocError> {
        // BRAN hashes every executed byte itself, before the interpreter runs.
        // The bridge's self-reported engine digests are cross-checked later as
        // defence in depth, never as the source of truth.
        let checks: [(&Path, &str); 5] = [
            (&self.program, &self.python_sha256),
            (&self.interpreter, &self.interpreter_sha256),
            (&self.bridge, &self.bridge_sha256),
            (&self.wheel, STRICTDOC_ARTIFACT_SHA256),
            (&self.hashes, &self.hashes_sha256),
        ];
        if checks.iter().any(|(path, expected)| {
            !path.is_file() || sha256(&fs::read(path).unwrap_or_default()) != *expected
        }) || !self.wheelhouse.is_dir()
            || wheelhouse_digest(&self.wheelhouse)? != self.wheelhouse_sha256
        {
            return Err(SdocError::QualifiedRuntimeMismatch);
        }
        // The bootstrap appends `site_packages` to `sys.path`, so every byte
        // under it is executable input, not just the files a RECORD claims.
        // Verifying the whole closure here is what stops an added package from
        // running while the receipt still matches (W6.1-P1-05).
        if !self.site_packages.is_dir()
            || site_closure_digest(&self.site_packages)? != self.site_packages_sha256
        {
            return Err(SdocError::UnpinnedRuntimeContent);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdocEngine {
    pub api: String,
    pub version: String,
    pub artifact_sha256: String,
    pub requirements_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdocLineRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdocDocument {
    pub mid: String,
    pub uid: String,
    pub title: String,
    pub metadata: BTreeMap<String, String>,
    pub line_range: SdocLineRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdocNode {
    pub mid: String,
    pub uid: String,
    pub node_type: String,
    pub fields: BTreeMap<String, String>,
    pub line_range: SdocLineRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdocRelation {
    pub mid: String,
    pub relation_type: String,
    pub reference_type: String,
    pub source_mid: String,
    pub target_mid: String,
    pub line_range: SdocLineRange,
    /// The source node's declared OWNER, when the bridge reported one.
    pub owner: Option<String>,
    /// The document's published revision, when the bridge reported one.
    pub revision: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdocReceipt {
    pub source_locator: String,
    pub source_sha256: String,
    pub validation: SdocValidation,
    pub engine: SdocEngine,
    pub document: SdocDocument,
    pub nodes: Vec<SdocNode>,
    pub relations: Vec<SdocRelation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SdocValidation {
    Valid,
}

#[derive(Clone, Debug)]
pub struct SdocScanner {
    root: PathBuf,
    bridge: SdocBridgeConfig,
}

impl SdocScanner {
    pub fn new(root: impl AsRef<Path>, bridge: SdocBridgeConfig) -> Result<Self, SdocError> {
        let requested = root.as_ref().to_path_buf();
        let root = fs::canonicalize(&requested).map_err(|_| SdocError::InvalidRoot(requested))?;
        if !root.is_dir() {
            return Err(SdocError::InvalidRoot(root));
        }
        Ok(Self { root, bridge })
    }

    /// Discovers `.sdoc` sources and invokes the bridge for each source only.
    /// Markdown and generated views are intentionally not candidates here.
    pub fn scan(&self) -> Result<Vec<SdocReceipt>, SdocError> {
        let mut sources = Vec::new();
        collect_sdoc_files(&self.root, &self.root, &mut sources)?;
        if sources.len() > MAX_SDOC_FILES
            || sources.iter().map(|(_, bytes)| bytes.len()).sum::<usize>() > MAX_SDOC_TOTAL_BYTES
        {
            return Err(SdocError::ScanLimit);
        }
        sources
            .iter()
            .map(|(locator, bytes)| self.scan_one(locator, bytes))
            .collect()
    }

    fn scan_one(&self, locator: &str, expected_source: &[u8]) -> Result<SdocReceipt, SdocError> {
        validate_locator(locator)?;
        let source_path = self.root.join(locator);
        let metadata = fs::symlink_metadata(&source_path)
            .map_err(|_| SdocError::SourceMissing(locator.to_owned()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(SdocError::SourceEscape(locator.to_owned()));
        }
        let source = fs::canonicalize(&source_path)
            .map_err(|_| SdocError::SourceMissing(locator.to_owned()))?;
        if !source.starts_with(&self.root) || !source.is_file() {
            return Err(SdocError::SourceEscape(locator.to_owned()));
        }
        let before = fs::read(&source).map_err(|_| SdocError::SourceMissing(locator.to_owned()))?;
        if before.as_slice() != expected_source {
            return Err(SdocError::SourceChanged(locator.to_owned()));
        }
        let expected_digest = sha256(&before);
        if !self.bridge.python_sha256.is_empty() {
            self.bridge.verify()?;
        }
        let mut command = if self.bridge.python_sha256.is_empty() {
            Command::new(&self.bridge.program)
        } else {
            // Run the hashed interpreter directly rather than the packaged
            // launcher: the launcher exports PYTHONPATH, which would let a
            // `sitecustomize` in that directory execute before verification.
            let mut command = Command::new(&self.bridge.interpreter);
            command
                .arg("-I")
                .arg("-S")
                .arg("-B")
                .arg("-c")
                .arg(BRIDGE_BOOTSTRAP)
                .arg(&self.bridge.site_packages);
            command
        };
        let output = command
            .arg(&self.bridge.bridge)
            .arg("--protocol")
            .arg(BRIDGE_PROTOCOL)
            .arg("--schema-version")
            .arg(BRIDGE_SCHEMA_VERSION)
            .arg("--mode")
            .arg(BRIDGE_MODE)
            .arg("--root")
            .arg(&self.root)
            .arg("--source")
            .arg(&source)
            .arg("--wheel")
            .arg(&self.bridge.wheel)
            .env_clear()
            .env("LC_ALL", "C");
        let output = if self.bridge.python_sha256.is_empty() {
            output.output()
        } else {
            output
                .arg("--wheelhouse")
                .arg(&self.bridge.wheelhouse)
                .arg("--hashes")
                .arg(&self.bridge.hashes)
                .arg("--python-sha256")
                .arg(&self.bridge.interpreter_sha256)
                .arg("--bridge-sha256")
                .arg(&self.bridge.bridge_sha256)
                .arg("--artifact-sha256")
                .arg(STRICTDOC_ARTIFACT_SHA256)
                .arg("--requirements-sha256")
                .arg(&self.bridge.hashes_sha256)
                .arg("--wheelhouse-sha256")
                .arg(&self.bridge.wheelhouse_sha256)
                .output()
        }
        .map_err(|_| SdocError::BridgeTransport)?;
        let after = fs::read(&source).map_err(|_| SdocError::SourceMissing(locator.to_owned()))?;
        if after != before {
            return Err(SdocError::SourceChanged(locator.to_owned()));
        }
        if !output.status.success() && output.stdout.is_empty() {
            return Err(SdocError::BridgeExit(output.status.code()));
        }
        let receipt = parse_bridge_envelope(&output.stdout, locator, &expected_digest);
        if output.status.success() {
            let receipt = receipt?;
            if !self.bridge.python_sha256.is_empty()
                && !runtime_receipt_matches(&output.stdout, &self.bridge)
            {
                return Err(SdocError::QualifiedRuntimeMismatch);
            }
            return Ok(receipt);
        }
        match receipt {
            Err(error) => Err(error),
            Ok(_) => Err(SdocError::BridgeExit(output.status.code())),
        }
    }
}

pub fn parse_bridge_envelope(
    bytes: &[u8],
    expected_locator: &str,
    expected_source_sha256: &str,
) -> Result<SdocReceipt, SdocError> {
    validate_locator(expected_locator)?;
    let value = serde_json::from_slice(bytes).map_err(|_| SdocError::MalformedJson)?;
    let top = closed_object(
        &value,
        &["protocol", "schema_version", "status"],
        &[
            "protocol",
            "schema_version",
            "status",
            "mode",
            "validation",
            "engine",
            "source",
            "document",
            "nodes",
            "relations",
            "error",
        ],
        "envelope",
    )?;
    exact_string(top, "protocol", BRIDGE_PROTOCOL, SdocError::Protocol)?;
    exact_string(
        top,
        "schema_version",
        BRIDGE_SCHEMA_VERSION,
        SdocError::SchemaVersion,
    )?;
    let status = string(top, "status", "envelope")?;
    if status != "ok" {
        return bridge_failure(top, status);
    }
    reject_present(top, "error", "envelope")?;
    exact_string(top, "mode", BRIDGE_MODE, SdocError::ModeMismatch)?;
    let validation = closed_object(
        field(top, "validation", "envelope")?,
        &["status"],
        &["status"],
        "validation",
    )?;
    exact_string(
        validation,
        "status",
        "valid",
        SdocError::InvalidEnvelope("validation status"),
    )?;
    let engine = parse_engine(field(top, "engine", "envelope")?)?;
    let (source_locator, source_sha256) = parse_source(field(top, "source", "envelope")?)?;
    if source_locator != expected_locator || source_sha256 != expected_source_sha256 {
        return Err(SdocError::SourceReceiptMismatch);
    }
    let document = parse_document(field(top, "document", "envelope")?)?;
    let nodes = array(field(top, "nodes", "envelope")?, "nodes")?
        .iter()
        .map(parse_node)
        .collect::<Result<Vec<_>, _>>()?;
    let relations = array(field(top, "relations", "envelope")?, "relations")?
        .iter()
        .map(parse_relation)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SdocReceipt {
        source_locator,
        source_sha256,
        validation: SdocValidation::Valid,
        engine,
        document,
        nodes,
        relations,
    })
}

fn bridge_failure(top: &Map<String, Value>, status: &str) -> Result<SdocReceipt, SdocError> {
    if !matches!(
        status,
        "invalid_request"
            | "engine_missing"
            | "engine_mismatch"
            | "parse_error"
            | "validation_error"
            | "source_changed"
            | "bridge_error"
    ) {
        return Err(SdocError::InvalidEnvelope("unknown status"));
    }
    if top
        .keys()
        .any(|key| !["protocol", "schema_version", "status", "error"].contains(&key.as_str()))
    {
        return Err(SdocError::InvalidEnvelope("failure envelope"));
    }
    let error = closed_object(
        field(top, "error", "envelope")?,
        &["code"],
        &["code"],
        "error",
    )?;
    Err(SdocError::BridgeStatus {
        status: status.to_owned(),
        code: nonempty(string(error, "code", "error")?, "error code")?.to_owned(),
    })
}

fn parse_engine(value: &Value) -> Result<SdocEngine, SdocError> {
    let engine = closed_object(
        value,
        &["api", "version", "artifact_sha256", "requirements_sha256"],
        &[
            "api",
            "version",
            "artifact_sha256",
            "requirements_sha256",
            "python_sha256",
            "bridge_sha256",
            "wheelhouse_sha256",
        ],
        "engine",
    )?;
    exact_string(engine, "api", STRICTDOC_API, SdocError::EngineMismatch)?;
    exact_string(
        engine,
        "version",
        STRICTDOC_VERSION,
        SdocError::EngineMismatch,
    )?;
    exact_string(
        engine,
        "artifact_sha256",
        STRICTDOC_ARTIFACT_SHA256,
        SdocError::EngineMismatch,
    )?;
    exact_string(
        engine,
        "requirements_sha256",
        STRICTDOC_REQUIREMENTS_SHA256,
        SdocError::EngineMismatch,
    )?;
    Ok(SdocEngine {
        api: STRICTDOC_API.to_owned(),
        version: STRICTDOC_VERSION.to_owned(),
        artifact_sha256: STRICTDOC_ARTIFACT_SHA256.to_owned(),
        requirements_sha256: STRICTDOC_REQUIREMENTS_SHA256.to_owned(),
    })
}

/// Cross-checks the bridge's self-reported engine digests against the pins.
/// This is defence in depth only; [`SdocBridgeConfig::verify`] has already
/// hashed the interpreter, bridge, wheel, and lock in Rust before execution.
fn runtime_receipt_matches(bytes: &[u8], config: &SdocBridgeConfig) -> bool {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| value.get("engine")?.as_object().cloned())
        .is_some_and(|engine| {
            engine.get("python_sha256").and_then(Value::as_str) == Some(&config.interpreter_sha256)
                && engine.get("bridge_sha256").and_then(Value::as_str)
                    == Some(&config.bridge_sha256)
                && engine.get("wheelhouse_sha256").and_then(Value::as_str)
                    == Some(&config.wheelhouse_sha256)
        })
}

fn parse_source(value: &Value) -> Result<(String, String), SdocError> {
    let source = closed_object(
        value,
        &["locator", "sha256"],
        &["locator", "sha256"],
        "source",
    )?;
    let locator = nonempty(string(source, "locator", "source")?, "source locator")?;
    validate_locator(locator)?;
    let digest = nonempty(string(source, "sha256", "source")?, "source digest")?;
    if !is_sha256(digest) {
        return Err(SdocError::InvalidEnvelope("source digest"));
    }
    Ok((locator.to_owned(), digest.to_owned()))
}

fn parse_document(value: &Value) -> Result<SdocDocument, SdocError> {
    let document = closed_object(
        value,
        &["mid", "uid", "title", "metadata", "line_range"],
        &["mid", "uid", "title", "metadata", "line_range"],
        "document",
    )?;
    Ok(SdocDocument {
        mid: identifier(document, "mid", "document")?,
        uid: identifier(document, "uid", "document")?,
        title: nonempty(string(document, "title", "document")?, "document title")?.to_owned(),
        metadata: string_map(
            field(document, "metadata", "document")?,
            "document metadata",
        )?,
        line_range: line_range(
            field(document, "line_range", "document")?,
            "document line range",
        )?,
    })
}

fn parse_node(value: &Value) -> Result<SdocNode, SdocError> {
    let node = closed_object(
        value,
        &["mid", "uid", "node_type", "fields", "line_range"],
        &["mid", "uid", "node_type", "fields", "line_range"],
        "node",
    )?;
    Ok(SdocNode {
        mid: identifier(node, "mid", "node")?,
        uid: identifier(node, "uid", "node")?,
        node_type: nonempty(string(node, "node_type", "node")?, "node type")?.to_owned(),
        fields: string_map(field(node, "fields", "node")?, "node fields")?,
        line_range: line_range(field(node, "line_range", "node")?, "node line range")?,
    })
}

fn parse_relation(value: &Value) -> Result<SdocRelation, SdocError> {
    let relation = closed_object(
        value,
        &[
            "mid",
            "type",
            "relation_type",
            "source_mid",
            "target_mid",
            "line_range",
        ],
        &[
            "mid",
            "type",
            "relation_type",
            "source_mid",
            "target_mid",
            "line_range",
            "owner",
            "revision",
        ],
        "relation",
    )?;
    Ok(SdocRelation {
        mid: identifier(relation, "mid", "relation")?,
        relation_type: nonempty(string(relation, "type", "relation")?, "relation type")?.to_owned(),
        reference_type: nonempty(
            string(relation, "relation_type", "relation")?,
            "relation reference type",
        )?
        .to_owned(),
        source_mid: identifier(relation, "source_mid", "relation")?,
        target_mid: identifier(relation, "target_mid", "relation")?,
        line_range: line_range(
            field(relation, "line_range", "relation")?,
            "relation line range",
        )?,
        owner: optional_nonempty(relation, "owner", "relation owner")?,
        revision: optional_nonempty(relation, "revision", "relation revision")?,
    })
}

/// An optional string member that, when present, must be a non-empty string.
fn optional_nonempty(
    object: &Map<String, Value>,
    key: &str,
    label: &'static str,
) -> Result<Option<String>, SdocError> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => {
            let text = value.as_str().ok_or(SdocError::InvalidEnvelope(label))?;
            Ok(Some(nonempty(text, label)?.to_owned()))
        }
    }
}

fn closed_object<'a>(
    value: &'a Value,
    required: &[&str],
    allowed: &[&str],
    label: &'static str,
) -> Result<&'a Map<String, Value>, SdocError> {
    let object = value.as_object().ok_or(SdocError::InvalidEnvelope(label))?;
    if required.iter().any(|key| !object.contains_key(*key))
        || object.keys().any(|key| !allowed.contains(&key.as_str()))
    {
        return Err(SdocError::InvalidEnvelope(label));
    }
    Ok(object)
}

fn field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    label: &'static str,
) -> Result<&'a Value, SdocError> {
    object.get(key).ok_or(SdocError::InvalidEnvelope(label))
}

fn string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    label: &'static str,
) -> Result<&'a str, SdocError> {
    field(object, key, label)?
        .as_str()
        .ok_or(SdocError::InvalidEnvelope(label))
}

fn exact_string(
    object: &Map<String, Value>,
    key: &str,
    expected: &str,
    error: SdocError,
) -> Result<(), SdocError> {
    (string(object, key, "envelope")? == expected)
        .then_some(())
        .ok_or(error)
}

fn reject_present(
    object: &Map<String, Value>,
    key: &str,
    label: &'static str,
) -> Result<(), SdocError> {
    (!object.contains_key(key))
        .then_some(())
        .ok_or(SdocError::InvalidEnvelope(label))
}

fn array<'a>(value: &'a Value, label: &'static str) -> Result<&'a Vec<Value>, SdocError> {
    value.as_array().ok_or(SdocError::InvalidEnvelope(label))
}

fn string_map(value: &Value, label: &'static str) -> Result<BTreeMap<String, String>, SdocError> {
    let map = value.as_object().ok_or(SdocError::InvalidEnvelope(label))?;
    map.iter()
        .map(|(key, value)| {
            let value = value.as_str().ok_or(SdocError::InvalidEnvelope(label))?;
            nonempty(key, label)?;
            Ok((key.clone(), value.to_owned()))
        })
        .collect()
}

fn line_range(value: &Value, label: &'static str) -> Result<SdocLineRange, SdocError> {
    let range = closed_object(value, &["start", "end"], &["start", "end"], label)?;
    let start = positive_line(field(range, "start", label)?, label)?;
    let end = positive_line(field(range, "end", label)?, label)?;
    (start <= end)
        .then_some(SdocLineRange { start, end })
        .ok_or(SdocError::InvalidEnvelope(label))
}

fn positive_line(value: &Value, label: &'static str) -> Result<usize, SdocError> {
    value
        .as_u64()
        .and_then(|line| usize::try_from(line).ok())
        .filter(|line| *line > 0)
        .ok_or(SdocError::InvalidEnvelope(label))
}

fn identifier(
    object: &Map<String, Value>,
    key: &str,
    label: &'static str,
) -> Result<String, SdocError> {
    let value = nonempty(string(object, key, label)?, label)?;
    (!value.chars().any(char::is_control))
        .then(|| value.to_owned())
        .ok_or(SdocError::InvalidEnvelope(label))
}

fn nonempty<'a>(value: &'a str, label: &'static str) -> Result<&'a str, SdocError> {
    (!value.trim().is_empty())
        .then_some(value)
        .ok_or(SdocError::InvalidEnvelope(label))
}

fn validate_locator(locator: &str) -> Result<(), SdocError> {
    if locator.is_empty()
        || locator.contains(['\\', '\0'])
        || Path::new(locator).is_absolute()
        || locator.as_bytes().get(0..3).is_some_and(|prefix| {
            prefix[0].is_ascii_alphabetic() && prefix[1] == b':' && prefix[2] == b'/'
        })
        || locator
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".." | ".git"))
    {
        return Err(SdocError::UnsafeLocator);
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn sha256(bytes: &[u8]) -> String {
    ResultId::sha256(bytes).value().to_owned()
}

fn pinned_path(value: Option<&'static str>) -> Result<PathBuf, SdocError> {
    value
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or(SdocError::PinnedRuntimeUnavailable)
}

fn pinned_digest(value: Option<&'static str>) -> Result<String, SdocError> {
    value
        .filter(|digest| is_sha256(digest))
        .map(str::to_owned)
        .ok_or(SdocError::PinnedRuntimeUnavailable)
}

fn wheelhouse_digest(wheelhouse: &Path) -> Result<String, SdocError> {
    let mut entries = fs::read_dir(wheelhouse)
        .map_err(|_| SdocError::QualifiedRuntimeMismatch)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            path.extension()
                .is_some_and(|ext| ext == "whl")
                .then(|| {
                    fs::read(&path)
                        .ok()
                        .map(|bytes| (entry.file_name(), sha256(&bytes)))
                })
                .flatten()
        })
        .collect::<Vec<_>>();
    entries.sort();
    (!entries.is_empty())
        .then(|| {
            sha256(
                entries
                    .iter()
                    .map(|(name, digest)| format!("{}:{digest}", name.to_string_lossy()))
                    .collect::<Vec<_>>()
                    .join("\n")
                    .as_bytes(),
            )
        })
        .ok_or(SdocError::QualifiedRuntimeMismatch)
}

/// Deterministic digest of the complete pinned import root.
///
/// Every regular file under `root` contributes one `relative/path:size:sha256`
/// line; the lines are sorted bytewise, joined by `\n`, and hashed.
/// `build/build-pinned.sh` computes the identical value, so a pin that no
/// longer describes the installed tree fails the build gate as well as this
/// check.
///
/// This is preferred over a RECORD-coverage set difference because it is one
/// definition both Rust and the build gate can compute identically: it needs no
/// RECORD parsing or quoting rules, no judgement about which file extensions
/// the interpreter can import, and it rejects edited and deleted files as well
/// as added ones. A path that is not a regular file, or whose name holds a
/// newline, cannot be described by that manifest and is rejected outright.
///
/// The cost is one full read of the import root per verification — 0.85s for
/// the 264 MiB 0.1.0-12 tree — and [`SdocScanner::scan_one`] verifies once per
/// source, so a scan of many `.sdoc` files pays it repeatedly. That is
/// deliberate: re-reading is what makes the check hold at the moment of
/// execution rather than at startup.
fn site_closure_digest(root: &Path) -> Result<String, SdocError> {
    let mut lines = Vec::new();
    collect_site_closure(root, root, &mut lines)?;
    lines.sort();
    (!lines.is_empty())
        .then(|| sha256(lines.join("\n").as_bytes()))
        .ok_or(SdocError::UnpinnedRuntimeContent)
}

fn collect_site_closure(
    root: &Path,
    directory: &Path,
    lines: &mut Vec<String>,
) -> Result<(), SdocError> {
    for entry in fs::read_dir(directory).map_err(|_| SdocError::UnpinnedRuntimeContent)? {
        let path = entry.map_err(|_| SdocError::UnpinnedRuntimeContent)?.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| SdocError::UnpinnedRuntimeContent)?;
        if metadata.is_dir() {
            collect_site_closure(root, &path, lines)?;
            continue;
        }
        // A symlink, device, or socket on `sys.path` supplies executable input
        // this manifest cannot describe by content, so it is never pinnable.
        if !metadata.is_file() {
            return Err(SdocError::UnpinnedRuntimeContent);
        }
        let relative = path
            .strip_prefix(root)
            .ok()
            .and_then(|relative| relative.to_str())
            .filter(|relative| !relative.contains('\n'))
            .ok_or(SdocError::UnpinnedRuntimeContent)?;
        let bytes = fs::read(&path).map_err(|_| SdocError::UnpinnedRuntimeContent)?;
        lines.push(format!("{relative}:{}:{}", bytes.len(), sha256(&bytes)));
    }
    Ok(())
}

fn collect_sdoc_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), SdocError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|_| SdocError::ScanLimit)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| SdocError::ScanLimit)?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| SdocError::ScanLimit)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_sdoc_files(root, &path, output)?;
            continue;
        }
        if !metadata.is_file() || path.extension().is_none_or(|ext| ext != "sdoc") {
            continue;
        }
        let bytes = fs::read(&path).map_err(|_| SdocError::ScanLimit)?;
        if bytes.len() > MAX_SDOC_FILE_BYTES {
            return Err(SdocError::ScanLimit);
        }
        let locator = path
            .strip_prefix(root)
            .map_err(|_| SdocError::SourceEscape(path.display().to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        validate_locator(&locator)?;
        output.push((locator, bytes));
    }
    Ok(())
}

#[derive(Debug)]
pub enum SdocError {
    InvalidRoot(PathBuf),
    RelativeBridgeProgram,
    RelativeBridgeInput,
    PinnedRuntimeUnavailable,
    QualifiedRuntimeMismatch,
    UnpinnedRuntimeContent,
    ScanLimit,
    Scan(ScanFailure),
    SourceMissing(String),
    SourceEscape(String),
    SourceChanged(String),
    BridgeTransport,
    BridgeExit(Option<i32>),
    MalformedJson,
    Protocol,
    SchemaVersion,
    ModeMismatch,
    EngineMismatch,
    SourceReceiptMismatch,
    UnsafeLocator,
    BridgeStatus { status: String, code: String },
    InvalidEnvelope(&'static str),
}

impl fmt::Display for SdocError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRoot(_) => formatter.write_str("invalid SDoc scanner root"),
            Self::RelativeBridgeProgram => formatter.write_str("bridge program must be absolute"),
            Self::RelativeBridgeInput => formatter.write_str("bridge inputs must be absolute"),
            Self::PinnedRuntimeUnavailable => {
                formatter.write_str("packaged StrictDoc runtime is unavailable")
            }
            Self::QualifiedRuntimeMismatch => {
                formatter.write_str("packaged StrictDoc runtime identity mismatches")
            }
            Self::UnpinnedRuntimeContent => {
                formatter.write_str("packaged StrictDoc import root holds unpinned content")
            }
            Self::ScanLimit => formatter.write_str("SDoc discovery limit exceeded"),
            Self::Scan(_) => formatter.write_str("SDoc discovery scan failed"),
            Self::SourceMissing(_) => formatter.write_str("SDoc source is missing"),
            Self::SourceEscape(_) => formatter.write_str("SDoc source escapes scanner root"),
            Self::SourceChanged(_) => formatter.write_str("SDoc source changed during bridge scan"),
            Self::BridgeTransport => formatter.write_str("SDoc bridge could not run"),
            Self::BridgeExit(_) => formatter.write_str("SDoc bridge returned nonzero"),
            Self::MalformedJson => formatter.write_str("SDoc bridge returned malformed JSON"),
            Self::Protocol => formatter.write_str("unsupported SDoc bridge protocol"),
            Self::SchemaVersion => formatter.write_str("unsupported SDoc bridge schema version"),
            Self::ModeMismatch => formatter.write_str("unsupported SDoc bridge mode"),
            Self::EngineMismatch => formatter.write_str("unqualified StrictDoc bridge engine"),
            Self::SourceReceiptMismatch => {
                formatter.write_str("SDoc bridge receipt mismatches source")
            }
            Self::UnsafeLocator => formatter.write_str("unsafe SDoc source locator"),
            Self::BridgeStatus { .. } => formatter.write_str("SDoc bridge rejected source"),
            Self::InvalidEnvelope(_) => formatter.write_str("invalid SDoc bridge envelope"),
        }
    }
}

impl std::error::Error for SdocError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bran-sdoc-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn envelope(locator: &str, source: &[u8]) -> String {
        format!(
            r#"{{"document":{{"line_range":{{"end":3,"start":1}},"metadata":{{"type":"architecture"}},"mid":"MID-DOCUMENT-001","title":"Spec","uid":"DOC-001"}},"engine":{{"api":"{STRICTDOC_API}","artifact_sha256":"{STRICTDOC_ARTIFACT_SHA256}","requirements_sha256":"{STRICTDOC_REQUIREMENTS_SHA256}","version":"{STRICTDOC_VERSION}"}},"mode":"{BRIDGE_MODE}","nodes":[{{"fields":{{"STATUS":"Draft"}},"line_range":{{"end":3,"start":2}},"mid":"MID-REQ-001","node_type":"Requirement","uid":"REQ-001"}}],"protocol":"{BRIDGE_PROTOCOL}","relations":[{{"line_range":{{"end":3,"start":3}},"mid":"MID-REL-001","relation_type":"Reference","source_mid":"MID-REQ-001","target_mid":"MID-DOCUMENT-001","type":"relates"}}],"schema_version":"{BRIDGE_SCHEMA_VERSION}","source":{{"locator":"{locator}","sha256":"{}"}},"status":"ok","validation":{{"status":"valid"}}}}"#,
            sha256(source)
        )
    }

    fn bridge(root: &Path, body: &str) -> SdocBridgeConfig {
        let script = root.join("bridge.sh");
        let wheel = root.join("strictdoc.whl");
        fs::write(&script, body).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(&wheel, b"qualified test wheel").unwrap();
        SdocBridgeConfig::new("/bin/sh", script, wheel).unwrap()
    }

    #[test]
    fn discovers_sdoc_with_absolute_bridge_and_preserves_exact_records() {
        let root = root();
        fs::create_dir_all(root.join("docs")).unwrap();
        let source = b"[DOCUMENT]\nTITLE: Spec\n";
        fs::write(root.join("docs/spec.sdoc"), source).unwrap();
        fs::write(root.join("docs/generated.md"), b"generated view").unwrap();
        let json = envelope("docs/spec.sdoc", source);
        let config = bridge(&root, &format!("#!/bin/sh\nprintf '%s' '{json}'\n"));
        let scanner = SdocScanner::new(&root, config).unwrap();
        let receipts = scanner.scan().unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].source_locator, "docs/spec.sdoc");
        assert_eq!(receipts[0].document.mid, "MID-DOCUMENT-001");
        assert_eq!(receipts[0].nodes[0].fields["STATUS"], "Draft");
        assert_eq!(receipts[0].relations[0].target_mid, "MID-DOCUMENT-001");
        assert_eq!(fs::read(root.join("docs/spec.sdoc")).unwrap(), source);
        assert!(matches!(
            SdocBridgeConfig::new("bridge", "/tmp/bridge", "/tmp/wheel"),
            Err(SdocError::RelativeBridgeProgram)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    /// W6.1-P1-01: the pinned invocation must not let a `sitecustomize` on the
    /// bridge's own import root execute before the bridge verifies anything.
    /// The packaged launcher exported `PYTHONPATH`, which did exactly that, so
    /// this pins the replacement shape: `-I -S` plus [`BRIDGE_BOOTSTRAP`].
    #[test]
    fn pinned_startup_never_executes_site_customisation_before_the_bridge() {
        let Ok(interpreter) = which_python3() else {
            return;
        };
        let root = root();
        let site = root.join("site-packages");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("sitecustomize.py"), b"print('SITE_CUSTOMIZE')\n").unwrap();
        fs::write(site.join("usercustomize.py"), b"print('USER_CUSTOMIZE')\n").unwrap();
        // Imported from the appended root, so a missing `-B` would leave a
        // `__pycache__` entry behind and move the closure digest.
        fs::write(site.join("pinned_module.py"), b"VALUE = 1\n").unwrap();
        let script = root.join("bridge.py");
        fs::write(
            &script,
            b"import sys\nimport pinned_module\nprint('BRIDGE', sys.argv[1])\n",
        )
        .unwrap();
        let pinned_closure = site_closure_digest(&site).unwrap();

        let output = Command::new(&interpreter)
            .arg("-I")
            .arg("-S")
            .arg("-B")
            .arg("-c")
            .arg(BRIDGE_BOOTSTRAP)
            .arg(&site)
            .arg(&script)
            .arg("--mode")
            .env_clear()
            .env("LC_ALL", "C")
            // A hostile inherited PYTHONPATH must be ignored as well.
            .env("PYTHONPATH", &site)
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(!stdout.contains("SITE_CUSTOMIZE"), "{stdout}");
        assert!(!stdout.contains("USER_CUSTOMIZE"), "{stdout}");
        assert!(stdout.contains("BRIDGE --mode"), "{stdout}");
        // Without `-B` the run writes `__pycache__` into the import root, so
        // the closure BRAN just verified would no longer match its pin on the
        // next run wherever site-packages is writable.
        assert_eq!(site_closure_digest(&site).unwrap(), pinned_closure);
        fs::remove_dir_all(root).unwrap();
    }

    /// Builds a small stand-in for the pinned import root: a top-level module,
    /// a package, and a compiled artifact, mirroring what a wheel installs.
    fn site_fixture(root: &Path) -> PathBuf {
        let site = root.join("site-packages");
        fs::create_dir_all(site.join("strictdoc/__pycache__")).unwrap();
        fs::create_dir_all(site.join("strictdoc-0.29.0.dist-info")).unwrap();
        fs::write(site.join("typing_extensions.py"), b"VERSION = 1\n").unwrap();
        fs::write(site.join("strictdoc/__init__.py"), b"import sys\n").unwrap();
        fs::write(site.join("strictdoc/__pycache__/__init__.pyc"), b"\x00pyc").unwrap();
        fs::write(
            site.join("strictdoc-0.29.0.dist-info/RECORD"),
            b"strictdoc/__init__.py,,\n",
        )
        .unwrap();
        site
    }

    /// W6.1-P1-05: the bridge verifies only the files a locked RECORD claims,
    /// so an *added* importable file shadowed a pinned distribution and still
    /// produced a fully matching receipt. Everything under the appended import
    /// root is executable input, so the closure digest must move for any added,
    /// edited, or removed byte, and must refuse content it cannot describe.
    #[test]
    fn unpinned_content_in_the_import_root_breaks_the_site_closure_digest() {
        let root = root();
        let site = site_fixture(&root);
        let pinned = site_closure_digest(&site).unwrap();
        assert!(is_sha256(&pinned));
        assert_eq!(site_closure_digest(&site).unwrap(), pinned);

        // The reviewer's reproduction: an untracked package that no RECORD
        // claims, shadowing the pinned top-level `typing_extensions` module.
        let extra = site.join("typing_extensions");
        fs::create_dir_all(&extra).unwrap();
        fs::write(extra.join("__init__.py"), b"import os\n").unwrap();
        let shadowed = site_closure_digest(&site).unwrap();
        assert_ne!(shadowed, pinned);
        fs::remove_dir_all(&extra).unwrap();
        assert_eq!(site_closure_digest(&site).unwrap(), pinned);

        // An edited pinned file and a removed one move the digest too, which a
        // RECORD-coverage check alone would not catch.
        fs::write(site.join("strictdoc/__init__.py"), b"import os\n").unwrap();
        assert_ne!(site_closure_digest(&site).unwrap(), pinned);
        fs::remove_file(site.join("strictdoc/__init__.py")).unwrap();
        assert_ne!(site_closure_digest(&site).unwrap(), pinned);

        // A symlink and an empty root are unpinnable rather than merely
        // different, so neither can be made to match by choosing a pin.
        let bare = root.join("bare");
        fs::create_dir_all(&bare).unwrap();
        assert!(matches!(
            site_closure_digest(&bare),
            Err(SdocError::UnpinnedRuntimeContent)
        ));
        std::os::unix::fs::symlink(root.join("elsewhere.py"), site.join("linked.py")).unwrap();
        assert!(matches!(
            site_closure_digest(&site),
            Err(SdocError::UnpinnedRuntimeContent)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    /// The build gate must reject a drifted pin *before* cargo runs, so its
    /// shell definition has to produce the byte-identical digest this module
    /// recomputes at run time. A silent divergence would ship a binary that
    /// fails closed on the user's machine, so the two are compared here.
    #[test]
    fn site_closure_digest_matches_the_build_gate_definition() {
        let gate = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../build/closure-digest.sh");
        let root = root();
        let site = site_fixture(&root);
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(". \"$1\"; site_closure_digest \"$2\"")
            .arg("sh")
            .arg(&gate)
            .arg(&site)
            .output()
            .unwrap();
        let shell = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if shell.is_empty() {
            // No GNU find or sha256sum here; the gate itself is not exercised.
            fs::remove_dir_all(root).unwrap();
            return;
        }
        assert_eq!(shell, site_closure_digest(&site).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    /// Resolves a system `python3` the way `sys.executable` would, or reports
    /// its absence so the startup regression skips rather than installs one.
    fn which_python3() -> Result<PathBuf, ()> {
        ["/usr/bin/python3", "/bin/python3", "/usr/local/bin/python3"]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.is_file())
            .map(|path| fs::canonicalize(&path).unwrap_or(path))
            .ok_or(())
    }

    /// The requirements-system bridge reports a relation's owner and revision
    /// when the document carries them (azedge#74).  They are optional: an
    /// older bridge omits them, a present value must be a non-empty string.
    #[test]
    fn relation_owner_and_revision_are_optional_but_never_blank() {
        let source = b"[DOCUMENT]\n";
        let valid = envelope("spec.sdoc", source);
        let digest = sha256(source);
        let bare = parse_bridge_envelope(valid.as_bytes(), "spec.sdoc", &digest).unwrap();
        assert_eq!(bare.relations[0].owner, None);
        assert_eq!(bare.relations[0].revision, None);

        let mut value: Value = serde_json::from_str(&valid).unwrap();
        let relation = value["relations"][0].as_object_mut().unwrap();
        relation.insert("owner".to_owned(), Value::from("Pipeline specification"));
        relation.insert("revision".to_owned(), Value::from("draft-unpublished"));
        let carried =
            parse_bridge_envelope(value.to_string().as_bytes(), "spec.sdoc", &digest).unwrap();
        assert_eq!(
            carried.relations[0].owner.as_deref(),
            Some("Pipeline specification")
        );
        assert_eq!(
            carried.relations[0].revision.as_deref(),
            Some("draft-unpublished")
        );

        value["relations"][0]["owner"] = Value::from("  ");
        assert!(matches!(
            parse_bridge_envelope(value.to_string().as_bytes(), "spec.sdoc", &digest),
            Err(SdocError::InvalidEnvelope("relation owner"))
        ));
    }

    /// Captured verbatim from the packaged requirements-system bridge
    /// (0.1.0-12) run with `--schema-version 2 --mode sdoc` over the qualified
    /// wheelhouse, so the parse and the runtime receipt are checked against
    /// what the engine actually emits rather than a hand-written shape.
    const SCHEMA_TWO_RUN: &str = r#"{"document":{"line_range":{"end":38,"start":1},"metadata":{"published_revision":"REV-1"},"mid":"b9a2890f72ad4c3da988648417e33ad4","title":"Bridge Fixture","uid":"FIXTURE-DOC"},"engine":{"api":"strictdoc.api","artifact_sha256":"fae511b228952ee5e1ff765650ac2701526ce39e32a6686f53ef384621486a90","bridge_sha256":"267a2225fe821921e13fcbaba33b3dc332fa6506ccc60b10ea246536dbd8d676","python_sha256":"a92f0f95e883390c7256b2e441484aac06b1002dbe1d924141a77c8d82f96223","requirements_sha256":"77b879886d9856ca748e181b592e78efa377d432953ec52d6e61803cf10ef9c8","version":"0.29.0","wheelhouse_sha256":"aa961f820c4a3d8fd3646e5dadb0f3993051e028a9ce8ab977e1db74619872a9"},"mode":"sdoc","nodes":[{"fields":{"OWNER":"Bridge fixture","STATEMENT":"The bridge reports a schema 2 envelope.","UID":"REQ-1"},"line_range":{"end":29,"start":25},"mid":"f9cbb2050ef541ffae88e67dc9eea43e","node_type":"REQUIREMENT","uid":"REQ-1"},{"fields":{"OWNER":"Bridge fixture","STATEMENT":"The consumer accepts it.","UID":"REQ-2"},"line_range":{"end":38,"start":30},"mid":"726c9e0c95ee400c915b7f73233571da","node_type":"REQUIREMENT","uid":"REQ-2"}],"protocol":"alphazede.strictdoc.bridge","relations":[{"line_range":{"end":38,"start":35},"mid":"0a46bb7e28b7e206f84fee642f7b398e","owner":"Bridge fixture","relation_type":"Parent","revision":"REV-1","source_mid":"726c9e0c95ee400c915b7f73233571da","target_mid":"f9cbb2050ef541ffae88e67dc9eea43e","type":"derives-from"}],"schema_version":"2","source":{"locator":"spec.sdoc","sha256":"0e1e16250e6052cf33edf4745330e3ae47a4f956892813e522140da071c3d4e7"},"status":"ok","validation":{"status":"valid"}}"#;
    const RUN_INTERPRETER_SHA256: &str =
        "a92f0f95e883390c7256b2e441484aac06b1002dbe1d924141a77c8d82f96223";
    const RUN_BRIDGE_SHA256: &str =
        "267a2225fe821921e13fcbaba33b3dc332fa6506ccc60b10ea246536dbd8d676";
    const RUN_WHEELHOUSE_SHA256: &str =
        "aa961f820c4a3d8fd3646e5dadb0f3993051e028a9ce8ab977e1db74619872a9";
    const RUN_SOURCE_SHA256: &str =
        "0e1e16250e6052cf33edf4745330e3ae47a4f956892813e522140da071c3d4e7";

    fn qualified_config() -> SdocBridgeConfig {
        SdocBridgeConfig {
            program: PathBuf::from("/usr/lib/engine/bin/python"),
            interpreter: PathBuf::from("/usr/bin/python3.12"),
            bridge: PathBuf::from("/usr/lib/engine/strictdoc_bridge.py"),
            wheel: PathBuf::from("/usr/lib/engine/wheels/strictdoc.whl"),
            wheelhouse: PathBuf::from("/usr/lib/engine/wheels"),
            site_packages: PathBuf::from("/usr/lib/engine/site-packages"),
            hashes: PathBuf::from("/usr/lib/engine/hashes.txt"),
            // The launcher file digest is local-only; it is deliberately not
            // the interpreter digest the bridge reports.
            python_sha256: "1".repeat(64),
            interpreter_sha256: RUN_INTERPRETER_SHA256.to_owned(),
            bridge_sha256: RUN_BRIDGE_SHA256.to_owned(),
            wheelhouse_sha256: RUN_WHEELHOUSE_SHA256.to_owned(),
            site_packages_sha256: "2".repeat(64),
            hashes_sha256: STRICTDOC_REQUIREMENTS_SHA256.to_owned(),
        }
    }

    #[test]
    fn parses_a_real_schema_two_bridge_run() {
        let receipt =
            parse_bridge_envelope(SCHEMA_TWO_RUN.as_bytes(), "spec.sdoc", RUN_SOURCE_SHA256)
                .unwrap();
        assert_eq!(receipt.engine.version, STRICTDOC_VERSION);
        assert_eq!(receipt.document.uid, "FIXTURE-DOC");
        assert_eq!(receipt.document.metadata["published_revision"], "REV-1");
        assert_eq!(receipt.nodes.len(), 2);
        assert_eq!(receipt.nodes[1].fields["UID"], "REQ-2");
        assert_eq!(receipt.relations.len(), 1);
        assert_eq!(receipt.relations[0].relation_type, "derives-from");
        assert_eq!(receipt.relations[0].reference_type, "Parent");
        assert_eq!(
            receipt.relations[0].owner.as_deref(),
            Some("Bridge fixture")
        );
        assert_eq!(receipt.relations[0].revision.as_deref(), Some("REV-1"));
    }

    /// The schema-2 engine block echoes the qualified runtime identity it
    /// verified.  Any drift in it, or an engine block that omits it, stays a
    /// runtime mismatch rather than a receipt.
    #[test]
    fn runtime_receipt_matches_only_the_pinned_qualified_runtime() {
        let config = qualified_config();
        assert!(runtime_receipt_matches(SCHEMA_TWO_RUN.as_bytes(), &config));

        for key in ["python_sha256", "bridge_sha256", "wheelhouse_sha256"] {
            let mut value: Value = serde_json::from_str(SCHEMA_TWO_RUN).unwrap();
            value["engine"][key] = Value::String("0".repeat(64));
            assert!(
                !runtime_receipt_matches(value.to_string().as_bytes(), &config),
                "{key} drift"
            );
            let mut value: Value = serde_json::from_str(SCHEMA_TWO_RUN).unwrap();
            value["engine"].as_object_mut().unwrap().remove(key);
            assert!(
                !runtime_receipt_matches(value.to_string().as_bytes(), &config),
                "{key} absent"
            );
        }

        // The launcher file digest is never what the bridge reports.
        let mut launcher_only = config.clone();
        launcher_only.interpreter_sha256 = launcher_only.python_sha256.clone();
        assert!(!runtime_receipt_matches(
            SCHEMA_TWO_RUN.as_bytes(),
            &launcher_only
        ));
        assert!(!runtime_receipt_matches(b"not-json", &config));
    }

    #[test]
    fn rejects_malformed_and_unqualified_envelopes() {
        let source = b"[DOCUMENT]\n";
        let valid = envelope("spec.sdoc", source);
        let mut cases = vec![("malformed", "not-json".to_owned(), SdocError::MalformedJson)];
        let mut value: Value = serde_json::from_str(&valid).unwrap();
        value.as_object_mut().unwrap().remove("validation");
        cases.push((
            "missing",
            value.to_string(),
            SdocError::InvalidEnvelope("envelope"),
        ));
        let mut value: Value = serde_json::from_str(&valid).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("status".to_owned(), Value::from(7));
        cases.push((
            "wrong-type",
            value.to_string(),
            SdocError::InvalidEnvelope("envelope"),
        ));
        let mut value: Value = serde_json::from_str(&valid).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("extra".to_owned(), Value::Bool(true));
        cases.push((
            "unknown",
            value.to_string(),
            SdocError::InvalidEnvelope("envelope"),
        ));
        let mut value: Value = serde_json::from_str(&valid).unwrap();
        value.as_object_mut().unwrap().insert(
            "protocol".to_owned(),
            Value::String("wrong.protocol".to_owned()),
        );
        cases.push(("protocol", value.to_string(), SdocError::Protocol));
        let mut value: Value = serde_json::from_str(&valid).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("schema_version".to_owned(), Value::String("3".to_owned()));
        cases.push(("schema", value.to_string(), SdocError::SchemaVersion));
        let mut value: Value = serde_json::from_str(&valid).unwrap();
        value.as_object_mut().unwrap().remove("mode");
        cases.push((
            "missing-mode",
            value.to_string(),
            SdocError::InvalidEnvelope("envelope"),
        ));
        let mut value: Value = serde_json::from_str(&valid).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("mode".to_owned(), Value::String("foreign-reqif".to_owned()));
        cases.push(("mode", value.to_string(), SdocError::ModeMismatch));
        let mut value: Value = serde_json::from_str(&valid).unwrap();
        value["engine"]["version"] = Value::String("0.28.0".to_owned());
        cases.push(("engine", value.to_string(), SdocError::EngineMismatch));
        let mut value: Value = serde_json::from_str(&valid).unwrap();
        value["source"]["locator"] = Value::String("../spec.sdoc".to_owned());
        cases.push(("locator", value.to_string(), SdocError::UnsafeLocator));
        let mut value: Value = serde_json::from_str(&valid).unwrap();
        value["source"]["sha256"] = Value::String("0".repeat(64));
        cases.push((
            "digest",
            value.to_string(),
            SdocError::SourceReceiptMismatch,
        ));
        let mut value: Value = serde_json::from_str(&valid).unwrap();
        value.as_object_mut().unwrap().remove("engine");
        cases.push((
            "missing-engine",
            value.to_string(),
            SdocError::InvalidEnvelope("envelope"),
        ));
        let mut value: Value = serde_json::from_str(&valid).unwrap();
        value["document"].as_object_mut().unwrap().remove("mid");
        cases.push((
            "missing-document-mid",
            value.to_string(),
            SdocError::InvalidEnvelope("document"),
        ));
        let mut value: Value = serde_json::from_str(&valid).unwrap();
        value["nodes"][0]
            .as_object_mut()
            .unwrap()
            .insert("extra".to_owned(), Value::Bool(true));
        cases.push((
            "unknown-node-field",
            value.to_string(),
            SdocError::InvalidEnvelope("node"),
        ));
        let mut value: Value = serde_json::from_str(&valid).unwrap();
        value["relations"][0]["line_range"]["start"] = Value::String("one".to_owned());
        cases.push((
            "wrong-line-type",
            value.to_string(),
            SdocError::InvalidEnvelope("relation line range"),
        ));
        for (name, input, expected) in cases {
            assert!(
                matches!(
                    parse_bridge_envelope(input.as_bytes(), "spec.sdoc", &sha256(source)),
                    Err(actual) if std::mem::discriminant(&actual) == std::mem::discriminant(&expected)
                ),
                "{name}"
            );
        }
        assert!(matches!(
            validate_locator("C:/not-relative"),
            Err(SdocError::UnsafeLocator)
        ));
    }

    #[test]
    fn nonzero_and_source_mutation_fail_closed() {
        let root = root();
        let source = b"[DOCUMENT]\nTITLE: Spec\n";
        fs::write(root.join("spec.sdoc"), source).unwrap();
        let nonzero = SdocScanner::new(&root, bridge(&root, "#!/bin/sh\nexit 7\n")).unwrap();
        assert!(matches!(
            nonzero.scan(),
            Err(SdocError::BridgeExit(Some(7)))
        ));

        let typed_failure = SdocScanner::new(
            &root,
            bridge(
                &root,
                "#!/bin/sh\nprintf '%s' '{\"error\":{\"code\":\"STRICTDOC_REJECTED\"},\"protocol\":\"alphazede.strictdoc.bridge\",\"schema_version\":\"2\",\"status\":\"parse_error\"}'\nexit 2\n",
            ),
        )
        .unwrap();
        assert!(matches!(
            typed_failure.scan(),
            Err(SdocError::BridgeStatus { status, code })
                if status == "parse_error" && code == "STRICTDOC_REJECTED"
        ));

        let json = envelope("spec.sdoc", source);
        let mutating = format!(
            "#!/bin/sh\nprevious=\nfor value in \"$@\"; do\n  if [ \"$previous\" = \"--source\" ]; then printf changed > \"$value\"; break; fi\n  previous=\"$value\"\ndone\nprintf '%s' '{json}'\n"
        );
        let scanner = SdocScanner::new(&root, bridge(&root, &mutating)).unwrap();
        assert!(
            matches!(scanner.scan(), Err(SdocError::SourceChanged(path)) if path == "spec.sdoc")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
