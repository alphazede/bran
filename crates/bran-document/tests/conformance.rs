//! Shared enterprise-document conformance corpus (issue #25).
//!
//! Every row is a synthetic package built in memory from the reviewable
//! `.parts` fixtures. Package rows run now through the shared intake. Rows
//! that need a format adapter are reported as unavailable with the adapter
//! issue that owns them; they are never counted as passing.

use bran_document::canonical::{sha256_hex, Json};
use bran_document::conformance::{self, Adapter, Anchor, Expect, Imported, PackageIntake};
use bran_document::zip::{self, WriteEntry};
use bran_document::{export, opc, xml, Cancel, Format, Limits, Refusal};
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// Recorded budgets. Runtime budgets are debug-build wall-clock ceilings for
// the whole tier; they gate the test suite, never an import outcome.
const FAST_RUNTIME_BUDGET: Duration = Duration::from_secs(10);
const FULL_RUNTIME_BUDGET: Duration = Duration::from_secs(120);
const FAST_PROPERTY_ITERATIONS: u32 = 2_000;
const FULL_PROPERTY_ITERATIONS: u32 = 50_000;
const MAX_FIXTURE_FILE_BYTES: u64 = 8 * 1024;
const MAX_FAST_PACKAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_FULL_PACKAGE_BYTES: usize = 24 * 1024 * 1024;

const FIXTURE_DIR: &str = "fixtures/enterprise-documents/conformance";
const DOCX_BASE: &str =
    include_str!("../../../fixtures/enterprise-documents/conformance/docx-base.parts");
const XLSX_BASE: &str =
    include_str!("../../../fixtures/enterprise-documents/conformance/xlsx-base.parts");
const PPTX_BASE: &str =
    include_str!("../../../fixtures/enterprise-documents/conformance/pptx-base.parts");
const CONFORMANCE_DOC: &str = include_str!("../../../docs/enterprise-document-conformance.md");
const CANARIES: &str =
    include_str!("../../../fixtures/public-boundary/rejected/synthetic-canaries.txt");
const OOXML: [Format; 3] = [Format::Docx, Format::Xlsx, Format::Pptx];
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tier {
    Fast,
    Full,
}

fn limits(tier: Tier) -> Limits {
    match tier {
        Tier::Fast => Limits {
            max_package_bytes: 1024 * 1024,
            max_parts: 64,
            max_part_bytes: 256 * 1024,
            max_total_bytes: 1024 * 1024,
            max_ratio: 100,
            max_xml_depth: 64,
            max_xml_nodes: 10_000,
        },
        Tier::Full => Limits::default(),
    }
}

/// Package rows: the shared intake, and later every registered adapter,
/// must produce exactly this outcome for each OOXML format.
const PACKAGE_ROWS: &[&str] = &[
    "ordinary",
    "macro-part",
    "macro-enabled-main",
    "ole-object",
    "external-media",
    "external-hyperlink",
    "zip-path-escape",
    "zip-absolute-path",
    "zip-backslash-path",
    "relationship-escape",
    "duplicate-part",
    "duplicate-part-case",
    "malformed-xml",
    "entity-expansion",
    "undeclared-entity",
    "decompression-bomb",
    "bomb-declared-size-lie",
    "encrypted-container",
    "encrypted-entry",
    "signed-package",
    "dlp-canary",
    "public-boundary-marker",
    "oversized-part",
    "oversized-package",
    "too-many-parts",
    "deep-nesting",
    "excessive-nodes",
    "missing-content-types",
    "not-a-zip",
    "cancelled",
    // Review repair round (findings 2-6).
    "foreign-attribute-macro",
    "canary-part-name",
    "canary-content-types",
    "truncated-deflate",
    "empty-deflate-stream",
    "local-header-method",
    "local-header-encryption",
    "xml-invalid-character",
    "xml-invalid-name",
];

/// Adapter rows need a content model. Each stays unavailable until an adapter
/// for its format registers; the issue number says who closes it.
const ADAPTER_ROWS: &[(Format, &str)] = &[
    (Format::Docx, "docx-ordinary-projection"),
    (Format::Docx, "docx-unsupported-benign-fidelity"),
    (Format::Docx, "docx-round-trip-anchors"),
    (Format::Xlsx, "xlsx-ordinary-projection"),
    (Format::Xlsx, "xlsx-unsupported-benign-fidelity"),
    (Format::Xlsx, "xlsx-round-trip-anchors"),
    (Format::Pptx, "pptx-ordinary-projection"),
    (Format::Pptx, "pptx-unsupported-benign-fidelity"),
    (Format::Pptx, "pptx-round-trip-anchors"),
    (Format::Pdf, "pdf-ordinary-projection"),
    (Format::Pdf, "pdf-malformed-object-graph"),
    (Format::Pdf, "pdf-recursive-structure"),
    (Format::Pdf, "pdf-active-action"),
    (Format::Pdf, "pdf-embedded-file"),
    (Format::Pdf, "pdf-encrypted"),
    (Format::Pdf, "pdf-signed"),
    (Format::Pdf, "pdf-dlp-canary"),
    (Format::Pdf, "pdf-oversized-stream"),
    (Format::Pdf, "pdf-round-trip-anchors"),
];

fn expect(row: &str) -> Expect {
    use Refusal::*;
    match row {
        "ordinary" => Expect::Admit(vec![]),
        "external-hyperlink" => Expect::Admit(vec!["hyperlink-not-fetched"]),
        "signed-package" => Expect::Admit(vec!["signature-not-verified"]),
        "dlp-canary" => Expect::Admit(vec!["dlp-findings"]),
        "public-boundary-marker" => Expect::Admit(vec!["public-boundary-violation"]),
        "macro-part" | "macro-enabled-main" | "ole-object" => Expect::Refuse(ActiveContent),
        "external-media" => Expect::Refuse(ExternalReference),
        "zip-path-escape" | "zip-absolute-path" | "zip-backslash-path" | "relationship-escape" => {
            Expect::Refuse(UnsafePartPath)
        }
        "duplicate-part" | "duplicate-part-case" => Expect::Refuse(DuplicatePart),
        "malformed-xml" | "undeclared-entity" => Expect::Refuse(MalformedXml),
        "entity-expansion" => Expect::Refuse(XmlDtdRefused),
        "decompression-bomb" => Expect::Refuse(DecompressionLimit),
        "bomb-declared-size-lie" | "missing-content-types" | "not-a-zip" => {
            Expect::Refuse(MalformedContainer)
        }
        "encrypted-container" | "encrypted-entry" => Expect::Refuse(Encrypted),
        "oversized-part" | "oversized-package" => Expect::Refuse(Oversized),
        "too-many-parts" => Expect::Refuse(TooManyParts),
        "deep-nesting" => Expect::Refuse(XmlDepthLimit),
        "excessive-nodes" => Expect::Refuse(XmlNodeLimit),
        "cancelled" => Expect::Refuse(Cancelled),
        "foreign-attribute-macro" => Expect::Refuse(ActiveContent),
        "canary-part-name" | "canary-content-types" => Expect::Admit(vec!["dlp-findings"]),
        "truncated-deflate" | "empty-deflate-stream" | "local-header-method" => {
            Expect::Refuse(MalformedContainer)
        }
        "local-header-encryption" => Expect::Refuse(Encrypted),
        "xml-invalid-character" | "xml-invalid-name" => Expect::Refuse(MalformedXml),
        other => panic!("row without expectation: {other}"),
    }
}

