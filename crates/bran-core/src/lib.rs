//! Exact-release verification primitives for Bran distribution metadata.

pub mod adapters;
pub mod agent;
pub mod boundary;
pub mod bundle;
pub mod derived_state;
pub mod export;
pub mod frontmatter;
pub mod graph;
pub mod metadata;
pub mod migration;
pub mod packet;
pub mod policy;
pub mod profile;
pub mod repair;
pub mod scan;
pub mod schema;
pub mod view;

// Profile validation exports (Slice 1.2 wiring only)
pub use crate::profile::{
    Diagnostic, ProfileOutcome, ProfileValidator, ValidationResult, ValidationStatus, BRAN_STRICT,
    OKF_V0_1, OKF_V0_2,
};

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

const DOWNLOAD_ROOT: &str = "https://github.com/alphazede/bran/releases/download/";
const CHECKSUMS: &str = "SHA256SUMS";
const CHECKSUMS_SIGNATURE: &str = "SHA256SUMS.sig";
const MANIFEST: &str = "bran-release-manifest.json";

/// A validated immutable Bran release tag.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ReleaseTag(String);

impl ReleaseTag {
    /// Parses `bran-vX.Y.Z`, where each semver component is numeric and canonical.
    pub fn parse(value: &str) -> Result<Self, ReleaseVerificationError> {
        let Some(version) = value.strip_prefix("bran-v") else {
            return Err(ReleaseVerificationError::MalformedTag(value.to_owned()));
        };

        let mut components = version.split('.');
        let valid = components.clone().count() == 3 && components.all(is_semver_number);
        if !valid {
            return Err(ReleaseVerificationError::MalformedTag(value.to_owned()));
        }

        Ok(Self(value.to_owned()))
    }

    /// Returns the exact release tag.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ReleaseTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// An immutable release asset and its public download URL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseAsset {
    pub name: String,
    pub url: String,
}

impl ReleaseAsset {
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
        }
    }
}

/// A declared asset entry from a Slice 1.1 release manifest.
///
/// Each carries its exact name, direct download URL, sha256 digest,
/// and media type. These are the seven hashed assets only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeclaredAsset {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub media_type: String,
}

impl DeclaredAsset {
    pub fn new(
        name: impl Into<String>,
        url: impl Into<String>,
        sha256: impl Into<String>,
        media_type: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            sha256: sha256.into(),
            media_type: media_type.into(),
        }
    }
}

/// Checksums metadata section. Binds the SHA256SUMS asset digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeclaredChecksums {
    pub asset: String,
    pub algorithm: String,
    pub sha256: String,
}

/// Declared OpenPGP signature metadata (shape only).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeclaredSignature {
    pub asset: String,
    pub format: String,
    pub key_fingerprint: String,
    pub signed_at: String,
}

/// Declared SLSA v1 provenance metadata (shape and binding only).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeclaredProvenance {
    pub format: String,
    pub predicate_type: String,
    pub source_repository: String,
    pub source_commit: String,
    pub lockfile_sha256: String,
    pub build_type: String,
}

/// The typed declared manifest structure owned by ReleaseVerifier.
///
/// ReleaseVerifier performs semantic verification over an instance of this
/// structure. Callers construct it directly (no JSON parsing or serde is
/// performed by this crate; Python fixtures + oracle remain the source of
/// truth for the contract).
///
/// This verifies declared Slice 1.1 metadata structure and semantic binding only.
/// It does NOT fetch bytes, recompute digests, execute OpenPGP verification,
/// or attest provenance. Those later cryptographic operations remain outside
/// this slice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeclaredReleaseManifest {
    pub schema_version: String,
    pub tag: String,
    pub repository: String,
    pub source_commit: String,
    pub lockfile_sha256: String,
    pub immutable: bool,
    pub manifest_asset: String,
    pub assets: Vec<DeclaredAsset>,
    pub checksums: DeclaredChecksums,
    pub signature: DeclaredSignature,
    pub provenance: DeclaredProvenance,
}

