//! Admission of adapter projections into the #20 evidence envelope (issue #46).
//!
//! An adapter's `import` is routed by [`Format`] through the conformance
//! registry; only the projection reader below is per-format. Import runs
//! first, so package, XML, hazard, DLP, and public-boundary refusals keep their
//! typed codes; the envelope is then built from the projection alone and refused (never
//! invented) when it cannot satisfy the schema and its semantic oracle.

use crate::canonical::{sha256_hex, Json};
use crate::conformance::Adapter;
use crate::{Cancel, Format, Limits, Refusal};
use bran_core::export::{validate_emitted_string, ExportError};
use std::collections::BTreeMap;

/// Envelope `schema_version` this module writes.
pub const ENVELOPE_VERSION: &str = "1.0.0";
const MAX_ANCHORS: usize = 4096;
const MAX_TEXT_BYTES: usize = 8192;
const MAX_ENVELOPE_BYTES: usize = 1_048_576;
const MAX_ID_BYTES: usize = 256;
const MAX_LOCATOR_BYTES: usize = 1024;
const PARSER_IDENTITY: &str = "bran-document-adapter";
/// Must match `SECRET_MARKERS` in `tools/ci/enterprise_contract_check.py`: a
/// filename or anchor text carrying one refuses the envelope, since the oracle
/// would reject it as `secret-reflection`.
const SECRET_MARKERS: [&str; 8] = [
    "-----BEGIN ",
    "AIza",
    "X-Goog-Credential=",
    "X-Goog-Signature=",
    "access_token=",
    "private_key",
    "refresh_token=",
    "ya29.",
];

/// Validate a locator before admission or reflection in a refusal. The
/// envelope oracle's secret markers supplement the shared export validator.
pub fn validate_locator(locator: &str) -> Result<(), Refusal> {
    match validate_emitted_string(locator) {
        Err(ExportError::DlpViolation(_)) => return Err(Refusal::DlpFindings),
        Err(_) => return Err(Refusal::PublicBoundary),
        Ok(()) => {}
    }
    if SECRET_MARKERS.iter().any(|marker| locator.contains(marker)) {
        return Err(Refusal::PublicBoundary);
    }
    Ok(())
}

/// Where one anchor's text comes from. Native parse output is `Embedded`; a
/// projection that marks text OCR-derived is `Ocr`, and query/packet paths
/// must never rank it as byte-derived source text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Derivation {
    Embedded,
    Ocr,
}

impl Derivation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Embedded => "embedded",
            Self::Ocr => "ocr",
        }
    }
}

/// One envelope anchor plus the derivation the envelope does not carry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceAnchor {
    pub id: String,
    pub role: String,
    pub locator: Json,
    pub text: String,
    pub text_digest: String,
    pub derivation: Derivation,
}

/// An admitted document: the valid envelope and the anchors admitted from it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Admitted {
    pub envelope: Json,
    pub anchors: Vec<EvidenceAnchor>,
    pub fidelity: Json,
}

/// The registered adapter for `format`, if its issue has landed.
pub fn adapter(format: Format) -> Option<&'static dyn Adapter> {
    crate::conformance::registered()
        .into_iter()
        .find(|adapter| adapter.format() == format)
}

pub const fn media_type(format: Format) -> &'static str {
    match format {
        Format::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Format::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Format::Pptx => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        Format::Pdf => "application/pdf",
    }
}

pub const fn family(format: Format) -> &'static str {
    match format {
        Format::Docx => "flow",
        Format::Xlsx => "grid",
        Format::Pptx => "presentation",
        Format::Pdf => "fixed-layout",
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'/' | b':' | b'_' | b'-'))
}

fn valid_role(format: Format, role: &str) -> bool {
    matches!(
        (format, role),
        (
            Format::Pdf,
            "heading" | "paragraph" | "table" | "figure" | "header" | "footer" | "title" | "list"
        ) | (
            Format::Docx,
            "heading" | "paragraph" | "table" | "list" | "header" | "footer" | "title"
        ) | (
            Format::Pptx,
            "heading" | "paragraph" | "table" | "list" | "title" | "notes" | "shape"
        ) | (
            Format::Xlsx,
            "sheet" | "cell" | "range" | "table" | "header" | "chart"
        )
    )
}

