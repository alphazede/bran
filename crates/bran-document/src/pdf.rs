//! PDF adapter (issue #23).
//!
//! Import reads fixed-layout evidence: pages, text blocks with bounding boxes
//! and stable locators, images, annotations, outline, form fields, signature
//! evidence, attachments, and metadata. Active content and external
//! references are refused, encryption is refused, and nothing is executed,
//! fetched, extracted, or OCRed. Export writes a new tagged PDF from the
//! projection alone, so no active content can survive it.

use crate::canonical::{sha256_hex, Json};
use crate::conformance::{Adapter, Anchor, Imported};
use crate::pdf_syntax::{is_name, key, text_string, Dict, Doc, Obj};
use crate::pdf_text::{self, win_ansi, Rect};
use crate::{Cancel, Format, Limits, Refusal, RECEIPT_VERSION};
use bran_core::export::{validate_emitted_string, ExportError};
use std::collections::{BTreeMap, BTreeSet};

/// Identifies the canonical projection this adapter emits.
pub const PROJECTION: &str = "bran-pdf-projection-v1";

/// Action types that run code, play media, submit data, or open embedded
/// documents. Any of them refuses the file.
const ACTIVE_ACTIONS: &[&str] = &[
    "JavaScript",
    "ECMAScript",
    "Launch",
    "SubmitForm",
    "ImportData",
    "Rendition",
    "Movie",
    "Sound",
    "RichMediaExecute",
    "GoToE",
    "GoTo3DView",
];
const ACTIVE_ANNOTATIONS: &[&str] = &["Movie", "Sound", "Screen", "RichMedia", "3D"];

pub struct PdfAdapter;

/// Import options. OCR is opt-in; no OCR engine ships, so a request is
/// reported as unavailable and nothing is recognised.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub ocr: bool,
}

/// A tagged PDF, its canonical export receipt, and the text it emits (for
/// the shared export gate's DLP check).
pub struct Exported {
    pub bytes: Vec<u8>,
    pub receipt: Vec<u8>,
    pub emitted_text: Vec<String>,
}

impl Adapter for PdfAdapter {
    fn format(&self) -> Format {
        Format::Pdf
    }

