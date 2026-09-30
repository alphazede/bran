//! DOCX adapter (issue #21): import, fidelity receipts, export, and round trips.
//!
//! Every package is built in memory from the reviewable `.parts` fixtures in
//! `fixtures/enterprise-documents/docx/`. The only binary is a one-pixel PNG,
//! kept inline below so readers can decode the image part.

use bran_document::canonical::sha256_hex;
use bran_document::conformance::{Adapter, Imported, PackageIntake};
use bran_document::docx::{
    self, Block, ChangeKind, Document, Docx, Inline, Kind, Link, Merge, Paragraph, RunContent,
    Status,
};
use bran_document::zip::{self, WriteEntry};
use bran_document::{Cancel, Format, Limits, Refusal};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

const REPRESENTATIVE: &str =
    include_str!("../../../fixtures/enterprise-documents/docx/representative.parts");
const UNSUPPORTED: &str =
    include_str!("../../../fixtures/enterprise-documents/docx/unsupported-benign.parts");
const CANARIES: &str =
    include_str!("../../../fixtures/public-boundary/rejected/synthetic-canaries.txt");
const MAX_FIXTURE_FILE_BYTES: usize = 8 * 1024;
const PIXEL_PNG: &[u8] = b"\x89\x50\x4e\x47\x0d\x0a\x1a\x0a\x00\x00\x00\x0d\x49\x48\x44\x52\x00\x00\x00\x01\x00\x00\x00\x01\x08\x02\x00\x00\x00\x90\x77\x53\xde\x00\x00\x00\x0c\x49\x44\x41\x54\x78\xda\x63\xd0\xcb\x5e\x01\x00\x02\x0c\x01\x42\x16\x7d\x65\x4c\x00\x00\x00\x00\x49\x45\x4e\x44\xae\x42\x60\x82";
const MAIN: &str = "word/document.xml";
const RELS: &str = "word/_rels/document.xml.rels";
const TYPES: &str = "[Content_Types].xml";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

type Parts = Vec<(String, Vec<u8>)>;

fn parse_parts(text: &str) -> Parts {
    let mut parts: Parts = Vec::new();
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
    parts
}

fn representative() -> Parts {
    let mut parts = parse_parts(REPRESENTATIVE);
    parts.push(("word/media/pixel.png".to_owned(), PIXEL_PNG.to_vec()));
    parts
}

fn edit(mut parts: Parts, name: &str, from: &str, to: &str) -> Parts {
    let part = parts
        .iter_mut()
        .find(|(part, _)| part == name)
        .expect("part");
    let text = String::from_utf8(part.1.clone()).expect("utf-8 part");
    assert!(text.contains(from), "edit anchor missing in {name}: {from}");
    part.1 = text.replacen(from, to, 1).into_bytes();
    parts
}

fn add(mut parts: Parts, name: &str, data: impl Into<Vec<u8>>) -> Parts {
    parts.push((name.to_owned(), data.into()));
    parts
}

fn override_type(parts: Parts, part: &str, content_type: &str) -> Parts {
    let line = format!("<Override PartName=\"/{part}\" ContentType=\"{content_type}\"/>\n</Types>");
    edit(parts, TYPES, "</Types>", &line)
}

fn relationship(parts: Parts, rels: &str, line: &str) -> Parts {
    edit(
        parts,
        rels,
        "</Relationships>",
        &format!("{line}\n</Relationships>"),
    )
}

fn zip_with(parts: &Parts, reverse: bool, deflate: bool, dos_date: u16) -> Vec<u8> {
    let mut entries: Vec<WriteEntry<'_>> = parts
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

fn package(parts: &Parts) -> Vec<u8> {
    zip_with(parts, false, true, 0x5921)
}

fn read(bytes: &[u8]) -> Result<Document, Refusal> {
    docx::read(bytes, &Limits::default(), &Cancel::default())
}

fn imported(bytes: &[u8]) -> Imported {
    Docx.import(bytes, &Limits::default(), &Cancel::default())
        .expect("import")
}

fn paragraphs(document: &Document) -> Vec<&Paragraph> {
    document
        .content
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Paragraph(paragraph) => Some(paragraph),
            Block::Table(_) => None,
        })
        .collect()
}