/// Package digests recorded from the shared intake. A change here is a change
/// to the canonical package projection and must be reviewed as one.
fn recorded_digest(format: Format) -> &'static str {
    match format {
        Format::Docx => "ba64f85dfd9fe0682eded458a98edd876440482c2cdd05b0bf8a9310f1db03f4",
        Format::Xlsx => "fe008b166581540851d7f421bbe865a3abe1475a9ae7bf029b9438db80321d6b",
        Format::Pptx => "f99b3050fcd2f78faab47113c9ce48cedb00c01e21c9708dd9721d72d01df30a",
        Format::Pdf => unreachable!("PDF is not an OPC package"),
    }
}

struct Layout {
    base: &'static str,
    main: &'static str,
    main_rels: &'static str,
    main_type: &'static str,
    macro_type: &'static str,
    dir: &'static str,
}

fn layout(format: Format) -> Layout {
    match format {
        Format::Docx => Layout {
            base: DOCX_BASE,
            main: "word/document.xml",
            main_rels: "word/_rels/document.xml.rels",
            main_type:
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
            macro_type: "application/vnd.ms-word.document.macroEnabled.main+xml",
            dir: "word",
        },
        Format::Xlsx => Layout {
            base: XLSX_BASE,
            main: "xl/workbook.xml",
            main_rels: "xl/_rels/workbook.xml.rels",
            main_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
            macro_type: "application/vnd.ms-excel.sheet.macroEnabled.main+xml",
            dir: "xl",
        },
        Format::Pptx => Layout {
            base: PPTX_BASE,
            main: "ppt/presentation.xml",
            main_rels: "ppt/_rels/presentation.xml.rels",
            main_type:
                "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml",
            macro_type: "application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml",
            dir: "ppt",
        },
        Format::Pdf => unreachable!("PDF is not an OPC package"),
    }
}

/// An ordered list of ZIP entries built from a `.parts` fixture.
#[derive(Clone)]
struct Parts(Vec<(String, Vec<u8>)>);

impl Parts {
    fn parse(text: &str) -> Self {
        let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
        for line in text.lines() {
            if let Some(name) = line.strip_prefix("--- ") {
                parts.push((name.to_owned(), Vec::new()));
            } else {
                let body = &mut parts.last_mut().expect("part header first").1;
                if !body.is_empty() {
                    body.push(b'\n');
                }
                body.extend_from_slice(line.as_bytes());
            }
        }
        Self(parts)
    }

    fn add(mut self, name: &str, data: impl Into<Vec<u8>>) -> Self {
        self.0.push((name.to_owned(), data.into()));
        self
    }

    fn remove(mut self, name: &str) -> Self {
        self.0.retain(|(part, _)| part != name);
        self
    }

    fn edit(mut self, name: &str, from: &str, to: &str) -> Self {
        let part = self
            .0
            .iter_mut()
            .find(|(part, _)| part == name)
            .expect("part");
        let text = String::from_utf8(part.1.clone()).expect("utf-8 part");
        assert!(text.contains(from), "edit anchor missing in {name}");
        part.1 = text.replacen(from, to, 1).into_bytes();
        self
    }

    fn set(mut self, name: &str, data: impl Into<Vec<u8>>) -> Self {
        let part = self
            .0
            .iter_mut()
            .find(|(part, _)| part == name)
            .expect("part");
        part.1 = data.into();
        self
    }

    fn content_type(self, part: &str, content_type: &str) -> Self {
        let line =
            format!("<Override PartName=\"/{part}\" ContentType=\"{content_type}\"/>\n</Types>");
        self.edit("[Content_Types].xml", "</Types>", &line)
    }

    fn zip_with(&self, reverse: bool, deflate: bool, dos_date: u16) -> Vec<u8> {
        let mut entries: Vec<WriteEntry<'_>> = self
            .0
            .iter()
            .map(|(name, data)| WriteEntry {
                name,
                data,
                deflate,
                dos_time: 0x6000,
                dos_date,
            })
            .collect();
        if reverse {
            entries.reverse();
        }
        zip::write(&entries)
    }

    fn zip(&self) -> Vec<u8> {
        self.zip_with(false, true, 0x5921)
    }
}

fn relationship(id: &str, kind: &str, target: &str, external: bool) -> String {
    let mode = if external {
        " TargetMode=\"External\""
    } else {
        ""
    };
    format!("<Relationship Id=\"{id}\" Type=\"{REL}/{kind}\" Target=\"{target}\"{mode}/>\n</Relationships>")
}

/// Rewrites one field of the named entry in both its central and local header.
/// Offsets of the named entry's central record and local header.
fn entry_offsets(bytes: &[u8], name: &str) -> (usize, usize) {
    let signature = [0x50, 0x4b, 0x01, 0x02];
    let mut at = 0;
    while at + 46 <= bytes.len() {
        if bytes[at..at + 4] == signature {
            let name_len = u16::from_le_bytes([bytes[at + 28], bytes[at + 29]]) as usize;
            if &bytes[at + 46..at + 46 + name_len] == name.as_bytes() {
                let local = u32::from_le_bytes(bytes[at + 42..at + 46].try_into().unwrap());
                return (at, local as usize);
            }
        }
        at += 1;
    }
    panic!("entry {name} not found");
}

/// Rewrites one field of the named entry in both its central and local header.
fn patch_entry(
    bytes: &mut [u8],
    name: &str,
    central_field: usize,
    local_field: usize,
    value: &[u8],
) {
    let (central, local) = entry_offsets(bytes, name);
    bytes[central + central_field..central + central_field + value.len()].copy_from_slice(value);
    bytes[local + local_field..local + local_field + value.len()].copy_from_slice(value);
}