fn valid_fidelity(fidelity: &Json) -> bool {
    match fidelity {
        Json::Obj(map) if !map.is_empty() => map.iter().all(|(key, value)| {
            let mut bytes = key.bytes();
            let head = bytes.next().is_some_and(|b| b.is_ascii_lowercase());
            head && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                && matches!(
                    value,
                    Json::Str(level)
                        if matches!(
                            level.as_str(),
                            "exact" | "normalized" | "approximated" | "unsupported"
                        )
                )
        }),
        _ => false,
    }
}

/// DOCX projection fidelity uses the adapter's hyphenated vocabulary, not the
/// envelope's flow keys; admission validates it and emits the conservative
/// envelope map below.
fn valid_docx_fidelity(fidelity: &Json) -> bool {
    match fidelity {
        // Plain text can have no observed normalization or omission.
        Json::Obj(map) => map.iter().all(|(key, value)| {
            let mut bytes = key.bytes();
            let head = bytes.next().is_some_and(|b| b.is_ascii_lowercase());
            head && bytes
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
                && matches!(
                    value,
                    Json::Str(level)
                        if matches!(
                            level.as_str(),
                            "exact" | "normalized" | "approximated" | "unsupported"
                        )
                )
        }),
        _ => false,
    }
}

/// Envelope `flow` fidelity the DOCX adapter can carry (per its adapter doc):
/// formatting is normalized, headers/footers and macros are unsupported.
fn docx_envelope_fidelity() -> Json {
    Json::Obj(BTreeMap::from([
        (
            "headers_footers".to_owned(),
            Json::Str("unsupported".to_owned()),
        ),
        ("headings".to_owned(), Json::Str("normalized".to_owned())),
        ("lists".to_owned(), Json::Str("normalized".to_owned())),
        ("macros".to_owned(), Json::Str("unsupported".to_owned())),
        ("paragraphs".to_owned(), Json::Str("normalized".to_owned())),
        ("tables".to_owned(), Json::Str("normalized".to_owned())),
        ("text".to_owned(), Json::Str("normalized".to_owned())),
    ]))
}

fn str_field<'a>(value: &'a Json, key: &str) -> Option<&'a str> {
    match value.get(key) {
        Some(Json::Str(text)) => Some(text),
        _ => None,
    }
}

fn int_field(value: &Json, key: &str) -> Option<i64> {
    match value.get(key) {
        Some(Json::Int(number)) => Some(*number),
        _ => None,
    }
}

