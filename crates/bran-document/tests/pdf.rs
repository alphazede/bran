//! PDF adapter corpus (issue #23).
//!
//! Every row is a synthetic PDF built in memory from the reviewable
//! `pdf-base.objects` fixture, including the hostile ones. The rows run
//! through the shared harness (`conformance::check`) against the adapter
//! registered for PDF, so the #25 guarantees apply unchanged.

use bran_document::canonical::{sha256_hex, Json};
use bran_document::conformance::{self, Adapter, Expect, Imported};
use bran_document::pdf::{self, Options};
use bran_document::{export, Cancel, Format, Limits, Refusal};
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// Recorded budgets. Runtime budgets are debug-build wall-clock ceilings for
// the whole tier; they gate the test suite, never an import outcome.
const FAST_RUNTIME_BUDGET: Duration = Duration::from_secs(10);
const FULL_RUNTIME_BUDGET: Duration = Duration::from_secs(120);
const MAX_FIXTURE_FILE_BYTES: u64 = 8 * 1024;
const MAX_FAST_INPUT_BYTES: usize = 2 * 1024 * 1024;
const FAST_MUTATIONS: u32 = 500;
const FULL_MUTATIONS: u32 = 20_000;
const MAX_FULL_INPUT_BYTES: usize = 24 * 1024 * 1024;

const FIXTURE: &str = "fixtures/enterprise-documents/conformance/pdf-base.objects";
const BASE: &str =
    include_str!("../../../fixtures/enterprise-documents/conformance/pdf-base.objects");
const PDF_DOC: &str = include_str!("../../../docs/enterprise-document-pdf-adapter.md");
const CANARIES: &str =
    include_str!("../../../fixtures/public-boundary/rejected/synthetic-canaries.txt");

/// Canonical projection digest of the ordinary row. A change here is a change
/// to the PDF projection and must be reviewed as one.
const ORDINARY_DIGEST: &str = "473eed3ab9ff7bd734ee8e7a017f5709e061394bc785947015e5e54052f2e948";

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

const ROWS: &[&str] = &[
    "pdf-ordinary-projection",
    "pdf-round-trip-anchors",
    "pdf-damaged-xref",
    "pdf-stream-length-mismatch",
    "pdf-malformed-object-graph",
    "pdf-recursive-structure",
    "pdf-dangling-reference",
    "pdf-deep-nesting",
    "pdf-excessive-objects",
    "pdf-too-many-pages",
    "pdf-active-action",
    "pdf-launch-action",
    "pdf-xfa-form",
    "pdf-remote-goto",
    "pdf-uri-link",
    "pdf-embedded-file",
    "pdf-encrypted",
    "pdf-signed",
    "pdf-form",
    "pdf-dlp-canary",
    "pdf-public-boundary-marker",
    "pdf-oversized-stream",
    "pdf-oversized-input",
    "pdf-scanned-page",
    "pdf-unmapped-glyphs",
    "pdf-not-a-pdf",
    "pdf-cancelled",
];

fn expect(row: &str) -> Expect {
    use Refusal::*;
    match row {
        "pdf-ordinary-projection" | "pdf-round-trip-anchors" | "pdf-form" => Expect::Admit(vec![]),
        "pdf-damaged-xref" => Expect::Admit(vec!["xref-repaired"]),
        "pdf-stream-length-mismatch" => Expect::Admit(vec!["stream-length-repaired"]),
        "pdf-dangling-reference" => Expect::Admit(vec!["dangling-reference"]),
        "pdf-uri-link" => Expect::Admit(vec!["hyperlink-not-fetched"]),
        "pdf-embedded-file" => Expect::Admit(vec!["embedded-file-not-extracted"]),
        "pdf-signed" => Expect::Admit(vec!["signature-not-verified"]),
        "pdf-dlp-canary" => Expect::Admit(vec!["dlp-findings"]),
        "pdf-public-boundary-marker" => Expect::Admit(vec!["public-boundary-violation"]),
        "pdf-scanned-page" => Expect::Admit(vec!["ocr-not-run"]),
        "pdf-unmapped-glyphs" => Expect::Admit(vec!["text-unmapped-glyphs"]),
        "pdf-malformed-object-graph" | "pdf-recursive-structure" | "pdf-not-a-pdf" => {
            Expect::Refuse(PdfMalformed)
        }
        "pdf-deep-nesting" => Expect::Refuse(PdfDepthLimit),
        "pdf-excessive-objects" => Expect::Refuse(PdfObjectLimit),
        "pdf-too-many-pages" => Expect::Refuse(PdfPageLimit),
        "pdf-active-action" | "pdf-launch-action" | "pdf-xfa-form" => Expect::Refuse(ActiveContent),
        "pdf-remote-goto" => Expect::Refuse(ExternalReference),
        "pdf-encrypted" => Expect::Refuse(Encrypted),
        "pdf-oversized-stream" => Expect::Refuse(DecompressionLimit),
        "pdf-oversized-input" => Expect::Refuse(Oversized),
        "pdf-cancelled" => Expect::Refuse(Cancelled),
        other => panic!("row without expectation: {other}"),
    }
}

// ---- Synthetic PDF builder over the `.objects` fixture. ----

#[derive(Clone)]
enum Body {
    Plain(String),
    Stream { dict: String, data: Vec<u8> },
}

/// Object bodies plus extra trailer entries. The builder writes the header,
/// object framing, stream lengths, cross-reference data, and trailer.
#[derive(Clone)]
struct Objects {
    trailer: String,
    objects: Vec<(u32, Body)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Layout {
    /// Objects in fixture order with a classic cross-reference table.
    Classic,
    /// Objects in reverse order, another producer and date, and a file ID.
    Reordered,
    /// Non-stream objects in an object stream, Flate streams, and a
    /// predictor-encoded cross-reference stream.
    Packed,
    /// A classic file plus an incremental update that rewrites the
    /// producer entry.
    Incremental,
}

impl Objects {
    fn parse(text: &str) -> Self {
        let mut trailer = String::new();
        let mut objects: Vec<(u32, Body)> = Vec::new();
        let mut lines = text.lines();
        while let Some(line) = lines.next() {
            if let Some(header) = line.strip_prefix("--- ") {
                if header == "trailer" {
                    trailer = lines.next().expect("trailer entries").to_owned();
                    continue;
                }
                let (number, kind) = header.split_once(' ').unwrap_or((header, ""));
                let number: u32 = number.parse().expect("object number");
                let body = if kind == "stream" {
                    let dict = lines.next().expect("stream dictionary").to_owned();
                    Body::Stream {
                        dict,
                        data: Vec::new(),
                    }
                } else {
                    Body::Plain(String::new())
                };
                objects.push((number, body));
                continue;
            }
            match &mut objects.last_mut().expect("object header first").1 {
                Body::Plain(text) => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(line);
                }
                Body::Stream { data, .. } => {
                    if !data.is_empty() {
                        data.push(b'\n');
                    }
                    data.extend_from_slice(line.as_bytes());
                }
            }
        }
        Self { trailer, objects }
    }