    fn import(&self, bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Imported, Refusal> {
        import(bytes, limits, cancel, Options::default())
    }

    fn export(&self, imported: &Imported) -> Result<Vec<u8>, Refusal> {
        export(imported).map(|exported| exported.bytes)
    }
}

fn text(value: impl Into<String>) -> Json {
    Json::Str(value.into())
}

fn object<const N: usize>(pairs: [(&str, Json); N]) -> Json {
    Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn round(value: f64) -> i64 {
    value.round() as i64
}

fn rect(value: Rect) -> Json {
    Json::Arr(value.iter().map(|v| Json::Int(round(*v))).collect())
}

fn name(value: &[u8]) -> String {
    String::from_utf8_lossy(value).into_owned()
}

fn rect_of(doc: &Doc<'_>, value: &Obj) -> Option<Rect> {
    let Obj::Arr(items) = doc.get(value) else {
        return None;
    };
    let numbers: Vec<f64> = items
        .iter()
        .filter_map(|item| doc.get(item).number())
        .collect();
    let [x0, y0, x1, y1] = numbers[..] else {
        return None;
    };
    Some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
}

/// Imports a PDF into the canonical projection, its receipt codes, and one
/// anchor per text block.
pub fn import(
    bytes: &[u8],
    limits: &Limits,
    cancel: &Cancel,
    options: Options,
) -> Result<Imported, Refusal> {
    cancel.check()?;
    if bytes.len() as u64 > limits.max_package_bytes {
        return Err(Refusal::Oversized);
    }
    let doc = Doc::open(bytes, limits, cancel)?;
    let mut receipts = doc.receipts.clone();
    let mut attachments = Vec::new();
    for object in doc.objects.values() {
        scan(&doc, object, &mut receipts, &mut attachments)?;
    }
    let Obj::Dict(catalog) = doc.lookup(&doc.trailer, "Root") else {
        return Err(Refusal::PdfMalformed);
    };
    let mut pages = Vec::new();
    let mut seen = BTreeSet::new();
    walk_pages(
        &doc,
        key(catalog, "Pages").unwrap_or(&Obj::Null),
        Inherited::default(),
        0,
        &mut seen,
        &mut pages,
    )?;
    let numbers: BTreeMap<u32, i64> = pages
        .iter()
        .enumerate()
        .filter_map(|(index, page)| page.reference.map(|r| (r, index as i64 + 1)))
        .collect();

    let mut anchors = Vec::new();
    let mut page_json = Vec::new();
    let mut unmapped = false;
    for (index, page) in pages.iter().enumerate() {
        cancel.check()?;
        let number = index + 1;
        let streams: Vec<&Obj> = match doc.lookup(page.dict, "Contents") {
            Obj::Arr(items) => items.iter().map(|item| doc.get(item)).collect(),
            other => vec![other],
        };
        let mut content = Vec::new();
        for stream in streams {
            if let Obj::Stream(dict, raw) = stream {
                match doc.decode(dict, raw)? {
                    Some(data) => {
                        content.extend(data);
                        content.push(b'\n');
                    }
                    None => {
                        receipts.insert("stream-not-decoded");
                    }
                }
            }
        }
        let extracted = pdf_text::page(&doc, &content, page.resources, cancel)?;
        unmapped |= extracted.unmapped_glyphs;
        if extracted.malformed {
            receipts.insert("malformed-content-stream");
        }
        if extracted.undecoded {
            receipts.insert("stream-not-decoded");
        }
        let mut blocks = Vec::new();
        for (ordinal, block) in extracted.blocks.iter().enumerate() {
            let id = format!("pdf:p{number}:b{}", ordinal + 1);
            let digest = sha256_hex(block.text.as_bytes());
            anchors.push(Anchor {
                id: id.clone(),
                text_digest: digest.clone(),
            });
            blocks.push(object([
                ("bbox", rect(block.bbox)),
                ("block", Json::Int(ordinal as i64 + 1)),
                ("derivation", text("embedded-text")),
                ("id", text(id)),
                (
                    "role",
                    text(if block.heading {
                        "heading"
                    } else {
                        "paragraph"
                    }),
                ),
                ("text", text(block.text.clone())),
                ("text_digest", text(digest)),
            ]));
        }
        let images: Vec<Json> = extracted
            .images
            .iter()
            .enumerate()
            .map(|(ordinal, image)| {
                object([
                    ("bbox", rect(image.bbox)),
                    ("height", Json::Int(image.height)),
                    ("id", text(format!("pdf:p{number}:i{}", ordinal + 1))),
                    ("width", Json::Int(image.width)),
                ])
            })
            .collect();
        if blocks.is_empty() && !images.is_empty() {
            receipts.insert("ocr-not-run");
        }
        page_json.push(object([
            (
                "annotations",
                Json::Arr(annotations(&doc, page.dict, number, &numbers)),
            ),
            ("blocks", Json::Arr(blocks.clone())),
            ("box", rect(page.media)),
            ("content_digest", text(sha256_hex(&content))),
            ("images", Json::Arr(images)),
            ("page", Json::Int(number as i64)),
            (
                "reading_order",
                object([
                    ("confidence", text("low")),
                    ("provenance", text("content-stream-order")),
                ]),
            ),
            ("rotate", Json::Int(page.rotate)),
            (
                "text_source",
                text(if blocks.is_empty() {
                    "none"
                } else {
                    "embedded-text"
                }),
            ),
        ]));
    }
    if unmapped {
        receipts.insert("text-unmapped-glyphs");
    }

    let mut outline_items = Vec::new();
    let mut visited = BTreeSet::new();
    if let Some(root) = doc.dict(doc.lookup(catalog, "Outlines")) {
        outline(
            &doc,
            key(root, "First"),
            1,
            &numbers,
            &mut visited,
            &mut outline_items,
        )?;
    }
    let (mut forms, mut signatures) = (Vec::new(), Vec::new());
    if let Some(acroform) = doc.dict(doc.lookup(catalog, "AcroForm")) {
        if let Obj::Arr(roots) = doc.lookup(acroform, "Fields") {
            let mut visited = BTreeSet::new();
            for root in roots {
                fields(
                    &doc,
                    root,
                    "",
                    None,
                    0,
                    &mut visited,
                    &mut forms,
                    &mut signatures,
                )?;
            }
        }
    }
    if !signatures.is_empty() {
        receipts.insert("signature-not-verified");
    }
    attachments.sort();
    if !attachments.is_empty() {
        receipts.insert("embedded-file-not-extracted");
    }
    let ocr = if options.ocr {
        receipts.insert("ocr-unavailable");
        object([
            ("effective", text("unavailable")),
            ("requested", Json::Bool(true)),
        ])
    } else {
        object([
            ("effective", text("not-run")),
            ("requested", Json::Bool(false)),
        ])
    };
    if doc.dangling_seen() {
        receipts.insert("dangling-reference");
    }

    let mut metadata = BTreeMap::new();
    if let Some(info) = doc.dict(doc.lookup(&doc.trailer, "Info")) {
        for (field, entry) in [
            ("author", "Author"),
            ("keywords", "Keywords"),
            ("subject", "Subject"),
            ("title", "Title"),
        ] {
            if let Obj::Str(value) = doc.lookup(info, entry) {
                metadata.insert(field.to_owned(), text(text_string(value)));
            }
        }
    }
    let language = match doc.lookup(catalog, "Lang") {
        Obj::Str(value) => text(text_string(value)),
        _ => Json::Null,
    };
    let tagged = doc
        .dict(doc.lookup(catalog, "MarkInfo"))
        .is_some_and(|mark| doc.lookup(mark, "Marked") == &Obj::Bool(true))
        && key(catalog, "StructTreeRoot").is_some();
    let header = bytes
        .windows(5)
        .position(|w| w == b"%PDF-")
        .map_or(0, |at| at + 5);
    let version = match doc.lookup(catalog, "Version") {
        Obj::Name(version) => name(version),
        _ => name(&bytes[header..(header + 3).min(bytes.len())]),
    };
    let fidelity = object([
        ("bounding_boxes", text("approximated")),
        ("figures", text("approximated")),
        ("javascript", text("unsupported")),
        ("ocr", text("unsupported")),
        ("reading_order", text("approximated")),
        ("tables", text("unsupported")),
        (
            "text",
            text(if unmapped {
                "approximated"
            } else {
                "normalized"
            }),
        ),
    ]);
    let attachments = attachments
        .into_iter()
        .map(|(file, size)| {
            object([
                ("declared_size", size.map_or(Json::Null, Json::Int)),
                ("extracted", Json::Bool(false)),
                ("name", text(file)),
            ])
        })
        .collect();
    let mut projection = object([
        ("attachments", Json::Arr(attachments)),
        ("fidelity", fidelity),
        ("forms", Json::Arr(forms)),
        ("language", language),
        ("metadata", Json::Obj(metadata)),
        ("ocr", ocr),
        ("outline", Json::Arr(outline_items)),
        ("pages", Json::Arr(page_json)),
        ("pdf_version", text(version)),
        ("projection", text(PROJECTION)),
        ("receipt_version", text(RECEIPT_VERSION)),
        ("signatures", Json::Arr(signatures)),
        // The file digest is not part of the projection: re-encodings of the
        // same document must project identically. Callers hash the bytes.
        ("source", object([("media_type", text("application/pdf"))])),
        ("tagged", Json::Bool(tagged)),
    ]);
    // DLP and the public boundary see every emitted string plus the raw
    // bytes, the same shared check the package intake applies.
    dlp(&projection, &mut receipts);
    dlp(&text(String::from_utf8_lossy(bytes)), &mut receipts);
    let codes: Vec<String> = receipts.iter().map(|code| (*code).to_owned()).collect();
    if let Json::Obj(map) = &mut projection {
        map.insert(
            "receipts".to_owned(),
            Json::Arr(codes.iter().cloned().map(Json::Str).collect()),
        );
    }
    Ok(Imported {
        canonical: projection.to_bytes(),
        receipt: codes,
        anchors,
    })
}

fn dlp(value: &Json, receipts: &mut BTreeSet<&'static str>) {
    match value {
        Json::Str(value) => match validate_emitted_string(value) {
            Err(ExportError::DlpViolation(_)) => {
                receipts.insert("dlp-findings");
            }
            Err(_) => {
                receipts.insert("public-boundary-violation");
            }
            Ok(()) => {}
        },
        Json::Arr(items) => items.iter().for_each(|item| dlp(item, receipts)),
        Json::Obj(map) => map.values().for_each(|item| dlp(item, receipts)),
        _ => {}
    }
}

/// Refuses active content and external references anywhere in the file,
/// reachable or not, and records hyperlinks and attachments.
fn scan(
    doc: &Doc<'_>,
    object: &Obj,
    receipts: &mut BTreeSet<&'static str>,
    attachments: &mut Vec<(String, Option<i64>)>,
) -> Result<(), Refusal> {
    let dict = match object {
        Obj::Dict(dict) => dict,
        // A stream whose data lives in an external file.
        Obj::Stream(dict, _) if key(dict, "F").is_some() => return Err(Refusal::ExternalReference),
        Obj::Stream(dict, _) => dict,
        Obj::Arr(items) => {
            for item in items {
                scan(doc, item, receipts, attachments)?;
            }
            return Ok(());
        }
        _ => return Ok(()),
    };
    if ["JS", "JavaScript", "XFA"]
        .iter()
        .any(|name| key(dict, name).is_some())
    {
        return Err(Refusal::ActiveContent);
    }
    let named = |entry: &str, list: &[&str]| {
        key(dict, entry)
            .and_then(Obj::name)
            .is_some_and(|value| list.iter().any(|item| item.as_bytes() == value))
    };
    if named("S", ACTIVE_ACTIONS) || named("Subtype", ACTIVE_ANNOTATIONS) {
        return Err(Refusal::ActiveContent);
    }
    if named("S", &["GoToR"])
        || named("FS", &["URL"])
        || (named("Subtype", &["Form"]) && key(dict, "Ref").is_some())
    {
        return Err(Refusal::ExternalReference);
    }
    if named("S", &["URI"]) {
        receipts.insert("hyperlink-not-fetched");
    }
    if is_name(dict, "Type", "EmbeddedFile") {
        receipts.insert("embedded-file-not-extracted");
    }
    if let Some(files) = key(dict, "EF").and_then(|ef| doc.dict(ef)) {
        let label = ["UF", "F"]
            .iter()
            .find_map(|entry| match doc.lookup(dict, entry) {
                Obj::Str(value) => Some(text_string(value)),
                _ => None,
            })
            .unwrap_or_default();
        let size = doc
            .dict(doc.lookup(files, "F"))
            .and_then(|stream| doc.dict(doc.lookup(stream, "Params")))
            .and_then(|params| doc.lookup(params, "Size").int());
        attachments.push((label, size));
    }
    for value in dict.values() {
        scan(doc, value, receipts, attachments)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Default)]
struct Inherited<'d> {
    resources: Option<&'d Dict>,
    media: Option<Rect>,
    rotate: Option<i64>,
}

struct Page<'d> {
    reference: Option<u32>,
    dict: &'d Dict,
    resources: Option<&'d Dict>,
    media: Rect,
    rotate: i64,
}