/// Reads one format's canonical projection into envelope anchors, the fidelity
/// map, and the processor identity. `None` means the projection is not one
/// this admission path understands; the caller refuses it.
fn read_projection(
    format: Format,
    projection: &Json,
) -> Option<(Vec<EvidenceAnchor>, Json, &'static str)> {
    match format {
        Format::Docx | Format::Pptx | Format::Xlsx => {
            let (fidelity, processor) = match format {
                Format::Docx => {
                    if str_field(projection, "schema_version") != Some(crate::docx::MODEL_VERSION)
                        || str_field(projection, "format") != Some("docx")
                        || !valid_docx_fidelity(projection.get("fidelity")?)
                    {
                        return None;
                    }
                    (docx_envelope_fidelity(), crate::docx::MODEL_VERSION)
                }
                Format::Pptx => {
                    if str_field(projection, "schema") != Some(crate::pptx::SCHEMA) {
                        return None;
                    }
                    (projection.get("fidelity")?.clone(), crate::pptx::SCHEMA)
                }
                Format::Xlsx => {
                    if str_field(projection, "schema_version") != Some(crate::xlsx::PROJECTION) {
                        return None;
                    }
                    (projection.get("fidelity")?.clone(), crate::xlsx::PROJECTION)
                }
                Format::Pdf => return None,
            };
            if !valid_fidelity(&fidelity) {
                return None;
            }
            let Json::Arr(anchors) = projection.get("anchors")? else {
                return None;
            };
            let mut out = Vec::with_capacity(anchors.len());
            for anchor in anchors {
                let (Some(id), Some(role), Some(text)) = (
                    str_field(anchor, "id"),
                    str_field(anchor, "role"),
                    str_field(anchor, "text"),
                ) else {
                    return None;
                };
                let locator = anchor.get("locator")?;
                if str_field(locator, "family") != Some(family(format)) {
                    return None;
                }
                // Keep the native identities; refuse ones outside the
                // envelope's locator bounds instead of inventing a citation.
                match format {
                    Format::Docx => {
                        let section = str_field(locator, "section")?;
                        let ordinal = int_field(locator, "ordinal")?;
                        if section.is_empty()
                            || section.chars().count() > 256
                            || !(1..=1_000_000).contains(&ordinal)
                        {
                            return None;
                        }
                    }
                    Format::Pptx => {
                        let slide = int_field(locator, "slide")?;
                        let shape = int_field(locator, "shape")?;
                        let z = int_field(locator, "z_index")?;
                        if !(1..=1_000_000).contains(&slide)
                            || !(1..=1_000_000).contains(&shape)
                            || !(0..=1_000_000).contains(&z)
                        {
                            return None;
                        }
                    }
                    _ => {}
                }
                if !(valid_id(id) && valid_role(format, role)) {
                    return None;
                }
                out.push(EvidenceAnchor {
                    id: id.to_owned(),
                    role: role.to_owned(),
                    locator: locator.clone(),
                    text: text.to_owned(),
                    text_digest: sha256_hex(text.as_bytes()),
                    derivation: Derivation::Embedded,
                });
            }
            Some((out, fidelity, processor))
        }
        Format::Pdf => {
            let fidelity = projection.get("fidelity")?.clone();
            if !valid_fidelity(&fidelity) {
                return None;
            }
            if str_field(projection, "projection") != Some(crate::pdf::PROJECTION) {
                return None;
            }
            let Json::Arr(pages) = projection.get("pages")? else {
                return None;
            };
            let mut out = Vec::new();
            for page in pages {
                let page_number = int_field(page, "page")?;
                let Json::Arr(blocks) = page.get("blocks")? else {
                    return None;
                };
                for block in blocks {
                    let (Some(id), Some(role), Some(text)) = (
                        str_field(block, "id"),
                        str_field(block, "role"),
                        str_field(block, "text"),
                    ) else {
                        return None;
                    };
                    if !(valid_id(id) && valid_role(format, role)) {
                        return None;
                    }
                    let ordinal = int_field(block, "block")?;
                    if page_number < 1 || ordinal < 1 {
                        return None;
                    }
                    let Json::Arr(bbox) = block.get("bbox")? else {
                        return None;
                    };
                    let mut coords = [0i64; 4];
                    if bbox.len() != 4 {
                        return None;
                    }
                    for (index, coord) in bbox.iter().enumerate() {
                        let Json::Int(value) = coord else {
                            return None;
                        };
                        // Bounding boxes are approximated (per the adapter's
                        // fidelity map), so clamp to the envelope's range
                        // rather than refuse citable text over geometry.
                        coords[index] = (*value).clamp(0, 1_000_000);
                    }
                    coords[2] = coords[2].max(coords[0]);
                    coords[3] = coords[3].max(coords[1]);
                    let locator = Json::Obj(BTreeMap::from([
                        ("family".to_owned(), Json::Str("fixed-layout".to_owned())),
                        ("page".to_owned(), Json::Int(page_number)),
                        ("block".to_owned(), Json::Int(ordinal)),
                        (
                            "bbox".to_owned(),
                            Json::Obj(BTreeMap::from([
                                ("x0".to_owned(), Json::Int(coords[0])),
                                ("y0".to_owned(), Json::Int(coords[1])),
                                ("x1".to_owned(), Json::Int(coords[2])),
                                ("y1".to_owned(), Json::Int(coords[3])),
                            ])),
                        ),
                    ]));
                    let derivation = match str_field(block, "derivation") {
                        Some(marked) if marked == "ocr" || marked.starts_with("ocr-") => {
                            Derivation::Ocr
                        }
                        // Unknown markings stay embedded only when the adapter
                        // names them so; anything else fails closed.
                        Some("embedded-text") | None => Derivation::Embedded,
                        _ => return None,
                    };
                    out.push(EvidenceAnchor {
                        id: id.to_owned(),
                        role: role.to_owned(),
                        locator,
                        text: text.to_owned(),
                        text_digest: sha256_hex(text.as_bytes()),
                        derivation,
                    });
                }
            }
            Some((out, fidelity, crate::pdf::PROJECTION))
        }
    }
}

fn truncate_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn keys_are(value: &Json, keys: &[&str]) -> bool {
    matches!(value, Json::Obj(map) if map.len() == keys.len() && keys.iter().all(|key| map.contains_key(*key)))
}