/// Rewrites one field of the named entry's local header only.
fn patch_local(bytes: &mut [u8], name: &str, local_field: usize, value: &[u8]) {
    let (_, local) = entry_offsets(bytes, name);
    bytes[local + local_field..local + local_field + value.len()].copy_from_slice(value);
}

/// Drops the last compressed byte of the named entry, which must be the last
/// entry before the central directory, and fixes every size and offset.
/// Output size and CRC keep their original values.
fn truncate_last_entry(bytes: &mut Vec<u8>, name: &str) {
    let (central, local) = entry_offsets(bytes, name);
    let field = |bytes: &[u8], at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let compressed = field(bytes, central + 20) - 1;
    bytes[central + 20..central + 24].copy_from_slice(&compressed.to_le_bytes());
    bytes[local + 18..local + 22].copy_from_slice(&compressed.to_le_bytes());
    let end = bytes.len() - 22;
    let cd_offset = field(bytes, end + 16);
    bytes.remove(cd_offset as usize - 1);
    let end = bytes.len() - 22;
    bytes[end + 16..end + 20].copy_from_slice(&(cd_offset - 1).to_le_bytes());
}

fn canary() -> &'static str {
    CANARIES
        .lines()
        .find(|line| !line.is_empty())
        .expect("synthetic canary")
}

fn build(row: &str, format: Format, tier: Tier, limits: &Limits) -> Vec<u8> {
    let l = layout(format);
    let base = Parts::parse(l.base);
    let media = format!("{}/media/synthetic.png", l.dir);
    let bomb_bytes = (limits.max_part_bytes as usize) * 2;
    match row {
        "ordinary" | "cancelled" => base.zip(),
        "macro-part" => {
            let part = format!("{}/vbaProject.bin", l.dir);
            base.add(&part, "synthetic inert bytes")
                .content_type(&part, "application/vnd.ms-office.vbaProject")
                .zip()
        }
        "macro-enabled-main" => base.edit("[Content_Types].xml", l.main_type, l.macro_type).zip(),
        "ole-object" => {
            let part = format!("{}/embeddings/oleObject1.bin", l.dir);
            base.add(&part, "synthetic inert bytes")
                .content_type(&part, "application/vnd.openxmlformats-officedocument.oleObject")
                .zip()
        }
        "external-media" => base
            .edit(l.main_rels, "</Relationships>", &relationship("rId9", "image", "https://example.invalid/remote.png", true))
            .zip(),
        "external-hyperlink" => base
            .edit(l.main_rels, "</Relationships>", &relationship("rId9", "hyperlink", "https://example.invalid/page", true))
            .zip(),
        "zip-path-escape" => base.add("../outside.xml", "<x/>").zip(),
        "zip-absolute-path" => base.add("/absolute.xml", "<x/>").zip(),
        "zip-backslash-path" => base.add(&format!("{}\\evil.xml", l.dir), "<x/>").zip(),
        "relationship-escape" => base
            .edit(l.main_rels, "</Relationships>", &relationship("rId9", "image", "../../outside.png", false))
            .zip(),
        "duplicate-part" => base.add(l.main, "<x/>").zip(),
        "duplicate-part-case" => base.add(&l.main.to_uppercase(), "<x/>").zip(),
        "malformed-xml" => base.edit("[Content_Types].xml", "</Types>", "").zip(),
        "entity-expansion" => base
            .edit(
                "_rels/.rels",
                "<Relationships",
                "<!DOCTYPE r [<!ENTITY a \"aaaaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;\"><!ENTITY c \"&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;\">]>\n<Relationships",
            )
            .zip(),
        "undeclared-entity" => base.edit("_rels/.rels", "Id=\"rId1\"", "Id=\"&undeclared;\"").zip(),
        "decompression-bomb" => base.add(&media, vec![0u8; bomb_bytes]).zip(),
        "bomb-declared-size-lie" => {
            let mut bytes = base.add(&media, vec![0u8; bomb_bytes]).zip();
            patch_entry(&mut bytes, &media, 24, 22, &1000u32.to_le_bytes());
            bytes
        }
        "encrypted-container" => {
            let mut bytes = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
            bytes.resize(512, 0);
            bytes
        }
        "encrypted-entry" => {
            let mut bytes = base.zip();
            patch_entry(&mut bytes, l.main, 8, 6, &1u16.to_le_bytes());
            bytes
        }
        "signed-package" => base
            .add("_xmlsignatures/origin.sigs", "")
            .add("_xmlsignatures/sig1.xml", "<Signature xmlns=\"http://www.w3.org/2000/09/xmldsig#\"/>")
            .content_type("_xmlsignatures/origin.sigs", "application/vnd.openxmlformats-package.digital-signature-origin")
            .content_type("_xmlsignatures/sig1.xml", "application/vnd.openxmlformats-package.digital-signature-xmlsignature+xml")
            .edit("_rels/.rels", "</Relationships>", "<Relationship Id=\"rId9\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/digital-signature/origin\" Target=\"_xmlsignatures/origin.sigs\"/>\n</Relationships>")
            .zip(),
        "dlp-canary" => {
            let text = String::from_utf8(base.0.iter().find(|(n, _)| n == l.main).unwrap().1.clone()).unwrap();
            base.clone().set(l.main, text.replace("?>", &format!("?><!-- {} -->", canary()))).zip()
        }
        "public-boundary-marker" => {
            let text = String::from_utf8(base.0.iter().find(|(n, _)| n == l.main).unwrap().1.clone()).unwrap();
            base.clone().set(l.main, text.replace("?>", "?><!-- important_boundary -->")).zip()
        }
        "oversized-part" => base
            .add(&media, noise(limits.max_part_bytes as usize + 1))
            .zip_with(false, false, 0x5921),
        "oversized-package" => {
            let (mut parts, mut remaining) = (base, limits.max_package_bytes as usize + 1);
            for index in 0.. {
                if remaining == 0 {
                    break;
                }
                let size = remaining.min(limits.max_part_bytes as usize);
                parts = parts.add(&format!("{}/media/filler{index}.png", l.dir), noise(size));
                remaining -= size;
            }
            parts.zip_with(false, false, 0x5921)
        }
        "too-many-parts" => {
            let mut parts = base;
            for index in 0..limits.max_parts {
                parts = parts.add(&format!("{}/media/part{index}.png", l.dir), "p");
            }
            parts.zip()
        }
        "deep-nesting" => {
            let depth = if tier == Tier::Full { 1_000_000 } else { limits.max_xml_depth + 1 };
            let body = format!("{}{}", "<x>".repeat(depth), "</x>".repeat(depth));
            // Stored, so the repetitive part reaches the XML reader instead of
            // tripping the compression-ratio limit first.
            base.edit(l.main_rels, "</Relationships>", &format!("{body}</Relationships>"))
                .zip_with(false, false, 0x5921)
        }
        "excessive-nodes" => {
            let body = "<x/>".repeat(limits.max_xml_nodes + 1);
            base.edit(l.main_rels, "</Relationships>", &format!("{body}</Relationships>"))
                .zip_with(false, false, 0x5921)
        }
        "missing-content-types" => base.remove("[Content_Types].xml").zip(),
        "not-a-zip" => b"Synthetic plain text that is not a ZIP container.".to_vec(),
        "foreign-attribute-macro" => base
            .edit("[Content_Types].xml", "<Types xmlns=", "<Types xmlns:x=\"urn:synthetic\" xmlns=")
            .edit(
                "[Content_Types].xml",
                &format!("ContentType=\"{}\"", l.main_type),
                &format!("x:ContentType=\"application/xml\" ContentType=\"{}\"", l.macro_type),
            )
            .zip(),
        "canary-part-name" => base.add(&format!("{}/{}.xml", l.dir, canary()), "<a/>").zip(),
        "canary-content-types" => base
            .edit("[Content_Types].xml", "<Default", &format!("<!-- {} --><Default", canary()))
            .zip(),
        "truncated-deflate" => {
            let part = format!("{}/extra.xml", l.dir);
            let mut bytes = base.add(&part, "<a>Hello</a>").zip();
            truncate_last_entry(&mut bytes, &part);
            bytes
        }
        "empty-deflate-stream" => {
            let part = format!("{}/empty.xml", l.dir);
            let mut bytes = base.add(&part, "").zip_with(false, false, 0x5921);
            patch_entry(&mut bytes, &part, 10, 8, &8u16.to_le_bytes());
            bytes
        }
        "local-header-method" => {
            let mut bytes = base.zip_with(false, false, 0x5921);
            patch_local(&mut bytes, l.main, 8, &99u16.to_le_bytes());
            bytes
        }
        "local-header-encryption" => {
            let mut bytes = base.zip_with(false, false, 0x5921);
            patch_local(&mut bytes, l.main, 6, &1u16.to_le_bytes());
            bytes
        }
        "xml-invalid-character" => base.edit("[Content_Types].xml", "</Types>", "\0</Types>").zip(),
        "xml-invalid-name" => base
            .edit(l.main_rels, "</Relationships>", "<1/></Relationships>")
            .zip(),
        other => panic!("row without builder: {other}"),
    }
}