/// Walks the page tree. A node seen twice (a cycle), a missing or
/// mistyped node, or a page without a media box refuses the file.
fn walk_pages<'d>(
    doc: &'d Doc<'_>,
    node: &'d Obj,
    inherited: Inherited<'d>,
    depth: usize,
    seen: &mut BTreeSet<u32>,
    out: &mut Vec<Page<'d>>,
) -> Result<(), Refusal> {
    if depth > doc.limits.max_xml_depth {
        return Err(Refusal::PdfDepthLimit);
    }
    let reference = match node {
        Obj::Ref(number) if !seen.insert(*number) => return Err(Refusal::PdfMalformed),
        Obj::Ref(number) => Some(*number),
        _ => None,
    };
    let Obj::Dict(dict) = doc.get(node) else {
        return Err(Refusal::PdfMalformed);
    };
    let inherited = Inherited {
        resources: key(dict, "Resources")
            .and_then(|r| doc.dict(r))
            .or(inherited.resources),
        media: key(dict, "MediaBox")
            .and_then(|m| rect_of(doc, m))
            .or(inherited.media),
        rotate: doc.lookup(dict, "Rotate").int().or(inherited.rotate),
    };
    if is_name(dict, "Type", "Pages") {
        let Obj::Arr(kids) = doc.lookup(dict, "Kids") else {
            return Err(Refusal::PdfMalformed);
        };
        for kid in kids {
            walk_pages(doc, kid, inherited, depth + 1, seen, out)?;
        }
        return Ok(());
    }
    if !is_name(dict, "Type", "Page") {
        return Err(Refusal::PdfMalformed);
    }
    if out.len() >= doc.limits.max_parts {
        return Err(Refusal::PdfPageLimit);
    }
    out.push(Page {
        reference,
        dict,
        resources: inherited.resources,
        media: inherited.media.ok_or(Refusal::PdfMalformed)?,
        rotate: inherited.rotate.unwrap_or(0).rem_euclid(360),
    });
    Ok(())
}

