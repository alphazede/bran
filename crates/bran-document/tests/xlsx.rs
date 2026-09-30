//! XLSX adapter tests (issue #26): projection, determinism, hostile input,
//! formula quarantine, DLP before export, and the import-export-import round
//! trip. Workbooks are built in memory from the reviewable `.parts` fixtures.

use bran_document::canonical::{sha256_hex, Json};
use bran_document::conformance::Imported;
use bran_document::xlsx::{self, Exported};
use bran_document::zip::{self, WriteEntry};
use bran_document::{export, opc, Cancel, Format, Limits, Refusal};
use std::collections::BTreeMap;

const BASE: &str =
    include_str!("../../../fixtures/enterprise-documents/conformance/xlsx-base.parts");
const FEATURES: &str =
    include_str!("../../../fixtures/enterprise-documents/conformance/xlsx-features.parts");
const CANARIES: &str =
    include_str!("../../../fixtures/public-boundary/rejected/synthetic-canaries.txt");
const SHEET1: &str = "xl/worksheets/sheet1.xml";
const WORKBOOK: &str = "xl/workbook.xml";
const WORKBOOK_RELS: &str = "xl/_rels/workbook.xml.rels";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

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

    fn edit(mut self, name: &str, from: &str, to: &str) -> Self {
        let part = self
            .0
            .iter_mut()
            .find(|(part, _)| part == name)
            .expect(name);
        let text = String::from_utf8(part.1.clone()).expect("utf-8 part");
        assert!(
            text.contains(from),
            "edit anchor {from:?} missing in {name}"
        );
        part.1 = text.replacen(from, to, 1).into_bytes();
        self
    }

    fn content_type(self, part: &str, content_type: &str) -> Self {
        let line =
            format!("<Override PartName=\"/{part}\" ContentType=\"{content_type}\"/>\n</Types>");
        self.edit("[Content_Types].xml", "</Types>", &line)
    }

    fn relationship(self, rels: &str, id: &str, kind: &str, target: &str, external: bool) -> Self {
        let mode = if external {
            " TargetMode=\"External\""
        } else {
            ""
        };
        let line = format!(
            "<Relationship Id=\"{id}\" Type=\"{REL}/{kind}\" Target=\"{target}\"{mode}/>\n</Relationships>"
        );
        self.edit(rels, "</Relationships>", &line)
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

fn limits() -> Limits {
    Limits {
        max_package_bytes: 1024 * 1024,
        max_parts: 64,
        max_part_bytes: 256 * 1024,
        max_total_bytes: 1024 * 1024,
        max_ratio: 100,
        max_xml_depth: 64,
        max_xml_nodes: 10_000,
    }
}

fn import(bytes: &[u8]) -> Result<Imported, Refusal> {
    xlsx::import(bytes, &limits(), &Cancel::default())
}

fn features() -> Imported {
    import(&Parts::parse(FEATURES).zip()).expect("feature workbook imports")
}

fn text(imported: &Imported) -> String {
    String::from_utf8(imported.canonical.clone()).expect("canonical bytes are UTF-8")
}

fn field<'a>(json: &'a Json, key: &str) -> &'a Json {
    match json {
        Json::Obj(map) => map.get(key).unwrap_or_else(|| panic!("missing {key}")),
        _ => panic!("not an object when reading {key}"),
    }
}

fn canary() -> &'static str {
    CANARIES
        .lines()
        .find(|line| !line.is_empty())
        .expect("synthetic canary")
}