/// Incompressible deterministic bytes (xorshift), so stored and deflated
/// sizes stay close and ratio limits do not fire first.
fn noise(len: usize) -> Vec<u8> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    (0..len).map(|_| rng.next() as u8).collect()
}

/// Logically equal re-encodings: reversed entry order, other timestamps, and
/// the other compression method.
fn variants(row: &str, format: Format) -> Vec<Vec<u8>> {
    if !matches!(expect(row), Expect::Admit(_)) || row == "cancelled" {
        return Vec::new();
    }
    let tier_limits = limits(Tier::Fast);
    let original = build(row, format, Tier::Fast, &tier_limits);
    let entries =
        zip::read(&original, &tier_limits, &Cancel::default()).expect("admitted row reads");
    let parts = Parts(
        entries
            .into_iter()
            .map(|entry| (entry.name, entry.data))
            .collect(),
    );
    vec![
        parts.zip_with(true, false, 0x4A21),
        parts.zip_with(false, true, 0x3C01),
    ]
}

struct Report {
    checked: usize,
    unavailable: Vec<(Format, &'static str, u32)>,
}

fn run_corpus(tier: Tier) -> Report {
    let limits = limits(tier);
    let max_package = if tier == Tier::Fast {
        MAX_FAST_PACKAGE_BYTES
    } else {
        MAX_FULL_PACKAGE_BYTES
    };
    let mut report = Report {
        checked: 0,
        unavailable: Vec::new(),
    };
    for format in OOXML {
        let intake = PackageIntake(format);
        let registered = conformance::registered();
        let adapters: Vec<&dyn Adapter> = std::iter::once(&intake as &dyn Adapter)
            .chain(
                registered
                    .into_iter()
                    .filter(|adapter| adapter.format() == format),
            )
            .collect();
        for row in PACKAGE_ROWS {
            let input = build(row, format, tier, &limits);
            assert!(
                input.len() <= max_package,
                "{format:?}/{row}: package exceeds size budget"
            );
            let variants = if tier == Tier::Fast {
                variants(row, format)
            } else {
                Vec::new()
            };
            let cancel = Cancel::default();
            if *row == "cancelled" {
                cancel.cancel();
            }
            for adapter in &adapters {
                conformance::check(*adapter, &input, &variants, &expect(row), &limits, &cancel)
                    .unwrap_or_else(|error| panic!("{format:?}/{row}: {error}"));
                report.checked += 1;
            }
        }
        check_recorded_digest(format);
        report.checked += 1;
    }
    for (format, row) in ADAPTER_ROWS {
        if conformance::registered()
            .iter()
            .all(|adapter| adapter.format() != *format)
        {
            report
                .unavailable
                .push((*format, row, format.adapter_issue()));
        } else {
            panic!("{format:?} adapter registered: replace {row} with an executable row");
        }
    }
    report
}

/// The ordinary package's canonical projection matches the recorded digest.
fn check_recorded_digest(format: Format) {
    let limits = limits(Tier::Fast);
    let package = opc::open(
        &build("ordinary", format, Tier::Fast, &limits),
        &limits,
        &Cancel::default(),
    )
    .expect("ordinary package opens");
    assert_eq!(
        sha256_hex(&package.canonical_bytes()),
        recorded_digest(format),
        "{format:?} projection changed"
    );
}

fn fixture_paths() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(FIXTURE_DIR);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(root)
        .expect("fixture directory")
        .map(|entry| entry.expect("entry").path())
        .collect();
    paths.sort();
    paths
}