fn paragraph<'a>(document: &'a Document, text: &str) -> &'a Paragraph {
    paragraphs(document)
        .into_iter()
        .find(|paragraph| paragraph.text().contains(text))
        .unwrap_or_else(|| panic!("no paragraph contains {text:?}"))
}

fn runs(paragraph: &Paragraph) -> Vec<&docx::Run> {
    paragraph
        .inlines
        .iter()
        .filter_map(|inline| match inline {
            Inline::Run(run) => Some(run),
            _ => None,
        })
        .collect()
}

fn run_with<'a>(paragraph: &'a Paragraph, text: &str) -> &'a docx::Run {
    runs(paragraph)
        .into_iter()
        .find(|run| matches!(&run.content, RunContent::Text(value) if value == text))
        .unwrap_or_else(|| panic!("no run {text:?}"))
}

fn canary() -> &'static str {
    CANARIES
        .lines()
        .find(|line| !line.is_empty())
        .expect("synthetic canary")
}

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

fn scratch() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bran-docx-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(root.join("out")).expect("create export root");
    root
}

#[test]
fn docx_fixtures_stay_within_budget() {
    for (name, text) in [
        ("representative", REPRESENTATIVE),
        ("unsupported-benign", UNSUPPORTED),
    ] {
        assert!(
            text.len() <= MAX_FIXTURE_FILE_BYTES,
            "{name} exceeds fixture budget"
        );
    }
}

#[test]
fn docx_import_preserves_structure() {
    let document = read(&package(&representative())).expect("representative imports");
    let kinds: Vec<Kind> = paragraphs(&document)
        .iter()
        .map(|paragraph| paragraph.kind.clone())
        .collect();
    let list = |num, level, format: &str| Kind::List {
        num,
        level,
        format: format.to_owned(),
    };
    assert_eq!(
        kinds,
        [
            Kind::Title,
            Kind::Heading(1),
            Kind::Body,
            list(1, 0, "bullet"),
            list(1, 1, "bullet"),
            list(2, 0, "decimal"),
            Kind::Heading(2),
            Kind::Body,
            Kind::Caption,
            Kind::Body,
            Kind::Body,
        ]
    );
    // Block order: the table sits between the review paragraph and its caption.
    assert!(matches!(document.content.blocks[8], Block::Table(_)));

    let body = paragraph(&document, "Plain text");
    assert!(run_with(body, "bold").bold);
    assert!(!run_with(body, "Plain text with ").bold);
    assert_eq!(
        run_with(body, "link").link,
        Some(Link::External("https://example.invalid/spec".to_owned()))
    );
    assert_eq!(
        run_with(body, "jump").link,
        Some(Link::Internal("overview".to_owned()))
    );
    assert!(runs(body)
        .iter()
        .any(|run| run.content == RunContent::FootnoteRef(1)));

    let heading = paragraph(&document, "Overview");
    assert_eq!(
        heading.inlines.first(),
        Some(&Inline::BookmarkStart("overview".to_owned()))
    );
    assert_eq!(
        heading.inlines.last(),
        Some(&Inline::BookmarkEnd("overview".to_owned()))
    );

    let Block::Table(rows) = &document.content.blocks[8] else {
        panic!("table expected");
    };
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[1][0].merge, Merge::Restart);
    assert_eq!(rows[2][0].merge, Merge::Continue);
    assert_eq!(rows[3][0].span, 2);
    assert!(
        matches!(rows[1][1].blocks[0], Block::Table(_)),
        "nested table kept"
    );
    assert_eq!(
        document.content.blocks[8].text(),
        "Metric\tValue\nGroup\tInner\n\n\t2\nMerged total"
    );

    let image = paragraphs(&document)[9];
    assert!(image.section_break, "sectPr in pPr ends section 1");
    let RunContent::Image(picture) = &runs(image)[0].content else {
        panic!("image run expected");
    };
    assert_eq!(picture.asset, sha256_hex(PIXEL_PNG));
    assert_eq!((picture.width, picture.height), (95250, 95250));
    assert_eq!(picture.description, "Synthetic one-pixel image");
    let asset = &document.content.assets[&picture.asset];
    assert_eq!(asset.media_type, "image/png");
    assert_eq!(asset.data, PIXEL_PNG);
}