/// A destination as `target_page` (explicit) or `target_name` (named).
fn target(
    doc: &Doc<'_>,
    destination: &Obj,
    pages: &BTreeMap<u32, i64>,
) -> Option<(&'static str, Json)> {
    match destination {
        Obj::Arr(items) => match items.first()? {
            Obj::Ref(number) => pages
                .get(number)
                .map(|page| ("target_page", Json::Int(*page))),
            _ => None,
        },
        Obj::Ref(_) => target(doc, doc.get(destination), pages),
        Obj::Name(value) => Some(("target_name", text(name(value)))),
        Obj::Str(value) => Some(("target_name", text(text_string(value)))),
        Obj::Dict(dict) => target(doc, key(dict, "D")?, pages),
        _ => None,
    }
}

fn link_fields(
    doc: &Doc<'_>,
    dict: &Dict,
    pages: &BTreeMap<u32, i64>,
    fields: &mut BTreeMap<String, Json>,
) {
    let destination = key(dict, "Dest").or_else(|| {
        let action = doc.dict(key(dict, "A")?)?;
        is_name(action, "S", "GoTo").then(|| key(action, "D"))?
    });
    if let Some((field, value)) = destination.and_then(|d| target(doc, d, pages)) {
        fields.insert(field.to_owned(), value);
    }
    if let Some(action) = key(dict, "A").and_then(|a| doc.dict(a)) {
        if is_name(action, "S", "URI") {
            if let Obj::Str(uri) = doc.lookup(action, "URI") {
                fields.insert("uri".to_owned(), text(name(uri)));
                fields.insert("fetched".to_owned(), Json::Bool(false));
            }
        }
    }
}