#[test]
fn enterprise_conformance_fast() {
    let started = Instant::now();
    for path in fixture_paths() {
        let size = std::fs::metadata(&path).expect("fixture").len();
        assert!(
            size <= MAX_FIXTURE_FILE_BYTES,
            "{} exceeds fixture budget",
            path.display()
        );
    }
    let report = run_corpus(Tier::Fast);
    properties(FAST_PROPERTY_ITERATIONS);
    let elapsed = started.elapsed();
    println!(
        "fast tier: {} checks, {} unavailable adapter rows, {:?} of {:?} budget",
        report.checked,
        report.unavailable.len(),
        elapsed,
        FAST_RUNTIME_BUDGET
    );
    for (format, row, issue) in &report.unavailable {
        println!("unavailable {format:?}/{row}: needs adapter #{issue}");
    }
    assert_eq!(report.unavailable.len(), ADAPTER_ROWS.len());
    assert!(
        elapsed <= FAST_RUNTIME_BUDGET,
        "fast tier exceeded its runtime budget: {elapsed:?}"
    );
}

#[test]
#[ignore = "full tier: run by tools/ci/check.sh --full (security gate)"]
fn enterprise_conformance_full() {
    let started = Instant::now();
    let report = run_corpus(Tier::Full);
    properties(FULL_PROPERTY_ITERATIONS);
    let elapsed = started.elapsed();
    println!(
        "full tier: {} checks, {:?} of {:?} budget",
        report.checked, elapsed, FULL_RUNTIME_BUDGET
    );
    assert!(
        elapsed <= FULL_RUNTIME_BUDGET,
        "full tier exceeded its runtime budget: {elapsed:?}"
    );
}

#[test]
fn every_corpus_row_is_documented() {
    for row in PACKAGE_ROWS
        .iter()
        .chain(ADAPTER_ROWS.iter().map(|(_, row)| row))
    {
        assert!(
            CONFORMANCE_DOC.contains(&format!("`{row}`")),
            "row {row} missing from the conformance document"
        );
    }
}

// ---- Harness self-verification: the checks must catch broken adapters. ----

struct Wrapped<F: Fn(&[u8], &Limits, &Cancel) -> Result<Imported, Refusal> + Sync>(F);

impl<F: Fn(&[u8], &Limits, &Cancel) -> Result<Imported, Refusal> + Sync> Adapter for Wrapped<F> {
    fn format(&self) -> Format {
        Format::Docx
    }
    fn import(&self, bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Imported, Refusal> {
        (self.0)(bytes, limits, cancel)
    }
}

fn harness_verdict(adapter: &dyn Adapter, row: &str) -> Result<conformance::Outcome, String> {
    let limits = limits(Tier::Fast);
    let input = build(row, Format::Docx, Tier::Fast, &limits);
    conformance::check(
        adapter,
        &input,
        &variants(row, Format::Docx),
        &expect(row),
        &limits,
        &Cancel::default(),
    )
}

fn intake(bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Imported, Refusal> {
    PackageIntake(Format::Docx).import(bytes, limits, cancel)
}

#[test]
fn harness_rejects_nondeterministic_adapter() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let adapter = Wrapped(|bytes: &[u8], limits: &Limits, cancel: &Cancel| {
        let mut imported = intake(bytes, limits, cancel)?;
        imported
            .canonical
            .push(CALLS.fetch_add(1, Ordering::SeqCst) as u8);
        Ok(imported)
    });
    let error = harness_verdict(&adapter, "ordinary").unwrap_err();
    assert!(error.contains("nondeterministic"), "{error}");
}

#[test]
fn harness_rejects_archive_order_dependence() {
    let adapter = Wrapped(|bytes: &[u8], limits: &Limits, cancel: &Cancel| {
        let mut imported = intake(bytes, limits, cancel)?;
        imported.canonical = sha256_hex(bytes).into_bytes();
        Ok(imported)
    });
    let error = harness_verdict(&adapter, "ordinary").unwrap_err();
    assert!(error.contains("re-encoding"), "{error}");
}

#[test]
fn harness_rejects_panicking_adapter() {
    let adapter = Wrapped(
        |_: &[u8], _: &Limits, _: &Cancel| -> Result<Imported, Refusal> {
            panic!("synthetic adapter panic")
        },
    );
    let error = harness_verdict(&adapter, "malformed-xml").unwrap_err();
    assert!(error.contains("panicked"), "{error}");
}

#[test]
fn harness_rejects_missing_fidelity_receipt() {
    let adapter = Wrapped(|bytes: &[u8], limits: &Limits, cancel: &Cancel| {
        let mut imported = intake(bytes, limits, cancel)?;
        imported.receipt.clear();
        Ok(imported)
    });
    let error = harness_verdict(&adapter, "external-hyperlink").unwrap_err();
    assert!(error.contains("hyperlink-not-fetched"), "{error}");
}

#[test]
fn harness_rejects_wrong_or_missing_refusal() {
    let lenient = Wrapped(|_: &[u8], _: &Limits, _: &Cancel| {
        Ok(Imported {
            canonical: Vec::new(),
            receipt: Vec::new(),
            anchors: Vec::new(),
        })
    });
    assert!(harness_verdict(&lenient, "external-media")
        .unwrap_err()
        .contains("external-reference"));
    let wrong = Wrapped(|_: &[u8], _: &Limits, _: &Cancel| Err(Refusal::MalformedXml));
    assert!(harness_verdict(&wrong, "external-media")
        .unwrap_err()
        .contains("external-reference"));
}

struct DriftingRoundTrip;

impl Adapter for DriftingRoundTrip {
    fn format(&self) -> Format {
        Format::Docx
    }
    fn import(&self, bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Imported, Refusal> {
        let anchor = |id: &str| {
            vec![Anchor {
                id: id.to_owned(),
                text_digest: sha256_hex(b"text"),
            }]
        };
        if bytes == b"exported" {
            return Ok(Imported {
                canonical: Vec::new(),
                receipt: Vec::new(),
                anchors: anchor("anchor-2"),
            });
        }
        let mut imported = intake(bytes, limits, cancel)?;
        imported.anchors = anchor("anchor-1");
        Ok(imported)
    }
    fn export(&self, _imported: &Imported) -> Result<Vec<u8>, Refusal> {
        Ok(b"exported".to_vec())
    }
}

#[test]
fn harness_rejects_round_trip_anchor_drift() {
    let error = harness_verdict(&DriftingRoundTrip, "ordinary").unwrap_err();
    assert!(error.contains("anchor"), "{error}");
}

#[test]
fn package_intake_reports_no_round_trip() {
    let outcome = harness_verdict(&PackageIntake(Format::Docx), "ordinary").expect("intake passes");
    assert!(
        !outcome.round_trip,
        "package intake has no export, so no round trip is claimed"
    );
}

// ---- Property targets (deterministic, seeded; no coverage-guided fuzzing). ----

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
    fn text(&mut self, alphabet: &[&str], max: usize) -> String {
        (0..self.below(max + 1))
            .map(|_| alphabet[self.below(alphabet.len())])
            .collect()
    }
}

const PATH_ALPHABET: &[&str] = &[
    "a", "B", "/", ".", "..", "\\", "%", "2F", "5c", ":", "\0", " ", "\u{e9}", "_rels", "x.xml",
    "#", "?", "\n",
];

fn properties(iterations: u32) {
    let mut rng = Rng(0x5EED_0025);
    for _ in 0..iterations {
        package_path_property(&mut rng);
        relationship_resolution_property(&mut rng);
        canonical_serialization_property(&mut rng);
    }
    parser_limit_property(&mut rng, iterations);
}

fn package_path_property(rng: &mut Rng) {
    let name = rng.text(PATH_ALPHABET, 12);
    if opc::validate_part_name(&name).is_ok() {
        assert!(
            !name.is_empty() && !name.starts_with('/') && !name.contains('\\'),
            "{name:?}"
        );
        assert!(!name.chars().any(char::is_control), "{name:?}");
        assert!(
            name.split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != ".."),
            "{name:?}"
        );
        assert!(
            !name.to_ascii_lowercase().contains("%2f")
                && !name.to_ascii_lowercase().contains("%5c"),
            "{name:?}"
        );
    }
}