#[test]
fn docx_import_keeps_review_state_without_accepting_edits() {
    let document = read(&package(&representative())).expect("representative imports");
    let review = paragraph(&document, "Kept text");
    let inserted = run_with(review, "inserted")
        .change
        .as_ref()
        .expect("insert");
    assert_eq!(inserted.kind, ChangeKind::Insert);
    assert_eq!(inserted.author, "Synthetic Author");
    assert_eq!(inserted.date, "2026-01-02T03:04:05Z");
    let removed = run_with(review, "removed").change.as_ref().expect("delete");
    assert_eq!(removed.kind, ChangeKind::Delete);
    assert_eq!(review.text(), "Kept text inserted", "citable text");
    assert_eq!(review.full_text(), "Kept text insertedremoved", "all text");
    assert_eq!(review.inlines.first(), Some(&Inline::CommentStart(0)));
    assert!(review.inlines.contains(&Inline::CommentEnd(0)));
    assert!(runs(review)
        .iter()
        .any(|run| run.content == RunContent::CommentRef(0)));
    assert!(runs(review)
        .iter()
        .any(|run| run.content == RunContent::EndnoteRef(1)));

    let comment = &document.content.comments[0];
    assert_eq!(
        (
            comment.id,
            comment.author.as_str(),
            comment.initials.as_str()
        ),
        (0, "Synthetic Reviewer", "SR")
    );
    assert_eq!(comment.blocks[0].text(), "Synthetic review comment.");
    let footnote = &document.content.footnotes;
    assert_eq!(footnote.len(), 1, "separator notes are not content");
    assert_eq!(footnote[0].id, 1);
    assert_eq!(footnote[0].blocks[0].text(), " Synthetic footnote text.");
    assert_eq!(
        document.content.endnotes[0].blocks[0].text(),
        " Synthetic endnote text."
    );

    for (feature, status) in [
        ("tracked-changes", Status::Exact),
        ("comments", Status::Normalized),
        ("footnotes", Status::Normalized),
        ("endnotes", Status::Normalized),
        ("hyperlinks", Status::Exact),
        ("bookmarks", Status::Exact),
        ("images", Status::Exact),
        ("headings", Status::Normalized),
        ("lists", Status::Normalized),
        ("tables", Status::Normalized),
        ("sections", Status::Normalized),
        ("captions", Status::Normalized),
    ] {
        assert_eq!(
            document.fidelity.get(feature),
            Some(&status),
            "fidelity of {feature}"
        );
    }
    let receipt = imported(&package(&representative())).receipt;
    assert!(receipt.contains(&"hyperlink-not-fetched".to_owned()));
    assert!(receipt.contains(&"tracked-changes:exact".to_owned()));
}

#[test]
fn docx_citation_anchors_are_stable_locators() {
    let document = read(&package(&representative())).expect("representative imports");
    let citations = document.citations();
    let found = |id: &str| {
        citations
            .iter()
            .find(|citation| citation.id == id)
            .unwrap_or_else(|| panic!("no citation {id}"))
    };
    let title = found("docx:s1:b1");
    assert_eq!(
        (title.role, title.section.as_str(), title.ordinal),
        ("title", "s1", 1)
    );
    assert_eq!(title.text, "Synthetic quarterly brief");
    assert_eq!(found("docx:s1:b2").role, "heading");
    assert_eq!(found("docx:s1:b4").role, "list");
    assert_eq!(found("docx:s1:b9").role, "table");
    assert_eq!(found("docx:s2:b1").text, "Second section text.");
    assert_eq!(found("docx:footnote:1").text, " Synthetic footnote text.");
    assert_eq!(found("docx:endnote:1").section, "endnotes");
    assert_eq!(found("docx:comment:0").text, "Synthetic review comment.");
    assert!(
        citations
            .iter()
            .all(|citation| citation.id != "docx:s1:b11"),
        "the image-only paragraph has no text to cite"
    );
    let anchors = imported(&package(&representative())).anchors;
    assert_eq!(anchors.len(), citations.len());
    assert!(anchors
        .iter()
        .zip(&citations)
        .all(|(anchor, citation)| anchor.id == citation.id
            && anchor.text_digest == sha256_hex(citation.text.as_bytes())));
}