    fn body(&mut self, number: u32) -> &mut Body {
        &mut self
            .objects
            .iter_mut()
            .find(|(n, _)| *n == number)
            .unwrap_or_else(|| panic!("object {number}"))
            .1
    }

    fn edit(mut self, number: u32, from: &str, to: &str) -> Self {
        let replace = |text: &str| {
            assert!(
                text.contains(from),
                "edit anchor missing in {number}: {from}"
            );
            text.replacen(from, to, 1)
        };
        match self.body(number) {
            Body::Plain(text) => *text = replace(text),
            Body::Stream { dict, data } => {
                if dict.contains(from) {
                    *dict = replace(dict);
                } else {
                    *data = replace(&String::from_utf8(data.clone()).unwrap()).into_bytes();
                }
            }
        }
        self
    }

    fn add(mut self, number: u32, text: &str) -> Self {
        self.objects.push((number, Body::Plain(text.to_owned())));
        self
    }

    fn add_stream(mut self, number: u32, dict: &str, data: impl Into<Vec<u8>>) -> Self {
        self.objects.push((
            number,
            Body::Stream {
                dict: dict.to_owned(),
                data: data.into(),
            },
        ));
        self
    }

    fn trailer(mut self, extra: &str) -> Self {
        self.trailer.push(' ');
        self.trailer.push_str(extra);
        self
    }

    fn next_number(&self) -> u32 {
        self.objects.iter().map(|(n, _)| *n).max().unwrap_or(0) + 1
    }

    fn pdf(&self) -> Vec<u8> {
        self.pdf_with(Layout::Classic)
    }

    fn pdf_with(&self, layout: Layout) -> Vec<u8> {
        let mut this = self.clone();
        if layout == Layout::Reordered {
            if this.objects.iter().any(|(n, _)| *n == 18) {
                this = this
                    .edit(18, "Synthetic producer one", "Another synthetic producer")
                    .edit(18, "D:20260101000000Z", "D:20270303030303Z");
            }
            this.objects.reverse();
            this.trailer.push_str(
                " /ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>]",
            );
        }
        if layout == Layout::Packed {
            return this.packed();
        }
        let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets = Vec::new();
        for (number, body) in &this.objects {
            offsets.push((*number, out.len()));
            write_object(&mut out, *number, body, false);
        }
        let size = this.next_number();
        let start = out.len();
        write_xref_table(&mut out, &offsets);
        out.extend(
            format!(
                "trailer\n<< /Size {size} {} >>\nstartxref\n{start}\n%%EOF\n",
                this.trailer
            )
            .bytes(),
        );
        if layout == Layout::Incremental && this.objects.iter().any(|(n, _)| *n == 18) {
            let info = this.clone().edit(
                18,
                "Synthetic producer one",
                "Incremental synthetic producer",
            );
            let at = out.len();
            let (_, body) = info.objects.iter().find(|(n, _)| *n == 18).unwrap();
            write_object(&mut out, 18, body, false);
            let update = out.len();
            write_xref_table(&mut out, &[(18, at)]);
            out.extend(
                format!(
                    "trailer\n<< /Size {size} {} /Prev {start} >>\nstartxref\n{update}\n%%EOF\n",
                    this.trailer
                )
                .bytes(),
            );
        }
        out
    }

    /// PDF 1.5 layout: an object stream and a cross-reference stream.
    fn packed(&self) -> Vec<u8> {
        let object_stream = self.next_number();
        let xref_stream = object_stream + 1;
        let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut entries: Vec<(u32, u8, u32, u16)> = Vec::new();
        let (mut header, mut packed) = (String::new(), Vec::new());
        let mut index = 0;
        for (number, body) in &self.objects {
            match body {
                Body::Plain(text) => {
                    header.push_str(&format!("{number} {} ", packed.len()));
                    packed.extend_from_slice(text.as_bytes());
                    packed.push(b'\n');
                    entries.push((*number, 2, object_stream, index));
                    index += 1;
                }
                Body::Stream { .. } => {
                    entries.push((*number, 1, out.len() as u32, 0));
                    write_object(&mut out, *number, body, true);
                }
            }
        }
        let first = header.len();
        let mut data = header.into_bytes();
        data.extend(packed);
        entries.push((object_stream, 1, out.len() as u32, 0));
        write_object(
            &mut out,
            object_stream,
            &Body::Stream {
                dict: format!("<< /Type /ObjStm /N {index} /First {first} >>"),
                data,
            },
            true,
        );
        let start = out.len();
        entries.push((xref_stream, 1, start as u32, 0));
        entries.sort();
        let size = xref_stream + 1;
        let mut rows = Vec::new();
        let mut previous = vec![0u8; 7];
        let mut numbered = entries.iter().peekable();
        for number in 0..size {
            let row: [u8; 7] = match numbered.peek() {
                Some((n, kind, field, generation)) if *n == number => {
                    numbered.next();
                    let mut row = [*kind, 0, 0, 0, 0, 0, 0];
                    row[1..5].copy_from_slice(&field.to_be_bytes());
                    row[5..7].copy_from_slice(&generation.to_be_bytes());
                    row
                }
                _ => [0, 0, 0, 0, 0, 0xFF, 0xFF],
            };
            // PNG "Up" predictor, as producers write cross-reference streams.
            rows.push(2u8);
            rows.extend(row.iter().zip(&previous).map(|(a, b)| a.wrapping_sub(*b)));
            previous = row.to_vec();
        }
        write_object(
            &mut out,
            xref_stream,
            &Body::Stream {
                dict: format!(
                    "<< /Type /XRef /Size {size} /W [1 4 2] /DecodeParms << /Predictor 12 /Columns 7 >> {} >>",
                    self.trailer
                ),
                data: rows,
            },
            true,
        );
        out.extend(format!("startxref\n{start}\n%%EOF\n").bytes());
        out
    }
}