fn relationship_resolution_property(rng: &mut Rng) {
    let sources = [
        "",
        "word/document.xml",
        "xl/worksheets/sheet1.xml",
        "a/b/c/d.xml",
    ];
    let source = sources[rng.below(sources.len())];
    let target = rng.text(PATH_ALPHABET, 10);
    if let Ok(resolved) = opc::resolve_target(source, &target) {
        assert_eq!(
            opc::validate_part_name(&resolved),
            Ok(()),
            "{source:?} + {target:?} -> {resolved:?}"
        );
    }
}

fn random_json(rng: &mut Rng, depth: usize) -> Json {
    match rng.below(if depth > 3 { 4 } else { 6 }) {
        0 => Json::Null,
        1 => Json::Bool(rng.below(2) == 0),
        2 => Json::Int(rng.next() as i64),
        3 => Json::Str(rng.text(
            &[
                "a",
                "\"",
                "\\",
                "\n",
                "\u{1}",
                "\u{7f}",
                "\u{e9}",
                "\u{2028}",
                "\u{1F600}",
                "/",
            ],
            6,
        )),
        4 => Json::Arr(
            (0..rng.below(4))
                .map(|_| random_json(rng, depth + 1))
                .collect(),
        ),
        _ => Json::Obj(
            (0..rng.below(4))
                .map(|_| {
                    (
                        rng.text(&["k", "\u{e9}", "Z", "a"], 3),
                        random_json(rng, depth + 1),
                    )
                })
                .collect(),
        ),
    }
}

fn canonical_serialization_property(rng: &mut Rng) {
    let value = random_json(rng, 0);
    let bytes = value.to_bytes();
    let text = String::from_utf8(bytes.clone()).expect("canonical bytes are UTF-8");
    assert!(
        !text.chars().any(|c| (c as u32) < 0x20),
        "raw control character in {text:?}"
    );
    if let Json::Obj(map) = &value {
        let mut reversed = BTreeMap::new();
        for (key, item) in map.iter().rev() {
            reversed.insert(key.clone(), item.clone());
        }
        assert_eq!(
            Json::Obj(reversed).to_bytes(),
            bytes,
            "insertion order changed canonical bytes"
        );
    }
    assert_eq!(
        value.clone().to_bytes(),
        bytes,
        "serialization is not repeatable"
    );
}

fn parser_limit_property(rng: &mut Rng, iterations: u32) {
    let limits = limits(Tier::Fast);
    let seeds: Vec<Vec<u8>> = OOXML
        .iter()
        .map(|format| build("ordinary", *format, Tier::Fast, &limits))
        .collect();
    for _ in 0..iterations / 4 {
        let mut bytes = seeds[rng.below(seeds.len())].clone();
        match rng.below(3) {
            0 => {
                for _ in 0..=rng.below(8) {
                    let at = rng.below(bytes.len());
                    bytes[at] ^= 1 << rng.below(8);
                }
            }
            1 => bytes.truncate(rng.below(bytes.len())),
            _ => {
                let at = rng.below(bytes.len());
                let insert: Vec<u8> = (0..rng.below(16)).map(|_| rng.next() as u8).collect();
                bytes.splice(at..at, insert);
            }
        }
        let opened = catch_unwind(AssertUnwindSafe(|| {
            opc::open(&bytes, &limits, &Cancel::default())
        }));
        let package = opened.expect("mutated package must not panic");
        if let Ok(package) = package {
            let total: usize = package.parts.iter().map(|part| part.data.len()).sum();
            assert!(total as u64 <= limits.max_total_bytes);
            assert!(package.parts.len() <= limits.max_parts);
        }
        let soup = rng.text(
            &[
                "<a>",
                "</a>",
                "<b/>",
                "&amp;",
                "&x;",
                "<!DOCTYPE",
                "<![CDATA[",
                "]]>",
                "<?p?>",
                "<!--",
                "-->",
                "\"",
                "=",
                " ",
            ],
            24,
        );
        let parsed = catch_unwind(AssertUnwindSafe(|| {
            xml::parse(soup.as_bytes(), &limits, &Cancel::default())
        }));
        parsed.expect("XML soup must not panic").ok();
    }
}

// ---- Canonical serialization golden (bytes produced by Python's json.dumps). ----

#[test]
fn canonical_json_matches_envelope_rule() {
    let mut inner = BTreeMap::new();
    inner.insert(
        "z".to_owned(),
        Json::Arr(vec![Json::Int(-7), Json::Bool(true), Json::Null]),
    );
    inner.insert(
        "a\u{e9}".to_owned(),
        Json::Str("quote\" slash\\ nl\n tab\t ctl\u{1} del\u{7f} \u{2028} /".to_owned()),
    );
    let value = Json::Obj(BTreeMap::from([
        ("outer".to_owned(), Json::Obj(inner)),
        ("b".to_owned(), Json::Int(0)),
    ]));
    let expected = "{\"b\":0,\"outer\":{\"a\u{e9}\":\"quote\\\" slash\\\\ nl\\n tab\\t ctl\\u0001 del\u{7f} \u{2028} /\",\"z\":[-7,true,null]}}";
    assert_eq!(String::from_utf8(value.to_bytes()).unwrap(), expected);
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

// ---- Shared export gate. ----

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

fn export_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bran-document-export-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(root.join("out")).expect("create export root");
    root
}