#[test]
fn docx_import_ignores_archive_order_timestamps_and_compression() {
    let parts = representative();
    let first = imported(&package(&parts));
    for bytes in [
        zip_with(&parts, true, false, 0x4A21),
        zip_with(&parts, false, false, 0x3C01),
        zip_with(&parts, true, true, 0x0021),
    ] {
        assert_eq!(imported(&bytes), first);
    }
    assert_eq!(imported(&package(&parts)), first, "repeatable");
}

#[test]
fn docx_reports_unsupported_features_explicitly() {
    let document = read(&package(&parse_parts(UNSUPPORTED))).expect("benign input imports");
    for (feature, status) in [
        ("charts", Status::Unsupported),
        ("math", Status::Unsupported),
        ("shapes", Status::Unsupported),
        ("headers-footers", Status::Unsupported),
        ("document-properties", Status::Unsupported),
        ("symbols", Status::Unsupported),
        ("formatting-changes", Status::Unsupported),
        ("paragraph-mark-changes", Status::Unsupported),
        ("fields", Status::Normalized),
        ("content-controls", Status::Normalized),
    ] {
        assert_eq!(
            document.fidelity.get(feature),
            Some(&status),
            "fidelity of {feature}"
        );
    }
    let text: Vec<String> = paragraphs(&document).iter().map(|p| p.text()).collect();
    assert_eq!(
        text,
        [
            "Chart follows: ",
            "Equation: ",
            "After the box.",
            "Controlled text",
            "Date field: 2026-01-02",
            "Symbol: ",
            "Reformatted text",
        ],
        "field results kept, field codes and skipped content absent"
    );
    assert!(run_with(paragraph(&document, "Reformatted"), "Reformatted text").italic);
}