fn valid_native_locator(format: Format, locator: &Json) -> bool {
    let bounded =
        |key, min, max| int_field(locator, key).is_some_and(|value| (min..=max).contains(&value));
    let named = |key| {
        str_field(locator, key)
            .is_some_and(|value| !value.trim().is_empty() && value.chars().count() <= 256)
    };
    if str_field(locator, "family") != Some(family(format)) {
        return false;
    }
    match format {
        Format::Docx => {
            keys_are(locator, &["family", "section", "ordinal"])
                && named("section")
                && bounded("ordinal", 1, 1_000_000)
        }
        Format::Xlsx => {
            keys_are(locator, &["family", "sheet", "row", "column"])
                && named("sheet")
                && bounded("row", 1, 1_048_576)
                && bounded("column", 1, 16_384)
        }
        Format::Pptx => {
            keys_are(locator, &["family", "slide", "shape", "z_index"])
                && bounded("slide", 1, 1_000_000)
                && bounded("shape", 1, 1_000_000)
                && bounded("z_index", 0, 1_000_000)
        }
        Format::Pdf => {
            let Some(bbox) = locator.get("bbox") else {
                return false;
            };
            keys_are(locator, &["family", "page", "block", "bbox"])
                && bounded("page", 1, 1_000_000)
                && bounded("block", 1, 1_000_000)
                && keys_are(bbox, &["x0", "y0", "x1", "y1"])
                && ["x0", "y0", "x1", "y1"].iter().all(|key| {
                    int_field(bbox, key).is_some_and(|value| (0..=1_000_000).contains(&value))
                })
                && int_field(bbox, "x0") <= int_field(bbox, "x1")
                && int_field(bbox, "y0") <= int_field(bbox, "y1")
        }
    }
}

/// Check every variable field of the complete, locally constructed envelope
/// against the V1 schema and semantic oracle. Its closed object shapes, fixed
/// policy/receipt states, family/media pairing and digests are by construction;
/// projection values and truncation counts still need a final admission gate.
fn validate_built_envelope(envelope: &Json, format: Format) -> Result<(), Refusal> {
    let malformed = Refusal::MalformedContainer;
    if envelope.to_bytes().len() > MAX_ENVELOPE_BYTES {
        return Err(Refusal::Oversized);
    }
    let original = envelope.get("original").ok_or(malformed)?;
    if !int_field(original, "byte_length").is_some_and(|value| (1..=20_971_520).contains(&value)) {
        return Err(Refusal::Oversized);
    }
    let truncation = envelope
        .get("receipts")
        .and_then(|receipts| receipts.get("truncation"))
        .ok_or(malformed)?;
    if !int_field(truncation, "omitted_anchor_count")
        .is_some_and(|value| (0..=MAX_ANCHORS as i64).contains(&value))
        || !int_field(truncation, "omitted_bytes")
            .is_some_and(|value| (0..=20_971_520).contains(&value))
    {
        return Err(Refusal::Oversized);
    }
    let Some(Json::Arr(anchors)) = envelope.get("anchors") else {
        return Err(malformed);
    };
    let mut previous = "";
    for anchor in anchors {
        let id = str_field(anchor, "id").ok_or(malformed)?;
        if !valid_id(id)
            || id <= previous
            || !valid_role(format, str_field(anchor, "role").ok_or(malformed)?)
            || !valid_native_locator(format, anchor.get("locator").ok_or(malformed)?)
        {
            return Err(malformed);
        }
        previous = id;
    }
    let features: &[&str] = match format {
        Format::Docx => &[
            "text",
            "paragraphs",
            "headings",
            "lists",
            "tables",
            "headers_footers",
            "macros",
        ],
        Format::Xlsx => &["text", "sheets", "cells", "formulas", "charts", "macros"],
        Format::Pptx => &[
            "text",
            "slides",
            "shapes",
            "speaker_notes",
            "z_order",
            "macros",
            "animations",
        ],
        Format::Pdf => &[
            "text",
            "reading_order",
            "bounding_boxes",
            "tables",
            "figures",
            "ocr",
            "javascript",
        ],
    };
    let fidelity = envelope.get("fidelity").ok_or(malformed)?;
    if !keys_are(fidelity, features) || !valid_fidelity(fidelity) {
        return Err(malformed);
    }
    if ["macros", "formulas", "javascript", "animations"]
        .iter()
        .any(|key| str_field(fidelity, key) == Some("exact"))
    {
        return Err(Refusal::UnsupportedContainer);
    }
    // Validate decoded strings, so JSON escaping cannot conceal a forbidden
    // locator or other field from the common DLP/public-boundary validator.
    let mut pending = vec![envelope];
    while let Some(value) = pending.pop() {
        match value {
            Json::Str(text) => validate_locator(text)?,
            Json::Arr(values) => pending.extend(values),
            Json::Obj(values) => pending.extend(values.values()),
            _ => {}
        }
    }
    Ok(())
}