fn write_object(out: &mut Vec<u8>, number: u32, body: &Body, compress: bool) {
    out.extend(format!("{number} 0 obj\n").bytes());
    match body {
        Body::Plain(text) => out.extend_from_slice(text.as_bytes()),
        Body::Stream { dict, data } => {
            let (dict, data) = if compress && !dict.contains("/Filter") {
                (
                    dict.replacen("<<", "<< /Filter /FlateDecode", 1),
                    zlib(data),
                )
            } else {
                (dict.clone(), data.clone())
            };
            let dict = dict.replacen("<<", &format!("<< /Length {}", data.len()), 1);
            out.extend(format!("{dict}\nstream\n").bytes());
            out.extend(&data);
            out.extend(b"\nendstream");
        }
    }
    out.extend(b"\nendobj\n");
}

fn write_xref_table(out: &mut Vec<u8>, offsets: &[(u32, usize)]) {
    let mut sorted = offsets.to_vec();
    sorted.sort();
    out.extend(b"xref\n0 1\n0000000000 65535 f \n");
    for (number, offset) in sorted {
        out.extend(format!("{number} 1\n{offset:010} 00000 n \n").bytes());
    }
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn canary() -> &'static str {
    CANARIES
        .lines()
        .find(|line| !line.is_empty())
        .expect("synthetic canary")
}

fn base() -> Objects {
    Objects::parse(BASE)
}

fn build(row: &str, tier: Tier, limits: &Limits) -> Vec<u8> {
    build_with(row, tier, limits, Layout::Classic)
}

/// Rows whose hostile bytes are patched after serialisation have no
/// logically equal re-encodings.
fn patched(row: &str) -> bool {
    matches!(row, "pdf-damaged-xref" | "pdf-stream-length-mismatch")
}

fn build_with(row: &str, tier: Tier, limits: &Limits, layout: Layout) -> Vec<u8> {
    let base = base();
    let next = base.next_number();
    let objects = match row {
        "pdf-ordinary-projection" | "pdf-round-trip-anchors" | "pdf-cancelled" => base,
        "pdf-damaged-xref" => {
            // Every recorded offset points three bytes early.
            let bytes = base.pdf();
            let at = bytes.windows(6).rposition(|window| window == b"\nxref\n").unwrap();
            let tail = String::from_utf8(bytes[at..].to_vec()).unwrap();
            let shifted: Vec<String> = tail
                .split('\n')
                .map(|line| match line.strip_suffix(" 00000 n ") {
                    Some(offset) => format!("{:010} 00000 n ", offset.parse::<usize>().unwrap() - 3),
                    None => line.to_owned(),
                })
                .collect();
            let mut out = bytes[..at].to_vec();
            out.extend(shifted.join("\n").bytes());
            return out;
        }
        "pdf-stream-length-mismatch" => {
            // Object 7 declares 12 bytes; the padding keeps every offset valid.
            let bytes = base.pdf();
            let key = b"\n7 0 obj\n<< /Length ";
            let at = bytes.windows(key.len()).position(|window| window == key).unwrap() + key.len();
            let end = at + bytes[at..].iter().position(|byte| *byte == b' ').unwrap();
            let mut out = bytes[..at].to_vec();
            out.extend(format!("{:<width$}", 12, width = end - at).bytes());
            out.extend(&bytes[end..]);
            return out;
        }
        "pdf-malformed-object-graph" => base.edit(2, "[3 0 R 4 0 R]", "[3 0 R 5 0 R]"),
        "pdf-recursive-structure" => base.edit(2, "[3 0 R 4 0 R]", "[3 0 R 2 0 R]"),
        "pdf-dangling-reference" => base.edit(3, "/Contents 7 0 R", "/Contents 99 0 R"),
        "pdf-deep-nesting" => {
            let depth = if tier == Tier::Full {
                1_000_000
            } else {
                limits.max_xml_depth + 1
            };
            let nested = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
            base.edit(3, "/Type /Page ", &format!("/Type /Page /Nested {nested} "))
        }
        "pdf-excessive-objects" => {
            let many = "0 ".repeat(limits.max_xml_nodes + 1);
            base.edit(3, "/Type /Page ", &format!("/Type /Page /Many [{many}] "))
        }
        "pdf-too-many-pages" => {
            let mut objects = base;
            let mut kids = String::new();
            for index in 0..limits.max_parts as u32 {
                let number = next + index;
                objects = objects.add(
                    number,
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>",
                );
                kids.push_str(&format!(" {number} 0 R"));
            }
            let count = limits.max_parts + 2;
            objects
                .edit(2, "4 0 R]", &format!("4 0 R{kids}]"))
                .edit(2, "/Count 2", &format!("/Count {count}"))
        }
        "pdf-active-action" => base.edit(
            1,
            "/Type /Catalog",
            "/Type /Catalog /OpenAction << /S /JavaScript /JS (app.alert\\(1\\)) >>",
        ),
        "pdf-launch-action" => base.edit(
            13,
            "/Dest [4 0 R /Fit]",
            "/A << /S /Launch /F (synthetic.exe) >>",
        ),
        "pdf-xfa-form" => base
            .edit(1, "/Type /Catalog", &format!("/Type /Catalog /AcroForm {next} 0 R"))
            .add(next, &format!("<< /Fields [] /XFA {} 0 R >>", next + 1))
            .add_stream(next + 1, "<< >>", "<xdp:xdp/>"),
        "pdf-remote-goto" => base.edit(
            13,
            "/Dest [4 0 R /Fit]",
            "/A << /S /GoToR /F (other.pdf) /D [0 /Fit] >>",
        ),
        "pdf-uri-link" => base.edit(
            13,
            "/Dest [4 0 R /Fit]",
            "/A << /S /URI /URI (https://example.invalid/page) >>",
        ),
        "pdf-embedded-file" => base
            .edit(
                1,
                "/Type /Catalog",
                &format!("/Type /Catalog /Names << /EmbeddedFiles << /Names [(synthetic.txt) {next} 0 R] >> >>"),
            )
            .add(
                next,
                &format!("<< /Type /Filespec /F (synthetic.txt) /UF (synthetic.txt) /EF << /F {} 0 R >> >>", next + 1),
            )
            .add_stream(
                next + 1,
                "<< /Type /EmbeddedFile /Params << /Size 21 >> >>",
                "synthetic inert bytes",
            ),
        "pdf-encrypted" => base.trailer(
            "/Encrypt << /Filter /Standard /V 2 /R 3 /Length 128 /P -3904 /O <00112233445566778899AABBCCDDEEFF00112233445566778899AABBCCDDEEFF> /U <00112233445566778899AABBCCDDEEFF00112233445566778899AABBCCDDEEFF> >> /ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>]",
        ),
        "pdf-signed" => base
            .edit(1, "/Type /Catalog", &format!("/Type /Catalog /AcroForm << /Fields [{next} 0 R] /SigFlags 3 >>"))
            .add(next, &format!("<< /FT /Sig /T (Synthetic signature) /V {} 0 R >>", next + 1))
            .add(
                next + 1,
                "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 100 200 300] /Contents <3082000A0102030405060708> >>",
            ),
        "pdf-form" => base
            .edit(1, "/Type /Catalog", &format!("/Type /Catalog /AcroForm << /Fields [{next} 0 R] >>"))
            .add(next, &format!("<< /T (applicant) /Kids [{} 0 R] >>", next + 1))
            .add(next + 1, &format!("<< /FT /Tx /T (name) /Parent {next} 0 R /V (Synthetic value) >>")),
        "pdf-dlp-canary" => base.edit(7, "(Synthetic PDF heading)", &format!("({})", canary())),
        "pdf-public-boundary-marker" => base.edit(7, "(Synthetic PDF heading)", "(important_boundary)"),
        "pdf-oversized-stream" => {
            let bomb = zlib(&vec![b' '; limits.max_part_bytes as usize * 2]);
            let mut objects = base;
            *objects.body(8) = Body::Stream {
                dict: "<< /Filter /FlateDecode >>".to_owned(),
                data: bomb,
            };
            objects
        }
        "pdf-oversized-input" => base.add_stream(next, "<< >>", noise(limits.max_package_bytes as usize + 1)),
        "pdf-scanned-page" => base.edit(
            8,
            "BT /F2 12 Tf 72 500 Td <00010002000300040008000500060007> Tj ET\n",
            "",
        ),
        "pdf-unmapped-glyphs" => base.edit(6, " /ToUnicode 16 0 R", ""),
        "pdf-not-a-pdf" => return b"Synthetic plain text that is not a PDF file.".to_vec(),
        other => panic!("row without builder: {other}"),
    };
    objects.pdf_with(layout)
}

