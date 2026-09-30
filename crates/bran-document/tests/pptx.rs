//! PPTX adapter (issue #22): import, export, round trip, DLP, and hostile decks.
//!
//! Every deck is synthetic and built in memory from the `.parts` fixtures.
//! No test here needs PowerPoint, a cloud service, or the network. The one
//! reader test is opt-in and reports each missing reader as unavailable.

use bran_document::canonical::{sha256_hex, Json};
use bran_document::conformance::{self, Adapter, Expect, Imported, PackageIntake};
use bran_document::pptx::{self, Pptx};
use bran_document::{export, opc, Cancel, Format, Limits, Refusal};
use std::io::Read;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::process::Command;

#[path = "pptx/decks.rs"]
mod decks;

use decks::{Deck, REL};

const CANARIES: &str =
    include_str!("../../../fixtures/public-boundary/rejected/synthetic-canaries.txt");
/// Canonical projection digest of the ordinary deck. A change here is a change
/// to the PPTX content model and must be reviewed as one.
const ORDINARY_DIGEST: &str = "30541582214bd6e1a249c2de0443c48a330d44d8e9fd7d23665351409420e070";
const SLIDE3: &str = "ppt/slides/slide3.xml";
const RELS3: &str = "ppt/slides/_rels/slide3.xml.rels";

fn import_bytes(bytes: &[u8]) -> Result<Imported, Refusal> {
    Pptx.import(bytes, &Limits::default(), &Cancel::default())
}

fn import(deck: &Deck) -> Imported {
    import_bytes(&deck.zip()).expect("deck imports")
}

fn content(bytes: &[u8]) -> Json {
    pptx::parse_canonical(bytes).expect("canonical JSON parses")
}

fn get<'a>(value: &'a Json, key: &str) -> &'a Json {
    match value {
        Json::Obj(map) => map
            .get(key)
            .unwrap_or_else(|| panic!("missing {key} in {value:?}")),
        other => panic!("not an object: {other:?}"),
    }
}

fn arr(value: &Json) -> &[Json] {
    match value {
        Json::Arr(items) => items,
        other => panic!("not an array: {other:?}"),
    }
}

fn text(value: &Json) -> &str {
    match value {
        Json::Str(text) => text,
        other => panic!("not a string: {other:?}"),
    }
}

fn int(value: &Json) -> i64 {
    match value {
        Json::Int(value) => *value,
        other => panic!("not an integer: {other:?}"),
    }
}

fn runs(paragraph: &Json) -> Vec<&str> {
    arr(get(paragraph, "runs"))
        .iter()
        .map(|run| text(get(run, "text")))
        .collect()
}

fn cell_texts(table: &Json) -> Vec<Vec<String>> {
    arr(get(table, "rows"))
        .iter()
        .map(|row| {
            arr(get(row, "cells"))
                .iter()
                .map(|cell| arr(cell).iter().map(|p| runs(p).concat()).collect())
                .collect()
        })
        .collect()
}

fn anchor<'a>(deck: &'a Json, id: &str) -> &'a Json {
    arr(get(deck, "anchors"))
        .iter()
        .find(|anchor| text(get(anchor, "id")) == id)
        .unwrap_or_else(|| panic!("anchor {id} missing"))
}

fn has(imported: &Imported, code: &str) -> bool {
    imported.receipt.iter().any(|item| item == code)
}

fn canary() -> &'static str {
    CANARIES
        .lines()
        .find(|line| !line.is_empty())
        .expect("synthetic canary")
}

// Exact reviewer ZIPs, deflated and hex-encoded to keep the corpus text-only.
fn review_deck(encoded: &str, digest: &str) -> Vec<u8> {
    let hex: String = encoded.split_whitespace().collect();
    let compressed: Vec<u8> = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    let mut bytes = Vec::new();
    flate2::read::DeflateDecoder::new(compressed.as_slice())
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(sha256_hex(&bytes), digest, "original reviewer input");
    bytes
}

#[test]
fn pptx_review_binary_dlp_rescan() {
    let bytes = review_deck(
        include_str!(
            "../../../fixtures/enterprise-documents/pptx-review/png-metadata-canary.deflate.hex"
        ),
        "072d0358bba5850397e1867fbfb46c077e5d576a09f6c8ed7bdb0e730aeaf19f",
    );
    let mut imported = import_bytes(&bytes).unwrap();
    assert!(
        has(&imported, "dlp-findings"),
        "intake detects PNG metadata"
    );
    assert_eq!(Pptx.export(&imported).err(), Some(Refusal::DlpFindings));
    imported.receipt.clear();
    assert_eq!(
        Pptx.export(&imported).err(),
        Some(Refusal::DlpFindings),
        "export independently scans binary metadata"
    );
}