#[test]
fn docx_refuses_hostile_packages() {
    let base = representative();
    let default = Limits::default();
    let small = Limits {
        max_part_bytes: 64 * 1024,
        max_xml_nodes: 2_000,
        ..Limits::default()
    };
    let nested_tables = format!(
        "{}<w:p/>{}",
        "<w:tbl><w:tr><w:tc>".repeat(40),
        "<w:p/></w:tc></w:tr></w:tbl>".repeat(40)
    );
    let rows: Vec<(&str, Vec<u8>, &Limits, Refusal)> = vec![
        (
            "zip path escape",
            package(&add(base.clone(), "../outside.xml", "<x/>")),
            &default,
            Refusal::UnsafePartPath,
        ),
        (
            "image relationship escape",
            package(&edit(
                base.clone(),
                RELS,
                "Target=\"media/pixel.png\"",
                "Target=\"../../outside.png\"",
            )),
            &default,
            Refusal::UnsafePartPath,
        ),
        (
            "oversized expansion",
            package(&add(
                base.clone(),
                "word/media/bomb.png",
                vec![0u8; 1024 * 1024],
            )),
            &small,
            Refusal::DecompressionLimit,
        ),
        (
            "malformed document XML",
            package(&edit(base.clone(), MAIN, "</w:body>", "")),
            &default,
            Refusal::MalformedXml,
        ),
        (
            "undeclared entity in document XML",
            package(&edit(base.clone(), MAIN, "Overview", "&undeclared;")),
            &default,
            Refusal::MalformedXml,
        ),
        (
            "DTD entity expansion in document XML",
            package(&edit(
                base.clone(),
                MAIN,
                "<w:document",
                "<!DOCTYPE d [<!ENTITY a \"aaaa\"><!ENTITY b \"&a;&a;&a;&a;\">]>\n<w:document",
            )),
            &default,
            Refusal::XmlDtdRefused,
        ),
        (
            "excessive nodes in document XML",
            package(&edit(
                base.clone(),
                MAIN,
                "<w:sectPr/>\n</w:body>",
                &format!("{}<w:sectPr/>\n</w:body>", "<w:p/>".repeat(3_000)),
            )),
            &small,
            Refusal::XmlNodeLimit,
        ),
        (
            "table nesting beyond the block budget",
            package(&edit(
                base.clone(),
                MAIN,
                "<w:sectPr/>\n</w:body>",
                &format!("{nested_tables}<w:sectPr/>\n</w:body>"),
            )),
            &default,
            Refusal::XmlDepthLimit,
        ),
        (
            "external attached template",
            package(&relationship(
                base.clone(),
                RELS,
                &format!("<Relationship Id=\"rId9\" Type=\"{REL}/attachedTemplate\" Target=\"https://example.invalid/t.dotm\" TargetMode=\"External\"/>"),
            )),
            &default,
            Refusal::ExternalReference,
        ),
        (
            "externally linked image",
            package(&relationship(
                base.clone(),
                RELS,
                &format!("<Relationship Id=\"rId9\" Type=\"{REL}/image\" Target=\"https://example.invalid/i.png\" TargetMode=\"External\"/>"),
            )),
            &default,
            Refusal::ExternalReference,
        ),
        (
            "macro-enabled main part",
            package(&edit(
                base.clone(),
                TYPES,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
                "application/vnd.ms-word.document.macroEnabled.main+xml",
            )),
            &default,
            Refusal::ActiveContent,
        ),
        (
            "VBA project part",
            package(&override_type(
                add(base.clone(), "word/vbaProject.bin", "synthetic inert bytes"),
                "word/vbaProject.bin",
                "application/vnd.ms-office.vbaProject",
            )),
            &default,
            Refusal::ActiveContent,
        ),
        (
            "OLE object part",
            package(&override_type(
                add(
                    base.clone(),
                    "word/embeddings/oleObject1.bin",
                    "synthetic inert bytes",
                ),
                "word/embeddings/oleObject1.bin",
                "application/vnd.openxmlformats-officedocument.oleObject",
            )),
            &default,
            Refusal::ActiveContent,
        ),
        (
            "office document that is not WordprocessingML",
            package(&edit(
                base.clone(),
                TYPES,
                "wordprocessingml.document.main+xml",
                "spreadsheetml.sheet.main+xml",
            )),
            &default,
            Refusal::UnsupportedContainer,
        ),
        (
            "missing main document part",
            package(&base.iter().filter(|(name, _)| name != MAIN).cloned().collect()),
            &default,
            Refusal::MalformedContainer,
        ),
    ];
    for (label, bytes, limits, expected) in rows {
        let result = std::panic::catch_unwind(|| docx::read(&bytes, limits, &Cancel::default()));
        let result = result.unwrap_or_else(|_| panic!("{label}: import panicked"));
        assert_eq!(result.err(), Some(expected), "{label}");
    }
}

#[test]
fn docx_export_round_trip_preserves_model_and_anchors() {
    let first = imported(&package(&representative()));
    let exported = docx::export(&first).expect("export");
    assert_eq!(
        docx::export(&first).expect("export again").bytes,
        exported.bytes,
        "export is deterministic"
    );
    let intake = PackageIntake(Format::Docx)
        .import(&exported.bytes, &Limits::default(), &Cancel::default())
        .expect("export is a valid package");
    assert!(
        !intake
            .receipt
            .iter()
            .any(|code| code == "missing-relationship-target"),
        "{:?}",
        intake.receipt
    );
    let second = imported(&exported.bytes);
    assert_eq!(second.anchors, first.anchors, "citation anchors survive");
    let before = Document::from_canonical(&first.canonical).expect("canonical reads");
    let after = Document::from_canonical(&second.canonical).expect("canonical reads");
    assert_eq!(after.content, before.content, "structural model survives");
    assert_eq!(
        docx::export(&second).expect("re-export").bytes,
        exported.bytes,
        "export of the re-import is byte-identical"
    );
}