fn annotations(doc: &Doc<'_>, page: &Dict, number: usize, pages: &BTreeMap<u32, i64>) -> Vec<Json> {
    let Obj::Arr(items) = doc.lookup(page, "Annots") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let Some(annotation) = doc.dict(item) else {
            continue;
        };
        let subtype = doc
            .lookup(annotation, "Subtype")
            .name()
            .map(name)
            .unwrap_or_default();
        if subtype == "Popup" {
            continue;
        }
        let mut fields = BTreeMap::new();
        fields.insert(
            "id".to_owned(),
            text(format!("pdf:p{number}:a{}", out.len() + 1)),
        );
        fields.insert("subtype".to_owned(), text(subtype));
        if let Some(bbox) = key(annotation, "Rect").and_then(|r| rect_of(doc, r)) {
            fields.insert("bbox".to_owned(), rect(bbox));
        }
        if let Obj::Str(contents) = doc.lookup(annotation, "Contents") {
            fields.insert("contents".to_owned(), text(text_string(contents)));
        }
        link_fields(doc, annotation, pages, &mut fields);
        out.push(Json::Obj(fields));
    }
    out
}

fn outline(
    doc: &Doc<'_>,
    first: Option<&Obj>,
    level: usize,
    pages: &BTreeMap<u32, i64>,
    visited: &mut BTreeSet<u32>,
    out: &mut Vec<Json>,
) -> Result<(), Refusal> {
    if level > doc.limits.max_xml_depth {
        return Err(Refusal::PdfDepthLimit);
    }
    let mut current = first;
    while let Some(Obj::Ref(number)) = current {
        if !visited.insert(*number) {
            break;
        }
        let Some(item) = doc.dict(current.unwrap()) else {
            break;
        };
        let mut fields = BTreeMap::new();
        let title = match doc.lookup(item, "Title") {
            Obj::Str(value) => text_string(value),
            _ => String::new(),
        };
        fields.insert("title".to_owned(), text(title));
        fields.insert("level".to_owned(), Json::Int(level as i64));
        link_fields(doc, item, pages, &mut fields);
        let page = fields.remove("target_page").unwrap_or(Json::Null);
        fields.insert("page".to_owned(), page);
        out.push(Json::Obj(fields));
        outline(doc, key(item, "First"), level + 1, pages, visited, out)?;
        current = key(item, "Next");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn fields(
    doc: &Doc<'_>,
    node: &Obj,
    parent: &str,
    parent_type: Option<&[u8]>,
    depth: usize,
    visited: &mut BTreeSet<u32>,
    forms: &mut Vec<Json>,
    signatures: &mut Vec<Json>,
) -> Result<(), Refusal> {
    if depth > doc.limits.max_xml_depth {
        return Err(Refusal::PdfDepthLimit);
    }
    if let Obj::Ref(number) = node {
        if !visited.insert(*number) {
            return Ok(());
        }
    }
    let Some(field) = doc.dict(node) else {
        return Ok(());
    };
    let full = match doc.lookup(field, "T") {
        Obj::Str(partial) if parent.is_empty() => text_string(partial),
        Obj::Str(partial) => format!("{parent}.{}", text_string(partial)),
        _ => parent.to_owned(),
    };
    let kind = doc.lookup(field, "FT").name().or(parent_type);
    let kids: Vec<&Obj> = match doc.lookup(field, "Kids") {
        Obj::Arr(kids) => kids
            .iter()
            .filter(|kid| doc.dict(kid).is_some_and(|d| key(d, "T").is_some()))
            .collect(),
        _ => Vec::new(),
    };
    if !kids.is_empty() {
        for kid in kids {
            fields(doc, kid, &full, kind, depth + 1, visited, forms, signatures)?;
        }
        return Ok(());
    }
    let value = doc.lookup(field, "V");
    if kind == Some(b"Sig") {
        if let Obj::Dict(signature) = value {
            signatures.push(signature_evidence(doc, signature, &full));
            return Ok(());
        }
    }
    let value = match value {
        Obj::Str(value) => text(text_string(value)),
        Obj::Name(value) => text(name(value)),
        _ => Json::Null,
    };
    forms.push(object([
        ("name", text(full)),
        ("type", text(kind.map(name).unwrap_or_default())),
        ("value", value),
    ]));
    Ok(())
}

/// Signature evidence only: BRAN does not verify the signature or the
/// signer, so `trust` is always `not-verified`.
fn signature_evidence(doc: &Doc<'_>, signature: &Dict, field: &str) -> Json {
    let byte_range: Vec<i64> = match doc.lookup(signature, "ByteRange") {
        Obj::Arr(items) => items
            .iter()
            .filter_map(|item| doc.get(item).int())
            .collect(),
        _ => Vec::new(),
    };
    let contents = match doc.lookup(signature, "Contents") {
        Obj::Str(bytes) => text(sha256_hex(bytes)),
        _ => Json::Null,
    };
    let covers =
        matches!(byte_range[..], [0, _, start, length] if start + length == doc.data.len() as i64);
    let label = |entry: &str| {
        doc.lookup(signature, entry)
            .name()
            .map_or(Json::Null, |value| text(name(value)))
    };
    object([
        (
            "byte_range",
            Json::Arr(byte_range.iter().map(|v| Json::Int(*v)).collect()),
        ),
        ("contents_sha256", contents),
        ("covers_whole_file", Json::Bool(covers)),
        ("field", text(field)),
        ("filter", label("Filter")),
        ("sub_filter", label("SubFilter")),
        ("trust", text("not-verified")),
    ])
}

// ---- Export ----

fn string_at<'j>(value: &'j Json, field: &str) -> Option<&'j str> {
    match value.get(field)? {
        Json::Str(value) => Some(value),
        _ => None,
    }
}