/// Reasons release metadata fails exact-release verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReleaseVerificationError {
    MalformedTag(String),
    LatestUrl(String),
    InvalidAssetUrl { expected: String, actual: String },
    UnexpectedAsset(String),
    DuplicateAsset(String),
    MissingAssets(Vec<String>),
    // Slice 1.1 manifest semantic errors (deterministic variants/messages)
    MalformedSchemaVersion(String),
    MalformedRepository(String),
    MalformedSourceCommit(String),
    MalformedLockfileDigest(String),
    ImmutableMustBeTrue,
    ManifestAssetMismatch(String),
    MalformedAssetSha256 { asset: String, value: String },
    EmptyAssetMediaType(String),
    ChecksumsAssetMismatch { expected: String, actual: String },
    ChecksumsAlgorithmMismatch,
    ChecksumsDigestMismatch { checksums: String, asset: String },
    MalformedChecksumsDigest(String),
    SignatureAssetMismatch { expected: String, actual: String },
    SignatureFormatMismatch { expected: String, actual: String },
    MalformedSignatureFingerprint(String),
    MalformedSignatureTimestamp(String),
    ProvenanceFormatMismatch,
    ProvenancePredicateTypeMismatch,
    ProvenanceSourceRepositoryMismatch(String),
    ProvenanceSourceCommitMismatch { expected: String, actual: String },
    ProvenanceLockfileMismatch { expected: String, actual: String },
    MalformedProvenanceBuildType(String),
}

impl fmt::Display for ReleaseVerificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedTag(tag) => write!(formatter, "malformed exact release tag: {tag}"),
            Self::LatestUrl(url) => {
                write!(formatter, "floating latest release URL is forbidden: {url}")
            }
            Self::InvalidAssetUrl { expected, actual } => {
                write!(
                    formatter,
                    "invalid immutable asset URL: expected {expected}, got {actual}"
                )
            }
            Self::UnexpectedAsset(name) => write!(formatter, "unexpected release asset: {name}"),
            Self::DuplicateAsset(name) => write!(formatter, "duplicate release asset: {name}"),
            Self::MissingAssets(names) => {
                write!(formatter, "missing release assets: {}", names.join(", "))
            }
            Self::MalformedSchemaVersion(v) => {
                write!(formatter, "schema_version must be 1.0.0: {v}")
            }
            Self::MalformedRepository(r) => {
                write!(formatter, "repository must be alphazede/bran: {r}")
            }
            Self::MalformedSourceCommit(c) => {
                write!(
                    formatter,
                    "source_commit must be a lowercase 40-hex git SHA-1: {c}"
                )
            }
            Self::MalformedLockfileDigest(d) => {
                write!(
                    formatter,
                    "lockfile_sha256 must be a lowercase 64-hex value: {d}"
                )
            }
            Self::ImmutableMustBeTrue => write!(formatter, "immutable must be true"),
            Self::ManifestAssetMismatch(m) => {
                write!(
                    formatter,
                    "manifest_asset must be bran-release-manifest.json: {m}"
                )
            }
            Self::MalformedAssetSha256 { asset, value } => {
                write!(
                    formatter,
                    "asset {asset} sha256 must be lowercase 64-hex: {value}"
                )
            }
            Self::EmptyAssetMediaType(name) => {
                write!(formatter, "asset {name} has empty media_type")
            }
            Self::ChecksumsAssetMismatch { expected, actual } => {
                write!(formatter, "checksums.asset must be {expected}: {actual}")
            }
            Self::ChecksumsAlgorithmMismatch => {
                write!(formatter, "checksums.algorithm must be sha256")
            }
            Self::ChecksumsDigestMismatch { checksums, asset } => {
                write!(
                    formatter,
                    "checksums.sha256 must match the SHA256SUMS asset sha256: checksums={checksums} asset={asset}"
                )
            }
            Self::MalformedChecksumsDigest(d) => {
                write!(formatter, "checksums.sha256 must be lowercase 64-hex: {d}")
            }
            Self::SignatureAssetMismatch { expected, actual } => {
                write!(formatter, "signature.asset must be {expected}: {actual}")
            }
            Self::SignatureFormatMismatch { expected, actual } => {
                write!(formatter, "signature.format must be {expected}: {actual}")
            }
            Self::MalformedSignatureFingerprint(f) => {
                write!(
                    formatter,
                    "signature.key_fingerprint must be exactly 40 or 64 lowercase hex: {f}"
                )
            }
            Self::MalformedSignatureTimestamp(t) => {
                write!(
                    formatter,
                    "signature.signed_at must be strict UTC YYYY-MM-DDTHH:MM:SSZ: {t}"
                )
            }
            Self::ProvenanceFormatMismatch => {
                write!(
                    formatter,
                    "provenance.format must be https://slsa.dev/provenance/v1"
                )
            }
            Self::ProvenancePredicateTypeMismatch => {
                write!(
                    formatter,
                    "provenance.predicate_type must be https://slsa.dev/provenance/v1"
                )
            }
            Self::ProvenanceSourceRepositoryMismatch(r) => {
                write!(
                    formatter,
                    "provenance.source_repository must be alphazede/bran: {r}"
                )
            }
            Self::ProvenanceSourceCommitMismatch { expected, actual } => {
                write!(
                    formatter,
                    "provenance.source_commit must match top-level source_commit: expected {expected}, got {actual}"
                )
            }
            Self::ProvenanceLockfileMismatch { expected, actual } => {
                write!(
                    formatter,
                    "provenance.lockfile_sha256 must match top-level lockfile_sha256: expected {expected}, got {actual}"
                )
            }
            Self::MalformedProvenanceBuildType(b) => {
                write!(formatter, "provenance.build_type must be an https URL: {b}")
            }
        }
    }
}