#[test]
fn docx_export_carries_a_fidelity_receipt() {
    let first = imported(&package(&representative()));
    let exported = docx::export(&first).expect("export");
    let receipt = String::from_utf8(exported.receipt).expect("UTF-8 receipt");
    for expected in [
        "\"schema_version\":\"1\"".to_owned(),
        "\"format\":\"docx\"".to_owned(),
        format!("\"output_sha256\":\"{}\"", sha256_hex(&exported.bytes)),
        format!(
            "\"source_canonical_sha256\":\"{}\"",
            sha256_hex(&first.canonical)
        ),
        "\"tracked-changes\":\"exact\"".to_owned(),
        "\"styles\":\"normalized\"".to_owned(),
        "\"model\":\"exact\"".to_owned(),
    ] {
        assert!(receipt.contains(&expected), "{expected} in {receipt}");
    }
}

#[test]
fn docx_canonical_json_reads_back_and_rejects_tampering() {
    let document = read(&package(&representative())).expect("representative imports");
    let canonical = document.canonical();
    assert_eq!(
        Document::from_canonical(&canonical).expect("reads back"),
        document
    );
    let text = String::from_utf8(canonical.clone()).expect("UTF-8");
    for bad in [
        canonical[..canonical.len() - 1].to_vec(),
        text.replacen(&sha256_hex(PIXEL_PNG), &sha256_hex(b"other"), 1)
            .into_bytes(),
        b"{\"schema_version\":\"1\"}".to_vec(),
        format!("{}{}", "[".repeat(100_000), "]".repeat(100_000)).into_bytes(),
    ] {
        assert_eq!(
            Document::from_canonical(&bad).err(),
            Some(Refusal::MalformedContainer)
        );
    }
}

fn split_marker(marker: &str) -> Parts {
    let (head, tail) = marker.split_at(marker.len() / 2);
    edit(
        representative(),
        MAIN,
        "<w:r><w:t>Second section text.</w:t></w:r>",
        &format!("<w:r><w:t>{head}</w:t></w:r><w:r><w:t>{tail}</w:t></w:r>"),
    )
}

#[test]
fn docx_dlp_and_boundary_refuse_before_any_file_is_written() {
    for (marker, code, refusal) in [
        (canary(), "dlp-findings", Refusal::DlpFindings),
        (
            "important_boundary",
            "public-boundary-violation",
            Refusal::PublicBoundary,
        ),
    ] {
        let bytes = package(&split_marker(marker));
        let package_receipt = PackageIntake(Format::Docx)
            .import(&bytes, &Limits::default(), &Cancel::default())
            .expect("intake")
            .receipt;
        assert!(
            !package_receipt.iter().any(|c| c == code),
            "the split marker is invisible to the package byte scan"
        );
        let first = imported(&bytes);
        assert!(
            first.receipt.iter().any(|c| c == code),
            "adapter finds {code}"
        );
        let mut forged = first.clone();
        forged.receipt.retain(|c| c != code);
        for input in [&first, &forged] {
            let root = scratch();
            assert_eq!(
                docx::export_file(&root, "out/report.docx", input).err(),
                Some(refusal)
            );
            assert_eq!(
                std::fs::read_dir(root.join("out")).unwrap().count(),
                0,
                "no file or temporary file may be written"
            );
            std::fs::remove_dir_all(root).unwrap();
            assert_eq!(docx::export(input).err(), Some(refusal));
        }
    }
}

#[test]
fn docx_export_file_needs_explicit_docx_destination_and_never_overwrites() {
    let first = imported(&package(&representative()));
    let root = scratch();
    let (path, receipt) = docx::export_file(&root, "out/brief.docx", &first).expect("export");
    assert_eq!(path, root.canonicalize().unwrap().join("out/brief.docx"));
    let written = std::fs::read(&path).unwrap();
    assert_eq!(written, docx::export(&first).unwrap().bytes);
    assert!(!receipt.is_empty());
    assert_eq!(
        docx::export_file(&root, "out/brief.docx", &first).err(),
        Some(Refusal::ExportExists)
    );
    assert_eq!(std::fs::read(&path).unwrap(), written, "unchanged");
    assert_eq!(
        docx::export_file(&root, "out/brief.pdf", &first).err(),
        Some(Refusal::ExportFormatMismatch)
    );
    std::fs::remove_dir_all(root).unwrap();
}