/// Incompressible deterministic bytes (xorshift).
fn noise(len: usize) -> Vec<u8> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn variants(row: &str) -> Vec<Vec<u8>> {
    if !matches!(expect(row), Expect::Admit(_)) || patched(row) {
        return Vec::new();
    }
    let tier_limits = limits(Tier::Fast);
    [Layout::Reordered, Layout::Packed, Layout::Incremental]
        .into_iter()
        .map(|layout| build_with(row, Tier::Fast, &tier_limits, layout))
        .collect()
}

fn adapter() -> &'static dyn Adapter {
    let registered: Vec<_> = conformance::registered()
        .into_iter()
        .filter(|adapter| adapter.format() == Format::Pdf)
        .collect();
    assert_eq!(registered.len(), 1, "exactly one PDF adapter registers");
    registered[0]
}

fn import(bytes: &[u8]) -> Imported {
    pdf::import(
        bytes,
        &limits(Tier::Fast),
        &Cancel::default(),
        Options::default(),
    )
    .expect("row imports")
}

fn projection(row: &str) -> Json {
    let limits = limits(Tier::Fast);
    Json::parse(&import(&build(row, Tier::Fast, &limits)).canonical).expect("canonical JSON")
}

fn at<'a>(value: &'a Json, path: &str) -> &'a Json {
    path.split('.').fold(value, |value, key| match value {
        Json::Obj(map) => map
            .get(key)
            .unwrap_or_else(|| panic!("missing {key} in {path}")),
        Json::Arr(items) => &items[key.parse::<usize>().expect("array index")],
        _ => panic!("{path}: {key} is not inside an object or array"),
    })
}

fn text(value: &str) -> Json {
    Json::Str(value.to_owned())
}

fn ints(values: &[i64]) -> Json {
    Json::Arr(values.iter().map(|value| Json::Int(*value)).collect())
}

fn run_rows(tier: Tier) -> usize {
    let limits = limits(tier);
    let max_input = if tier == Tier::Fast {
        MAX_FAST_INPUT_BYTES
    } else {
        MAX_FULL_INPUT_BYTES
    };
    let adapter = adapter();
    let mut checked = 0;
    for row in ROWS {
        let input = build(row, tier, &limits);
        assert!(input.len() <= max_input, "{row}: input exceeds size budget");
        let variants = if tier == Tier::Fast {
            variants(row)
        } else {
            Vec::new()
        };
        let cancel = Cancel::default();
        if *row == "pdf-cancelled" {
            cancel.cancel();
        }
        let outcome =
            conformance::check(adapter, &input, &variants, &expect(row), &limits, &cancel)
                .unwrap_or_else(|error| panic!("{row}: {error}"));
        if *row == "pdf-round-trip-anchors" {
            assert!(outcome.round_trip, "ordinary PDF must round-trip");
        }
        checked += 1;
    }
    checked
}