fn array_at<'j>(value: &'j Json, field: &str) -> &'j [Json] {
    match value.get(field) {
        Some(Json::Arr(items)) => items,
        _ => &[],
    }
}

fn ints_at(value: &Json, field: &str) -> Option<[i64; 4]> {
    let numbers: Vec<i64> = array_at(value, field)
        .iter()
        .filter_map(|item| match item {
            Json::Int(v) => Some(*v),
            _ => None,
        })
        .collect();
    numbers.try_into().ok()
}

/// A PDF literal string in WinAnsiEncoding, or `None` when a character has
/// no WinAnsi code.
fn literal(value: &str) -> Option<String> {
    let mut out = String::from("(");
    for c in value.chars() {
        // ponytail: linear search of 224 codes per character; a reverse
        // table would matter only for very large exports.
        let byte = (0x20..=0xFF).find(|b| win_ansi(*b) == Some(c))?;
        match byte {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(char::from(byte));
            }
            0x20..=0x7E => out.push(char::from(byte)),
            _ => out.push_str(&format!("\\{byte:03o}")),
        }
    }
    out.push(')');
    Some(out)
}

fn utf16_hex(value: &str) -> String {
    let mut out = String::from("<FEFF");
    for unit in value.encode_utf16() {
        out.push_str(&format!("{unit:04X}"));
    }
    out.push('>');
    out
}