/// Drops blank anchors silently and over-long or over-count anchors with
/// truncation accounting. Pure so tests pin the rule without a workbook.
fn apply_anchor_budgets(mut anchors: Vec<EvidenceAnchor>) -> (Vec<EvidenceAnchor>, usize, usize) {
    anchors.sort_by(|left, right| left.id.cmp(&right.id));
    let mut omitted_anchors = 0usize;
    let mut omitted_bytes = 0usize;
    anchors.retain(|anchor| {
        if anchor.text.trim().is_empty() {
            return false;
        }
        if anchor.text.len() > MAX_TEXT_BYTES {
            omitted_anchors += 1;
            omitted_bytes += anchor.text.len();
            return false;
        }
        true
    });
    if anchors.len() > MAX_ANCHORS {
        for anchor in anchors.drain(MAX_ANCHORS..) {
            omitted_anchors += 1;
            omitted_bytes += anchor.text.len();
        }
    }
    (anchors, omitted_anchors, omitted_bytes)
}

/// Admits one document's bytes into a valid evidence envelope. `locator` is
/// the repository-relative path the envelope cites. Import refusals (including
/// DLP and public-boundary findings the adapters record) pass through with
/// their codes; anything that cannot become a valid envelope is refused.
pub fn admit(bytes: &[u8], format: Format, locator: &str) -> Result<Admitted, Refusal> {
    if locator.is_empty() || locator.len() > MAX_LOCATOR_BYTES || locator.trim().is_empty() {
        return Err(Refusal::MalformedContainer);
    }
    validate_locator(locator)?;
    let adapter = adapter(format).ok_or(Refusal::UnsupportedContainer)?;
    let imported = adapter.import(bytes, &Limits::default(), &Cancel::default())?;
    if imported.receipt.iter().any(|code| code == "dlp-findings") {
        return Err(Refusal::DlpFindings);
    }
    if imported
        .receipt
        .iter()
        .any(|code| code == "public-boundary-violation")
    {
        return Err(Refusal::PublicBoundary);
    }
    let projection = Json::parse(&imported.canonical).ok_or(Refusal::MalformedContainer)?;
    let (anchors, fidelity, processor) =
        read_projection(format, &projection).ok_or(Refusal::UnsupportedContainer)?;
    let (anchors, omitted_anchors, mut omitted_bytes) = apply_anchor_budgets(anchors);
    if anchors.is_empty() {
        return Err(Refusal::NoAdmissibleText);
    }
    let joined = anchors
        .iter()
        .map(|anchor| anchor.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let normalized_text = truncate_bytes(&joined, MAX_TEXT_BYTES);
    let truncated = normalized_text.len() != joined.len() || omitted_anchors > 0;
    if truncated && normalized_text.len() != joined.len() {
        omitted_bytes += joined.len() - normalized_text.len();
    }
    let mut emitted: Vec<&str> = anchors.iter().map(|anchor| anchor.text.as_str()).collect();
    emitted.push(locator);
    emitted.push(normalized_text);
    for text in emitted {
        validate_locator(text)?;
    }
    let source_digest = sha256_hex(bytes);
    let content = Json::Obj(BTreeMap::from([
        ("family".to_owned(), Json::Str(family(format).to_owned())),
        ("language".to_owned(), Json::Str("en".to_owned())),
        ("text".to_owned(), Json::Str(normalized_text.to_owned())),
    ]));
    let unsupported = match &fidelity {
        Json::Obj(map) => {
            let mut features: Vec<String> = map
                .iter()
                .filter(|(_, level)| matches!(level, Json::Str(level) if level == "unsupported"))
                .map(|(feature, _)| feature.clone())
                .collect();
            features.sort();
            features
        }
        _ => Vec::new(),
    };
    let envelope_anchors: Vec<Json> = anchors
        .iter()
        .map(|anchor| {
            Json::Obj(BTreeMap::from([
                ("family".to_owned(), Json::Str(family(format).to_owned())),
                ("id".to_owned(), Json::Str(anchor.id.clone())),
                ("locator".to_owned(), anchor.locator.clone()),
                ("role".to_owned(), Json::Str(anchor.role.clone())),
                ("text".to_owned(), Json::Str(anchor.text.clone())),
                (
                    "text_digest".to_owned(),
                    Json::Str(anchor.text_digest.clone()),
                ),
            ]))
        })
        .collect();
    let mut envelope = BTreeMap::from([
        (
            "admission".to_owned(),
            Json::Obj(BTreeMap::from([
                ("packet".to_owned(), Json::Str("eligible".to_owned())),
                ("query".to_owned(), Json::Str("eligible".to_owned())),
                ("reasons".to_owned(), Json::Arr(Vec::new())),
                ("status".to_owned(), Json::Str("admitted".to_owned())),
            ])),
        ),
        ("anchors".to_owned(), Json::Arr(envelope_anchors)),
        ("assets".to_owned(), Json::Arr(Vec::new())),
        (
            "evidence_id".to_owned(),
            Json::Str(format!(
                "evd:{}:{}",
                format.extension(),
                &source_digest[..16]
            )),
        ),
        ("fidelity".to_owned(), fidelity.clone()),
        (
            "hazards".to_owned(),
            Json::Obj(BTreeMap::from([
                (
                    "active_content".to_owned(),
                    Json::Obj(BTreeMap::from([
                        ("kinds".to_owned(), Json::Arr(Vec::new())),
                        ("present".to_owned(), Json::Bool(false)),
                    ])),
                ),
                (
                    "external_references".to_owned(),
                    Json::Obj(BTreeMap::from([
                        ("count".to_owned(), Json::Int(0)),
                        ("present".to_owned(), Json::Bool(false)),
                    ])),
                ),
            ])),
        ),
        (
            "normalized".to_owned(),
            Json::Obj(BTreeMap::from([
                ("content".to_owned(), content.clone()),
                (
                    "digest".to_owned(),
                    Json::Str(sha256_hex(&content.to_bytes())),
                ),
            ])),
        ),
        (
            "original".to_owned(),
            Json::Obj(BTreeMap::from([
                ("byte_length".to_owned(), Json::Int(bytes.len() as i64)),
                (
                    "media_type".to_owned(),
                    Json::Str(media_type(format).to_owned()),
                ),
                ("sha256".to_owned(), Json::Str(source_digest)),
            ])),
        ),
        (
            "parser".to_owned(),
            Json::Obj(BTreeMap::from([
                (
                    "attestation".to_owned(),
                    Json::Str("unavailable".to_owned()),
                ),
                ("identity".to_owned(), Json::Str(PARSER_IDENTITY.to_owned())),
                ("processor".to_owned(), Json::Str(processor.to_owned())),
                (
                    "version".to_owned(),
                    Json::Str(env!("CARGO_PKG_VERSION").to_owned()),
                ),
            ])),
        ),
        (
            "policy".to_owned(),
            Json::Obj(BTreeMap::from([
                (
                    "classification".to_owned(),
                    Json::Obj(BTreeMap::from([
                        ("status".to_owned(), Json::Str("unavailable".to_owned())),
                        ("value".to_owned(), Json::Null),
                    ])),
                ),
                (
                    "dlp".to_owned(),
                    Json::Obj(BTreeMap::from([
                        ("findings".to_owned(), Json::Arr(Vec::new())),
                        ("status".to_owned(), Json::Str("passed".to_owned())),
                    ])),
                ),
                (
                    "public_boundary".to_owned(),
                    Json::Obj(BTreeMap::from([
                        ("outcome".to_owned(), Json::Str("unavailable".to_owned())),
                        ("value".to_owned(), Json::Null),
                    ])),
                ),
            ])),
        ),
        (
            "receipts".to_owned(),
            Json::Obj(BTreeMap::from([
                (
                    "malformed_input".to_owned(),
                    Json::Obj(BTreeMap::from([
                        ("present".to_owned(), Json::Bool(false)),
                        ("reason".to_owned(), Json::Null),
                    ])),
                ),
                (
                    "truncation".to_owned(),
                    Json::Obj(BTreeMap::from([
                        (
                            "omitted_anchor_count".to_owned(),
                            Json::Int(omitted_anchors as i64),
                        ),
                        ("omitted_bytes".to_owned(), Json::Int(omitted_bytes as i64)),
                        (
                            "reason".to_owned(),
                            if truncated {
                                Json::Str("envelope-budget".to_owned())
                            } else {
                                Json::Null
                            },
                        ),
                        ("truncated".to_owned(), Json::Bool(truncated)),
                    ])),
                ),
                (
                    "unavailable".to_owned(),
                    Json::Obj(BTreeMap::from([
                        (
                            "features".to_owned(),
                            Json::Arr(unsupported.into_iter().map(Json::Str).collect()),
                        ),
                        ("parser_attestation".to_owned(), Json::Bool(true)),
                        ("revision".to_owned(), Json::Bool(true)),
                    ])),
                ),
            ])),
        ),
        ("relations".to_owned(), Json::Arr(Vec::new())),
        (
            "schema_version".to_owned(),
            Json::Str(ENVELOPE_VERSION.to_owned()),
        ),
        (
            "source".to_owned(),
            Json::Obj(BTreeMap::from([
                ("locator".to_owned(), Json::Str(locator.to_owned())),
                (
                    "revision".to_owned(),
                    Json::Obj(BTreeMap::from([
                        ("state".to_owned(), Json::Str("unavailable".to_owned())),
                        ("value".to_owned(), Json::Null),
                    ])),
                ),
            ])),
        ),
    ]);
    let digest = sha256_hex(&Json::Obj(envelope.clone()).to_bytes());
    envelope.insert("envelope_digest".to_owned(), Json::Str(digest));
    let envelope = Json::Obj(envelope);
    validate_built_envelope(&envelope, format)?;
    Ok(Admitted {
        envelope,
        anchors,
        fidelity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_projection(anchors: Vec<Json>, fidelity: Json) -> Json {
        Json::Obj(BTreeMap::from([
            (
                "schema_version".to_owned(),
                Json::Str(crate::xlsx::PROJECTION.to_owned()),
            ),
            ("anchors".to_owned(), Json::Arr(anchors)),
            ("fidelity".to_owned(), fidelity),
        ]))
    }

    fn grid_fidelity() -> Json {
        Json::Obj(BTreeMap::from([
            ("cells".to_owned(), Json::Str("exact".to_owned())),
            ("charts".to_owned(), Json::Str("unsupported".to_owned())),
            ("formulas".to_owned(), Json::Str("normalized".to_owned())),
            ("macros".to_owned(), Json::Str("unsupported".to_owned())),
            ("sheets".to_owned(), Json::Str("exact".to_owned())),
            ("text".to_owned(), Json::Str("exact".to_owned())),
        ]))
    }

    fn grid_anchor(id: &str, text: &str) -> Json {
        Json::Obj(BTreeMap::from([
            ("family".to_owned(), Json::Str("grid".to_owned())),
            ("id".to_owned(), Json::Str(id.to_owned())),
            (
                "locator".to_owned(),
                Json::Obj(BTreeMap::from([
                    ("column".to_owned(), Json::Int(1)),
                    ("family".to_owned(), Json::Str("grid".to_owned())),
                    ("row".to_owned(), Json::Int(1)),
                    ("sheet".to_owned(), Json::Str("Sheet1".to_owned())),
                ])),
            ),
            ("role".to_owned(), Json::Str("cell".to_owned())),
            ("text".to_owned(), Json::Str(text.to_owned())),
            (
                "text_digest".to_owned(),
                Json::Str(sha256_hex(text.as_bytes())),
            ),
        ]))
    }

    #[test]
    fn ocr_marked_blocks_derive_ocr_anchors() {
        let block = |derivation: Option<&str>| {
            let mut map = BTreeMap::from([
                (
                    "bbox".to_owned(),
                    Json::Arr(vec![
                        Json::Int(0),
                        Json::Int(0),
                        Json::Int(10),
                        Json::Int(10),
                    ]),
                ),
                ("block".to_owned(), Json::Int(1)),
                ("id".to_owned(), Json::Str("pdf:p1:b1".to_owned())),
                ("role".to_owned(), Json::Str("paragraph".to_owned())),
                ("text".to_owned(), Json::Str("scanned words".to_owned())),
                (
                    "text_digest".to_owned(),
                    Json::Str(sha256_hex("scanned words".as_bytes())),
                ),
            ]);
            if let Some(marked) = derivation {
                map.insert("derivation".to_owned(), Json::Str(marked.to_owned()));
            }
            Json::Obj(map)
        };
        let projection = |blocks: Vec<Json>| {
            Json::Obj(BTreeMap::from([
                (
                    "projection".to_owned(),
                    Json::Str(crate::pdf::PROJECTION.to_owned()),
                ),
                (
                    "pages".to_owned(),
                    Json::Arr(vec![Json::Obj(BTreeMap::from([
                        ("page".to_owned(), Json::Int(1)),
                        ("blocks".to_owned(), Json::Arr(blocks)),
                    ]))]),
                ),
                (
                    "fidelity".to_owned(),
                    Json::Obj(BTreeMap::from([
                        (
                            "bounding_boxes".to_owned(),
                            Json::Str("approximated".to_owned()),
                        ),
                        ("figures".to_owned(), Json::Str("approximated".to_owned())),
                        ("javascript".to_owned(), Json::Str("unsupported".to_owned())),
                        ("ocr".to_owned(), Json::Str("unsupported".to_owned())),
                        (
                            "reading_order".to_owned(),
                            Json::Str("approximated".to_owned()),
                        ),
                        ("tables".to_owned(), Json::Str("unsupported".to_owned())),
                        ("text".to_owned(), Json::Str("normalized".to_owned())),
                    ])),
                ),
            ]))
        };
        let (anchors, _, _) =
            read_projection(Format::Pdf, &projection(vec![block(Some("ocr-tesseract"))]))
                .expect("ocr projection reads");
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].derivation, Derivation::Ocr);
        let (anchors, _, _) =
            read_projection(Format::Pdf, &projection(vec![block(Some("embedded-text"))]))
                .expect("embedded projection reads");
        assert_eq!(anchors[0].derivation, Derivation::Embedded);
        let (anchors, _, _) = read_projection(Format::Pdf, &projection(vec![block(None)]))
            .expect("unmarked projection reads");
        assert_eq!(anchors[0].derivation, Derivation::Embedded);
        assert!(
            read_projection(Format::Pdf, &projection(vec![block(Some("vendor-magic"))])).is_none()
        );
    }

    #[test]
    fn oversized_anchors_are_omitted_with_receipts() {
        let long = "x".repeat(MAX_TEXT_BYTES + 1);
        let projection = grid_projection(
            vec![
                grid_anchor("anc:xlsx:s1:r1c1", "kept"),
                grid_anchor("anc:xlsx:s1:r1c2", &long),
                grid_anchor("anc:xlsx:s1:r1c3", "   "),
            ],
            grid_fidelity(),
        );
        let (anchors, _, _) =
            read_projection(Format::Xlsx, &projection).expect("grid projection reads");
        assert_eq!(anchors.len(), 3);
        let (kept, omitted, omitted_bytes) = apply_anchor_budgets(anchors);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "anc:xlsx:s1:r1c1");
        assert_eq!(omitted, 1);
        assert_eq!(omitted_bytes, long.len());
    }

    #[test]
    fn unknown_projections_and_roles_fail_closed() {
        let projection = grid_projection(
            vec![grid_anchor("anc:xlsx:s1:r1c1", "kept")],
            grid_fidelity(),
        );
        assert!(read_projection(Format::Docx, &projection).is_none());
        assert!(read_projection(Format::Pptx, &projection).is_none());
        let mut bad_role = grid_anchor("anc:xlsx:s1:r1c1", "kept");
        if let Json::Obj(map) = &mut bad_role {
            map.insert("role".to_owned(), Json::Str("notes".to_owned()));
        }
        assert!(read_projection(
            Format::Xlsx,
            &grid_projection(vec![bad_role], grid_fidelity())
        )
        .is_none());
        let mut bad_id = grid_anchor("anc:xlsx:s1:r1 c1", "kept");
        if let Json::Obj(map) = &mut bad_id {
            map.insert("id".to_owned(), Json::Str("has space".to_owned()));
        }
        assert!(read_projection(
            Format::Xlsx,
            &grid_projection(vec![bad_id], grid_fidelity())
        )
        .is_none());
    }
}