/// Parser-limit property: seeded mutations of the ordinary row (classic and
/// packed layouts) never panic on import or export, and an admitted result
/// is canonical JSON with one anchor per block.
fn mutation_property(iterations: u32) {
    let limits = limits(Tier::Fast);
    let adapter = adapter();
    let bases = [
        build("pdf-ordinary-projection", Tier::Fast, &limits),
        build_with(
            "pdf-ordinary-projection",
            Tier::Fast,
            &limits,
            Layout::Packed,
        ),
    ];
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % bound.max(1) as u64) as usize
    };
    for iteration in 0..iterations {
        let mut bytes = bases[iteration as usize % 2].clone();
        let position = next(bytes.len());
        match next(3) {
            0 => bytes[position] ^= 1 << next(8),
            1 => bytes.truncate(position),
            _ => {
                let insert: Vec<u8> = (0..1 + next(8)).map(|_| next(256) as u8).collect();
                bytes.splice(position..position, insert);
            }
        }
        let imported = catch_unwind(AssertUnwindSafe(|| {
            adapter.import(&bytes, &limits, &Cancel::default())
        }))
        .unwrap_or_else(|_| panic!("import panicked on mutation {iteration}"));
        if let Ok(imported) = imported {
            let doc = Json::parse(&imported.canonical).expect("canonical JSON");
            let blocks: usize = match at(&doc, "pages") {
                Json::Arr(pages) => pages
                    .iter()
                    .map(|page| match at(page, "blocks") {
                        Json::Arr(blocks) => blocks.len(),
                        _ => 0,
                    })
                    .sum(),
                _ => 0,
            };
            assert_eq!(blocks, imported.anchors.len(), "mutation {iteration}");
            let _ = catch_unwind(AssertUnwindSafe(|| adapter.export(&imported)))
                .unwrap_or_else(|_| panic!("export panicked on mutation {iteration}"));
        }
    }
}

#[test]
fn pdf_conformance_fast() {
    let started = Instant::now();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(FIXTURE);
    assert!(std::fs::metadata(fixture).unwrap().len() <= MAX_FIXTURE_FILE_BYTES);
    let checked = run_rows(Tier::Fast);
    mutation_property(FAST_MUTATIONS);
    let limits = limits(Tier::Fast);
    let ordinary = import(&build("pdf-ordinary-projection", Tier::Fast, &limits));
    assert_eq!(
        sha256_hex(&ordinary.canonical),
        ORDINARY_DIGEST,
        "PDF projection changed"
    );
    let elapsed = started.elapsed();
    println!("pdf fast tier: {checked} rows, {elapsed:?} of {FAST_RUNTIME_BUDGET:?} budget");
    assert!(
        elapsed <= FAST_RUNTIME_BUDGET,
        "fast tier exceeded its budget"
    );
}

#[test]
#[ignore = "full tier: run by tools/ci/check.sh --full (security gate)"]
fn pdf_conformance_full() {
    let started = Instant::now();
    let checked = run_rows(Tier::Full);
    mutation_property(FULL_MUTATIONS);
    let elapsed = started.elapsed();
    println!("pdf full tier: {checked} rows, {elapsed:?} of {FULL_RUNTIME_BUDGET:?} budget");
    assert!(
        elapsed <= FULL_RUNTIME_BUDGET,
        "full tier exceeded its budget"
    );
}

#[test]
fn pdf_rows_are_documented() {
    for row in ROWS {
        assert!(
            PDF_DOC.contains(&format!("`{row}`")),
            "row {row} missing from the PDF adapter document"
        );
    }
}