// ---- Opt-in independent-reader check (never part of the normal gate). ----

/// Runs one reader; `None` means the reader is not installed.
fn reader(program: &str, args: &[&str]) -> Option<Result<String, String>> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    Some(if output.status.success() {
        Ok(text)
    } else {
        Err(format!(
            "{program} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ))
    })
}

/// Set BRAN_READER_DIR to a directory the readers may use (a LibreOffice snap
/// needs a non-hidden directory under the home directory) and BRAN_READER_PYTHON
/// to a Python with python-docx. A missing reader is reported as unavailable;
/// fewer than two available readers fails rather than passing.
#[test]
#[ignore = "opt-in: needs LibreOffice, poppler, and python-docx installed"]
fn docx_export_opens_in_independent_readers() {
    let directory = std::env::var_os("BRAN_READER_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let python = std::env::var("BRAN_READER_PYTHON").unwrap_or_else(|_| "python3".to_owned());
    let document = directory.join("bran-docx-reader.docx");
    let pdf = directory.join("bran-docx-reader.pdf");
    let _ = std::fs::remove_file(&document);
    let _ = std::fs::remove_file(&pdf);
    let first = imported(&package(&representative()));
    std::fs::write(&document, docx::export(&first).expect("export").bytes).unwrap();
    let path = |path: &Path| path.to_str().expect("UTF-8 path").to_owned();
    let expected = [
        "Synthetic quarterly brief",
        "Overview",
        "Merged total",
        "Second section text.",
    ];
    let mut passed = Vec::new();
    let mut unavailable = Vec::new();

    let script = "import sys, docx\nd = docx.Document(sys.argv[1])\n\
print('\\n'.join(p.text for p in d.paragraphs))\n\
print('\\n'.join(c.text for t in d.tables for r in t.rows for c in r.cells))\n\
print('inline_shapes', len(d.inline_shapes))";
    match reader(&python, &["-c", script, &path(&document)]) {
        None => unavailable.push("python-docx"),
        Some(Err(error)) if error.contains("No module named") => unavailable.push("python-docx"),
        Some(Err(error)) => panic!("python-docx failed: {error}"),
        Some(Ok(text)) => {
            for needle in expected.iter().chain(&["inline_shapes 1"]) {
                assert!(text.contains(needle), "python-docx misses {needle}: {text}");
            }
            passed.push("python-docx");
        }
    }
    let convert = [
        "--headless",
        "--convert-to",
        "pdf",
        &path(&document),
        "--outdir",
        &path(&directory),
    ];
    match reader("libreoffice", &convert) {
        None => unavailable.push("libreoffice"),
        Some(Err(error)) => panic!("LibreOffice failed: {error}"),
        Some(Ok(_)) if !pdf.exists() => panic!(
            "LibreOffice wrote no PDF; a snap install needs BRAN_READER_DIR under the home directory"
        ),
        Some(Ok(_)) => match reader("pdftotext", &[&path(&pdf), "-"]) {
            None => unavailable.push("libreoffice (pdftotext missing)"),
            Some(Err(error)) => panic!("LibreOffice output unreadable: {error}"),
            Some(Ok(text)) => {
                for needle in expected.iter().chain(&[
                    "Synthetic footnote text.",
                    "Synthetic endnote text.",
                ]) {
                    assert!(text.contains(needle), "LibreOffice misses {needle}: {text}");
                }
                passed.push("libreoffice");
            }
        },
    }
    let _ = std::fs::remove_file(&document);
    let _ = std::fs::remove_file(&pdf);
    println!("readers passed: {passed:?}; unavailable: {unavailable:?}");
    assert!(
        passed.len() >= 2,
        "unavailable: fewer than two independent readers ran ({unavailable:?})"
    );
}