#[test]
fn export_gate_refuses_dlp_before_writing() {
    let root = export_root();
    let result = export::write_new(
        &root,
        "out/report.docx",
        Format::Docx,
        b"bytes",
        &["clean", canary()],
    );
    assert_eq!(result, Err(Refusal::DlpFindings));
    let result = export::write_new(
        &root,
        "out/report.docx",
        Format::Docx,
        b"bytes",
        &["important_boundary"],
    );
    assert_eq!(result, Err(Refusal::PublicBoundary));
    assert!(
        !root.join("out/report.docx").exists(),
        "no file may be written after a DLP refusal"
    );
    assert_eq!(
        std::fs::read_dir(root.join("out")).unwrap().count(),
        0,
        "no temporary file may remain"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn export_gate_never_overwrites() {
    let root = export_root();
    let written = export::write_new(&root, "out/deck.pptx", Format::Pptx, b"first", &[])
        .expect("first export");
    assert_eq!(std::fs::read(&written).unwrap(), b"first");
    let again = export::write_new(&root, "out/deck.pptx", Format::Pptx, b"second", &[]);
    assert_eq!(again, Err(Refusal::ExportExists));
    assert_eq!(
        std::fs::read(&written).unwrap(),
        b"first",
        "existing export must be unchanged"
    );
    assert_eq!(
        std::fs::read_dir(root.join("out")).unwrap().count(),
        1,
        "no temporary file may remain"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn export_gate_requires_contained_destination_and_matching_format() {
    let root = export_root();
    for bad in [
        "../escape.docx",
        "/abs.docx",
        "out\\x.docx",
        "missing/x.docx",
        "out/./x.docx",
        "",
    ] {
        assert_eq!(
            export::write_new(&root, bad, Format::Docx, b"x", &[]),
            Err(Refusal::ExportContainment),
            "{bad:?}"
        );
    }
    assert_eq!(
        export::write_new(&root, "out/x.pdf", Format::Docx, b"x", &[]),
        Err(Refusal::ExportFormatMismatch)
    );
    #[cfg(unix)]
    {
        let outside = export_root();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        assert_eq!(
            export::write_new(&root, "link/x.docx", Format::Docx, b"x", &[]),
            Err(Refusal::ExportContainment)
        );
        std::fs::remove_dir_all(outside).unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}

// ---- Review repair round: the reviewer's exact reproducers. ----

const REVIEW_TYPES: &[u8] = b"<Types xmlns='http://schemas.openxmlformats.org/package/2006/content-types'><Default Extension='xml' ContentType='application/xml'/></Types>";

fn review_entry<'a>(name: &'a str, data: &'a [u8], deflate: bool) -> WriteEntry<'a> {
    WriteEntry {
        name,
        data,
        deflate,
        dos_time: 0,
        dos_date: 0,
    }
}

fn review_package(types: &[u8], name: &str) -> Vec<u8> {
    zip::write(&[
        review_entry("[Content_Types].xml", types, false),
        review_entry(name, b"<a/>", false),
    ])
}

fn review_u32(bytes: &[u8], at: usize) -> usize {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
}

fn review_patch32(bytes: &mut [u8], at: usize, value: usize) {
    bytes[at..at + 4].copy_from_slice(&(value as u32).to_le_bytes());
}

#[test]
fn review_failed_export_preserves_preexisting_file() {
    let root = export_root();
    let path = root.join(".out.docx.bran-export");
    std::fs::write(&path, b"pre-existing user data").unwrap();
    let result = export::write_new(&root, "out.docx", Format::Docx, b"new bytes", &["safe"]);
    let kept = std::fs::read(&path);
    let written = std::fs::read(root.join("out.docx"));
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(
        kept.ok().as_deref(),
        Some(b"pre-existing user data".as_slice()),
        "export removed an unowned file"
    );
    // A stale temporary name must not block the export either.
    assert_eq!(
        result.map(|path| path.file_name().unwrap().to_owned()),
        Ok("out.docx".into())
    );
    assert_eq!(written.ok().as_deref(), Some(b"new bytes".as_slice()));
}

#[test]
fn review_foreign_attribute_cannot_mask_macro_content_type() {
    let types = b"<Types xmlns='http://schemas.openxmlformats.org/package/2006/content-types' xmlns:x='urn:synthetic'><Default Extension='xml' ContentType='application/xml'/><Override PartName='/a.xml' x:ContentType='application/xml' ContentType='application/vnd.ms-word.document.macroEnabled.main+xml'/></Types>";
    let opened = opc::open(
        &review_package(types, "a.xml"),
        &Limits::default(),
        &Cancel::default(),
    );
    assert_eq!(opened, Err(Refusal::ActiveContent));
}

#[test]
fn review_dlp_scans_part_names() {
    let name = format!("{}.xml", canary());
    let package = opc::open(
        &review_package(REVIEW_TYPES, &name),
        &Limits::default(),
        &Cancel::default(),
    )
    .unwrap();
    assert!(
        package.diagnostics.contains("dlp-findings"),
        "{:?}",
        package.diagnostics
    );
}

#[test]
fn review_dlp_scans_content_types() {
    let types = format!(
        "<Types><!-- {} --><Default Extension='xml' ContentType='application/xml'/></Types>",
        canary()
    );
    let package = opc::open(
        &review_package(types.as_bytes(), "a.xml"),
        &Limits::default(),
        &Cancel::default(),
    )
    .unwrap();
    assert!(
        package.diagnostics.contains("dlp-findings"),
        "{:?}",
        package.diagnostics
    );
}

#[test]
fn review_dlp_scans_normalized_metadata() {
    // A character reference hides the canary from a raw byte scan, but the
    // normalized content type is emitted in the canonical bytes.
    let hidden = canary().replacen('A', "&#65;", 1);
    let types =
        format!("<Types><Default Extension='xml' ContentType='application/{hidden}'/></Types>");
    let package = opc::open(
        &review_package(types.as_bytes(), "a.xml"),
        &Limits::default(),
        &Cancel::default(),
    )
    .unwrap();
    assert!(
        package.diagnostics.contains("dlp-findings"),
        "{:?}",
        package.diagnostics
    );
}

#[test]
fn review_deflate_requires_stream_end() {
    let mut bytes = zip::write(&[review_entry("a.xml", b"<a>Hello</a>", true)]);
    let cd = review_u32(&bytes, bytes.len() - 6);
    let compressed = review_u32(&bytes, 18);
    bytes.remove(cd - 1);
    review_patch32(&mut bytes, 18, compressed - 1);
    review_patch32(&mut bytes, cd - 1 + 20, compressed - 1);
    let end = bytes.len() - 6;
    review_patch32(&mut bytes, end, cd - 1);
    assert_eq!(
        zip::read(&bytes, &Limits::default(), &Cancel::default()),
        Err(Refusal::MalformedContainer)
    );
}

#[test]
fn review_empty_deflate_stream_is_refused() {
    let mut bytes = zip::write(&[review_entry("a.xml", b"", false)]);
    patch_entry(&mut bytes, "a.xml", 10, 8, &8u16.to_le_bytes());
    assert_eq!(
        zip::read(&bytes, &Limits::default(), &Cancel::default()),
        Err(Refusal::MalformedContainer)
    );
}

#[test]
fn review_deflate_rejects_trailing_compressed_bytes() {
    let mut bytes = zip::write(&[review_entry("a.xml", b"<a>Hello</a>", true)]);
    let cd = review_u32(&bytes, bytes.len() - 6);
    let compressed = review_u32(&bytes, 18);
    bytes.insert(cd, 0);
    review_patch32(&mut bytes, 18, compressed + 1);
    review_patch32(&mut bytes, cd + 1 + 20, compressed + 1);
    let end = bytes.len() - 6;
    review_patch32(&mut bytes, end, cd + 1);
    assert_eq!(
        zip::read(&bytes, &Limits::default(), &Cancel::default()),
        Err(Refusal::MalformedContainer)
    );
}

#[test]
fn review_local_header_must_match_central_method() {
    let mut bytes = zip::write(&[review_entry("a.xml", b"<a/>", false)]);
    bytes[8..10].copy_from_slice(&99u16.to_le_bytes());
    assert_eq!(
        zip::read(&bytes, &Limits::default(), &Cancel::default()),
        Err(Refusal::MalformedContainer)
    );
}

#[test]
fn review_local_encryption_flag_is_refused() {
    let mut bytes = zip::write(&[review_entry("a.xml", b"<a/>", false)]);
    bytes[6..8].copy_from_slice(&1u16.to_le_bytes());
    assert_eq!(
        zip::read(&bytes, &Limits::default(), &Cancel::default()),
        Err(Refusal::Encrypted)
    );
}

#[test]
fn review_local_sizes_must_match_central() {
    let mut bytes = zip::write(&[review_entry("a.xml", b"<a/>", false)]);
    bytes[22..26].copy_from_slice(&3u32.to_le_bytes());
    assert_eq!(
        zip::read(&bytes, &Limits::default(), &Cancel::default()),
        Err(Refusal::MalformedContainer)
    );
}

#[test]
fn review_xml_invalid_characters_refuse() {
    let types = b"<Types><Default Extension='xml' ContentType='application/xml'/>\0</Types>";
    let opened = opc::open(
        &review_package(types, "a.xml"),
        &Limits::default(),
        &Cancel::default(),
    );
    assert_eq!(opened, Err(Refusal::MalformedXml));
    for text in [
        "<a>&#0;</a>",
        "<a b='&#1;'/>",
        "<a>\u{FFFE}</a>",
        "<a><!-- x -- y --></a>",
        "<a>]]></a>",
        "<a b='<'/>",
    ] {
        assert_eq!(
            xml::parse(text.as_bytes(), &Limits::default(), &Cancel::default()),
            Err(Refusal::MalformedXml),
            "{text:?}"
        );
    }
}

#[test]
fn review_xml_invalid_names_refuse() {
    assert_eq!(
        xml::parse(b"<1/>", &Limits::default(), &Cancel::default()),
        Err(Refusal::MalformedXml)
    );
    for text in ["<a 1b='x'/>", "<:a/>", "<a:/>", "<a:b:c/>", "<\u{B7}a/>"] {
        assert_eq!(
            xml::parse(text.as_bytes(), &Limits::default(), &Cancel::default()),
            Err(Refusal::MalformedXml),
            "{text:?}"
        );
    }
    let accepted = "<w:p xmlns:w='urn:w' a.b-c='1' _x='2'>\u{e9}\u{B7}</w:p>";
    assert!(xml::parse(accepted.as_bytes(), &Limits::default(), &Cancel::default()).is_ok());
}

#[test]
fn review_attribute_keys_keep_their_prefix() {
    let events = xml::parse(
        b"<p:sldId xmlns:p='urn:p' xmlns:r='urn:r' id='256' r:id='rId1'/>",
        &Limits::default(),
        &Cancel::default(),
    )
    .unwrap();
    let xml::Event::Open { name, attributes } = &events[0] else {
        panic!("open event")
    };
    assert_eq!(name, "sldId");
    assert!(
        attributes.contains(&("id".to_owned(), "256".to_owned())),
        "{attributes:?}"
    );
    assert!(
        attributes.contains(&("r:id".to_owned(), "rId1".to_owned())),
        "{attributes:?}"
    );
}

// ---- Static audit: importers have no network or process access. ----

#[test]
fn importers_have_no_network_or_process_access() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let forbidden = [
        "std::net",
        "TcpStream",
        "UdpSocket",
        "std::process",
        "Command::new",
        "reqwest::",
        "hyper::",
        "ureq::",
        "curl::",
    ];
    for entry in std::fs::read_dir(crate_root.join("src")).unwrap() {
        let path = entry.unwrap().path();
        let source = std::fs::read_to_string(&path).unwrap();
        for token in forbidden {
            assert!(!source.contains(token), "{} uses {token}", path.display());
        }
    }
    let manifest = std::fs::read_to_string(crate_root.join("Cargo.toml")).unwrap();
    let dependencies: Vec<&str> = manifest
        .split("[dependencies]")
        .nth(1)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once(" = ").map(|(name, _)| name.trim()))
        .collect();
    assert_eq!(
        dependencies,
        ["bran-core", "crc32fast", "flate2", "quick-xml"],
        "a new dependency needs the dependency review updated first"
    );
}