#[test]
fn ordinary_projection_has_stable_locators() {
    let doc = projection("pdf-ordinary-projection");
    assert_eq!(at(&doc, "source.media_type"), &text("application/pdf"));
    assert_eq!(at(&doc, "language"), &text("en-US"));
    assert_eq!(at(&doc, "metadata.title"), &text("Synthetic PDF fixture"));
    let Json::Obj(metadata) = at(&doc, "metadata") else {
        panic!()
    };
    assert!(!metadata.contains_key("producer") && !metadata.contains_key("creation_date"));
    assert_eq!(
        at(&doc, "ocr"),
        &Json::parse(br#"{"effective":"not-run","requested":false}"#).unwrap()
    );

    let heading = at(&doc, "pages.0.blocks.0");
    assert_eq!(at(heading, "id"), &text("pdf:p1:b1"));
    assert_eq!(at(heading, "role"), &text("heading"));
    assert_eq!(at(heading, "text"), &text("Synthetic PDF heading"));
    assert_eq!(
        at(heading, "text_digest"),
        &text(&sha256_hex(b"Synthetic PDF heading"))
    );
    assert_eq!(at(heading, "derivation"), &text("embedded-text"));
    assert_eq!(at(heading, "bbox"), &ints(&[72, 716, 261, 734]));
    let paragraph = at(&doc, "pages.0.blocks.1");
    assert_eq!(at(paragraph, "id"), &text("pdf:p1:b2"));
    assert_eq!(at(paragraph, "role"), &text("paragraph"));
    assert_eq!(
        at(paragraph, "text"),
        &text("First line of the synthetic paragraph.\nSecond line with a gap.")
    );
    assert_eq!(at(paragraph, "bbox"), &ints(&[72, 674, 281, 699]));
    assert_eq!(
        at(&doc, "pages.0.reading_order"),
        &Json::parse(br#"{"confidence":"low","provenance":"content-stream-order"}"#).unwrap()
    );
    assert_eq!(at(&doc, "pages.0.annotations.0.id"), &text("pdf:p1:a1"));
    assert_eq!(at(&doc, "pages.0.annotations.0.subtype"), &text("Text"));
    assert_eq!(
        at(&doc, "pages.0.annotations.0.contents"),
        &text("Synthetic reviewer note")
    );
    assert_eq!(at(&doc, "pages.0.annotations.1.subtype"), &text("Link"));
    assert_eq!(at(&doc, "pages.0.annotations.1.target_page"), &Json::Int(2));

    let second = at(&doc, "pages.1");
    assert_eq!(at(second, "page"), &Json::Int(2));
    assert_eq!(at(second, "box"), &ints(&[0, 0, 595, 842]));
    assert_eq!(at(second, "rotate"), &Json::Int(90));
    assert_eq!(at(second, "blocks.0.id"), &text("pdf:p2:b1"));
    assert_eq!(at(second, "blocks.0.text"), &text("Grüße abc"));
    assert_eq!(at(second, "blocks.0.bbox"), &ints(&[72, 498, 128, 510]));
    assert_eq!(at(second, "images.0.id"), &text("pdf:p2:i1"));
    assert_eq!(at(second, "images.0.bbox"), &ints(&[72, 400, 172, 450]));
    assert_eq!(at(second, "text_source"), &text("embedded-text"));

    assert_eq!(at(&doc, "outline.0.title"), &text("Synthetic heading"));
    assert_eq!(at(&doc, "outline.0.page"), &Json::Int(1));
    assert_eq!(at(&doc, "outline.1.page"), &Json::Int(2));

    let limits = limits(Tier::Fast);
    let imported = import(&build("pdf-ordinary-projection", Tier::Fast, &limits));
    let ids: Vec<&str> = imported
        .anchors
        .iter()
        .map(|anchor| anchor.id.as_str())
        .collect();
    assert_eq!(ids, ["pdf:p1:b1", "pdf:p1:b2", "pdf:p2:b1"]);
}

#[test]
fn pdf_structures_are_recorded_without_trust_or_extraction() {
    let signed = projection("pdf-signed");
    let signature = at(&signed, "signatures.0");
    assert_eq!(at(signature, "field"), &text("Synthetic signature"));
    assert_eq!(at(signature, "sub_filter"), &text("adbe.pkcs7.detached"));
    assert_eq!(at(signature, "byte_range"), &ints(&[0, 100, 200, 300]));
    assert_eq!(
        at(signature, "contents_sha256"),
        &text(&sha256_hex(&[
            0x30, 0x82, 0x00, 0x0A, 1, 2, 3, 4, 5, 6, 7, 8
        ]))
    );
    assert_eq!(at(signature, "trust"), &text("not-verified"));

    let form = projection("pdf-form");
    assert_eq!(at(&form, "forms.0.name"), &text("applicant.name"));
    assert_eq!(at(&form, "forms.0.type"), &text("Tx"));
    assert_eq!(at(&form, "forms.0.value"), &text("Synthetic value"));

    let attached = projection("pdf-embedded-file");
    assert_eq!(at(&attached, "attachments.0.name"), &text("synthetic.txt"));
    assert_eq!(at(&attached, "attachments.0.declared_size"), &Json::Int(21));
    assert_eq!(at(&attached, "attachments.0.extracted"), &Json::Bool(false));

    let linked = projection("pdf-uri-link");
    assert_eq!(
        at(&linked, "pages.0.annotations.1.uri"),
        &text("https://example.invalid/page")
    );
    assert_eq!(
        at(&linked, "pages.0.annotations.1.fetched"),
        &Json::Bool(false)
    );
}

#[test]
fn ocr_is_opt_in_and_never_faked() {
    let limits = limits(Tier::Fast);
    let scanned = build("pdf-scanned-page", Tier::Fast, &limits);
    let doc = Json::parse(&import(&scanned).canonical).unwrap();
    assert_eq!(at(&doc, "pages.1.text_source"), &text("none"));
    assert_eq!(at(&doc, "pages.1.blocks"), &Json::Arr(Vec::new()));
    assert_eq!(at(&doc, "fidelity.ocr"), &text("unsupported"));
    let requested =
        pdf::import(&scanned, &limits, &Cancel::default(), Options { ocr: true }).unwrap();
    assert!(requested
        .receipt
        .iter()
        .any(|code| code == "ocr-unavailable"));
    let doc = Json::parse(&requested.canonical).unwrap();
    assert_eq!(at(&doc, "ocr.requested"), &Json::Bool(true));
    assert_eq!(at(&doc, "ocr.effective"), &text("unavailable"));
    // No OCR engine ran, so every citation is still byte-derived text.
    assert_eq!(
        at(&doc, "pages.0.blocks.0.derivation"),
        &text("embedded-text")
    );
}

#[test]
fn export_is_tagged_inert_and_carries_a_fidelity_receipt() {
    let limits = limits(Tier::Fast);
    let source = import(&build("pdf-uri-link", Tier::Fast, &limits));
    let exported = pdf::export(&source).expect("export");
    let body = String::from_utf8_lossy(&exported.bytes);
    for required in [
        "/StructTreeRoot",
        "/MarkInfo << /Marked true >>",
        "/ParentTree",
        "/S /H1",
        "/S /P",
        "/MCID 0",
        "/Lang (en-US)",
        "/DisplayDocTitle true",
    ] {
        assert!(body.contains(required), "export lacks {required}");
    }
    for forbidden in [
        "/JS",
        "/JavaScript",
        "/Launch",
        "/OpenAction",
        "/AA",
        "/URI",
        "/EmbeddedFile",
        "/Annots",
        "/AcroForm",
    ] {
        assert!(!body.contains(forbidden), "export carries {forbidden}");
    }
    let receipt = Json::parse(&exported.receipt).unwrap();
    assert_eq!(at(&receipt, "tagged"), &Json::Bool(true));
    assert_eq!(at(&receipt, "claims.pdf_a"), &text("not-claimed"));
    assert_eq!(at(&receipt, "claims.pdf_ua"), &text("not-claimed"));
    assert_eq!(at(&receipt, "blocks.0.derivation"), &text("embedded-text"));
    assert_eq!(
        at(&receipt, "removed"),
        &Json::Arr(vec![text("annotations"), text("images"), text("outline")])
    );

    let again = pdf::import(
        &exported.bytes,
        &limits,
        &Cancel::default(),
        Options::default(),
    )
    .unwrap();
    assert_eq!(again.anchors, source.anchors);
    let doc = Json::parse(&again.canonical).unwrap();
    assert_eq!(at(&doc, "tagged"), &Json::Bool(true));
    assert_eq!(at(&doc, "pages.0.blocks.0.role"), &text("heading"));
    assert_eq!(at(&doc, "pages.1.rotate"), &Json::Int(90));
    assert_eq!(pdf::export(&source).unwrap().bytes, exported.bytes);
}

#[test]
fn export_runs_dlp_first_and_never_overwrites() {
    let limits = limits(Tier::Fast);
    for (row, refusal) in [
        ("pdf-dlp-canary", Refusal::DlpFindings),
        ("pdf-public-boundary-marker", Refusal::PublicBoundary),
    ] {
        let imported = import(&build(row, Tier::Fast, &limits));
        assert_eq!(pdf::export(&imported).err(), Some(refusal), "{row}");
    }
    let root = export_root();
    let exported = pdf::export(&import(&build(
        "pdf-ordinary-projection",
        Tier::Fast,
        &limits,
    )))
    .unwrap();
    let emitted: Vec<&str> = exported.emitted_text.iter().map(String::as_str).collect();
    let written =
        export::write_new(&root, "out.pdf", Format::Pdf, &exported.bytes, &emitted).unwrap();
    assert_eq!(std::fs::read(&written).unwrap(), exported.bytes);
    assert_eq!(
        export::write_new(&root, "out.pdf", Format::Pdf, &exported.bytes, &emitted),
        Err(Refusal::ExportExists)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn export_refuses_text_it_cannot_encode() {
    let limits = limits(Tier::Fast);
    let objects = base().edit(16, "<0002> <0072>", "<0002> <4E2D>");
    let imported = pdf::import(
        &objects.pdf(),
        &limits,
        &Cancel::default(),
        Options::default(),
    )
    .unwrap();
    assert_eq!(
        pdf::export(&imported).err(),
        Some(Refusal::ExportUnsupported)
    );
}

fn export_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("bran-pdf-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// Opt-in: opens the ordinary fixture and its export in poppler and
/// Ghostscript when they are installed. A missing reader is reported as
/// unavailable, never as a pass.
#[test]
#[ignore = "opt-in: needs local PDF readers; run with --ignored"]
fn pdf_opens_in_independent_readers() {
    let limits = limits(Tier::Fast);
    let source = build("pdf-ordinary-projection", Tier::Fast, &limits);
    let exported = pdf::export(&import(&source)).unwrap().bytes;
    let dir = std::env::var_os("BRAN_PDF_READER_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(export_root);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, bytes) in [("fixture.pdf", &source), ("export.pdf", &exported)] {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        for (tool, args) in [
            ("pdfinfo", vec![path.as_os_str().to_owned()]),
            ("pdftotext", vec![path.as_os_str().to_owned(), "-".into()]),
            (
                "gs",
                vec![
                    "-q".into(),
                    "-dNOPAUSE".into(),
                    "-dBATCH".into(),
                    "-dSAFER".into(),
                    "-sDEVICE=nullpage".into(),
                    path.as_os_str().to_owned(),
                ],
            ),
        ] {
            match std::process::Command::new(tool).args(&args).output() {
                Err(_) => println!("unavailable {tool}: not installed"),
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    println!(
                        "{tool} {name}: exit {:?}\n{stdout}{stderr}",
                        output.status.code()
                    );
                    assert!(output.status.success(), "{tool} rejected {name}");
                    assert!(
                        !stderr.to_lowercase().contains("error"),
                        "{tool} reported an error for {name}"
                    );
                    if tool == "pdftotext" {
                        assert!(
                            stdout.contains("Synthetic PDF heading"),
                            "{tool} lost text in {name}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn rotated_text_and_actual_text_join_like_readers() {
    // Words placed one by one along a rotated baseline, and a glyph whose
    // marked-content /ActualText carries the real text, as LibreOffice and
    // cairo write them.
    let objects = base().edit(
        7,
        "BT /F1 18 Tf 72 720 Td (Synthetic PDF heading) Tj ET",
        "BT /F1 12 Tf 0 1 -1 0 300 100 Tm (Rotated) Tj 0 1 -1 0 300 150 Tm (words) Tj ET\nBT /F1 12 Tf 72 600 Td (Of) Tj /Span << /ActualText (fi) >> BDC (Z) Tj EMC (ce) Tj ET",
    );
    let limits = limits(Tier::Fast);
    let imported = pdf::import(
        &objects.pdf(),
        &limits,
        &Cancel::default(),
        Options::default(),
    )
    .unwrap();
    let doc = Json::parse(&imported.canonical).unwrap();
    assert_eq!(at(&doc, "pages.0.blocks.0.text"), &text("Rotated words"));
    assert_eq!(at(&doc, "pages.0.blocks.1.text"), &text("Office"));
}

// ---- Repair round for the #23 independent review (findings R1-R7). ----

/// R1: a destination that refers to itself, through an annotation and through
/// a bookmark, is a typed refusal, not a stack overflow.
#[test]
fn repair_r1_cyclic_destination_refused() {
    let limits = limits(Tier::Fast);
    let annotation = base()
        .edit(13, "/Dest [4 0 R /Fit]", "/Dest 19 0 R")
        .add(19, "<< /D 19 0 R >>");
    assert_eq!(
        pdf::import(
            &annotation.pdf(),
            &limits,
            &Cancel::default(),
            Options::default()
        )
        .err(),
        Some(Refusal::PdfMalformed),
    );
    let bookmark = base()
        .edit(11, "/Dest [3 0 R /XYZ 72 740 0]", "/Dest 19 0 R")
        .add(19, "<< /D 19 0 R >>");
    assert_eq!(
        pdf::import(
            &bookmark.pdf(),
            &limits,
            &Cancel::default(),
            Options::default()
        )
        .err(),
        Some(Refusal::PdfMalformed),
    );
    // A finite but over-deep destination chain trips the depth limit.
    let mut deep = base().edit(13, "/Dest [4 0 R /Fit]", "/Dest 19 0 R");
    for number in 19..19 + limits.max_xml_depth as u32 + 1 {
        deep = deep.add(number, &format!("<< /D {} 0 R >>", number + 1));
    }
    deep = deep.add(
        19 + limits.max_xml_depth as u32 + 1,
        "<< /D [4 0 R /Fit] >>",
    );
    assert_eq!(
        pdf::import(&deep.pdf(), &limits, &Cancel::default(), Options::default()).err(),
        Some(Refusal::PdfDepthLimit),
    );
}

/// R2: predictor expansion is budgeted before its buffers are allocated.
/// A 256 KiB row expansion with a 1 KiB part limit refuses even when the
/// total budget could hold it; the reviewer's exact input refuses too.
#[test]
fn repair_r2_predictor_budgets_refuse_before_allocating() {
    let mut row_limits = limits(Tier::Fast);
    row_limits.max_part_bytes = 1024;
    row_limits.max_total_bytes = 256 * 1024 * 1024;
    let mut row = base();
    *row.body(7) = Body::Stream {
        dict: "<< /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 4096 /Colors 32 /BitsPerComponent 16 >> >>"
            .to_owned(),
        data: zlib(&[0]),
    };
    assert_eq!(
        pdf::import(
            &row.pdf(),
            &row_limits,
            &Cancel::default(),
            Options::default()
        )
        .err(),
        Some(Refusal::DecompressionLimit),
    );
    let mut review_limits = limits(Tier::Fast);
    review_limits.max_part_bytes = 1024;
    review_limits.max_total_bytes = 4096;
    let mut review = base();
    *review.body(7) = Body::Stream {
        dict: "<< /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 1048576 /Colors 32 /BitsPerComponent 16 >> >>"
            .to_owned(),
        data: zlib(&[0]),
    };
    assert_eq!(
        pdf::import(
            &review.pdf(),
            &review_limits,
            &Cancel::default(),
            Options::default()
        )
        .err(),
        Some(Refusal::DecompressionLimit),
    );
}

/// R3: one code mapped to 2,048 characters over 2,048 codes refuses against
/// a 32 KiB total budget instead of admitting 4 MiB of text.
#[test]
fn repair_r3_cmap_expansion_is_budgeted() {
    let mut limits = limits(Tier::Fast);
    limits.max_total_bytes = 32 * 1024;
    let mapping = "0042".repeat(2048);
    let objects = base()
        .edit(
            5,
            "/Encoding /WinAnsiEncoding",
            "/Encoding /WinAnsiEncoding /ToUnicode 19 0 R",
        )
        .edit(
            7,
            "(Synthetic PDF heading)",
            &format!("({})", "A".repeat(2048)),
        )
        .add_stream(
            19,
            "<< >>",
            format!("1 beginbfchar\n<41> <{mapping}>\nendbfchar"),
        );
    assert_eq!(
        pdf::import(
            &objects.pdf(),
            &limits,
            &Cancel::default(),
            Options::default()
        )
        .err(),
        Some(Refusal::DecompressionLimit),
    );
}

/// R4: raw stream bytes count against the part limit even when no decoder
/// runs: a 17 MiB image under default limits, and an unused oversized
/// stream under small limits.
#[test]
fn repair_r4_raw_streams_enforce_part_limit() {
    let image = base().add_stream(
        19,
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 >>",
        vec![b'Z'; 17 * 1024 * 1024],
    );
    assert_eq!(
        pdf::import(
            &image.pdf(),
            &Limits::default(),
            &Cancel::default(),
            Options::default()
        )
        .err(),
        Some(Refusal::Oversized),
    );
    let limits = limits(Tier::Fast);
    let unused = base().add_stream(19, "<< >>", vec![b'x'; limits.max_part_bytes as usize + 1]);
    assert_eq!(
        pdf::import(
            &unused.pdf(),
            &limits,
            &Cancel::default(),
            Options::default()
        )
        .err(),
        Some(Refusal::Oversized),
    );
}

/// R5: an out-of-range `/FirstChar` falls back to default widths instead of
/// panicking; the text still extracts.
#[test]
fn repair_r5_malformed_font_bounds_do_not_panic() {
    let limits = limits(Tier::Fast);
    let objects = base().edit(
        5,
        "/Encoding /WinAnsiEncoding",
        "/Encoding /WinAnsiEncoding /FirstChar -9223372036854775808",
    );
    let imported = pdf::import(
        &objects.pdf(),
        &limits,
        &Cancel::default(),
        Options::default(),
    )
    .expect("out-of-range FirstChar imports");
    let doc = Json::parse(&imported.canonical).unwrap();
    assert_eq!(
        at(&doc, "pages.0.blocks.0.text"),
        &text("Synthetic PDF heading")
    );
}

/// R6: an overflowing signature byte range is recorded as not covering the
/// file instead of panicking.
#[test]
fn repair_r6_malformed_signature_ranges_do_not_panic() {
    let limits = limits(Tier::Fast);
    let objects = base()
        .edit(1, "/Type /Catalog", "/Type /Catalog /AcroForm << /Fields [19 0 R] /SigFlags 3 >>")
        .add(19, "<< /FT /Sig /T (Synthetic signature) /V 20 0 R >>")
        .add(
            20,
            "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 1 9223372036854775807 1] /Contents <00> >>",
        );
    let imported = pdf::import(
        &objects.pdf(),
        &limits,
        &Cancel::default(),
        Options::default(),
    )
    .expect("overflowing ByteRange imports");
    let doc = Json::parse(&imported.canonical).unwrap();
    assert_eq!(
        at(&doc, "signatures.0.covers_whole_file"),
        &Json::Bool(false)
    );
    assert!(imported
        .receipt
        .iter()
        .any(|code| code == "signature-not-verified"));
}

/// R7: an action or file-spec name stored as an indirect object classifies
/// exactly like the direct name.
#[test]
fn repair_r7_indirect_action_names_classified() {
    let limits = limits(Tier::Fast);
    for (name, refusal) in [
        ("Launch", Refusal::ActiveContent),
        ("GoToR", Refusal::ExternalReference),
    ] {
        let objects = base()
            .edit(1, "/Type /Catalog", "/Type /Catalog /OpenAction 19 0 R")
            .add(19, "<< /S 20 0 R /F (other.pdf) >>")
            .add(20, &format!("/{name}"));
        assert_eq!(
            pdf::import(
                &objects.pdf(),
                &limits,
                &Cancel::default(),
                Options::default()
            )
            .err(),
            Some(refusal),
            "{name}",
        );
    }
    // `scan` covers every object, so an unwired file specification with an
    // indirect `/FS` must refuse like the direct name.
    let remote = base()
        .add(19, "<< /FS 20 0 R /F (other.pdf) >>")
        .add(20, "/URL");
    assert_eq!(
        pdf::import(
            &remote.pdf(),
            &limits,
            &Cancel::default(),
            Options::default()
        )
        .err(),
        Some(Refusal::ExternalReference),
    );
}