impl Error for ReleaseVerificationError {}

/// Validates release metadata without network or publishing behavior.
pub struct ReleaseVerifier;

impl ReleaseVerifier {
    /// The complete immutable (hashed) asset names for this release.
    ///
    /// Exactly seven assets are required and order-insensitive:
    /// five platform archives plus SHA256SUMS and SHA256SUMS.sig.
    /// The bran-release-manifest.json is declared via manifest_asset
    /// but is distributed without a self-digest entry in the hashed assets.
    ///
    /// Platform archives use `<exact-tag>-<target>.<archive>`:
    /// `.tar.gz` for Unix targets and `.zip` for Windows.
    pub fn required_asset_names(tag: &ReleaseTag) -> BTreeSet<String> {
        [
            format!("{tag}-x86_64-unknown-linux-gnu.tar.gz"),
            format!("{tag}-aarch64-unknown-linux-gnu.tar.gz"),
            format!("{tag}-x86_64-apple-darwin.tar.gz"),
            format!("{tag}-aarch64-apple-darwin.tar.gz"),
            format!("{tag}-x86_64-pc-windows-msvc.zip"),
            CHECKSUMS.to_owned(),
            CHECKSUMS_SIGNATURE.to_owned(),
        ]
        .into_iter()
        .collect()
    }

    /// Validates that a URL is a direct download for an expected asset.
    pub fn validate_asset_url(
        tag: &ReleaseTag,
        name: &str,
        url: &str,
    ) -> Result<(), ReleaseVerificationError> {
        if url.contains("/releases/latest") {
            return Err(ReleaseVerificationError::LatestUrl(url.to_owned()));
        }

        let expected = format!("{DOWNLOAD_ROOT}{tag}/{name}");
        if url != expected {
            return Err(ReleaseVerificationError::InvalidAssetUrl {
                expected,
                actual: url.to_owned(),
            });
        }

        Ok(())
    }