/// Every supported feature of the fixture lands in the grid projection as a
/// separate typed field, with stable locators and a fidelity receipt.
#[test]
fn xlsx_features_import() {
    let imported = features();
    let canonical = text(&imported);
    let expect = |fragment: &str| {
        assert!(
            canonical.contains(fragment),
            "projection lacks {fragment}\n{canonical}"
        )
    };
    // Workbook order, names, sheet identities, visibility, and dimensions.
    expect(r#""dimension":"A1:E5","hyperlinks":"#);
    expect(r#""name":"Metrics","sheet_id":1,"state":"visible""#);
    expect(r#""name":"Notes","sheet_id":4,"state":"hidden""#);
    assert!(canonical.find(r#""name":"Metrics""#) < canonical.find(r#""name":"Notes","sheet_id""#));
    // Typed values, number formats, formulas and cached results kept apart.
    expect(
        r#"{"column":2,"labels":{"column":"Value","row":"Alpha"},"ref":"B2","row":2,"value":{"type":"number","value":"40"}}"#,
    );
    expect(
        r#"{"column":3,"labels":{"row":"Alpha"},"number_format":"0.0%","ref":"C2","row":2,"value":{"type":"number","value":"0.25"}}"#,
    );
    expect(r#""number_format":14,"ref":"C3""#);
    expect(
        r#"{"cached":{"type":"number","value":"42"},"column":2,"formula":{"text":"SUM(Table1[Value])"},"labels":{"column":"Value","row":"Total"},"ref":"B4","row":4}"#,
    );
    expect(
        r#"{"cached":{"type":"string","value":"Alpha!"},"column":3,"formula":{"text":"A2&\"!\""},"#,
    );
    expect(r#""ref":"C4","row":4,"value":{"type":"boolean","value":true}}"#);
    expect(r##""ref":"B5","row":5,"value":{"type":"error","value":"#DIV/0!"}}"##);
    expect(r#""ref":"A5","row":5,"value":{"type":"string","value":"=1+1 stays text"}}"#);
    // Rich text is flattened and phonetic runs are not cell text.
    expect(r#""value":{"type":"string","value":"Beta"}"#);
    assert!(
        !canonical.contains("reading"),
        "phonetic run leaked into text"
    );
    // Tables, named ranges, merged cells, comments, links, validations.
    expect(
        r#""tables":[{"columns":["Metric","Value"],"display_name":"Table1","header_row":true,"id":1,"name":"Table1","ref":"A1:B3","totals_row":false}]"#,
    );
    expect(
        r#""defined_names":[{"hidden":true,"name":"LocalNotes","scope":"Notes","value":"Notes!$A$1:$A$2"},{"name":"TotalValue","value":"Metrics!$B$4"}]"#,
    );
    expect(r#""merged_cells":["D1:E2"]"#);
    expect(r#""comments":[{"author":"Reviewer","ref":"B2","text":"Synthetic note"}]"#);
    expect(
        r#""hyperlinks":[{"ref":"A2","target":"https://example.invalid/alpha"},{"display":"Notes","location":"Notes!A1","ref":"A3"}]"#,
    );
    expect(
        r#""data_validations":[{"attributes":{"allowBlank":"1","type":"list"},"formula1":"\"Alpha,Beta\"","sqref":"A2:A3"}]"#,
    );
    // Charts, drawings, and images are typed relationships, not content.
    expect(
        r#"{"kind":"chart","relationship":"http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart","sha256":""#,
    );
    expect(r#""source":"xl/drawings/drawing1.xml","target":"xl/charts/chart1.xml"}"#);
    expect(r#""source":"xl/drawings/drawing1.xml","target":"xl/media/image1.png"}"#);
    expect(r#""source":"xl/worksheets/sheet1.xml","target":"xl/drawings/drawing1.xml"}"#);
    expect(
        r#"{"kind":"conditional-formatting","sheet":"Metrics","source":"xl/worksheets/sheet1.xml"}"#,
    );
    expect(
        r#""fidelity":{"cells":"normalized","charts":"unsupported","formulas":"normalized","macros":"unsupported","sheets":"exact","text":"normalized"}"#,
    );
    for code in [
        "document-properties-not-imported",
        "formula-cached-result-not-recalculated",
        "hyperlink-not-fetched",
        "layout-not-imported",
        "rich-text-flattened",
        "styles-reduced-to-number-formats",
        "table-details-not-imported",
        "unsupported-chart",
        "unsupported-conditional-formatting",
        "unsupported-drawing",
        "unsupported-image",
    ] {
        assert!(
            imported.receipt.iter().any(|r| r == code),
            "receipt lacks {code}"
        );
    }
    // Grid anchors match the envelope's grid family: sheet, cell, header,
    // table, and range roles with sheet/row/column locators.
    expect(
        r#"{"family":"grid","id":"anc:xlsx:s1:r4c2","locator":{"column":2,"family":"grid","row":4,"sheet":"Metrics"},"role":"cell","text":"=SUM(Table1[Value]) [cached: 42]","#,
    );
    expect(
        r#"{"family":"grid","id":"anc:xlsx:s1:r1c2","locator":{"column":2,"family":"grid","row":1,"sheet":"Metrics"},"role":"header","text":"Value","#,
    );
    expect(
        r#"{"family":"grid","id":"anc:xlsx:s4","locator":{"column":1,"family":"grid","row":1,"sheet":"Notes"},"role":"sheet","text":"Notes","#,
    );
    expect(
        r#"{"family":"grid","id":"anc:xlsx:s1:table-1","locator":{"column":1,"family":"grid","row":1,"sheet":"Metrics"},"role":"table","text":"Table1 A1:B3: Metric, Value","#,
    );
    expect(
        r#"{"family":"grid","id":"anc:xlsx:name-TotalValue","locator":{"column":2,"family":"grid","row":4,"sheet":"Metrics"},"role":"range","text":"TotalValue = Metrics!$B$4","#,
    );
    expect(
        r#"{"family":"grid","id":"anc:xlsx:s4:name-LocalNotes","locator":{"column":1,"family":"grid","row":1,"sheet":"Notes"},"role":"range","#,
    );
    let ids: Vec<&str> = imported.anchors.iter().map(|a| a.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(ids, sorted, "anchor ids are unique and sorted");
    let formula = imported
        .anchors
        .iter()
        .find(|a| a.id == "anc:xlsx:s1:r4c2")
        .expect("formula anchor");
    assert_eq!(
        formula.text_digest,
        sha256_hex(b"=SUM(Table1[Value]) [cached: 42]")
    );
}

/// The ordinary base workbook keeps its formula and cached value apart, and
/// a formula without a cached value never gains one.
#[test]
fn xlsx_cached_results_are_never_presented_as_calculated() {
    let canonical = text(&import(&Parts::parse(BASE).zip()).expect("base imports"));
    assert!(canonical.contains(
        r#"{"cached":{"type":"number","value":"84"},"column":2,"formula":{"text":"B1*2"},"#
    ));
    let uncached = Parts::parse(BASE)
        .edit(
            "xl/worksheets/sheet1.xml",
            "<f>B1*2</f><v>84</v>",
            "<f>B1*2</f>",
        )
        .zip();
    let imported = import(&uncached).expect("uncached formula imports");
    let canonical = text(&imported);
    assert!(
        canonical.contains(r#"{"column":2,"formula":{"text":"B1*2"},"#),
        "{canonical}"
    );
    assert!(canonical.contains(r#""text":"=B1*2","#), "{canonical}");
    assert!(
        !canonical.contains(r#""value":"84""#),
        "no value may be invented"
    );
    assert!(!imported
        .receipt
        .iter()
        .any(|code| code == "formula-cached-result-not-recalculated"));
}

/// ZIP entry order, compression, timestamps, shared-string table order, and
/// document-property timestamps do not change the import.
#[test]
fn xlsx_import_is_deterministic() {
    let parts = Parts::parse(FEATURES);
    let reference = features();
    for bytes in [
        parts.zip_with(true, false, 0x4A21),
        parts.zip_with(false, true, 0x3C01),
    ] {
        assert_eq!(import(&bytes).expect("re-encoding imports"), reference);
    }
    let retimed = parts
        .clone()
        .edit(
            "docProps/core.xml",
            "2026-01-02T03:04:05Z",
            "2031-12-30T23:59:59Z",
        )
        .zip();
    assert_eq!(import(&retimed).expect("retimed imports"), reference);
    // Same strings, same cells, different shared-string table order.
    let reordered = parts
        .edit(
            "xl/sharedStrings.xml",
            "<si><t>Metric</t></si><si><t>Value</t></si>",
            "<si><t>Value</t></si><si><t>Metric</t></si>",
        )
        .edit(
            SHEET1,
            r#"<c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c>"#,
            r#"<c r="A1" t="s"><v>1</v></c><c r="B1" t="s"><v>0</v></c>"#,
        )
        .zip();
    assert_eq!(import(&reordered).expect("reordered imports"), reference);
}

/// Hostile workbooks fail with typed refusals and never a partial result.
#[test]
fn xlsx_hostile_workbooks_are_refused() {
    use Refusal::*;
    let base = || Parts::parse(FEATURES);
    let external_link = "xl/externalLinks/externalLink1.xml";
    let refusals: Vec<(&str, Vec<u8>, Refusal)> = vec![
        (
            "external workbook link part",
            base()
                .add(external_link, "<externalLink xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"/>")
                .content_type(external_link, "application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml")
                .relationship(WORKBOOK_RELS, "rId9", "externalLink", "externalLinks/externalLink1.xml", false)
                .zip(),
            ExternalReference,
        ),
        (
            "external workbook path",
            base()
                .relationship(WORKBOOK_RELS, "rId9", "externalLinkPath", "file:///synthetic/other.xlsx", true)
                .zip(),
            ExternalReference,
        ),
        (
            "query table",
            base()
                .add("xl/queryTables/queryTable1.xml", "<queryTable xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"/>")
                .content_type("xl/queryTables/queryTable1.xml", "application/vnd.openxmlformats-officedocument.spreadsheetml.queryTable+xml")
                .zip(),
            ExternalReference,
        ),
        (
            "data connection",
            base()
                .add("xl/connections.xml", "<connections xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"/>")
                .content_type("xl/connections.xml", "application/vnd.openxmlformats-officedocument.spreadsheetml.connections+xml")
                .zip(),
            ExternalReference,
        ),
        (
            "Power Query mashup (UTF-8)",
            base()
                .add("customXml/item1.xml", "<DataMashup xmlns=\"http://schemas.microsoft.com/DataMashup\">AAAA</DataMashup>")
                .zip(),
            ActiveContent,
        ),
        (
            "Power Query mashup (UTF-16)",
            base()
                .add(
                    "customXml/item1.xml",
                    [0xFF, 0xFE]
                        .into_iter()
                        .chain("<DataMashup xmlns=\"http://schemas.microsoft.com/DataMashup\">AAAA</DataMashup>".encode_utf16().flat_map(u16::to_le_bytes))
                        .collect::<Vec<u8>>(),
                )
                .zip(),
            ActiveContent,
        ),
        (
            "macro-enabled workbook",
            base()
                .edit(
                    "[Content_Types].xml",
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
                    "application/vnd.ms-excel.sheet.macroEnabled.main+xml",
                )
                .zip(),
            ActiveContent,
        ),
        (
            "VBA project",
            base()
                .add("xl/vbaProject.bin", "synthetic inert bytes")
                .content_type("xl/vbaProject.bin", "application/vnd.ms-office.vbaProject")
                .zip(),
            ActiveContent,
        ),
        (
            "malformed worksheet XML",
            base().edit(SHEET1, "</sheetData>", "").zip(),
            MalformedXml,
        ),
        (
            "DTD in worksheet",
            base()
                .edit(SHEET1, "<worksheet ", "<!DOCTYPE w [<!ENTITY a \"aa\">]><worksheet ")
                .zip(),
            XmlDtdRefused,
        ),
        (
            "worksheet path escape",
            base()
                .relationship("xl/worksheets/_rels/sheet1.xml.rels", "rId9", "image", "../../../outside.png", false)
                .zip(),
            UnsafePartPath,
        ),
        (
            "worksheet decompression bomb",
            base()
                .edit(SHEET1, "</worksheet>", &format!("<!--{}--></worksheet>", " ".repeat(400 * 1024)))
                .zip(),
            DecompressionLimit,
        ),
        (
            "shared string out of range",
            base().edit(SHEET1, r#"<c r="A1" t="s"><v>0</v>"#, r#"<c r="A1" t="s"><v>99</v>"#).zip(),
            MalformedContainer,
        ),
        (
            "cell reference outside the grid",
            base().edit(SHEET1, r#"<c r="B2">"#, r#"<c r="XFE2">"#).zip(),
            MalformedContainer,
        ),
        (
            "duplicate cell",
            base().edit(SHEET1, r#"<c r="B3">"#, r#"<c r="B2">"#).zip(),
            MalformedContainer,
        ),
        (
            "cell outside its row",
            base().edit(SHEET1, r#"<c r="B3">"#, r#"<c r="B4">"#).zip(),
            MalformedContainer,
        ),
        (
            "non-numeric number",
            base().edit(SHEET1, "<v>40</v>", "<v>forty</v>").zip(),
            MalformedContainer,
        ),
        (
            "duplicate sheet name",
            base().edit(WORKBOOK, r#"name="Notes""#, r#"name="METRICS""#).zip(),
            MalformedContainer,
        ),
        (
            "duplicate sheet id",
            base().edit(WORKBOOK, r#"sheetId="4""#, r#"sheetId="1""#).zip(),
            MalformedContainer,
        ),
        (
            "missing worksheet part",
            base().edit(WORKBOOK_RELS, "worksheets/sheet2.xml", "worksheets/sheet9.xml").zip(),
            MalformedContainer,
        ),
        (
            "unknown style index",
            base().edit(SHEET1, r#"<c r="C2" s="1">"#, r#"<c r="C2" s="7">"#).zip(),
            MalformedContainer,
        ),
        (
            "not a workbook",
            base()
                .edit("[Content_Types].xml", "spreadsheetml.sheet.main+xml", "wordprocessingml.document.main+xml")
                .zip(),
            MalformedContainer,
        ),
    ];
    for (label, bytes, expected) in refusals {
        assert_eq!(import(&bytes), Err(expected), "{label}");
    }
    let cancel = Cancel::default();
    cancel.cancel();
    assert_eq!(
        xlsx::import(&Parts::parse(FEATURES).zip(), &limits(), &cancel),
        Err(Cancelled)
    );
}

/// Formulas and links that would reach outside the workbook when opened are
/// kept as inert evidence, flagged, and never exported as live content.
#[test]
fn xlsx_active_formulas_are_quarantined() {
    for formula in [
        "cmd|' /C calc'!A0",
        "WEBSERVICE(\"https://example.invalid\")",
        "_xlfn.IMAGE(\"https://example.invalid/i.png\")",
        "[1]Sheet1!A1",
        "'[Other.xlsx]Sheet1'!A1",
        "RTD(\"server\",,\"topic\")",
    ] {
        let escaped = formula
            .replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;");
        let bytes = Parts::parse(FEATURES)
            .edit(
                SHEET1,
                "<f>SUM(Table1[Value])</f>",
                &format!("<f>{escaped}</f>"),
            )
            .zip();
        let imported = import(&bytes).unwrap_or_else(|r| panic!("{formula}: {r}"));
        assert!(
            imported.receipt.iter().any(|c| c == "formula-quarantined"),
            "{formula} not quarantined"
        );
        assert!(
            text(&imported).contains(r#""quarantined":true"#),
            "{formula}"
        );
        assert_eq!(
            xlsx::export(&imported).err(),
            Some(Refusal::ActiveContent),
            "{formula} exported"
        );
    }
    // A string literal that mentions a pipe or a function is not active.
    let benign = Parts::parse(FEATURES)
        .edit(
            SHEET1,
            "<f>SUM(Table1[Value])</f>",
            "<f>\"a|b WEBSERVICE(\"&amp;A2</f>",
        )
        .zip();
    let imported = import(&benign).expect("benign formula imports");
    assert!(!imported.receipt.iter().any(|c| c == "formula-quarantined"));
    assert!(xlsx::export(&imported).is_ok());
    // Names and validation formulas get the same check.
    let name = Parts::parse(FEATURES)
        .edit(
            WORKBOOK,
            "Metrics!$B$4",
            "WEBSERVICE(\"https://example.invalid\")",
        )
        .zip();
    assert!(import(&name)
        .unwrap()
        .receipt
        .iter()
        .any(|c| c == "formula-quarantined"));
    let link = Parts::parse(FEATURES)
        .edit(
            "xl/worksheets/_rels/sheet1.xml.rels",
            "https://example.invalid/alpha",
            "file:///synthetic/run.exe",
        )
        .zip();
    let imported = import(&link).expect("file link imports as inert evidence");
    assert!(imported
        .receipt
        .iter()
        .any(|c| c == "hyperlink-quarantined"));
    assert_eq!(xlsx::export(&imported).err(), Some(Refusal::ActiveContent));
}

/// Attribute keys keep their prefix as written: a relationship id resolves
/// through its namespace binding, whatever the prefix, and an `id` in a
/// foreign namespace never stands in for it. Control characters travel as
/// `_xHHHH_` escapes, which the strict XML reader accepts; a raw control
/// character reference is refused.
#[test]
fn xlsx_relationship_ids_resolve_through_namespace_bindings() {
    let reference = features();
    let mut renamed = Parts::parse(FEATURES)
        .edit(WORKBOOK, "xmlns:r=", "xmlns:rel=")
        .edit(WORKBOOK, r#"r:id="rId1""#, r#"rel:id="rId1""#)
        .edit(WORKBOOK, r#"r:id="rId2""#, r#"rel:id="rId2""#)
        .edit(SHEET1, "xmlns:r=", "xmlns:q=");
    for id in ["rId1", "rId2", "rId3", "rId4"] {
        renamed = renamed.edit(SHEET1, &format!("r:id=\"{id}\""), &format!("q:id=\"{id}\""));
    }
    assert_eq!(import(&renamed.zip()).expect("any bound prefix"), reference);

    let foreign = Parts::parse(FEATURES)
        .edit(
            WORKBOOK,
            r#"r:id="rId2""#,
            r#"x:id="rId2" xmlns:x="urn:synthetic:other""#,
        )
        .zip();
    assert_eq!(import(&foreign), Err(Refusal::MalformedContainer));
    let foreign_link = Parts::parse(FEATURES)
        .edit(
            SHEET1,
            r#"<hyperlink ref="A2" r:id="rId1"/>"#,
            r#"<hyperlink ref="A2" x:id="rId1" xmlns:x="urn:synthetic:other"/>"#,
        )
        .zip();
    let canonical = text(&import(&foreign_link).expect("imports"));
    assert!(
        canonical.contains(r#""hyperlinks":[{"ref":"A2"},"#),
        "{canonical}"
    );
    // The relationship nothing claimed is still listed, never dropped.
    assert!(
        canonical.contains(r#"{"kind":"hyperlink","relationship":"#),
        "{canonical}"
    );

    let control = Parts::parse(FEATURES)
        .edit(
            "xl/sharedStrings.xml",
            "<t>Alpha</t>",
            "<t>Alpha_x0007_</t>",
        )
        .zip();
    let first = import(&control).expect("escaped control imports");
    assert!(text(&first).contains(r#""value":"Alpha\u0007""#));
    let again = import(&exported(&first).bytes).expect("export passes the strict reader");
    assert_eq!(again.anchors, first.anchors);
    let raw = Parts::parse(FEATURES)
        .edit("xl/sharedStrings.xml", "<t>Alpha</t>", "<t>Alpha&#7;</t>")
        .zip();
    assert_eq!(import(&raw), Err(Refusal::MalformedXml));
}

fn exported(imported: &Imported) -> Exported {
    xlsx::export(imported).expect("export succeeds")
}

/// Import, export, import: sheet identities, cell types, formulas, cached
/// values, number formats, tables, names, merges, comments, links,
/// validations, and anchors survive. Unsupported content is listed as
/// omitted in the export receipt, never silently dropped.
#[test]
fn xlsx_round_trip_preserves_supported_content() {
    let first = features();
    let out = exported(&first);
    assert_eq!(exported(&first).bytes, out.bytes, "export is deterministic");
    let second = import(&out.bytes).expect("export re-imports");
    let supported = |imported: &Imported| {
        let json = xlsx::parse_canonical(&imported.canonical).expect("canonical parses");
        (
            field(&json, "sheets").clone(),
            field(&json, "defined_names").clone(),
            field(&json, "anchors").clone(),
        )
    };
    assert_eq!(supported(&second), supported(&first));
    assert_eq!(second.anchors, first.anchors);
    // Exporting the re-import rewrites every workbook part byte for byte;
    // only the receipt differs, because it names a different source.
    let workbook_parts = |bytes: &[u8]| {
        zip::read(bytes, &limits(), &Cancel::default())
            .unwrap()
            .into_iter()
            .filter(|entry| entry.name != xlsx::RECEIPT_PART)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        workbook_parts(&exported(&second).bytes),
        workbook_parts(&out.bytes),
        "export of the re-import is a fixpoint"
    );

    let receipt = String::from_utf8(out.receipt.clone()).unwrap();
    for fragment in [
        r#""formulas_recalculated":false"#,
        r#""projection":"bran-xlsx-grid/1""#,
        r#"{"kind":"chart","#,
        r#"{"kind":"image","#,
        r#"{"kind":"conditional-formatting","#,
        &format!(
            r#""source_canonical_sha256":"{}""#,
            sha256_hex(&first.canonical)
        ),
    ] {
        assert!(
            receipt.contains(fragment),
            "receipt lacks {fragment}: {receipt}"
        );
    }
    // The receipt travels inside the package and matches the returned one.
    let package =
        opc::open(&out.bytes, &limits(), &Cancel::default()).expect("export is a clean package");
    let part = package
        .parts
        .iter()
        .find(|part| part.name == "bran/fidelity-receipt.json")
        .expect("receipt part");
    assert_eq!(part.data, out.receipt);
    assert!(package
        .relationships
        .iter()
        .any(|r| r.source.is_empty() && r.target == "bran/fidelity-receipt.json"));
    assert_eq!(
        package.diagnostics.iter().copied().collect::<Vec<_>>(),
        ["hyperlink-not-fetched"]
    );
    // No part carries a timestamp that could vary: the ZIP uses one fixed time.
    let entries = zip::read(&out.bytes, &limits(), &Cancel::default()).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "parts are written in sorted order");
}

/// Text that looks like a formula stays a string cell on export: nothing
/// derived from an untrusted text cell becomes a live formula.
#[test]
fn xlsx_export_never_turns_text_into_formulas() {
    let bytes = Parts::parse(FEATURES)
        .edit(SHEET1, "=1+1 stays text", "=cmd|' /C calc'!A0")
        .zip();
    let out = exported(&import(&bytes).expect("imports"));
    let entries = zip::read(&out.bytes, &limits(), &Cancel::default()).unwrap();
    let sheet = entries
        .iter()
        .find(|e| e.name == "xl/worksheets/sheet1.xml")
        .expect("sheet part");
    let xml = String::from_utf8(sheet.data.clone()).unwrap();
    assert!(
        xml.contains(
            r#"<c r="A5" t="inlineStr"><is><t xml:space="preserve">=cmd|' /C calc'!A0</t></is></c>"#
        ),
        "{xml}"
    );
    assert_eq!(
        xml.matches("<f>").count(),
        2,
        "only the two source formulas: {xml}"
    );
}

/// DLP and the public boundary run before any export bytes exist.
#[test]
fn xlsx_export_runs_dlp_and_public_boundary_first() {
    let dlp = Parts::parse(FEATURES)
        .edit("xl/comments1.xml", "Synthetic note", canary())
        .zip();
    let imported = import(&dlp).expect("DLP findings are admitted with a receipt");
    assert!(imported.receipt.iter().any(|c| c == "dlp-findings"));
    assert_eq!(xlsx::export(&imported).err(), Some(Refusal::DlpFindings));
    // A canary split across rich-text runs is invisible to the byte scan; the
    // adapter's check on extracted text still finds it.
    let (head, tail) = canary().split_at(canary().len() / 2);
    let split = Parts::parse(FEATURES)
        .edit(
            "xl/sharedStrings.xml",
            "<r><t>Be</t></r><r><rPr><b/></rPr><t>ta</t></r>",
            &format!("<r><t>{head}</t></r><r><rPr><b/></rPr><t>{tail}</t></r>"),
        )
        .zip();
    let package = opc::open(&split, &limits(), &Cancel::default()).unwrap();
    assert!(
        !package.diagnostics.contains("dlp-findings"),
        "byte scan sees a split canary"
    );
    let imported = import(&split).expect("imports");
    assert!(imported.receipt.iter().any(|c| c == "dlp-findings"));
    assert_eq!(xlsx::export(&imported).err(), Some(Refusal::DlpFindings));

    let boundary = Parts::parse(FEATURES)
        .edit(SHEET1, "=1+1 stays text", "important_boundary")
        .zip();
    let imported = import(&boundary).expect("imports");
    assert!(imported
        .receipt
        .iter()
        .any(|c| c == "public-boundary-violation"));
    assert_eq!(xlsx::export(&imported).err(), Some(Refusal::PublicBoundary));

    // A projection edited after import is re-checked, not trusted.
    let mut tampered = features();
    let canonical = text(&tampered).replace("Synthetic note", canary());
    tampered.canonical = canonical.into_bytes();
    assert_eq!(xlsx::export(&tampered).err(), Some(Refusal::DlpFindings));

    // Writing to disk goes through the shared gate: never overwrites.
    let root = std::env::temp_dir().join(format!("bran-xlsx-export-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let out = exported(&features());
    let written = export::write_new(&root, "grid.xlsx", Format::Xlsx, &out.bytes, &[]);
    assert!(written.is_ok(), "{written:?}");
    assert_eq!(
        export::write_new(&root, "grid.xlsx", Format::Xlsx, &out.bytes, &[]),
        Err(Refusal::ExportExists)
    );
    std::fs::remove_dir_all(&root).unwrap();
}

/// A canonical projection that is not the adapter's own is refused by the
/// exporter, not partially written.
#[test]
fn xlsx_export_refuses_foreign_projections() {
    let mut foreign = features();
    for canonical in [
        b"not json".to_vec(),
        br#"{"schema_version":"other"}"#.to_vec(),
        br#"{"schema_version":"bran-xlsx-grid/1","sheets":[]}"#.to_vec(),
        br#"{"schema_version":"bran-xlsx-grid/1","defined_names":[],"sheets":[{"name":"a/b","sheet_id":1,"state":"visible"}]}"#.to_vec(),
        "[".repeat(10_000).into_bytes(),
    ] {
        foreign.canonical = canonical;
        assert_eq!(xlsx::export(&foreign).err(), Some(Refusal::ExportUnsupported));
    }
}

/// A projection edited after import is re-checked whole, including the
/// `unsupported` entries the export receipt embeds as `omitted`: a canary or
/// boundary marker there refuses the export instead of leaking into it.
#[test]
fn xlsx_export_checks_receipt_content_for_dlp_and_boundary() {
    let mut tampered = features();
    let canonical = text(&tampered).replace("xl/charts/chart1.xml", canary());
    assert_ne!(text(&tampered), canonical, "tamper anchor moved");
    tampered.canonical = canonical.into_bytes();
    assert_eq!(xlsx::export(&tampered).err(), Some(Refusal::DlpFindings));

    let mut tampered = features();
    let canonical = text(&tampered).replace("xl/charts/chart1.xml", "important_boundary");
    tampered.canonical = canonical.into_bytes();
    assert_eq!(
        xlsx::export(&tampered).err(),
        Some(Refusal::PublicBoundary)
    );
}

/// A table whose range corners run backwards (`B1:A3`) is refused on
/// import, and a projection edited to one is refused on export: neither
/// path may reach table arithmetic that underflows.
#[test]
fn xlsx_reversed_table_range_is_refused() {
    let bytes = Parts::parse(FEATURES)
        .edit("xl/tables/table1.xml", r#"ref="A1:B3""#, r#"ref="B1:A3""#)
        .edit(
            "xl/tables/table1.xml",
            r#"<tableColumns count="2"><tableColumn id="1" name="Metric"/><tableColumn id="2" name="Value"/></tableColumns>"#,
            r#"<tableColumns count="1"><tableColumn id="1" name="Metric"/></tableColumns>"#,
        )
        .zip();
    assert_eq!(import(&bytes), Err(Refusal::MalformedContainer));

    let mut tampered = features();
    let canonical = text(&tampered).replace(r#""ref":"A1:B3""#, r#""ref":"B1:A3""#);
    assert_ne!(text(&tampered), canonical, "tamper anchor moved");
    tampered.canonical = canonical.into_bytes();
    assert_eq!(
        xlsx::export(&tampered).err(),
        Some(Refusal::ExportUnsupported)
    );
}

/// Opt-in: writes exported workbooks for independent readers. Set
/// `BRAN_XLSX_READER_DIR` to a writable directory; the readers themselves
/// (LibreOffice, openpyxl) run outside this crate. Without the variable the
/// check reports unavailable and writes nothing.
#[test]
#[ignore = "opt-in reader samples: set BRAN_XLSX_READER_DIR"]
fn xlsx_reader_samples() {
    let Some(dir) = std::env::var_os("BRAN_XLSX_READER_DIR") else {
        println!("unavailable: BRAN_XLSX_READER_DIR is not set");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let samples: BTreeMap<&str, Imported> = BTreeMap::from([
        (
            "bran-xlsx-base.xlsx",
            import(&Parts::parse(BASE).zip()).unwrap(),
        ),
        ("bran-xlsx-features.xlsx", features()),
    ]);
    for (name, imported) in samples {
        let out = exported(&imported);
        std::fs::write(dir.join(name), &out.bytes).unwrap();
        std::fs::write(dir.join(format!("{name}.receipt.json")), &out.receipt).unwrap();
        std::fs::write(
            dir.join(format!("{name}.projection.json")),
            &imported.canonical,
        )
        .unwrap();
        println!("wrote {name}");
    }
}