#[test]
fn pptx_review_shared_notes_budget() {
    if std::env::var_os("BRAN_PPTX_BUDGET_CHILD").is_none() {
        let exe = std::env::current_exe().unwrap();
        let mut command = if cfg!(target_os = "linux") {
            let mut command = Command::new("sh");
            command.args([
                "-c",
                "ulimit -v 524288; ulimit -c 0; exec \"$@\"",
                "pptx-budget",
            ]);
            command.arg(&exe);
            command
        } else {
            Command::new(&exe)
        };
        let output = command
            .args([
                "--exact",
                "pptx_review_shared_notes_budget",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("BRAN_PPTX_BUDGET_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "bounded child {}: {} {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    // Positive control under the identical address-space limit.
    import(&Deck::ordinary());
    let bytes = review_deck(
        include_str!("../../../fixtures/enterprise-documents/pptx-review/shared-notes-amplification.deflate.hex"),
        "d757b0217a09e3ddb6b327ef23ce4c19e5d6240dd143e025adf6868ff0d91075",
    );
    assert_eq!(import_bytes(&bytes).err(), Some(Refusal::Oversized));
}

#[test]
fn pptx_review_compressible_text_round_trip() {
    let bytes = review_deck(
        include_str!(
            "../../../fixtures/enterprise-documents/pptx-review/compressible-text.deflate.hex"
        ),
        "a9aab4eb18bd3274ad86dc5a821938c709753495c3705169f0b80aec3285b94e",
    );
    let first = import_bytes(&bytes).unwrap();
    let exported = Pptx.export(&first).unwrap();
    assert_eq!(Pptx.export(&first).unwrap(), exported);
    let again = import_bytes(&exported).expect("export obeys the same intake limits");
    assert_eq!(again.canonical, first.canonical);
    assert_eq!(again.anchors, first.anchors);
}

#[test]
fn pptx_review_unbound_relationship_prefix() {
    import(&Deck::ordinary());
    let bytes = review_deck(
        include_str!("../../../fixtures/enterprise-documents/pptx-review/undeclared-r.deflate.hex"),
        "27a310d94d2e43aaa5416c8972ad3047874c25bc1a214ee9609aba7b75e2a6b3",
    );
    assert_eq!(import_bytes(&bytes).err(), Some(Refusal::MalformedXml));
    // Empty bindings and undeclared element prefixes are also malformed.
    for (from, to) in [
        (format!("xmlns:r=\"{REL}\""), "xmlns:r=\"\""),
        (
            "xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"".to_owned(),
            "",
        ),
    ] {
        let deck = Deck::ordinary().edit("ppt/presentation.xml", &from, to);
        assert_eq!(import_bytes(&deck.zip()).err(), Some(Refusal::MalformedXml));
    }
}

#[test]
fn pptx_import_maps_presentation_structure() {
    let imported = import(&Deck::ordinary());
    let deck = content(&imported.canonical);
    assert_eq!(text(get(&deck, "schema")), pptx::SCHEMA);
    assert_eq!(text(get(&deck, "family")), "presentation");
    assert_eq!(int(get(get(&deck, "slide_size"), "cx")), 12_192_000);

    // Deck order follows sldIdLst, not part names or slide ids.
    let slides = arr(get(&deck, "slides"));
    let ids: Vec<i64> = slides.iter().map(|slide| int(get(slide, "id"))).collect();
    assert_eq!(ids, [256, 300, 258]);
    let layouts: Vec<&str> = slides.iter().map(|s| text(get(s, "layout"))).collect();
    assert_eq!(
        layouts,
        ["Title Slide", "Title and Content", "Title and Content"]
    );
    let hidden: Vec<&Json> = slides.iter().map(|s| get(s, "hidden")).collect();
    assert_eq!(
        hidden,
        [&Json::Bool(false), &Json::Bool(false), &Json::Bool(true)]
    );
    assert_eq!(text(get(&slides[1], "name")), "Findings");
    let sections: Vec<(&str, Vec<i64>)> = arr(get(&deck, "sections"))
        .iter()
        .map(|s| {
            let ids = arr(get(s, "slides")).iter().map(int).collect();
            (text(get(s, "name")), ids)
        })
        .collect();
    assert_eq!(
        sections,
        [("Opening", vec![256]), ("Details", vec![300, 258])]
    );

    // Reading order is shape-tree order, groups included, not shape-id order.
    let shapes = arr(get(&slides[1], "shapes"));
    let order: Vec<(&str, i64)> = shapes
        .iter()
        .map(|s| (text(get(s, "kind")), int(get(s, "id"))))
        .collect();
    assert_eq!(
        order,
        [
            ("shape", 7),
            ("shape", 4),
            ("group", 10),
            ("picture", 5),
            ("shape", 6),
            ("shape", 8)
        ]
    );
    let children: Vec<(&str, i64)> = arr(get(&shapes[2], "children"))
        .iter()
        .map(|s| (text(get(s, "kind")), int(get(s, "id"))))
        .collect();
    assert_eq!(children, [("shape", 11), ("connector", 12)]);
    assert_eq!(get(&shapes[1], "text_box"), &Json::Bool(true));
    assert_eq!(
        text(get(get(&shapes[0], "placeholder"), "type")),
        "title",
        "placeholder kind is kept"
    );

    // Text boxes keep paragraphs, levels, line breaks, fields, and run links.
    let paragraphs = arr(get(&shapes[1], "paragraphs"));
    assert_eq!(
        runs(&paragraphs[0]),
        ["Revenue grew", "\n", "in every region"]
    );
    assert_eq!(int(get(&paragraphs[1], "level")), 1);
    assert_eq!(
        runs(&paragraphs[1]),
        ["See ", "the report", " on slide ", "2"]
    );
    let link = get(&arr(get(&paragraphs[1], "runs"))[1], "link");
    assert_eq!(
        text(get(link, "url")),
        "https://example.invalid/synthetic-report"
    );

    // Shape-level links: a slide jump resolves to the target slide id; an
    // action-only link keeps its action and nothing else.
    let jump = get(&shapes[4], "link");
    assert_eq!(int(get(jump, "slide")), 258);
    assert_eq!(text(get(jump, "action")), "ppaction://hlinksldjump");
    let next = get(&shapes[5], "link");
    assert_eq!(get(next, "url"), &Json::Null);
    assert_eq!(get(next, "slide"), &Json::Null);
    assert_eq!(
        text(get(next, "action")),
        "ppaction://hlinkshowjump?jump=nextslide"
    );

    // Alt text and title, and images as content-addressed assets used twice.
    let picture = &shapes[3];
    assert_eq!(
        text(get(picture, "alt_text")),
        "Synthetic bar chart of regional growth"
    );
    assert_eq!(text(get(picture, "alt_title")), "Growth chart");
    let assets = arr(get(&deck, "assets"));
    assert_eq!(assets.len(), 1, "one image referenced twice is one asset");
    let png = decks::png([37, 99, 235]);
    assert_eq!(text(get(&assets[0], "sha256")), sha256_hex(&png));
    assert_eq!(text(get(&assets[0], "media_type")), "image/png");
    assert_eq!(int(get(&assets[0], "byte_length")), png.len() as i64);
    let asset_id = text(get(&assets[0], "id"));
    assert_eq!(text(get(picture, "image")), asset_id);
    let logo = &arr(get(&slides[2], "shapes"))[2];
    assert_eq!(text(get(logo, "image")), asset_id);

    // Tables, notes, and comments.
    let table = &arr(get(&slides[2], "shapes"))[1];
    assert_eq!(text(get(table, "kind")), "table");
    assert_eq!(cell_texts(table), [["Region", "Growth"], ["North", "12%"]]);
    let columns: Vec<i64> = arr(get(table, "columns")).iter().map(int).collect();
    assert_eq!(columns, [3_000_000, 3_000_000]);
    let notes = get(&slides[0], "notes");
    assert_eq!(int(get(notes, "shape")), 3);
    let note_lines: Vec<String> = arr(get(notes, "paragraphs"))
        .iter()
        .map(|p| runs(p).concat())
        .collect();
    assert_eq!(
        note_lines,
        [
            "Open with the regional summary.",
            "Keep it under two minutes."
        ]
    );
    let comments = arr(get(&slides[0], "comments"));
    assert_eq!(comments.len(), 1);
    assert_eq!(text(get(&comments[0], "author")), "Synthetic Reviewer");
    assert_eq!(text(get(&comments[0], "initials")), "SR");
    assert_eq!(
        text(get(&comments[0], "text")),
        "Check the subtitle wording."
    );
    assert_eq!(get(&slides[1], "notes"), &Json::Null);

    // Anchors: envelope-shaped, sorted, stable ids, and digests of their text.
    let anchors = arr(get(&deck, "anchors"));
    let anchor_ids: Vec<&str> = anchors.iter().map(|a| text(get(a, "id"))).collect();
    let mut sorted = anchor_ids.clone();
    sorted.sort_unstable();
    assert_eq!(anchor_ids, sorted, "anchors are sorted by id");
    for item in anchors {
        assert_eq!(text(get(item, "family")), "presentation");
        assert_eq!(
            text(get(item, "text_digest")),
            sha256_hex(text(get(item, "text")).as_bytes())
        );
    }
    let expected = [
        (
            "anc:pptx:slide-256:shape-2",
            "title",
            "Synthetic quarterly review",
            1,
            2,
            0,
        ),
        (
            "anc:pptx:slide-256:shape-3",
            "heading",
            "Prepared for the fixture team",
            1,
            3,
            1,
        ),
        (
            "anc:pptx:slide-256:notes",
            "notes",
            "Open with the regional summary.\nKeep it under two minutes.",
            1,
            3,
            0,
        ),
        (
            "anc:pptx:slide-256:comment-1",
            "notes",
            "Check the subtitle wording.",
            1,
            1,
            0,
        ),
        (
            "anc:pptx:slide-300:shape-11",
            "paragraph",
            "Grouped label",
            2,
            11,
            3,
        ),
        (
            "anc:pptx:slide-300:shape-5:alt",
            "shape",
            "Synthetic bar chart of regional growth",
            2,
            5,
            5,
        ),
        (
            "anc:pptx:slide-258:shape-3",
            "table",
            "Region\tGrowth\nNorth\t12%",
            3,
            3,
            1,
        ),
        (
            "anc:pptx:slide-258:shape-3:alt",
            "shape",
            "Growth by region",
            3,
            3,
            1,
        ),
    ];
    for (id, role, body, slide, shape, z) in expected {
        let item = anchor(&deck, id);
        assert_eq!(text(get(item, "role")), role, "{id}");
        assert_eq!(text(get(item, "text")), body, "{id}");
        let locator = get(item, "locator");
        assert_eq!(
            (
                int(get(locator, "slide")),
                int(get(locator, "shape")),
                int(get(locator, "z_index"))
            ),
            (slide, shape, z),
            "{id}"
        );
    }
    let harness: Vec<(&str, &str)> = imported
        .anchors
        .iter()
        .map(|a| (a.id.as_str(), a.text_digest.as_str()))
        .collect();
    let from_content: Vec<(&str, &str)> = anchors
        .iter()
        .map(|a| (text(get(a, "id")), text(get(a, "text_digest"))))
        .collect();
    assert_eq!(harness, from_content);

    // Normalizations and omissions are receipted, never silent.
    for code in [
        "hyperlink-not-fetched",
        "field-as-text",
        "text-formatting-normalized",
        "shape-styling-normalized",
        "layout-design-normalized",
        "table-formatting-normalized",
        "comment-metadata-normalized",
        "unmapped-parts-omitted",
    ] {
        assert!(has(&imported, code), "receipt is missing {code}");
    }
    let fidelity = get(&deck, "fidelity");
    for (feature, status) in [
        ("animations", "unsupported"),
        ("macros", "unsupported"),
        ("slides", "exact"),
        ("z_order", "exact"),
    ] {
        assert_eq!(text(get(fidelity, feature)), status, "{feature}");
    }
}

#[test]
fn pptx_import_ignores_archive_order_timestamps_and_compression() {
    let deck = Deck::ordinary();
    let first = import_bytes(&deck.zip()).unwrap();
    for bytes in [
        deck.zip_with(true, false, 0x4A21),
        deck.zip_with(false, false, 0x3C01),
        Deck(deck.0.iter().rev().cloned().collect()).zip(),
    ] {
        assert_eq!(import_bytes(&bytes).unwrap(), first);
    }
}

#[test]
fn pptx_ordinary_projection_is_recorded() {
    let imported = import(&Deck::ordinary());
    assert_eq!(sha256_hex(&imported.canonical), ORDINARY_DIGEST);
}

#[test]
fn pptx_unsupported_features_are_receipted() {
    let imported = import(&decks::unsupported_benign());
    for code in decks::BENIGN_CODES {
        assert!(has(&imported, code), "receipt is missing {code}");
    }
    let deck = content(&imported.canonical);
    let shapes = arr(get(&arr(get(&deck, "slides"))[1], "shapes"));
    let ids: Vec<i64> = shapes.iter().map(|s| int(get(s, "id"))).collect();
    assert!(
        !ids.contains(&20) && !ids.contains(&21),
        "charts and SmartArt are not projected"
    );
    let fallback = shapes.iter().find(|s| int(get(s, "id")) == 22).unwrap();
    assert_eq!(text(get(fallback, "name")), "Fallback text");
    let video = shapes.iter().find(|s| int(get(s, "id")) == 23).unwrap();
    assert_eq!(
        text(get(video, "kind")),
        "picture",
        "the poster frame stays"
    );
    assert!(!String::from_utf8_lossy(&imported.canonical).contains("Choice text"));
}

#[test]
fn pptx_adversarial_decks_are_refused() {
    use Refusal::*;
    let o = Deck::ordinary;
    let image = format!("{REL}/image");
    let nested = |levels: usize| {
        let open = "<p:grpSp><p:nvGrpSpPr><p:cNvPr id=\"90\" name=\"G\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>";
        format!("{}{}", open.repeat(levels), "</p:grpSp>".repeat(levels))
    };
    let rows: Vec<(&str, Vec<u8>, Refusal)> = vec![
        (
            "external-media",
            o().relate(RELS3, "rId9", &format!("{REL}/video"), "https://example.invalid/remote.mp4", true)
                .zip(),
            ExternalReference,
        ),
        (
            "external-linked-image",
            o().shapes(SLIDE3, "<p:pic><p:nvPicPr><p:cNvPr id=\"30\" name=\"Linked\"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:link=\"rId9\"/></p:blipFill><p:spPr/></p:pic>")
                .relate(RELS3, "rId9", &image, "https://example.invalid/remote.png", true)
                .zip(),
            ExternalReference,
        ),
        (
            "macro-enabled-deck",
            o().edit(
                "[Content_Types].xml",
                "presentationml.presentation.main+xml",
                "ms-powerpoint.presentation.macroEnabled.main+xml",
            )
            .add("ppt/vbaProject.bin", "synthetic inert bytes")
            .content_type("ppt/vbaProject.bin", "application/vnd.ms-office.vbaProject")
            .zip(),
            ActiveContent,
        ),
        (
            "run-program-action",
            o().edit(SLIDE3, "ppaction://hlinksldjump", "ppaction://program").zip(),
            ActiveContent,
        ),
        (
            "run-macro-action",
            o().edit(SLIDE3, "ppaction://hlinkshowjump?jump=nextslide", "ppaction://macro?name=Synthetic")
                .zip(),
            ActiveContent,
        ),
        (
            "hover-program-action",
            o().edit(
                SLIDE3,
                "<a:hlinkClick r:id=\"rId4\" action=\"ppaction://hlinksldjump\"/>",
                "<a:hlinkHover r:id=\"\" action=\"ppaction://program\"/>",
            )
            .zip(),
            ActiveContent,
        ),
        (
            "embedded-package",
            o().shapes(SLIDE3, "<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"31\" name=\"Object\"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm/><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/presentationml/2006/ole\"><p:oleObj r:id=\"rId9\" progId=\"Excel.Sheet.12\"/></a:graphicData></a:graphic></p:graphicFrame>")
                .relate(RELS3, "rId9", &format!("{REL}/package"), "../embeddings/sheet1.xlsx", false)
                .add("ppt/embeddings/sheet1.xlsx", "synthetic inert bytes")
                .content_type(
                    "ppt/embeddings/sheet1.xlsx",
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                )
                .zip(),
            ActiveContent,
        ),
        (
            "activex-control",
            o().edit(
                SLIDE3,
                "</p:spTree></p:cSld>",
                "</p:spTree><p:controls><p:control r:id=\"rId9\" name=\"Synthetic\"/></p:controls></p:cSld>",
            )
            .relate(RELS3, "rId9", &format!("{REL}/control"), "../activeX/activeX1.xml", false)
            .add("ppt/activeX/activeX1.xml", "<ax:ocx xmlns:ax=\"http://schemas.microsoft.com/office/2006/activeX\"/>")
            .content_type("ppt/activeX/activeX1.xml", "application/vnd.ms-office.activeX+xml")
            .zip(),
            ActiveContent,
        ),
        (
            "relationship-path-escape",
            o().relate(RELS3, "rId9", &image, "../../../escape.png", false).zip(),
            UnsafePartPath,
        ),
        ("zip-path-escape", o().add("../escape.xml", "<x/>").zip(), UnsafePartPath),
        (
            "decompression-limit",
            o().add("ppt/media/huge.png", vec![0u8; 8 * 1024 * 1024]).zip(),
            DecompressionLimit,
        ),
        (
            "relationship-without-target",
            o().edit(RELS3, " Target=\"slide2.xml\"", "").zip(),
            MalformedContainer,
        ),
        (
            "duplicate-relationship-id",
            o().edit(RELS3, "Id=\"rId4\"", "Id=\"rId3\"").zip(),
            MalformedContainer,
        ),
        (
            "dangling-slide-relationship",
            o().edit("ppt/presentation.xml", "r:id=\"rId4\"", "r:id=\"rId99\"").zip(),
            MalformedContainer,
        ),
        (
            "slide-relationship-to-theme",
            o().edit("ppt/presentation.xml", "r:id=\"rId4\"", "r:id=\"rId5\"").zip(),
            MalformedContainer,
        ),
        (
            "duplicate-slide-id",
            o().edit("ppt/presentation.xml", "id=\"258\"", "id=\"256\"").zip(),
            MalformedContainer,
        ),
        (
            "slide-listed-twice",
            o().edit("ppt/presentation.xml", "r:id=\"rId4\"", "r:id=\"rId2\"").zip(),
            MalformedContainer,
        ),
        (
            "missing-slide-part",
            o().remove("ppt/slides/slide2.xml").zip(),
            MalformedContainer,
        ),
        (
            "dangling-link-relationship",
            o().edit(SLIDE3, "r:id=\"rId3\"", "r:id=\"rId42\"").zip(),
            MalformedContainer,
        ),
        (
            "no-main-part",
            o().edit("_rels/.rels", "relationships/officeDocument", "relationships/other")
                .zip(),
            MalformedContainer,
        ),
        (
            "not-a-presentation",
            o().edit(
                "[Content_Types].xml",
                "presentationml.presentation.main+xml",
                "wordprocessingml.document.main+xml",
            )
            .zip(),
            UnsupportedContainer,
        ),
        ("malformed-slide-xml", o().edit(SLIDE3, "</p:sld>", "").zip(), MalformedXml),
        (
            "slide-id-out-of-range",
            o().edit("ppt/presentation.xml", "id=\"258\"", "id=\"12\"").zip(),
            MalformedXml,
        ),
        ("deep-group-nesting", o().shapes(SLIDE3, &nested(100)).zip(), XmlDepthLimit),
        ("deeper-group-nesting", o().shapes(SLIDE3, &nested(300)).zip(), XmlDepthLimit),
    ];
    for (name, bytes, refusal) in rows {
        conformance::check(
            &Pptx,
            &bytes,
            &[],
            &Expect::Refuse(refusal),
            &Limits::default(),
            &Cancel::default(),
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}

#[test]
fn pptx_round_trip_preserves_structure() {
    for deck in [Deck::ordinary(), decks::unsupported_benign()] {
        let first = import(&deck);
        let exported = Pptx.export(&first).expect("export");
        assert_eq!(
            Pptx.export(&first).unwrap(),
            exported,
            "export is deterministic"
        );
        let again = import_bytes(&exported).expect("re-import of the export");
        // Canonical content excludes package inventory and receipts, so equal
        // bytes mean slide order, ids, sections, layouts, shapes, text, tables,
        // notes, comments, alt text, links, assets, and anchors all survived.
        assert_eq!(
            String::from_utf8(again.canonical.clone()).unwrap(),
            String::from_utf8(first.canonical.clone()).unwrap()
        );
        assert_eq!(again.anchors, first.anchors);
        let third = import_bytes(&Pptx.export(&again).unwrap()).unwrap();
        assert_eq!(third.canonical, first.canonical);
    }
}

#[test]
fn pptx_export_carries_fidelity_receipt() {
    let first = import(&Deck::ordinary());
    let exported = Pptx.export(&first).unwrap();
    let package = opc::open(&exported, &Limits::default(), &Cancel::default()).unwrap();
    let part = package
        .parts
        .iter()
        .find(|part| part.name == pptx::RECEIPT_PART)
        .expect("receipt part");
    assert_eq!(part.content_type, "application/json");
    assert!(package.relationships.iter().any(|r| r.source.is_empty()
        && r.kind == pptx::RECEIPT_RELATIONSHIP
        && r.target == pptx::RECEIPT_PART));
    let receipt = content(&part.data);
    assert_eq!(
        text(get(&receipt, "content_sha256")),
        sha256_hex(&first.canonical)
    );
    let codes: Vec<&str> = arr(get(&receipt, "import_receipt"))
        .iter()
        .map(text)
        .collect();
    assert_eq!(codes, first.receipt);
    let export_codes: Vec<&str> = arr(get(&receipt, "export_receipt"))
        .iter()
        .map(text)
        .collect();
    assert!(export_codes.contains(&"generic-master-layout-theme"));
    assert!(export_codes.contains(&"comments-written-legacy"));
    assert_eq!(
        get(&receipt, "fidelity"),
        get(&content(&first.canonical), "fidelity")
    );

    // Written through the shared gate: explicit format, contained destination,
    // never overwriting.
    let root = std::env::temp_dir().join(format!("bran-pptx-export-{}", std::process::id()));
    std::fs::create_dir_all(root.join("out")).unwrap();
    let written = export::write_new(&root, "out/deck.pptx", Format::Pptx, &exported, &[]).unwrap();
    assert_eq!(std::fs::read(&written).unwrap(), exported);
    assert_eq!(
        export::write_new(&root, "out/deck.pptx", Format::Pptx, &exported, &[]),
        Err(Refusal::ExportExists)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pptx_export_refuses_dlp_before_writing() {
    // A canary split across two runs: the shared byte scan cannot see it, the
    // adapter's text scan must.
    let (head, tail) = canary().split_at(canary().len() / 2);
    let split = Deck::ordinary()
        .edit(
            "ppt/slides/slide1.xml",
            "<a:t>fixture team</a:t>",
            &format!("<a:t>{head}</a:t></a:r><a:r><a:t>{tail}</a:t>"),
        )
        .zip();
    let intake = PackageIntake(Format::Pptx)
        .import(&split, &Limits::default(), &Cancel::default())
        .unwrap();
    assert!(!has(&intake, "dlp-findings"));
    let imported = import_bytes(&split).unwrap();
    assert!(has(&imported, "dlp-findings"));
    assert_eq!(Pptx.export(&imported), Err(Refusal::DlpFindings));
    let mut stripped = imported.clone();
    stripped.receipt.retain(|code| code != "dlp-findings");
    assert_eq!(
        Pptx.export(&stripped),
        Err(Refusal::DlpFindings),
        "export scans what it would emit, not only the receipt"
    );

    let marked = Deck::ordinary().edit(
        "ppt/notesSlides/notesSlide1.xml",
        "Keep it under two minutes.",
        "important_boundary",
    );
    let imported = import(&marked);
    assert!(has(&imported, "public-boundary-violation"));
    assert_eq!(Pptx.export(&imported), Err(Refusal::PublicBoundary));
    let mut stripped = imported.clone();
    stripped
        .receipt
        .retain(|code| code != "public-boundary-violation");
    assert_eq!(Pptx.export(&stripped), Err(Refusal::PublicBoundary));
}

#[test]
fn pptx_export_refuses_malformed_or_foreign_content() {
    let first = import(&Deck::ordinary());
    let mut broken = first.clone();
    broken.canonical.truncate(broken.canonical.len() / 2);
    assert_eq!(Pptx.export(&broken), Err(Refusal::MalformedContainer));
    let mut foreign = first.clone();
    foreign.canonical = b"{\"schema\":\"other\"}".to_vec();
    assert_eq!(Pptx.export(&foreign), Err(Refusal::ExportUnsupported));
    // A shape without an identity cannot be projected faithfully.
    let anonymous = Deck::ordinary().edit(
        SLIDE3,
        "<p:cNvPr id=\"8\" name=\"Button 7\">",
        "<p:cNvPr name=\"Button 7\">",
    );
    let imported = import(&anonymous);
    assert!(has(&imported, "shape-identity-missing"));
    assert_eq!(Pptx.export(&imported), Err(Refusal::ExportUnsupported));
}

#[test]
fn pptx_mutated_decks_never_panic() {
    let seeds = [Deck::ordinary(), decks::unsupported_benign()];
    let mut state = 0x5EED_0022_u64;
    let mut next = move |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % bound as u64) as usize
    };
    let mut admitted = 0;
    // 240 rounds keep this near 5 s in a debug build; each round imports,
    // exports, and re-imports a deck.
    for round in 0..240 {
        let deck = &seeds[round % seeds.len()];
        let bytes = if round % 3 == 0 {
            // Container-level damage.
            let mut bytes = deck.zip();
            let at = next(bytes.len());
            bytes[at] ^= 1 << next(8);
            bytes
        } else {
            // Content-level damage inside one XML part, re-zipped cleanly.
            let mut deck = deck.clone();
            let xml: Vec<usize> = (0..deck.0.len())
                .filter(|&i| deck.0[i].0.ends_with(".xml"))
                .collect();
            let data = &mut deck.0[xml[next(xml.len())]].1;
            let at = next(data.len());
            let len = next(48).min(data.len() - at);
            if round % 2 == 0 {
                data.drain(at..at + len);
            } else {
                let copy = data[at..at + len].to_vec();
                data.splice(at..at, copy);
            }
            deck.zip()
        };
        let imported = catch_unwind(AssertUnwindSafe(|| import_bytes(&bytes)))
            .unwrap_or_else(|_| panic!("round {round}: import panicked"));
        let Ok(imported) = imported else { continue };
        admitted += 1;
        let exported = catch_unwind(AssertUnwindSafe(|| Pptx.export(&imported)))
            .unwrap_or_else(|_| panic!("round {round}: export panicked"));
        if let Ok(exported) = exported {
            let again = import_bytes(&exported)
                .unwrap_or_else(|refusal| panic!("round {round}: re-import refused: {refusal}"));
            assert_eq!(
                again.anchors, imported.anchors,
                "round {round}: anchors drifted"
            );
        }
    }
    assert!(admitted > 0, "some mutations must stay admissible");
}

/// Rewrites every relationship attribute prefix in one slide part.
fn rebind(deck: Deck, part: &str, declaration: &str, prefix: &str) -> Deck {
    let mut deck = deck;
    let data = &mut deck.0.iter_mut().find(|(name, _)| name == part).unwrap().1;
    let text = String::from_utf8(data.clone()).unwrap();
    let text = text
        .replace(&format!("xmlns:r=\"{REL}\""), declaration)
        .replace(" r:id=", &format!(" {prefix}:id="))
        .replace(" r:embed=", &format!(" {prefix}:embed="));
    *data = text.into_bytes();
    deck
}

#[test]
fn pptx_relationship_attributes_resolve_by_namespace() {
    let ordinary = import(&Deck::ordinary());
    // Another prefix bound to the relationship namespace reads the same.
    let renamed = rebind(
        Deck::ordinary(),
        SLIDE3,
        &format!("xmlns:rel=\"{REL}\""),
        "rel",
    );
    let renamed = import(&renamed);
    assert_eq!(renamed.canonical, ordinary.canonical);
    // `r` bound to a foreign namespace is not a relationship attribute: the
    // picture has no image and the links name no relationship.
    let foreign = rebind(
        Deck::ordinary(),
        SLIDE3,
        "xmlns:r=\"urn:example:not-relationships\"",
        "r",
    );
    let deck = content(&import(&foreign).canonical);
    let shapes = arr(get(&arr(get(&deck, "slides"))[1], "shapes"));
    assert_eq!(get(&shapes[3], "image"), &Json::Null);
    assert_eq!(get(get(&shapes[4], "link"), "slide"), &Json::Null);
}

/// Opt-in: exports the synthetic decks and opens them in independent readers.
/// Set `BRAN_PPTX_READER_DIR` to a directory the readers may use (a snap
/// LibreOffice needs a non-hidden directory under the home directory) and
/// `BRAN_PPTX_PYTHON` to an interpreter with python-pptx. A missing reader is
/// reported as unavailable, never as passing.
#[test]
#[ignore = "opt-in reader check; needs LibreOffice and python-pptx"]
fn pptx_export_opens_in_independent_readers() {
    let dir = std::env::var_os("BRAN_PPTX_READER_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("bran-pptx-readers"));
    std::fs::create_dir_all(&dir).unwrap();
    // The synthetic source deck is opened too, so reader output can be
    // compared with what the adapter imported.
    let mut paths = Vec::new();
    for (name, deck) in [
        ("ordinary", Deck::ordinary()),
        ("unsupported-benign", decks::unsupported_benign()),
    ] {
        let exported = Pptx.export(&import(&deck)).unwrap();
        for (file, bytes) in [
            (format!("{name}-export.pptx"), exported),
            (format!("{name}-source.pptx"), deck.zip()),
        ] {
            let _ = std::fs::remove_file(dir.join(&file));
            paths.push(export::write_new(&dir, &file, Format::Pptx, &bytes, &[]).unwrap());
        }
    }
    let mut opened = Vec::new();

    let python = std::env::var("BRAN_PPTX_PYTHON").unwrap_or_else(|_| "python3".to_owned());
    let script = "import hashlib, sys\nfrom pptx import Presentation\nfor path in sys.argv[1:]:\n    deck = Presentation(path)\n    print('deck', path.rsplit('/', 1)[-1], len(deck.slides))\n    for slide in deck.slides:\n        print(' slide', slide.slide_id, slide.slide_layout.name)\n        for shape in slide.shapes:\n            descr = shape._element.xpath('./*[1]/p:cNvPr/@descr')\n            text = shape.text_frame.text.replace('\\n', ' | ') if shape.has_text_frame else ''\n            kind = shape._element.tag.rsplit('}', 1)[-1]\n            image = hashlib.sha256(shape.image.blob).hexdigest()[:16] if hasattr(type(shape), 'image') else ''\n            print('  shape', shape.shape_id, kind, repr(text), descr, image)\n        if slide.has_notes_slide:\n            print('  notes', repr(slide.notes_slide.notes_text_frame.text))\n";
    match Command::new(&python)
        .arg("-c")
        .arg(script)
        .args(&paths)
        .output()
    {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            println!("python-pptx:\n{stdout}");
            for expected in [
                "slide 256",
                "slide 300",
                "slide 258",
                "Findings",
                "Region",
                "Keep it under two minutes.",
            ] {
                assert!(
                    stdout.contains(expected),
                    "python-pptx output lacks {expected}"
                );
            }
            opened.push("python-pptx");
        }
        Ok(out) if String::from_utf8_lossy(&out.stderr).contains("No module named 'pptx'") => {
            println!("unavailable: python-pptx is not installed for {python}")
        }
        Ok(out) => panic!(
            "python-pptx failed to open an export: {}",
            String::from_utf8_lossy(&out.stderr)
        ),
        Err(error) => println!("unavailable: {python}: {error}"),
    }

    // A private profile keeps this run apart from any other LibreOffice
    // instance; otherwise a second instance can exit 0 without converting.
    let profile = format!(
        "-env:UserInstallation=file://{}",
        dir.join("lo-profile").display()
    );
    let mut converted = 0;
    for path in &paths {
        let out = match Command::new("libreoffice")
            .arg(&profile)
            .args(["--headless", "--convert-to", "pdf", "--outdir"])
            .arg(&dir)
            .arg(path)
            .output()
        {
            Ok(out) => out,
            Err(error) => {
                println!("unavailable: libreoffice: {error}");
                break;
            }
        };
        let pdf = path.with_extension("pdf");
        let bytes = std::fs::read(&pdf).unwrap_or_default();
        assert!(
            out.status.success() && bytes.starts_with(b"%PDF"),
            "LibreOffice did not convert {}: {}{}",
            path.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if let Ok(text) = Command::new("pdftotext").arg(&pdf).arg("-").output() {
            let text = String::from_utf8_lossy(&text.stdout);
            println!("LibreOffice PDF text ({}):\n{text}", pdf.display());
        }
        converted += 1;
    }
    if converted == paths.len() {
        opened.push("LibreOffice");
    }
    println!("readers that opened every export: {opened:?}");
}