    /// Validates the exact tag, direct immutable URLs, and complete asset set.
    pub fn verify(tag: &str, assets: &[ReleaseAsset]) -> Result<(), ReleaseVerificationError> {
        let tag = ReleaseTag::parse(tag)?;
        let required = Self::required_asset_names(&tag);
        let mut received = BTreeSet::new();

        for asset in assets {
            if !required.contains(&asset.name) {
                return Err(ReleaseVerificationError::UnexpectedAsset(
                    asset.name.clone(),
                ));
            }
            if !received.insert(asset.name.clone()) {
                return Err(ReleaseVerificationError::DuplicateAsset(asset.name.clone()));
            }
            Self::validate_asset_url(&tag, &asset.name, &asset.url)?;
        }

        let missing: Vec<_> = required.difference(&received).cloned().collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(ReleaseVerificationError::MissingAssets(missing))
        }
    }

    /// Verifies a declared Slice 1.1 release manifest for structure and
    /// semantic binding.
    ///
    /// Covers:
    /// - schema_version == "1.0.0"
    /// - canonical bran-vX.Y.Z tag (via ReleaseTag)
    /// - repository == "alphazede/bran"
    /// - lowercase 40-hex source_commit (git SHA-1)
    /// - lowercase 64-hex lockfile_sha256
    /// - immutable == true
    /// - manifest_asset == "bran-release-manifest.json" (distributed without self-digest)
    /// - exactly seven order-insensitive hashed assets (5 platform + SHA256SUMS + SHA256SUMS.sig)
    /// - exact tag/name/direct URL binding for every asset (no /latest)
    /// - each asset has lowercase 64-hex sha256 and non-blank media_type
    /// - checksums bound to the SHA256SUMS asset's sha256 (with correct asset/algorithm)
    /// - OpenPGP signature metadata: asset, format=openpgp, exactly 40 or 64 lowercase hex fingerprint, strict UTC YYYY-MM-DDTHH:MM:SSZ signed_at (full Gregorian calendar/leap-day validated)
    /// - SLSA v1 provenance: format/predicate consts, repository const, source_commit/lockfile_sha256 cross-bound to top level (40/64 hex), build_type https://...
    ///
    /// This verifies declared Slice 1.1 metadata structure/semantic binding only.
    /// It does NOT fetch bytes, recompute digests, execute OpenPGP verification,
    /// or attest provenance—those later cryptographic operations remain outside this slice.
    ///
    /// Python (tools/ci/release_contract_check.py) is the fixture/schema semantic oracle.
    /// No serde or JSON parsing is used here.
    pub fn verify_manifest(
        manifest: &DeclaredReleaseManifest,
    ) -> Result<(), ReleaseVerificationError> {
        if manifest.schema_version != "1.0.0" {
            return Err(ReleaseVerificationError::MalformedSchemaVersion(
                manifest.schema_version.clone(),
            ));
        }

        let tag = ReleaseTag::parse(&manifest.tag)?;

        if manifest.repository != "alphazede/bran" {
            return Err(ReleaseVerificationError::MalformedRepository(
                manifest.repository.clone(),
            ));
        }

        if !is_git_sha(&manifest.source_commit) {
            return Err(ReleaseVerificationError::MalformedSourceCommit(
                manifest.source_commit.clone(),
            ));
        }

        if !is_sha256(&manifest.lockfile_sha256) {
            return Err(ReleaseVerificationError::MalformedLockfileDigest(
                manifest.lockfile_sha256.clone(),
            ));
        }

        if !manifest.immutable {
            return Err(ReleaseVerificationError::ImmutableMustBeTrue);
        }

        if manifest.manifest_asset != MANIFEST {
            return Err(ReleaseVerificationError::ManifestAssetMismatch(
                manifest.manifest_asset.clone(),
            ));
        }

        // Exactly seven hashed assets, order-insensitive, no manifest self-hash.
        let required = Self::required_asset_names(&tag);
        let mut received = BTreeSet::new();

        for asset in &manifest.assets {
            if !required.contains(&asset.name) {
                return Err(ReleaseVerificationError::UnexpectedAsset(
                    asset.name.clone(),
                ));
            }
            if !received.insert(asset.name.clone()) {
                return Err(ReleaseVerificationError::DuplicateAsset(asset.name.clone()));
            }
            Self::validate_asset_url(&tag, &asset.name, &asset.url)?;

            if !is_sha256(&asset.sha256) {
                return Err(ReleaseVerificationError::MalformedAssetSha256 {
                    asset: asset.name.clone(),
                    value: asset.sha256.clone(),
                });
            }
            if asset.media_type.trim().is_empty() {
                return Err(ReleaseVerificationError::EmptyAssetMediaType(
                    asset.name.clone(),
                ));
            }
        }

        let missing: Vec<_> = required.difference(&received).cloned().collect();
        if !missing.is_empty() {
            return Err(ReleaseVerificationError::MissingAssets(missing));
        }

        // checksums binding
        let ch = &manifest.checksums;
        if ch.asset != CHECKSUMS {
            return Err(ReleaseVerificationError::ChecksumsAssetMismatch {
                expected: CHECKSUMS.to_owned(),
                actual: ch.asset.clone(),
            });
        }
        if ch.algorithm != "sha256" {
            return Err(ReleaseVerificationError::ChecksumsAlgorithmMismatch);
        }
        if !is_sha256(&ch.sha256) {
            return Err(ReleaseVerificationError::MalformedChecksumsDigest(
                ch.sha256.clone(),
            ));
        }
        let sums_asset_sha = manifest
            .assets
            .iter()
            .find(|a| a.name == CHECKSUMS)
            .map(|a| a.sha256.as_str())
            .unwrap_or("");
        if ch.sha256 != sums_asset_sha {
            return Err(ReleaseVerificationError::ChecksumsDigestMismatch {
                checksums: ch.sha256.clone(),
                asset: sums_asset_sha.to_owned(),
            });
        }

        // signature metadata (declared shape)
        let sig = &manifest.signature;
        if sig.asset != CHECKSUMS_SIGNATURE {
            return Err(ReleaseVerificationError::SignatureAssetMismatch {
                expected: CHECKSUMS_SIGNATURE.to_owned(),
                actual: sig.asset.clone(),
            });
        }
        if sig.format != "openpgp" {
            return Err(ReleaseVerificationError::SignatureFormatMismatch {
                expected: "openpgp".to_owned(),
                actual: sig.format.clone(),
            });
        }
        if !is_fingerprint(&sig.key_fingerprint) {
            return Err(ReleaseVerificationError::MalformedSignatureFingerprint(
                sig.key_fingerprint.clone(),
            ));
        }
        if !is_strict_utc_datetime(&sig.signed_at) {
            return Err(ReleaseVerificationError::MalformedSignatureTimestamp(
                sig.signed_at.clone(),
            ));
        }

        // provenance (declared, with cross-field binding to top level)
        let prov = &manifest.provenance;
        if prov.format != "https://slsa.dev/provenance/v1" {
            return Err(ReleaseVerificationError::ProvenanceFormatMismatch);
        }
        if prov.predicate_type != "https://slsa.dev/provenance/v1" {
            return Err(ReleaseVerificationError::ProvenancePredicateTypeMismatch);
        }
        if prov.source_repository != "alphazede/bran" {
            return Err(
                ReleaseVerificationError::ProvenanceSourceRepositoryMismatch(
                    prov.source_repository.clone(),
                ),
            );
        }
        if !is_git_sha(&prov.source_commit) {
            return Err(ReleaseVerificationError::MalformedSourceCommit(
                prov.source_commit.clone(),
            ));
        }
        if prov.source_commit != manifest.source_commit {
            return Err(ReleaseVerificationError::ProvenanceSourceCommitMismatch {
                expected: manifest.source_commit.clone(),
                actual: prov.source_commit.clone(),
            });
        }
        if !is_sha256(&prov.lockfile_sha256) {
            return Err(ReleaseVerificationError::MalformedLockfileDigest(
                prov.lockfile_sha256.clone(),
            ));
        }
        if prov.lockfile_sha256 != manifest.lockfile_sha256 {
            return Err(ReleaseVerificationError::ProvenanceLockfileMismatch {
                expected: manifest.lockfile_sha256.clone(),
                actual: prov.lockfile_sha256.clone(),
            });
        }
        if !prov.build_type.starts_with("https://") {
            return Err(ReleaseVerificationError::MalformedProvenanceBuildType(
                prov.build_type.clone(),
            ));
        }

        Ok(())
    }
}