/// Writes a new tagged PDF from an imported projection. DLP and the public
/// boundary run first; text with no WinAnsi code is a typed refusal.
pub fn export(imported: &Imported) -> Result<Exported, Refusal> {
    for code in &imported.receipt {
        match code.as_str() {
            "dlp-findings" => return Err(Refusal::DlpFindings),
            "public-boundary-violation" => return Err(Refusal::PublicBoundary),
            _ => {}
        }
    }
    let projection = Json::parse(&imported.canonical).ok_or(Refusal::ExportUnsupported)?;
    if string_at(&projection, "projection") != Some(PROJECTION) {
        return Err(Refusal::ExportUnsupported);
    }
    let title = projection
        .get("metadata")
        .and_then(|m| string_at(m, "title"));
    let language = string_at(&projection, "language");
    let pages = array_at(&projection, "pages");
    let mut emitted: Vec<String> = title.map(str::to_owned).into_iter().collect();
    for page in pages {
        for block in array_at(page, "blocks") {
            emitted.push(string_at(block, "text").unwrap_or_default().to_owned());
        }
    }
    for value in &emitted {
        match validate_emitted_string(value) {
            Err(ExportError::DlpViolation(_)) => return Err(Refusal::DlpFindings),
            Err(_) => return Err(Refusal::PublicBoundary),
            Ok(()) => {}
        }
    }

    // Objects: 1 catalog, 2 page tree, 3 structure tree root, 4 document
    // element, 5 font, 6 information dictionary, then a page and a content
    // stream per page, then one structure element per block.
    let first_element = 7 + 2 * pages.len();
    let mut objects: Vec<String> = Vec::new();
    let mut elements = Vec::new();
    let mut parent_tree = String::new();
    let mut receipt_blocks = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        let (page_number, content_number) = (7 + 2 * index, 8 + 2 * index);
        let mut content = String::new();
        let mut page_elements = String::new();
        for (mcid, block) in array_at(page, "blocks").iter().enumerate() {
            let heading = string_at(block, "role") == Some("heading");
            let (tag, size, leading) = if heading {
                ("H1", 16, 20)
            } else {
                ("P", 11, 14)
            };
            let [x0, _, _, y1] = ints_at(block, "bbox").ok_or(Refusal::ExportUnsupported)?;
            content.push_str(&format!(
                "/{tag} << /MCID {mcid} >> BDC\nBT\n/F1 {size} Tf\n{x0} {} Td\n",
                y1 - size
            ));
            for (line_index, line) in string_at(block, "text")
                .unwrap_or_default()
                .lines()
                .enumerate()
            {
                if line_index > 0 {
                    content.push_str(&format!("0 -{leading} Td\n"));
                }
                content.push_str(&literal(line).ok_or(Refusal::ExportUnsupported)?);
                content.push_str(" Tj\n");
            }
            content.push_str("ET\nEMC\n");
            let element = first_element + elements.len();
            elements.push(format!(
                "<< /Type /StructElem /S /{tag} /P 4 0 R /Pg {page_number} 0 R /K {mcid} >>"
            ));
            page_elements.push_str(&format!(" {element} 0 R"));
            receipt_blocks.push(object([
                (
                    "derivation",
                    text(string_at(block, "derivation").unwrap_or("unavailable")),
                ),
                ("id", text(string_at(block, "id").unwrap_or_default())),
                ("role", text(if heading { "heading" } else { "paragraph" })),
            ]));
        }
        parent_tree.push_str(&format!(" {index} [{}]", page_elements.trim_start()));
        let [a, b, c, d] = ints_at(page, "box").ok_or(Refusal::ExportUnsupported)?;
        let rotate = match page.get("rotate") {
            Some(Json::Int(rotate)) => *rotate,
            _ => 0,
        };
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [{a} {b} {c} {d}] /Rotate {rotate} /Resources << /Font << /F1 5 0 R >> >> /Contents {content_number} 0 R /StructParents {index} /Tabs /S >>"
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ));
    }
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", 7 + 2 * i))
        .collect();
    let children: Vec<String> = (0..elements.len())
        .map(|i| format!("{} 0 R", first_element + i))
        .collect();
    let mut catalog = String::from(
        "<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 3 0 R",
    );
    if title.is_some() {
        catalog.push_str(" /ViewerPreferences << /DisplayDocTitle true >>");
    }
    if let Some(language) = language {
        catalog.push_str(&format!(
            " /Lang {}",
            literal(language).ok_or(Refusal::ExportUnsupported)?
        ));
    }
    catalog.push_str(" >>");
    let mut all = vec![
        catalog,
        format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len()),
        format!(
            "<< /Type /StructTreeRoot /K 4 0 R /ParentTree << /Nums [{}] >> /ParentTreeNextKey {} >>",
            parent_tree.trim_start(),
            pages.len()
        ),
        format!("<< /Type /StructElem /S /Document /P 3 0 R /K [{}] >>", children.join(" ")),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_owned(),
        match title {
            Some(title) => format!("<< /Title {} >>", utf16_hex(title)),
            None => "<< >>".to_owned(),
        },
    ];
    all.extend(objects);
    all.extend(elements);
    let mut bytes = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in all.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend(format!("{} 0 obj\n{body}\nendobj\n", index + 1).bytes());
    }
    let start = bytes.len();
    bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", all.len() + 1).bytes());
    for offset in offsets {
        bytes.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    bytes.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info 6 0 R >>\nstartxref\n{start}\n%%EOF\n",
            all.len() + 1
        )
        .bytes(),
    );

    let present = |field: &str| !array_at(&projection, field).is_empty();
    let on_pages = |field: &str| pages.iter().any(|page| !array_at(page, field).is_empty());
    let removed: Vec<Json> = [
        ("annotations", on_pages("annotations")),
        ("attachments", present("attachments")),
        ("forms", present("forms")),
        ("images", on_pages("images")),
        ("outline", present("outline")),
        ("signatures", present("signatures")),
    ]
    .into_iter()
    .filter(|(_, found)| *found)
    .map(|(feature, _)| text(feature))
    .collect();
    let optional = |value: Option<&str>| value.map_or(Json::Null, text);
    let receipt = object([
        ("blocks", Json::Arr(receipt_blocks)),
        (
            "claims",
            // No PDF/A or PDF/UA validator runs here, so nothing is claimed.
            object([
                ("pdf_a", text("not-claimed")),
                ("pdf_ua", text("not-claimed")),
            ]),
        ),
        (
            "fidelity",
            object([
                ("bounding_boxes", text("approximated")),
                ("fonts", text("normalized")),
                ("reading_order", text("exact")),
                ("text", text("exact")),
            ]),
        ),
        ("format", text("application/pdf")),
        ("language", optional(language)),
        ("receipt_version", text(RECEIPT_VERSION)),
        ("removed", Json::Arr(removed)),
        ("projection_sha256", text(sha256_hex(&imported.canonical))),
        ("tagged", Json::Bool(true)),
        ("title", optional(title)),
    ]);
    Ok(Exported {
        bytes,
        receipt: receipt.to_bytes(),
        emitted_text: emitted,
    })
}