fn is_semver_number(component: &str) -> bool {
    !component.is_empty()
        && component.bytes().all(|byte| byte.is_ascii_digit())
        && (component == "0" || !component.starts_with('0'))
}

/// Lowercase hex of exact length (no uppercase allowed).
fn is_lowercase_hex(s: &str, expected_len: usize) -> bool {
    s.len() == expected_len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_git_sha(s: &str) -> bool {
    is_lowercase_hex(s, 40)
}

fn is_sha256(s: &str) -> bool {
    is_lowercase_hex(s, 64)
}

fn is_fingerprint(s: &str) -> bool {
    let l = s.len();
    (l == 40 || l == 64) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Exact strict UTC shape: YYYY-MM-DDTHH:MM:SSZ (20 bytes, ASCII digits + separators + Z).
/// Then numeric range + real proleptic Gregorian calendar (incl. leap days) using
/// divisibility rules (no external crate, matches Python stdlib datetime semantics).
fn is_strict_utc_datetime(s: &str) -> bool {
    if s.len() != 20 {
        return false;
    }
    let b = s.as_bytes();
    if !(b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[19] == b'Z')
    {
        return false;
    }
    // YYYY-MM-DDTHH:MM:SS  digit segments
    let segs: [&[u8]; 6] = [
        &b[0..4],
        &b[5..7],
        &b[8..10],
        &b[11..13],
        &b[14..16],
        &b[17..19],
    ];
    for seg in &segs {
        if !seg.iter().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    let y = parse_u16(segs[0]);
    let m = parse_u8(segs[1]);
    let d = parse_u8(segs[2]);
    let h = parse_u8(segs[3]);
    let mi = parse_u8(segs[4]);
    let se = parse_u8(segs[5]);

    if !(1..=9999).contains(&y)
        || !(1..=12).contains(&m)
        || !(0..=23).contains(&h)
        || !(0..=59).contains(&mi)
        || !(0..=59).contains(&se)
    {
        return false;
    }
    let max_day = days_in_month(y, m);
    (1..=max_day).contains(&d)
}

fn parse_u16(digits: &[u8]) -> u16 {
    digits
        .iter()
        .fold(0u16, |acc, &c| acc * 10 + (c - b'0') as u16)
}

fn parse_u8(digits: &[u8]) -> u8 {
    digits.iter().fold(0u8, |acc, &c| acc * 10 + (c - b'0'))
}

#[allow(clippy::manual_is_multiple_of)]
fn is_leap_year(y: u16) -> bool {
    (y % 4 == 0) && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_month(y: u16, m: u8) -> u8 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}
